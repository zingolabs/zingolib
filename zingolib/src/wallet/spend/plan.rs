//! The pure plan layer of the in-tree spend pipeline (ADR 0010).
//!
//! [`plan_transfer`] and [`plan_shield`] map a read-only wallet view and a
//! payment request to an owned [`Proposal`] — note selection, change,
//! ZIP 317 fee sizing, and ZIP 320 TEX step-splitting — with no wallet
//! mutation. Fee arithmetic stays upstream ([`super::fee`]); this module
//! decides only what the fee rule is fed.
//!
//! The migration's comparative equivalence suite proved this planner
//! fee-for-fee and input-for-input equal to `zcash_client_backend`
//! 0.24's `propose_transfer`/`propose_shielding` on every scenario
//! class before that dependency was removed; the invariant tests at
//! the bottom are its survivors.

use std::collections::BTreeMap;

use zcash_keys::address::Address;
use zcash_primitives::transaction::fees::transparent::InputSize;
use zcash_protocol::consensus::{BlockHeight, NetworkUpgrade, Parameters};
use zcash_protocol::memo::MemoBytes;
use zcash_protocol::value::{BalanceError, Zatoshis};
use zcash_protocol::{PoolType, ShieldedPool};
use zcash_transparent::address::TransparentAddress;
use zip321::{Payment, TransactionRequest};

use pepper_sync::wallet::OutputInterface as _;

use super::fee::{TransactionShape, op_return_output_size};
use super::op_return::OpReturnData;
use super::proposal::{
    ChangeValue, Proposal, ShieldProposal, ShieldedInput, Step, TexTransferProposal,
    TransferProposal, TransparentInput,
};
use crate::wallet::LightWallet;
use crate::wallet::error::WalletError;

/// The minimum value a shielding must move, mirroring the threshold the
/// facade passes to `propose_shielding`.
const SHIELDING_THRESHOLD: Zatoshis = Zatoshis::const_from_u64(10_000);

/// The serialized size of a standard P2PKH transparent input.
const P2PKH_INPUT_SIZE: usize = 150;

/// Ways planning a spend can fail.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    /// The wallet has no synced data to plan against.
    #[error("wallet has no synced data to plan against")]
    SyncRequired,
    /// A payment has no stated amount.
    #[error("the payment at index {0} has no amount")]
    PaymentAmountMissing(usize),
    /// A payment address has no receiver this wallet can pay.
    #[error("no supported receiver on the payment address at index {0}")]
    UnsupportedAddress(usize),
    /// The spendable funds cannot cover the payments plus the fee.
    #[error(
        "insufficient funds: {} available of {} required",
        available.into_u64(),
        required.into_u64()
    )]
    InsufficientFunds {
        /// The total value the planner could select.
        available: Zatoshis,
        /// The outputs-plus-fee total it had to cover.
        required: Zatoshis,
    },
    /// A wallet query failed.
    #[error(transparent)]
    Wallet(#[from] WalletError),
    /// Amounts overflowed.
    #[error("balance overflow")]
    Balance(#[from] BalanceError),
    /// The upstream fee rule refused the transaction shape.
    #[error("the fee rule refused the transaction shape: {0:?}")]
    Fee(zcash_primitives::transaction::fees::zip317::FeeError),
}

impl From<zcash_primitives::transaction::fees::zip317::FeeError> for PlanError {
    fn from(fee_error: zcash_primitives::transaction::fees::zip317::FeeError) -> Self {
        PlanError::Fee(fee_error)
    }
}

/// One payment routed to the pool that will pay it.
struct RoutedPayment {
    payment: Payment,
    pool: PoolType,
    /// Serialized output size when the pool is transparent.
    transparent_output_size: Option<usize>,
}

/// The request split by destination: the final-step TEX payments and
/// everything else.
struct RoutedRequest {
    standard: Vec<RoutedPayment>,
    tex: Vec<RoutedPayment>,
}

impl RoutedRequest {
    fn standard_total(&self) -> Result<Zatoshis, PlanError> {
        sum_payments(self.standard.iter().map(|routed| &routed.payment))
    }

    fn tex_total(&self) -> Result<Zatoshis, PlanError> {
        sum_payments(self.tex.iter().map(|routed| &routed.payment))
    }
}

fn sum_payments<'a>(
    mut payments: impl Iterator<Item = &'a Payment>,
) -> Result<Zatoshis, PlanError> {
    payments.try_fold(Zatoshis::ZERO, |acc, payment| {
        let amount = payment.amount().unwrap_or(Zatoshis::ZERO);
        (acc + amount).ok_or(PlanError::Balance(BalanceError::Overflow))
    })
}

/// Routes each payment to the pool that will pay it, splitting TEX
/// payments (whose memos ZIP 320 forbids) into the exposure step.
fn route_request(
    request: &TransactionRequest,
    wallet: &LightWallet,
    ironwood_active: bool,
) -> Result<RoutedRequest, PlanError> {
    let network = wallet.chain_type.network_type();
    let orchard_family_pool = if ironwood_active {
        PoolType::IRONWOOD
    } else {
        PoolType::ORCHARD
    };

    let mut standard = Vec::new();
    let mut tex = Vec::new();
    for (&index, payment) in request.payments() {
        if payment.amount().is_none() {
            return Err(PlanError::PaymentAmountMissing(index));
        }
        let address = payment
            .recipient_address()
            .clone()
            .convert_if_network::<Address>(network)
            .map_err(|_| PlanError::UnsupportedAddress(index))?;

        match address {
            Address::Transparent(transparent_address) => standard.push(RoutedPayment {
                payment: payment.clone(),
                pool: PoolType::TRANSPARENT,
                transparent_output_size: Some(transparent_output_size(&transparent_address)),
            }),
            Address::Tex(_) => tex.push(RoutedPayment {
                // ZIP 320 forbids a memo on a TEX payment.
                payment: Payment::without_memo(
                    payment.recipient_address().clone(),
                    payment.amount().expect("checked above"),
                ),
                pool: PoolType::TRANSPARENT,
                // A TEX address is a P2PKH hash by construction.
                transparent_output_size: Some(34),
            }),
            Address::Sapling(_) => standard.push(RoutedPayment {
                payment: payment.clone(),
                pool: PoolType::SAPLING,
                transparent_output_size: None,
            }),
            Address::Unified(unified_address) => {
                if unified_address.orchard().is_some() {
                    standard.push(RoutedPayment {
                        payment: payment.clone(),
                        pool: orchard_family_pool,
                        transparent_output_size: None,
                    });
                } else if unified_address.sapling().is_some() {
                    standard.push(RoutedPayment {
                        payment: payment.clone(),
                        pool: PoolType::SAPLING,
                        transparent_output_size: None,
                    });
                } else if let Some(transparent_address) = unified_address.transparent() {
                    standard.push(RoutedPayment {
                        payment: payment.clone(),
                        pool: PoolType::TRANSPARENT,
                        transparent_output_size: Some(transparent_output_size(transparent_address)),
                    });
                } else {
                    return Err(PlanError::UnsupportedAddress(index));
                }
            }
        }
    }

    Ok(RoutedRequest { standard, tex })
}

fn transparent_output_size(address: &TransparentAddress) -> usize {
    match address {
        TransparentAddress::PublicKeyHash(_) => 34,
        TransparentAddress::ScriptHash(_) => 32,
    }
}

/// The change pool selection, mirroring the upstream single-output
/// strategy: the pool with spend flows wins in Orchard → Ironwood →
/// Sapling order, the fallback covers a fully-transparent transaction,
/// and after Ironwood activation the ZIP 318 Turnstile reroutes Orchard
/// change to Ironwood unless the transaction spends strictly more
/// Orchard value than the change would return.
#[allow(clippy::too_many_arguments)]
fn select_change_pool(
    sapling_flows: bool,
    orchard_flows: bool,
    orchard_input_value: Zatoshis,
    ironwood_flows: bool,
    fallback: ShieldedPool,
    ironwood_active: bool,
    change_upper_bound: Zatoshis,
) -> ShieldedPool {
    let base = if orchard_flows {
        ShieldedPool::Orchard
    } else if ironwood_flows {
        ShieldedPool::Ironwood
    } else if sapling_flows {
        ShieldedPool::Sapling
    } else {
        fallback
    };

    if ironwood_active && base == ShieldedPool::Orchard && orchard_input_value <= change_upper_bound
    {
        ShieldedPool::Ironwood
    } else {
        base
    }
}

/// The inputs a planning iteration actually uses, after trimming the
/// selected notes to the pools needed to cover the requirement.
#[derive(Default)]
struct TrimmedInputs {
    sapling: Vec<ShieldedInput>,
    orchard: Vec<ShieldedInput>,
    ironwood: Vec<ShieldedInput>,
}

impl TrimmedInputs {
    fn total(&self) -> Result<Zatoshis, PlanError> {
        [&self.sapling, &self.orchard, &self.ironwood]
            .into_iter()
            .flatten()
            .try_fold(Zatoshis::ZERO, |acc, input| {
                (acc + input.value()).ok_or(PlanError::Balance(BalanceError::Overflow))
            })
    }

    fn all(self) -> Vec<ShieldedInput> {
        let mut all = self.sapling;
        all.extend(self.orchard);
        all.extend(self.ironwood);
        all
    }
}

/// Per-pool selected values in preference order, trimmed so only the
/// pools needed to cover `amount_required` contribute inputs.
fn trim_to_required_pools(
    selected: &BTreeMap<ShieldedPool, Vec<ShieldedInput>>,
    pool_preference: &[ShieldedPool],
    amount_required: Zatoshis,
) -> Result<TrimmedInputs, PlanError> {
    let pool_value = |pool: &ShieldedPool| -> Result<Zatoshis, PlanError> {
        selected
            .get(pool)
            .into_iter()
            .flatten()
            .try_fold(Zatoshis::ZERO, |acc, input| {
                (acc + input.value()).ok_or(PlanError::Balance(BalanceError::Overflow))
            })
    };

    // A single pool that covers the requirement is used alone.
    let mut required_pools = Vec::new();
    if let Some(single) = pool_preference
        .iter()
        .find(|pool| matches!(pool_value(pool), Ok(value) if value >= amount_required))
    {
        required_pools.push(*single);
    } else {
        let mut running_total = Zatoshis::ZERO;
        for pool in pool_preference {
            required_pools.push(*pool);
            running_total = (running_total + pool_value(pool)?).ok_or(BalanceError::Overflow)?;
            if running_total >= amount_required {
                break;
            }
        }
    }

    let mut trimmed = TrimmedInputs::default();
    for pool in required_pools {
        let inputs = selected.get(&pool).cloned().unwrap_or_default();
        match pool {
            ShieldedPool::Sapling => trimmed.sapling = inputs,
            ShieldedPool::Orchard => trimmed.orchard = inputs,
            ShieldedPool::Ironwood => trimmed.ironwood = inputs,
        }
    }
    Ok(trimmed)
}

/// The outcome of one balance computation: either a funded step balance
/// or the requirement the next selection round must cover.
enum BalanceOutcome {
    Funded {
        change: Option<ChangeValue>,
        fee: Zatoshis,
    },
    Insufficient {
        required: Zatoshis,
    },
}

/// One step's balance under the single-output change strategy with
/// `AllowDustChange`: change always lands in a single shielded output
/// (dust included), and the fee is the larger of the no-change and
/// with-change shapes.
#[allow(clippy::too_many_arguments)]
fn compute_step_balance(
    wallet: &LightWallet,
    target_height: BlockHeight,
    ironwood_active: bool,
    inputs: &TrimmedInputs,
    payment_shape: &TransactionShape,
    subtotal_out: Zatoshis,
    change_memo: Option<&MemoBytes>,
    fallback_change_pool: ShieldedPool,
) -> Result<BalanceOutcome, PlanError> {
    let total_in = inputs.total()?;

    let mut shape = payment_shape.clone();
    shape.sapling.0 = inputs.sapling.len();
    shape.orchard.0 = inputs.orchard.len();
    shape.ironwood.0 = inputs.ironwood.len();

    let min_fee = shape.fee(&wallet.chain_type, target_height)?;
    let required_with_min_fee = (subtotal_out + min_fee).ok_or(BalanceError::Overflow)?;
    if total_in < required_with_min_fee {
        return Ok(BalanceOutcome::Insufficient {
            required: required_with_min_fee,
        });
    }

    let change_upper_bound = (total_in - required_with_min_fee).expect("compared above");
    let change_pool = select_change_pool(
        !inputs.sapling.is_empty() || shape.sapling.1 > 0,
        !inputs.orchard.is_empty() || shape.orchard.1 > 0,
        [&inputs.orchard]
            .into_iter()
            .flatten()
            .try_fold(Zatoshis::ZERO, |acc, input| acc + input.value())
            .ok_or(BalanceError::Overflow)?,
        !inputs.ironwood.is_empty() || shape.ironwood.1 > 0,
        fallback_change_pool,
        ironwood_active,
        change_upper_bound,
    );

    let mut shape_with_change = shape.clone();
    match change_pool {
        ShieldedPool::Sapling => shape_with_change.sapling.1 += 1,
        ShieldedPool::Orchard => shape_with_change.orchard.1 += 1,
        ShieldedPool::Ironwood => shape_with_change.ironwood.1 += 1,
    }
    let fee = min_fee.max(shape_with_change.fee(&wallet.chain_type, target_height)?);

    let required = (subtotal_out + fee).ok_or(BalanceError::Overflow)?;
    let Some(change_value) = total_in - required else {
        return Ok(BalanceOutcome::Insufficient { required });
    };

    // AllowDustChange: the change output is emitted however small; a
    // zero-valued change output survives only to carry a memo.
    let change = if change_value > Zatoshis::ZERO || change_memo.is_some() {
        Some(ChangeValue::from_parts(
            PoolType::Shielded(change_pool),
            change_value,
            change_memo.cloned(),
        ))
    } else {
        None
    };

    Ok(BalanceOutcome::Funded { change, fee })
}

/// Plans a transfer: a pure map from the wallet's current view and a
/// payment request to a proposal. OP_RETURN Data, if given, rides the
/// final transaction — the exposure step of a TEX flow, or the single
/// step otherwise.
pub fn plan_transfer(
    wallet: &LightWallet,
    request: TransactionRequest,
    account_id: zip32::AccountId,
    op_return_data: Option<OpReturnData>,
) -> Result<Proposal, PlanError> {
    let (target_height, anchor_height) = wallet
        .target_and_anchor_heights(wallet.wallet_settings.min_confirmations)
        .ok_or(PlanError::SyncRequired)?;
    let ironwood_active = wallet
        .chain_type
        .activation_height(NetworkUpgrade::Nu6_3)
        .is_some_and(|activation| target_height >= activation);
    let fallback_change_pool = if ironwood_active {
        ShieldedPool::Ironwood
    } else {
        ShieldedPool::Orchard
    };

    let routed = route_request(&request, wallet, ironwood_active)?;
    let change_memo = wallet.change_memo_from_transaction_request(&request);

    // The exposure step's fee and the ephemeral output that funds it are
    // fixed by the TEX payments alone, so they are computed once, up
    // front. OP_RETURN Data rides the exposure step when one exists.
    let exposure = if routed.tex.is_empty() {
        None
    } else {
        let mut exposure_shape = TransactionShape::default().with_ephemeral_input();
        exposure_shape.transparent_output_sizes = routed
            .tex
            .iter()
            .filter_map(|payment| payment.transparent_output_size)
            .collect();
        if let Some(data) = &op_return_data {
            exposure_shape
                .transparent_output_sizes
                .push(op_return_output_size(data.as_bytes().len()));
        }
        let exposure_fee = exposure_shape.fee(&wallet.chain_type, target_height)?;
        let ephemeral_value = (routed.tex_total()? + exposure_fee).ok_or(BalanceError::Overflow)?;
        Some((exposure_fee, ephemeral_value))
    };

    // The shielding/single step's fixed outputs: the standard payments,
    // plus the ephemeral output when a TEX flow follows, plus OP_RETURN
    // Data when this is the final (only) step.
    let mut payment_shape = TransactionShape::default();
    for routed_payment in &routed.standard {
        match routed_payment.pool {
            PoolType::Transparent => payment_shape.transparent_output_sizes.push(
                routed_payment
                    .transparent_output_size
                    .expect("transparent payments carry a size"),
            ),
            PoolType::Shielded(ShieldedPool::Sapling) => payment_shape.sapling.1 += 1,
            PoolType::Shielded(ShieldedPool::Orchard) => payment_shape.orchard.1 += 1,
            PoolType::Shielded(ShieldedPool::Ironwood) => payment_shape.ironwood.1 += 1,
        }
    }
    let mut subtotal_out = routed.standard_total()?;
    if let Some((_, ephemeral_value)) = exposure {
        payment_shape = payment_shape.with_ephemeral_output();
        subtotal_out = (subtotal_out + ephemeral_value).ok_or(BalanceError::Overflow)?;
    } else if let Some(data) = &op_return_data {
        payment_shape
            .transparent_output_sizes
            .push(op_return_output_size(data.as_bytes().len()));
    }

    // Pool preference: the orchard family leads when an orchard-family
    // payment exists; a legacy-Orchard pool trails otherwise.
    let prefer_orchard_family = payment_shape.orchard.1 > 0 || payment_shape.ironwood.1 > 0;
    let pool_preference: Vec<ShieldedPool> = if prefer_orchard_family {
        if ironwood_active {
            vec![
                ShieldedPool::Ironwood,
                ShieldedPool::Orchard,
                ShieldedPool::Sapling,
            ]
        } else {
            vec![ShieldedPool::Orchard, ShieldedPool::Sapling]
        }
    } else if ironwood_active {
        vec![
            ShieldedPool::Sapling,
            ShieldedPool::Ironwood,
            ShieldedPool::Orchard,
        ]
    } else {
        vec![ShieldedPool::Sapling, ShieldedPool::Orchard]
    };

    // The selection loop: compute the balance, learn the requirement,
    // re-select; stop when funded or when selection stops growing.
    let mut selected: BTreeMap<ShieldedPool, Vec<ShieldedInput>> = BTreeMap::new();
    let mut amount_required = Zatoshis::ZERO;
    let mut prior_available = Zatoshis::ZERO;
    loop {
        let trimmed = trim_to_required_pools(&selected, &pool_preference, amount_required)?;
        match compute_step_balance(
            wallet,
            target_height,
            ironwood_active,
            &trimmed,
            &payment_shape,
            subtotal_out,
            Some(&change_memo),
            fallback_change_pool,
        )? {
            BalanceOutcome::Funded { change, fee } => {
                return build_transfer_proposal(
                    account_id,
                    target_height,
                    anchor_height,
                    routed,
                    trimmed,
                    change,
                    fee,
                    exposure,
                    op_return_data,
                );
            }
            BalanceOutcome::Insufficient { required } => {
                amount_required = required;
            }
        }

        selected = wallet.select_spendable_shielded_inputs(
            account_id,
            amount_required,
            &pool_preference,
            anchor_height,
        )?;
        let new_available = selected
            .values()
            .flatten()
            .try_fold(Zatoshis::ZERO, |acc, input| acc + input.value())
            .ok_or(BalanceError::Overflow)?;
        if new_available <= prior_available {
            return Err(PlanError::InsufficientFunds {
                available: new_available,
                required: amount_required,
            });
        }
        prior_available = new_available;
    }
}

impl LightWallet {
    /// The target height (one above the wallet's chain view) and the
    /// spend anchor at the given minimum confirmations, bounded by the
    /// highest checkpoint. `None` when the wallet has no synced view.
    pub(crate) fn target_and_anchor_heights(
        &self,
        min_confirmations: std::num::NonZeroU32,
    ) -> Option<(BlockHeight, BlockHeight)> {
        use shardtree::store::ShardStore as _;

        let target_height = self.sync_state.last_known_chain_height()? + 1;
        let max_checkpoint_height = self
            .shard_trees
            .sapling
            .store()
            .max_checkpoint_id()
            .expect("infallible")
            .expect("should be at least 1 checkpoint");
        let anchor_height = std::cmp::min(
            max_checkpoint_height,
            target_height - min_confirmations.get(),
        );
        Some((target_height, std::cmp::max(1.into(), anchor_height)))
    }

    /// Greedily selects spendable shielded notes across the source pools
    /// until `at_least` is covered, guaranteed-unspent notes first, the
    /// requested pools first, with notes soft-reserved for pending
    /// migration parts withheld until nothing else can satisfy the
    /// request.
    pub(crate) fn select_spendable_shielded_inputs(
        &self,
        account: zip32::AccountId,
        at_least: Zatoshis,
        sources: &[ShieldedPool],
        anchor_height: BlockHeight,
    ) -> Result<BTreeMap<ShieldedPool, Vec<ShieldedInput>>, WalletError> {
        use pepper_sync::wallet::{IronwoodNote, OrchardNote, OutputId, SaplingNote};

        use crate::wallet::output::{OutputRef, RemainingNeeded};

        let mut exclude_sapling: Vec<OutputId> = Vec::new();
        let mut exclude_orchard: Vec<OutputId> = Vec::new();
        let mut exclude_ironwood: Vec<OutputId> = Vec::new();

        // Soft reservation: notes bound to pending migration parts are
        // withheld from ordinary selection first, and offered again only
        // if the request cannot be satisfied without them. The
        // reservation biases selection and never blocks a spend.
        let reserved_orchard: Vec<OutputId> = self
            .migration
            .as_ref()
            .map(|migration| migration.reserved_output_ids().into_iter().collect())
            .unwrap_or_default();

        let mut remaining_value_needed = RemainingNeeded::Positive(at_least);

        // prioritises selecting spendable notes that are guaranteed to be
        // unspent first
        let mut selected_sapling: Vec<(OutputId, u64)> = Vec::new();
        let mut selected_orchard: Vec<(OutputId, u64)> = Vec::new();
        let mut selected_ironwood: Vec<(OutputId, u64)> = Vec::new();
        exclude_orchard.extend(reserved_orchard.iter().copied());
        for withhold_reserved in [true, false] {
            if !withhold_reserved {
                let unmet = matches!(
                    remaining_value_needed,
                    RemainingNeeded::Positive(value) if value.into_u64() > 0
                );
                if reserved_orchard.is_empty() || !unmet {
                    break;
                }
                exclude_orchard.retain(|output_id| !reserved_orchard.contains(output_id));
            }
            for include_potentially_spent_notes in [false, true] {
                let mut select_pool = |pool: ShieldedPool,
                                       remaining: &mut RemainingNeeded|
                 -> Result<(), WalletError> {
                    match pool {
                        ShieldedPool::Sapling => {
                            let notes: Vec<(OutputId, u64)> = self
                                .select_spendable_notes_by_pool::<SaplingNote>(
                                    remaining,
                                    anchor_height,
                                    &exclude_sapling,
                                    account,
                                    include_potentially_spent_notes,
                                )?
                                .into_iter()
                                .map(|note| (note.output_id(), note.value()))
                                .collect();
                            exclude_sapling.extend(notes.iter().map(|(id, _)| *id));
                            selected_sapling.extend(notes);
                        }
                        ShieldedPool::Orchard => {
                            let notes: Vec<(OutputId, u64)> = self
                                .select_spendable_notes_by_pool::<OrchardNote>(
                                    remaining,
                                    anchor_height,
                                    &exclude_orchard,
                                    account,
                                    include_potentially_spent_notes,
                                )?
                                .into_iter()
                                .map(|note| (note.output_id(), note.value()))
                                .collect();
                            exclude_orchard.extend(notes.iter().map(|(id, _)| *id));
                            selected_orchard.extend(notes);
                        }
                        ShieldedPool::Ironwood => {
                            let notes: Vec<(OutputId, u64)> = self
                                .select_spendable_notes_by_pool::<IronwoodNote>(
                                    remaining,
                                    anchor_height,
                                    &exclude_ironwood,
                                    account,
                                    include_potentially_spent_notes,
                                )?
                                .into_iter()
                                .map(|note| (note.output_id(), note.value()))
                                .collect();
                            exclude_ironwood.extend(notes.iter().map(|(id, _)| *id));
                            selected_ironwood.extend(notes);
                        }
                    }
                    Ok(())
                };

                // prioritise note selection for the given `sources`, then
                // fall through to every pool
                for pool in sources {
                    select_pool(*pool, &mut remaining_value_needed)?;
                }
                for pool in [
                    ShieldedPool::Sapling,
                    ShieldedPool::Orchard,
                    ShieldedPool::Ironwood,
                ] {
                    select_pool(pool, &mut remaining_value_needed)?;
                }
            }
        }

        let to_inputs = |selected: Vec<(OutputId, u64)>, pool: ShieldedPool| {
            selected
                .into_iter()
                .map(|(output_id, value)| {
                    ShieldedInput::from_parts(
                        OutputRef::new(output_id, PoolType::Shielded(pool)),
                        Zatoshis::from_u64(value).expect("note values are valid"),
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut by_pool = BTreeMap::new();
        by_pool.insert(
            ShieldedPool::Sapling,
            to_inputs(selected_sapling, ShieldedPool::Sapling),
        );
        by_pool.insert(
            ShieldedPool::Orchard,
            to_inputs(selected_orchard, ShieldedPool::Orchard),
        );
        by_pool.insert(
            ShieldedPool::Ironwood,
            to_inputs(selected_ironwood, ShieldedPool::Ironwood),
        );
        Ok(by_pool)
    }
}

#[allow(clippy::too_many_arguments)]
fn build_transfer_proposal(
    account_id: zip32::AccountId,
    target_height: BlockHeight,
    anchor_height: BlockHeight,
    routed: RoutedRequest,
    inputs: TrimmedInputs,
    change: Option<ChangeValue>,
    fee: Zatoshis,
    exposure: Option<(Zatoshis, Zatoshis)>,
    op_return_data: Option<OpReturnData>,
) -> Result<Proposal, PlanError> {
    let step_request = |payments: &[RoutedPayment]| -> TransactionRequest {
        if payments.is_empty() {
            TransactionRequest::empty()
        } else {
            TransactionRequest::new(payments.iter().map(|p| p.payment.clone()).collect())
                .expect("payments were parsed from a valid request")
        }
    };
    let step_pools = |payments: &[RoutedPayment]| -> BTreeMap<usize, PoolType> {
        payments
            .iter()
            .enumerate()
            .map(|(index, payment)| (index, payment.pool))
            .collect()
    };

    if let Some((exposure_fee, _ephemeral_value)) = exposure {
        let shielding_step = Step::from_parts(
            step_request(&routed.standard),
            step_pools(&routed.standard),
            inputs.all(),
            vec![],
            change.into_iter().collect(),
            fee,
            None,
        );
        let exposure_step = Step::from_parts(
            step_request(&routed.tex),
            step_pools(&routed.tex),
            vec![],
            // The exposure step's sole input is the shielding step's
            // ephemeral output, which has no wallet OutputId until the
            // shielding transaction exists; the variant models it.
            vec![],
            vec![],
            exposure_fee,
            op_return_data,
        );
        Ok(Proposal::TexTransfer(
            TexTransferProposal::new(
                account_id,
                target_height,
                anchor_height,
                shielding_step,
                exposure_step,
            )
            .expect("OP_RETURN Data was placed on the exposure step only"),
        ))
    } else {
        Ok(Proposal::Transfer(TransferProposal::new(
            account_id,
            target_height,
            anchor_height,
            Step::from_parts(
                step_request(&routed.standard),
                step_pools(&routed.standard),
                inputs.all(),
                vec![],
                change.into_iter().collect(),
                fee,
                op_return_data,
            ),
        )))
    }
}

/// Plans a shielding: every economic transparent coin moves to a single
/// shielded change output, provided the shielded amount clears the
/// threshold. Pure, like [`plan_transfer`].
pub fn plan_shield(
    wallet: &LightWallet,
    account_id: zip32::AccountId,
) -> Result<Proposal, PlanError> {
    let (target_height, anchor_height) = wallet
        .target_and_anchor_heights(wallet.wallet_settings.min_confirmations)
        .ok_or(PlanError::SyncRequired)?;
    let ironwood_active = wallet
        .chain_type
        .activation_height(NetworkUpgrade::Nu6_3)
        .is_some_and(|activation| target_height >= activation);

    // Gather: every spendable coin, largest first, uneconomic coins
    // (value at or below the marginal fee, which every 150-byte input
    // costs) dropped — the planner's closed form of upstream's
    // dust-input retry.
    let mut coins: Vec<TransparentInput> = wallet
        .spendable_transparent_coins(target_height, false, false)
        .into_iter()
        .filter(|coin| coin.value() > 5_000)
        .map(|coin| {
            TransparentInput::from_parts(
                coin.output_id(),
                Zatoshis::from_u64(coin.value()).expect("coin values are valid"),
            )
        })
        .collect();
    coins.sort_by(|a, b| b.value().cmp(&a.value()).then(a.coin().cmp(&b.coin())));

    let total_in = coins
        .iter()
        .try_fold(Zatoshis::ZERO, |acc, coin| acc + coin.value())
        .ok_or(BalanceError::Overflow)?;

    // The shield's one output is the shielded change; its pool is the
    // fallback, rerouted by the Turnstile after Ironwood activation.
    let change_pool = select_change_pool(
        false,
        false,
        Zatoshis::ZERO,
        false,
        ShieldedPool::Orchard,
        ironwood_active,
        total_in,
    );

    let mut shape = TransactionShape {
        transparent_input_sizes: vec![InputSize::Known(P2PKH_INPUT_SIZE); coins.len()],
        ..Default::default()
    };
    match change_pool {
        ShieldedPool::Sapling => shape.sapling.1 = 1,
        ShieldedPool::Orchard => shape.orchard.1 = 1,
        ShieldedPool::Ironwood => shape.ironwood.1 = 1,
    }
    let fee = shape.fee(&wallet.chain_type, target_height)?;

    let shielded_amount = (total_in - fee).unwrap_or(Zatoshis::ZERO);
    if shielded_amount < SHIELDING_THRESHOLD {
        return Err(PlanError::InsufficientFunds {
            available: shielded_amount,
            required: SHIELDING_THRESHOLD,
        });
    }

    let step = Step::from_parts(
        TransactionRequest::empty(),
        BTreeMap::new(),
        vec![],
        coins,
        vec![ChangeValue::from_parts(
            PoolType::Shielded(change_pool),
            shielded_amount,
            None,
        )],
        fee,
        None,
    );

    Ok(Proposal::Shield(
        ShieldProposal::new(account_id, target_height, anchor_height, step)
            .expect("a shield step built here carries no OP_RETURN Data or shielded inputs"),
    ))
}

#[cfg(test)]
mod tests {
    //! Invariant tests, the survivors of the migration's comparative
    //! equivalence suite (which died with the old zcb path at the P5
    //! cutover, having proven the planner fee-for-fee equal to
    //! `propose_transfer`/`propose_shielding` on every scenario class).

    use zcash_protocol::value::Zatoshis;

    use super::plan_transfer;
    use crate::testutils::lightclient::from_inputs::transaction_request_from_send_inputs;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::keys::unified::ReceiverSelection;
    use crate::wallet::spend::op_return::OpReturnData;

    /// OP_RETURN Data is fee-counted by its real serialized size: a
    /// 92-byte null-data output on a purely shielded send adds exactly
    /// ceil(92/34) = 3 marginal fees.
    #[test]
    fn op_return_data_is_priced_into_the_fee() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(500_000)
                .build();
        let (_, address) = wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
            .unwrap();
        let address = address.encode(&wallet.chain_type);
        let request = transaction_request_from_send_inputs(vec![(address.as_str(), 100_000, None)])
            .expect("valid send inputs form a request");

        let without = plan_transfer(&wallet, request.clone(), zip32::AccountId::ZERO, None)
            .expect("plans without data");
        let with = plan_transfer(
            &wallet,
            request,
            zip32::AccountId::ZERO,
            Some(OpReturnData::new(vec![0xAB; 80]).expect("80 bytes is within the limit")),
        )
        .expect("plans with data");

        assert_eq!(with.op_return_data().unwrap().as_bytes(), &[0xAB; 80]);
        assert_eq!(
            (with.final_step().fee() - without.final_step().fee()).unwrap(),
            Zatoshis::const_from_u64(15_000),
        );
    }

    /// A TEX flow's ephemeral output funds the exposure step exactly:
    /// its value is the TEX payments plus the exposure fee, so the
    /// exposure step balances to zero change.
    #[test]
    fn tex_ephemeral_output_funds_the_exposure_step_exactly() {
        use pepper_sync::keys::decode_address;
        use zcash_keys::address::Address;
        use zcash_transparent::address::TransparentAddress;
        use zip321::{Payment, TransactionRequest};

        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(5_000_000)
            .build();
        let external =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let taddr = external
            .transparent_addresses()
            .values()
            .next()
            .unwrap()
            .clone();
        let Address::Transparent(TransparentAddress::PublicKeyHash(taddr_bytes)) =
            decode_address(&external.chain_type(), &taddr).unwrap()
        else {
            panic!("a wallet-generated first taddr is p2pkh")
        };
        let tex_address =
            crate::testutils::interpret_taddr_as_tex_addr(taddr_bytes, &external.chain_type());
        let request = TransactionRequest::new(vec![Payment::without_memo(
            zcash_address::ZcashAddress::try_from_encoded(&tex_address).unwrap(),
            Zatoshis::const_from_u64(100_000),
        )])
        .unwrap();

        let proposal = plan_transfer(&wallet, request, zip32::AccountId::ZERO, None)
            .expect("planner plans the TEX flow");
        let super::Proposal::TexTransfer(tex) = &proposal else {
            panic!("a TEX payment plans a TexTransfer");
        };

        assert_eq!(
            tex.ephemeral_value().unwrap(),
            (tex.exposure().payment_total().unwrap() + tex.exposure().fee()).unwrap(),
        );
        assert!(tex.exposure().change().is_empty(), "no exposure change");
    }
}
