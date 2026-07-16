//! The pure plan layer of the in-tree spend pipeline (ADR 0010).
//!
//! [`plan_transfer`] and [`plan_shield`] map a read-only wallet view and a
//! payment request to an owned [`Proposal`] — note selection, change,
//! ZIP 317 fee sizing, and ZIP 320 TEX step-splitting — with no wallet
//! mutation. Fee arithmetic stays upstream ([`super::fee`]); this module
//! decides only what the fee rule is fed.
//!
//! During the migration the wallet queries go through the same
//! `zcash_client_backend` trait impls the old path uses, so the
//! equivalence tests at the bottom compare this planner against
//! `propose_transfer`/`propose_shielding` input-for-input and
//! fee-for-fee. The P5 cutover inlines those query bodies and deletes
//! the trait impls.

use std::collections::BTreeMap;

use zcash_client_backend::data_api::wallet::ConfirmationsPolicy;
use zcash_client_backend::data_api::{InputSource, TargetValue, WalletRead};
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
    let confirmations_policy =
        ConfirmationsPolicy::new_symmetrical(wallet.wallet_settings.min_confirmations, false);
    let (zcb_target_height, anchor_height) =
        WalletRead::get_target_and_anchor_heights(wallet, confirmations_policy.trusted())?
            .ok_or(PlanError::SyncRequired)?;
    let target_height = BlockHeight::from(zcb_target_height);
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

        let received = InputSource::select_spendable_notes(
            wallet,
            account_id,
            TargetValue::AtLeast(amount_required),
            &pool_preference,
            zcb_target_height,
            confirmations_policy,
            &[],
        )?;
        selected = received_notes_by_pool(&received);
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

fn received_notes_by_pool(
    received: &zcash_client_backend::data_api::ReceivedNotes<crate::wallet::output::OutputRef>,
) -> BTreeMap<ShieldedPool, Vec<ShieldedInput>> {
    let mut by_pool: BTreeMap<ShieldedPool, Vec<ShieldedInput>> = BTreeMap::new();
    by_pool.insert(
        ShieldedPool::Sapling,
        received
            .sapling()
            .iter()
            .map(|note| {
                ShieldedInput::from_parts(
                    note.internal_note_id().clone(),
                    Zatoshis::from_u64(note.note().value().inner()).expect("note values are valid"),
                )
            })
            .collect(),
    );
    by_pool.insert(
        ShieldedPool::Orchard,
        received
            .orchard()
            .iter()
            .map(|note| {
                ShieldedInput::from_parts(
                    note.internal_note_id().clone(),
                    Zatoshis::from_u64(note.note().value().inner()).expect("note values are valid"),
                )
            })
            .collect(),
    );
    by_pool.insert(
        ShieldedPool::Ironwood,
        received
            .ironwood()
            .iter()
            .map(|note| {
                ShieldedInput::from_parts(
                    note.internal_note_id().clone(),
                    Zatoshis::from_u64(note.note().value().inner()).expect("note values are valid"),
                )
            })
            .collect(),
    );
    by_pool
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
    let confirmations_policy =
        ConfirmationsPolicy::new_symmetrical(wallet.wallet_settings.min_confirmations, false);
    let (zcb_target_height, anchor_height) =
        WalletRead::get_target_and_anchor_heights(wallet, confirmations_policy.trusted())?
            .ok_or(PlanError::SyncRequired)?;
    let target_height = BlockHeight::from(zcb_target_height);
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
mod equivalence {
    //! Migration scaffolding (deleted at the P5 cutover): the planner
    //! must match `zcash_client_backend`'s proposals input-for-input and
    //! fee-for-fee on the same wallet state.

    use std::collections::BTreeSet;

    use zcash_protocol::value::Zatoshis;

    use super::{plan_shield, plan_transfer};
    use crate::data::proposal::ProportionalFeeProposal;
    use crate::testutils::lightclient::from_inputs::transaction_request_from_send_inputs;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;
    use crate::wallet::keys::unified::ReceiverSelection;
    use crate::wallet::spend::proposal::Proposal;

    /// Asserts our proposal and zcb's agree on step count, per-step fee,
    /// non-ephemeral change (value and pool), and selected shielded
    /// inputs.
    fn assert_equivalent(ours: &Proposal, theirs: &ProportionalFeeProposal) {
        let our_steps = ours.steps();
        assert_eq!(our_steps.len(), theirs.steps().len(), "step count");

        for (index, (our_step, their_step)) in
            our_steps.iter().zip(theirs.steps().iter()).enumerate()
        {
            assert_eq!(
                our_step.fee(),
                their_step.balance().fee_required(),
                "fee of step {index}"
            );

            let their_change: Vec<_> = their_step
                .balance()
                .proposed_change()
                .iter()
                .filter(|change| !change.is_ephemeral())
                .collect();
            assert_eq!(
                our_step.change().len(),
                their_change.len(),
                "change count of step {index}"
            );
            for (our_change, their_change) in our_step.change().iter().zip(their_change) {
                assert_eq!(
                    our_change.value(),
                    their_change.value(),
                    "change value of step {index}"
                );
                assert_eq!(
                    our_change.pool(),
                    their_change.output_pool(),
                    "change pool of step {index}"
                );
            }

            let our_inputs: BTreeSet<_> = our_step
                .shielded_inputs()
                .iter()
                .map(|input| input.note().output_id())
                .collect();
            let their_inputs: BTreeSet<_> = their_step
                .shielded_inputs()
                .map(|inputs| {
                    inputs
                        .notes()
                        .iter()
                        .map(|note| note.internal_note_id().output_id())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(our_inputs, their_inputs, "inputs of step {index}");
        }
    }

    fn assert_transfer_equivalent(
        wallet: &mut LightWallet,
        receivers: Vec<(&str, u64, Option<&str>)>,
    ) {
        let request = transaction_request_from_send_inputs(receivers)
            .expect("valid send inputs form a request");
        let ours = plan_transfer(wallet, request.clone(), zip32::AccountId::ZERO, None)
            .expect("planner plans what zcb proposes");
        let theirs = wallet
            .create_send_proposal(request, zip32::AccountId::ZERO)
            .expect("zcb proposes what the planner plans");
        assert_equivalent(&ours, &theirs);
    }

    #[test]
    fn orchard_to_orchard_only_ua() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(200_000)
                .build();
        let (_, address) = wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
            .unwrap();
        let address = address.encode(&wallet.chain_type);

        assert_transfer_equivalent(&mut wallet, vec![(address.as_str(), 100_000, None)]);
    }

    #[test]
    fn orchard_to_sapling_cross_pool_with_memo() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(100_000)
                .orchard_note(50_000)
                .build();
        let mut external =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
        let (_, sapling_destination) = external
            .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
            .unwrap();
        let sapling_destination = sapling_destination.encode(&external.chain_type);

        assert_transfer_equivalent(
            &mut wallet,
            vec![(sapling_destination.as_str(), 10_000, Some("hello"))],
        );
    }

    #[test]
    fn multi_payment_multi_pool() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(500_000)
                .sapling_note(300_000)
                .build();
        let (_, orchard_address) = wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
            .unwrap();
        let (_, sapling_address) = wallet
            .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
            .unwrap();
        let orchard_address = orchard_address.encode(&wallet.chain_type);
        let sapling_address = sapling_address.encode(&wallet.chain_type);

        assert_transfer_equivalent(
            &mut wallet,
            vec![
                (orchard_address.as_str(), 100_000, None),
                (sapling_address.as_str(), 100_000, Some("crossing")),
            ],
        );
    }

    #[test]
    fn multiple_notes_selected_when_one_cannot_cover() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(60_000)
                .orchard_note(60_000)
                .orchard_note(60_000)
                .build();
        let (_, address) = wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
            .unwrap();
        let address = address.encode(&wallet.chain_type);

        assert_transfer_equivalent(&mut wallet, vec![(address.as_str(), 150_000, None)]);
    }

    /// Ironwood spends pin the V6 bundle's cross-address action count:
    /// three spends against two outputs (payment and change) must count
    /// as max(3, 2) actions, not 3 + 2.
    #[test]
    fn ironwood_notes_multi_selection() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .ironwood_note(60_000)
                .ironwood_note(60_000)
                .ironwood_note(60_000)
                .build();
        let (_, address) = wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
            .unwrap();
        let address = address.encode(&wallet.chain_type);

        assert_transfer_equivalent(&mut wallet, vec![(address.as_str(), 150_000, None)]);
    }

    #[test]
    fn insufficient_funds_reports_matching_amounts() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(50_000)
                .build();
        let (_, address) = wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
            .unwrap();
        let address = address.encode(&wallet.chain_type);
        let request = transaction_request_from_send_inputs(vec![(address.as_str(), 100_000, None)])
            .expect("valid send inputs form a request");

        let ours = plan_transfer(&wallet, request.clone(), zip32::AccountId::ZERO, None)
            .expect_err("planner reports the shortfall");
        let theirs = wallet
            .create_send_proposal(request, zip32::AccountId::ZERO)
            .expect_err("zcb reports the shortfall");

        let super::PlanError::InsufficientFunds {
            available,
            required,
        } = ours
        else {
            panic!("expected InsufficientFunds, got {ours:?}");
        };
        let crate::wallet::error::ProposeSendError::Proposal(
            zcash_client_backend::data_api::error::Error::InsufficientFunds {
                available: their_available,
                required: their_required,
            },
        ) = theirs
        else {
            panic!("expected zcb InsufficientFunds, got {theirs:?}");
        };
        assert_eq!(available, their_available);
        assert_eq!(required, their_required);
    }

    /// The ZIP 320 two-step flow: step fees, the shielding step's inputs
    /// and non-ephemeral change, and the ephemeral output's value (TEX
    /// payments plus the exposure fee, which the shielding step funds in
    /// advance) must all match zcb's proposal.
    #[test]
    fn tex_two_step_equivalence() {
        use pepper_sync::keys::decode_address;
        use zcash_client_backend::address::Address;
        use zcash_transparent::address::TransparentAddress;
        use zip321::{Payment, TransactionRequest};

        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
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

        let ours = plan_transfer(&wallet, request.clone(), zip32::AccountId::ZERO, None)
            .expect("planner plans the TEX flow");
        let theirs = wallet
            .create_send_proposal(request, zip32::AccountId::ZERO)
            .expect("zcb proposes the TEX flow");
        assert_equivalent(&ours, &theirs);

        // The ephemeral output the shielding step funds must be worth the
        // TEX payments plus the exposure step's fee.
        let their_ephemeral: Vec<_> = theirs
            .steps()
            .first()
            .balance()
            .proposed_change()
            .iter()
            .filter(|change| change.is_ephemeral())
            .collect();
        assert_eq!(their_ephemeral.len(), 1, "one ephemeral output");
        let our_exposure = ours.final_step();
        assert_eq!(
            their_ephemeral[0].value(),
            (our_exposure.payment_total().unwrap() + our_exposure.fee()).unwrap(),
            "ephemeral output value",
        );
    }

    #[test]
    fn shield_equivalence() {
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .transparent_coin(80_000)
                .transparent_coin(30_000)
                .build();

        let ours =
            plan_shield(&wallet, zip32::AccountId::ZERO).expect("planner shields what zcb shields");
        let theirs = wallet
            .create_shield_proposal(zip32::AccountId::ZERO)
            .expect("zcb shields what the planner shields");

        let our_step = ours.final_step();
        let their_step = theirs.steps().first();
        assert_eq!(our_step.fee(), their_step.balance().fee_required(), "fee");
        let their_change = their_step.balance().proposed_change();
        assert_eq!(our_step.change().len(), their_change.len(), "change count");
        assert_eq!(
            our_step.change()[0].value(),
            their_change[0].value(),
            "shielded amount"
        );
        assert_eq!(
            our_step.change()[0].pool(),
            their_change[0].output_pool(),
            "change pool"
        );
        let our_coins: BTreeSet<_> = our_step
            .transparent_inputs()
            .iter()
            .map(|coin| coin.coin())
            .collect();
        let their_coins: BTreeSet<_> = their_step
            .transparent_inputs()
            .iter()
            .map(|utxo| {
                pepper_sync::wallet::OutputId::new(*utxo.outpoint().txid(), utxo.outpoint().n())
            })
            .collect();
        assert_eq!(our_coins, their_coins, "coins");
    }

    #[test]
    fn op_return_data_raises_the_fee_when_it_crosses_a_size_boundary() {
        use crate::wallet::spend::op_return::OpReturnData;

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

        // A purely shielded send has no transparent outputs; 80 bytes of
        // OP_RETURN Data adds a 92-byte output = ceil(92/34) = 3 logical
        // actions over the shielded baseline, so the fee strictly rises
        // by exactly those three marginal fees.
        assert_eq!(with.op_return_data().unwrap().as_bytes(), &[0xAB; 80]);
        assert_eq!(
            (with.final_step().fee() - without.final_step().fee()).unwrap(),
            Zatoshis::const_from_u64(15_000),
        );
    }
}
