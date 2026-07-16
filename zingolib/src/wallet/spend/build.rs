//! The wallet-pure build layer of the in-tree spend pipeline (ADR 0010).
//!
//! [`LightWallet::spend_materials`] is the edge: it resolves a proposal's
//! input references into self-contained spend materials — notes, scopes,
//! merkle witnesses, anchors, coins, and the derived (never reserved)
//! ephemeral Refund Address for a TEX flow. [`build_transactions`] is the
//! wallet-pure core: proposal + materials + keys + provers to signed
//! transactions, with no wallet access. Its internal randomness (proofs,
//! signature nonces) is accepted by design; the purity criterion is
//! freedom from wallet-state effects.
//!
//! Fee pinning comes from upstream: the builder recomputes the ZIP 317
//! fee from what was actually added and refuses to build unless the
//! transaction's value balance equals it, so a plan whose fee or change
//! were mis-sized cannot produce a transaction.

use nonempty::NonEmpty;
use rand::rngs::OsRng;

use sapling_crypto::prover::{OutputProver, SpendProver};
use zcash_client_backend::data_api::WalletCommitmentTrees;
use zcash_keys::address::Address;
use zcash_keys::keys::{UnifiedFullViewingKey, UnifiedSpendingKey};
use zcash_primitives::transaction::Transaction;
use zcash_primitives::transaction::builder::{BuildConfig, Builder};
use zcash_primitives::transaction::fees::zip317;
use zcash_protocol::consensus::{BlockHeight, NetworkUpgrade, Parameters};
use zcash_protocol::memo::MemoBytes;
use zcash_protocol::value::{BalanceError, Zatoshis};
use zcash_protocol::{PoolType, ShieldedPool};
use zcash_transparent::address::TransparentAddress;
use zcash_transparent::builder::TransparentSigningSet;
use zcash_transparent::bundle::{OutPoint, TxOut};
use zcash_transparent::keys::{NonHardenedChildIndex, TransparentKeyScope};

use pepper_sync::keys::KeyId;
use pepper_sync::wallet::{
    IronwoodNote, NoteInterface, OrchardNote, OutputId, OutputInterface, SaplingNote,
    TransparentCoin,
};

use super::proposal::{Proposal, Step};
use crate::wallet::LightWallet;
use crate::wallet::error::KeyError;

/// Ways building a proposal's transactions can fail.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// A proposal input has no matching note or coin in the wallet.
    #[error("no wallet output found for proposal input {0}")]
    InputNotFound(OutputId),
    /// A spent note has no commitment tree position.
    #[error("note {0} has no commitment tree position")]
    PositionMissing(OutputId),
    /// No tree checkpoint exists at the proposal's anchor height.
    #[error("no {pool:?} tree checkpoint at anchor height {height}")]
    AnchorNotFound {
        /// The pool whose tree lacks the checkpoint.
        pool: ShieldedPool,
        /// The proposal's anchor height.
        height: BlockHeight,
    },
    /// No witness could be computed for a spent note at the anchor.
    #[error("no witness for note {0} at the proposal's anchor")]
    WitnessNotFound(OutputId),
    /// A transparent coin's address is not in the wallet's address book.
    #[error("the address of coin {0} is not a wallet address")]
    CoinAddressNotFound(OutputId),
    /// A payment address lacks the receiver its chosen pool requires.
    #[error("the payment at index {0} lacks the receiver its pool requires")]
    ReceiverMissing(usize),
    /// The wallet lacks a key component the build requires.
    #[error("the wallet lacks a {0} key")]
    MissingKey(&'static str),
    /// Key derivation failed.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// Amounts overflowed.
    #[error("balance overflow")]
    Balance(#[from] BalanceError),
    /// The shard tree failed to answer an anchor or witness query.
    #[error("shard tree error: {0}")]
    ShardTree(String),
    /// The upstream transaction builder refused the transaction.
    #[error("transaction builder error: {0}")]
    Builder(String),
}

impl From<shardtree::error::ShardTreeError<std::convert::Infallible>> for BuildError {
    fn from(error: shardtree::error::ShardTreeError<std::convert::Infallible>) -> Self {
        BuildError::ShardTree(format!("{error:?}"))
    }
}

/// One sapling spend, self-contained.
pub struct SaplingSpendMaterial {
    scope: zip32::Scope,
    note: sapling_crypto::Note,
    merkle_path: sapling_crypto::MerklePath,
}

/// One orchard-family (Orchard or Ironwood) spend, self-contained.
pub struct OrchardSpendMaterial {
    note: orchard::Note,
    merkle_path: orchard::tree::MerklePath,
}

/// One transparent coin spend, self-contained: the outpoint and coin to
/// commit, and the derivation coordinates of the key that signs it.
pub struct TransparentSpendMaterial {
    scope: TransparentKeyScope,
    address_index: NonHardenedChildIndex,
    outpoint: OutPoint,
    coin: TxOut,
}

/// Everything one step's build needs beyond the proposal itself.
#[derive(Default)]
pub struct StepMaterials {
    sapling_anchor: Option<sapling_crypto::Anchor>,
    orchard_anchor: Option<orchard::Anchor>,
    ironwood_anchor: Option<orchard::Anchor>,
    sapling_spends: Vec<SaplingSpendMaterial>,
    orchard_spends: Vec<OrchardSpendMaterial>,
    ironwood_spends: Vec<OrchardSpendMaterial>,
    transparent_spends: Vec<TransparentSpendMaterial>,
}

/// The materials for a whole proposal: one [`StepMaterials`] per step,
/// plus the derived (not reserved) ephemeral Refund Address of a TEX
/// flow's shielding step.
pub struct SpendMaterials {
    steps: Vec<StepMaterials>,
    ephemeral: Option<EphemeralAddressMaterial>,
}

/// The derived ephemeral address and its key coordinates.
pub struct EphemeralAddressMaterial {
    address: TransparentAddress,
    address_index: NonHardenedChildIndex,
}

impl LightWallet {
    /// Resolves a proposal's input references into self-contained spend
    /// materials — the effectful edge in front of the wallet-pure
    /// [`build_transactions`]. Takes `&mut self` only for the shard
    /// trees' internal witness caching; the wallet is semantically
    /// unchanged, and in particular the ephemeral Refund Address is
    /// *derived*, never reserved.
    pub fn spend_materials(&mut self, proposal: &Proposal) -> Result<SpendMaterials, BuildError> {
        let anchor_height = proposal.anchor_height();

        let mut steps = Vec::new();
        for step in proposal.steps() {
            let mut materials = StepMaterials::default();

            let mut sapling_ids = Vec::new();
            let mut orchard_ids = Vec::new();
            let mut ironwood_ids = Vec::new();
            for input in step.shielded_inputs() {
                match input.note().pool_type() {
                    PoolType::Shielded(ShieldedPool::Sapling) => {
                        sapling_ids.push(input.note().output_id());
                    }
                    PoolType::Shielded(ShieldedPool::Orchard) => {
                        orchard_ids.push(input.note().output_id());
                    }
                    PoolType::Shielded(ShieldedPool::Ironwood) => {
                        ironwood_ids.push(input.note().output_id());
                    }
                    PoolType::Transparent => unreachable!("shielded inputs are shielded"),
                }
            }

            // A pool's anchor is needed whenever the step *involves* it:
            // an output-only bundle still requires its anchor at build.
            let output_pools: Vec<PoolType> = step
                .payment_pools()
                .values()
                .copied()
                .chain(step.change().iter().map(super::proposal::ChangeValue::pool))
                .collect();
            let involves = |pool: ShieldedPool| output_pools.contains(&PoolType::Shielded(pool));

            if !sapling_ids.is_empty() || involves(ShieldedPool::Sapling) {
                let sapling_notes = self.note_materials::<SaplingNote>(&sapling_ids)?;
                let (anchor, spends) = self.with_sapling_tree_mut::<_, _, BuildError>(|tree| {
                    let anchor = tree
                        .root_at_checkpoint_id(&anchor_height)
                        .map_err(|e| BuildError::ShardTree(format!("{e:?}")))?
                        .ok_or(BuildError::AnchorNotFound {
                            pool: ShieldedPool::Sapling,
                            height: anchor_height,
                        })?;
                    let spends = sapling_notes
                        .iter()
                        .map(|(output_id, scope, note, position)| {
                            let merkle_path = tree
                                .witness_at_checkpoint_id_caching(*position, &anchor_height)
                                .map_err(|e| BuildError::ShardTree(format!("{e:?}")))?
                                .ok_or(BuildError::WitnessNotFound(*output_id))?;
                            Ok(SaplingSpendMaterial {
                                scope: *scope,
                                note: note.clone(),
                                merkle_path,
                            })
                        })
                        .collect::<Result<Vec<_>, BuildError>>()?;
                    Ok((anchor.into(), spends))
                })?;
                materials.sapling_anchor = Some(anchor);
                materials.sapling_spends = spends;
            }

            if !orchard_ids.is_empty() || involves(ShieldedPool::Orchard) {
                let orchard_notes = self.note_materials::<OrchardNote>(&orchard_ids)?;
                let (anchor, spends) = self.with_orchard_tree_mut::<_, _, BuildError>(|tree| {
                    let anchor = tree
                        .root_at_checkpoint_id(&anchor_height)
                        .map_err(|e| BuildError::ShardTree(format!("{e:?}")))?
                        .ok_or(BuildError::AnchorNotFound {
                            pool: ShieldedPool::Orchard,
                            height: anchor_height,
                        })?;
                    let spends = orchard_notes
                        .iter()
                        .map(|(output_id, _, note, position)| {
                            let merkle_path = tree
                                .witness_at_checkpoint_id_caching(*position, &anchor_height)
                                .map_err(|e| BuildError::ShardTree(format!("{e:?}")))?
                                .ok_or(BuildError::WitnessNotFound(*output_id))?;
                            Ok(OrchardSpendMaterial {
                                note: *note,
                                merkle_path: merkle_path.into(),
                            })
                        })
                        .collect::<Result<Vec<_>, BuildError>>()?;
                    Ok((orchard::Anchor::from(anchor), spends))
                })?;
                materials.orchard_anchor = Some(anchor);
                materials.orchard_spends = spends;
            }

            if !ironwood_ids.is_empty() || involves(ShieldedPool::Ironwood) {
                let ironwood_notes = self.note_materials::<IronwoodNote>(&ironwood_ids)?;
                let (anchor, spends) = self
                    .with_ironwood_tree_mut::<_, _, BuildError>(|tree| {
                        let anchor = tree
                            .root_at_checkpoint_id(&anchor_height)
                            .map_err(|e| BuildError::ShardTree(format!("{e:?}")))?
                            .ok_or(BuildError::AnchorNotFound {
                                pool: ShieldedPool::Ironwood,
                                height: anchor_height,
                            })?;
                        let spends = ironwood_notes
                            .iter()
                            .map(|(output_id, _, note, position)| {
                                let merkle_path = tree
                                    .witness_at_checkpoint_id_caching(*position, &anchor_height)
                                    .map_err(|e| BuildError::ShardTree(format!("{e:?}")))?
                                    .ok_or(BuildError::WitnessNotFound(*output_id))?;
                                Ok(OrchardSpendMaterial {
                                    note: *note,
                                    merkle_path: merkle_path.into(),
                                })
                            })
                            .collect::<Result<Vec<_>, BuildError>>()?;
                        Ok((orchard::Anchor::from(anchor), spends))
                    })?
                    .ok_or_else(|| {
                        BuildError::ShardTree("the wallet has no ironwood tree".to_string())
                    })?;
                materials.ironwood_anchor = Some(anchor);
                materials.ironwood_spends = spends;
            }

            for input in step.transparent_inputs() {
                materials
                    .transparent_spends
                    .push(self.coin_material(input.coin())?);
            }

            steps.push(materials);
        }

        let ephemeral = if let Proposal::TexTransfer(_) = proposal {
            let (address_id, address) = self
                .derive_refund_addresses(1, proposal.account_id())?
                .pop()
                .expect("derive_refund_addresses returns exactly n addresses");
            Some(EphemeralAddressMaterial {
                address,
                address_index: address_id.address_index(),
            })
        } else {
            None
        };

        Ok(SpendMaterials { steps, ephemeral })
    }

    /// Resolves output ids to `(id, scope, note, position)` for one pool.
    #[allow(clippy::type_complexity)]
    fn note_materials<N: NoteInterface<KeyId = KeyId>>(
        &self,
        output_ids: &[OutputId],
    ) -> Result<
        Vec<(
            OutputId,
            zip32::Scope,
            N::ZcashNote,
            incrementalmerkletree::Position,
        )>,
        BuildError,
    >
    where
        N::ZcashNote: Clone,
    {
        let notes = self.wallet_outputs::<N>();
        output_ids
            .iter()
            .map(|output_id| {
                let note = notes
                    .iter()
                    .find(|note| note.output_id() == *output_id)
                    .ok_or(BuildError::InputNotFound(*output_id))?;
                Ok((
                    *output_id,
                    note.key_id().scope,
                    note.note().clone(),
                    note.position()
                        .ok_or(BuildError::PositionMissing(*output_id))?,
                ))
            })
            .collect()
    }

    /// Resolves a transparent coin into its spend material.
    fn coin_material(&self, output_id: OutputId) -> Result<TransparentSpendMaterial, BuildError> {
        let coin = self
            .wallet_outputs::<TransparentCoin>()
            .into_iter()
            .find(|coin| coin.output_id() == output_id)
            .ok_or(BuildError::InputNotFound(output_id))?;

        let address_id = self
            .transparent_addresses
            .iter()
            .find(|(_, encoded)| *encoded == coin.address())
            .map(|(address_id, _)| *address_id)
            .ok_or(BuildError::CoinAddressNotFound(output_id))?;

        Ok(TransparentSpendMaterial {
            scope: address_id.scope().into(),
            address_index: address_id.address_index(),
            outpoint: coin.output_id().into(),
            coin: TxOut::new(
                coin.value()
                    .try_into()
                    .map_err(|_| BuildError::Balance(BalanceError::Overflow))?,
                coin.script().clone(),
            ),
        })
    }
}

/// One built transaction and the fee its proposal budgeted (which the
/// builder's value-balance assertion proved it actually pays).
pub struct BuiltStep {
    /// The fully built and signed transaction.
    pub transaction: Transaction,
    /// The ZIP 317 fee the transaction pays.
    pub fee: Zatoshis,
}

/// Builds and signs a proposal's transactions. Wallet-pure: everything
/// needed was resolved into `materials` at the edge, and nothing here
/// touches the wallet. OP_RETURN Data, if the proposal carries it, lands
/// on the final transaction via the upstream null-data primitive.
pub fn build_transactions(
    proposal: &Proposal,
    materials: &SpendMaterials,
    usk: &UnifiedSpendingKey,
    chain_type: &crate::config::ChainType,
    spend_prover: &impl SpendProver,
    output_prover: &impl OutputProver,
) -> Result<NonEmpty<BuiltStep>, BuildError> {
    let ufvk = usk.to_unified_full_viewing_key();
    let ironwood_active = chain_type
        .activation_height(NetworkUpgrade::Nu6_3)
        .is_some_and(|activation| proposal.target_height() >= activation);

    match proposal {
        Proposal::Transfer(transfer) => {
            let built = build_step(
                transfer.step(),
                &materials.steps[0],
                proposal,
                usk,
                &ufvk,
                chain_type,
                ironwood_active,
                None,
                None,
                spend_prover,
                output_prover,
            )?;
            Ok(NonEmpty::singleton(built))
        }
        Proposal::Shield(shield) => {
            let built = build_step(
                shield.step(),
                &materials.steps[0],
                proposal,
                usk,
                &ufvk,
                chain_type,
                ironwood_active,
                None,
                None,
                spend_prover,
                output_prover,
            )?;
            Ok(NonEmpty::singleton(built))
        }
        Proposal::TexTransfer(tex) => {
            let ephemeral = materials
                .ephemeral
                .as_ref()
                .expect("TEX materials carry the derived ephemeral address");

            let shielding = build_step(
                tex.shielding(),
                &materials.steps[0],
                proposal,
                usk,
                &ufvk,
                chain_type,
                ironwood_active,
                Some((&ephemeral.address, tex.ephemeral_value()?)),
                None,
                spend_prover,
                output_prover,
            )?;

            // The exposure step's sole input is the shielding step's
            // ephemeral output: the last transparent output the shielding
            // build added (payments first, ephemeral last, and OP_RETURN
            // Data never rides the shielding step).
            let shielding_vout = shielding
                .transaction
                .transparent_bundle()
                .expect("the shielding step created a transparent output")
                .vout
                .len()
                - 1;
            let outpoint = OutPoint::new(
                shielding.transaction.txid().into(),
                u32::try_from(shielding_vout).expect("vout indexes fit u32"),
            );
            let coin = shielding
                .transaction
                .transparent_bundle()
                .expect("checked above")
                .vout[shielding_vout]
                .clone();

            let exposure = build_step(
                tex.exposure(),
                &materials.steps[1],
                proposal,
                usk,
                &ufvk,
                chain_type,
                ironwood_active,
                None,
                Some((ephemeral, outpoint, coin)),
                spend_prover,
                output_prover,
            )?;

            Ok(NonEmpty::from_vec(vec![shielding, exposure]).expect("two steps"))
        }
    }
}

/// Builds one step's transaction.
#[allow(clippy::too_many_arguments)]
fn build_step(
    step: &Step,
    materials: &StepMaterials,
    proposal: &Proposal,
    usk: &UnifiedSpendingKey,
    ufvk: &UnifiedFullViewingKey,
    chain_type: &crate::config::ChainType,
    ironwood_active: bool,
    ephemeral_output: Option<(&TransparentAddress, Zatoshis)>,
    ephemeral_input: Option<(&EphemeralAddressMaterial, OutPoint, TxOut)>,
    spend_prover: &impl SpendProver,
    output_prover: &impl OutputProver,
) -> Result<BuiltStep, BuildError> {
    let has_ephemeral_input = ephemeral_input.is_some();
    let mut builder = Builder::new(
        chain_type.clone(),
        proposal.target_height(),
        BuildConfig::Standard {
            sapling_anchor: materials.sapling_anchor,
            orchard_anchor: materials.orchard_anchor,
            ironwood_anchor: materials.ironwood_anchor,
            orchard_pool_bundle_type: orchard::builder::BundleType::DEFAULT,
        },
    );
    let mut transparent_signing_set = TransparentSigningSet::new();

    // Spends.
    let sapling_dfvk = ufvk.sapling();
    for spend in &materials.sapling_spends {
        let dfvk = sapling_dfvk.ok_or(BuildError::MissingKey("sapling"))?;
        let fvk = match spend.scope {
            zip32::Scope::External => dfvk.fvk().clone(),
            zip32::Scope::Internal => dfvk.to_internal_fvk(),
        };
        builder
            .add_sapling_spend::<zip317::FeeError>(
                fvk,
                spend.note.clone(),
                spend.merkle_path.clone(),
            )
            .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
    }
    let orchard_fvk = ufvk.orchard();
    for spend in &materials.orchard_spends {
        builder
            .add_orchard_spend::<zip317::FeeError>(
                orchard_fvk
                    .cloned()
                    .ok_or(BuildError::MissingKey("orchard"))?,
                spend.note,
                spend.merkle_path.clone(),
            )
            .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
    }
    for spend in &materials.ironwood_spends {
        builder
            .add_ironwood_spend::<zip317::FeeError>(
                orchard_fvk
                    .cloned()
                    .ok_or(BuildError::MissingKey("orchard"))?,
                spend.note,
                spend.merkle_path.clone(),
            )
            .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
    }
    for spend in &materials.transparent_spends {
        let pubkey = ufvk
            .transparent()
            .ok_or(BuildError::MissingKey("transparent"))?
            .derive_address_pubkey(spend.scope, spend.address_index)
            .map_err(|_| BuildError::MissingKey("transparent child"))?;
        transparent_signing_set.add_key(
            usk.transparent()
                .derive_secret_key(spend.scope, spend.address_index)
                .map_err(|_| BuildError::MissingKey("transparent child"))?,
        );
        builder
            .add_transparent_p2pkh_input(pubkey, spend.outpoint.clone(), spend.coin.clone())
            .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
    }
    if let Some((ephemeral, outpoint, coin)) = ephemeral_input {
        let scope = TransparentKeyScope::EPHEMERAL;
        let pubkey = ufvk
            .transparent()
            .ok_or(BuildError::MissingKey("transparent"))?
            .derive_address_pubkey(scope, ephemeral.address_index)
            .map_err(|_| BuildError::MissingKey("ephemeral child"))?;
        transparent_signing_set.add_key(
            usk.transparent()
                .derive_secret_key(scope, ephemeral.address_index)
                .map_err(|_| BuildError::MissingKey("ephemeral child"))?,
        );
        builder
            .add_transparent_p2pkh_input(pubkey, outpoint, coin)
            .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
    }

    // The sender-side OVK for payment outputs, selected by input pools in
    // Orchard-family > Sapling > Transparent priority (OvkPolicy::Sender).
    let mut input_pools = Vec::new();
    if !materials.orchard_spends.is_empty() || !materials.ironwood_spends.is_empty() {
        input_pools.push(PoolType::ORCHARD);
    }
    if !materials.sapling_spends.is_empty() {
        input_pools.push(PoolType::SAPLING);
    }
    if !materials.transparent_spends.is_empty() || has_ephemeral_input {
        input_pools.push(PoolType::Transparent);
    }
    let external_ovk = NonEmpty::from_vec(input_pools)
        .and_then(|pools| ufvk.select_ovk(zip32::Scope::External, &pools));

    // Payments.
    let network = chain_type.network_type();
    for (index, pool) in step.payment_pools() {
        let payment = &step.transaction_request().payments()[index];
        let amount = payment.amount().expect("plan validated payment amounts");
        let memo = payment
            .memo()
            .map_or_else(MemoBytes::empty, |memo| memo.clone());
        let address = payment
            .recipient_address()
            .clone()
            .convert_if_network::<Address>(network)
            .map_err(|_| BuildError::ReceiverMissing(*index))?;

        match (pool, address) {
            (PoolType::Shielded(ShieldedPool::Sapling), address) => {
                let to = match address {
                    Address::Sapling(to) => to,
                    Address::Unified(ua) => {
                        *ua.sapling().ok_or(BuildError::ReceiverMissing(*index))?
                    }
                    _ => return Err(BuildError::ReceiverMissing(*index)),
                };
                builder
                    .add_sapling_output::<zip317::FeeError>(
                        external_ovk.clone().map(Into::into),
                        to,
                        amount,
                        memo,
                    )
                    .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
            }
            (PoolType::Shielded(ShieldedPool::Orchard), Address::Unified(ua)) => {
                builder
                    .add_orchard_output::<zip317::FeeError>(
                        external_ovk.clone().map(Into::into),
                        *ua.orchard().ok_or(BuildError::ReceiverMissing(*index))?,
                        amount,
                        memo,
                    )
                    .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
            }
            (PoolType::Shielded(ShieldedPool::Ironwood), Address::Unified(ua)) => {
                builder
                    .add_ironwood_output::<zip317::FeeError>(
                        external_ovk.clone().map(Into::into),
                        *ua.orchard().ok_or(BuildError::ReceiverMissing(*index))?,
                        amount,
                        memo,
                    )
                    .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
            }
            (PoolType::Transparent, address) => {
                let to = match address {
                    Address::Transparent(to) => to,
                    Address::Tex(data) => TransparentAddress::PublicKeyHash(data),
                    Address::Unified(ua) => *ua
                        .transparent()
                        .ok_or(BuildError::ReceiverMissing(*index))?,
                    _ => return Err(BuildError::ReceiverMissing(*index)),
                };
                builder
                    .add_transparent_output(&to, amount)
                    .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
            }
            _ => return Err(BuildError::ReceiverMissing(*index)),
        }
    }

    // Change: single shielded output to the internal change address.
    for change in step.change() {
        let memo = change
            .memo()
            .map_or_else(MemoBytes::empty, |memo| memo.clone());
        match change.pool() {
            PoolType::Shielded(ShieldedPool::Sapling) => {
                let dfvk = sapling_dfvk.ok_or(BuildError::MissingKey("sapling"))?;
                builder
                    .add_sapling_output::<zip317::FeeError>(
                        None,
                        dfvk.change_address().1,
                        change.value(),
                        memo,
                    )
                    .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
            }
            PoolType::Shielded(ShieldedPool::Orchard) => {
                let fvk = orchard_fvk
                    .cloned()
                    .ok_or(BuildError::MissingKey("orchard"))?;
                let change_address = fvk.address_at(0u32, orchard::keys::Scope::Internal);
                if ironwood_active {
                    // Post-NU6.3 the Orchard bundle forbids ordinary
                    // outputs; change returns to a spent note's own
                    // address via the dedicated change-output API.
                    builder
                        .add_orchard_change_output::<zip317::FeeError>(
                            fvk,
                            None,
                            change_address,
                            change.value(),
                            memo,
                        )
                        .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
                } else {
                    builder
                        .add_orchard_output::<zip317::FeeError>(
                            None,
                            change_address,
                            change.value(),
                            memo,
                        )
                        .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
                }
            }
            PoolType::Shielded(ShieldedPool::Ironwood) => {
                let fvk = orchard_fvk
                    .cloned()
                    .ok_or(BuildError::MissingKey("orchard"))?;
                let change_address = fvk.address_at(0u32, orchard::keys::Scope::Internal);
                builder
                    .add_ironwood_output::<zip317::FeeError>(
                        None,
                        change_address,
                        change.value(),
                        memo,
                    )
                    .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
            }
            PoolType::Transparent => {
                unreachable!("the planner produces shielded change only")
            }
        }
    }

    // The TEX shielding step's ephemeral output, last among transparent
    // outputs so the exposure step's input index is deterministic.
    if let Some((address, value)) = ephemeral_output {
        builder
            .add_transparent_output(address, value)
            .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
    }

    // OP_RETURN Data rides the final step; the proposal constructors
    // guarantee no other step carries it.
    if let Some(data) = step.op_return_data() {
        builder
            .add_transparent_null_data_output::<zip317::FeeError>(data.as_bytes())
            .map_err(|e| BuildError::Builder(format!("{e:?}")))?;
    }

    let sapling_extsks = [usk.sapling().clone(), usk.sapling().derive_internal()];
    let orchard_saks = [usk.orchard().into()];
    let build_result = builder
        .build(
            &transparent_signing_set,
            &sapling_extsks,
            &orchard_saks,
            OsRng,
            spend_prover,
            output_prover,
            &zip317::FeeRule::standard(),
        )
        .map_err(|e| BuildError::Builder(format!("{e:?}")))?;

    Ok(BuiltStep {
        transaction: build_result.transaction().clone(),
        fee: step.fee(),
    })
}

#[cfg(test)]
mod structural {
    //! Migration scaffolding (deleted at the P5 cutover): the build layer
    //! must produce transactions structurally equivalent to
    //! `create_proposed_transactions`' on the same proposal — the
    //! deterministic skeleton only, never bitwise (build randomness is by
    //! design): version, expiry, fee, transparent output set, shielded
    //! output counts, and the TEX ephemeral chain.

    use zcash_primitives::transaction::Transaction;
    use zcash_protocol::value::Zatoshis;

    use super::build_transactions;
    use crate::testutils::lightclient::from_inputs::transaction_request_from_send_inputs;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;
    use crate::wallet::keys::unified::ReceiverSelection;
    use crate::wallet::spend::plan::plan_transfer;
    use crate::wallet::spend::proposal::Proposal;

    fn provers() -> zcash_proofs::prover::LocalTxProver {
        let (sapling_output, sapling_spend) =
            crate::wallet::utils::read_sapling_params().expect("params embedded or fetched");
        zcash_proofs::prover::LocalTxProver::from_bytes(&sapling_spend, &sapling_output)
    }

    pub(super) fn build_ours(wallet: &mut LightWallet, proposal: &Proposal) -> Vec<Transaction> {
        let materials = wallet
            .spend_materials(proposal)
            .expect("materials resolve from wallet state");
        let usk: zcash_keys::keys::UnifiedSpendingKey = wallet
            .unified_key_store
            .get(&zip32::AccountId::ZERO)
            .expect("account zero exists")
            .try_into()
            .expect("spending wallet");
        let prover = provers();
        build_transactions(
            proposal,
            &materials,
            &usk,
            &wallet.chain_type,
            &prover,
            &prover,
        )
        .expect("the plan's fee and change satisfy the builder's balance assertion")
        .into_iter()
        .map(|built| built.transaction)
        .collect()
    }

    async fn build_theirs(
        wallet: &mut LightWallet,
        request: zip321::TransactionRequest,
    ) -> Vec<Transaction> {
        let proposal = wallet
            .create_send_proposal(request, zip32::AccountId::ZERO)
            .expect("zcb proposes");
        let txids = wallet
            .calculate_transactions(proposal, zip32::AccountId::ZERO)
            .await
            .expect("zcb builds");
        txids
            .into_iter()
            .map(|txid| {
                let mut bytes = vec![];
                wallet
                    .wallet_transactions
                    .get(&txid)
                    .expect("calculated tx is stored")
                    .transaction()
                    .write(&mut bytes)
                    .expect("transactions serialize");
                Transaction::read(
                    bytes.as_slice(),
                    zcash_protocol::consensus::BranchId::for_height(
                        &wallet.chain_type,
                        wallet
                            .wallet_transactions
                            .get(&txid)
                            .unwrap()
                            .transaction()
                            .expiry_height(),
                    ),
                )
                .expect("transactions round-trip")
            })
            .collect()
    }

    /// The deterministic skeleton two independent builds of the same
    /// proposal must share.
    pub(super) fn assert_structurally_equivalent(ours: &Transaction, theirs: &Transaction) {
        assert_eq!(ours.version(), theirs.version(), "tx version");
        assert_eq!(ours.expiry_height(), theirs.expiry_height(), "expiry");

        let vout_set = |tx: &Transaction| -> Vec<(u64, Vec<u8>)> {
            let mut set: Vec<(u64, Vec<u8>)> = tx
                .transparent_bundle()
                .map(|bundle| {
                    bundle
                        .vout
                        .iter()
                        .map(|out| {
                            let mut script = vec![];
                            out.script_pubkey()
                                .write(&mut script)
                                .expect("scripts serialize");
                            (out.value().into_u64(), script)
                        })
                        .collect()
                })
                .unwrap_or_default();
            set.sort();
            set
        };
        assert_eq!(vout_set(ours), vout_set(theirs), "transparent output set");

        let sapling_shape = |tx: &Transaction| {
            tx.sapling_bundle().map(|bundle| {
                (
                    bundle.shielded_spends().len(),
                    bundle.shielded_outputs().len(),
                )
            })
        };
        assert_eq!(sapling_shape(ours), sapling_shape(theirs), "sapling shape");

        let orchard_actions =
            |tx: &Transaction| tx.orchard_bundle().map(|bundle| bundle.actions().len());
        assert_eq!(
            orchard_actions(ours),
            orchard_actions(theirs),
            "orchard actions"
        );

        let ironwood_actions =
            |tx: &Transaction| tx.ironwood_bundle().map(|bundle| bundle.actions().len());
        assert_eq!(
            ironwood_actions(ours),
            ironwood_actions(theirs),
            "ironwood actions"
        );
    }

    fn wallet_with_orchard() -> LightWallet {
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(1_000_000)
            .build()
    }

    #[tokio::test]
    async fn single_orchard_transfer() {
        let mut ours_wallet = wallet_with_orchard();
        let mut theirs_wallet = wallet_with_orchard();
        let (_, address) = ours_wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
            .unwrap();
        let address = address.encode(&ours_wallet.chain_type);
        let request = transaction_request_from_send_inputs(vec![(address.as_str(), 250_000, None)])
            .expect("valid send inputs form a request");
        // The recipient address must exist in both wallets' key stores
        // identically; both derive from the same seed so it does.
        theirs_wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
            .unwrap();

        let proposal = plan_transfer(&ours_wallet, request.clone(), zip32::AccountId::ZERO, None)
            .expect("planner plans");
        let ours = build_ours(&mut ours_wallet, &proposal);
        let theirs = build_theirs(&mut theirs_wallet, request).await;

        assert_eq!(ours.len(), theirs.len(), "step count");
        assert_structurally_equivalent(&ours[0], &theirs[0]);
    }

    #[tokio::test]
    async fn tex_two_step_chains_and_matches() {
        use pepper_sync::keys::decode_address;
        use zcash_client_backend::address::Address;
        use zcash_transparent::address::TransparentAddress;
        use zip321::{Payment, TransactionRequest};

        let make_wallet = || {
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(5_000_000)
                .build()
        };
        let mut ours_wallet = make_wallet();
        let mut theirs_wallet = make_wallet();

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

        let proposal = plan_transfer(&ours_wallet, request.clone(), zip32::AccountId::ZERO, None)
            .expect("planner plans the TEX flow");
        let ours = build_ours(&mut ours_wallet, &proposal);
        let theirs = build_theirs(&mut theirs_wallet, request).await;

        assert_eq!(ours.len(), 2, "two steps");
        assert_eq!(theirs.len(), 2, "two steps");
        assert_structurally_equivalent(&ours[0], &theirs[0]);
        assert_structurally_equivalent(&ours[1], &theirs[1]);

        // The exposure step's sole input spends the shielding step's
        // ephemeral output.
        let exposure_vin = &ours[1]
            .transparent_bundle()
            .expect("exposure step is transparent")
            .vin;
        assert_eq!(exposure_vin.len(), 1, "one input");
        assert_eq!(
            *exposure_vin[0].prevout().txid(),
            ours[0].txid(),
            "spends the shielding step"
        );

        // And its value is the TEX payment plus the exposure fee.
        let Proposal::TexTransfer(tex) = &proposal else {
            panic!("a TEX payment plans a TexTransfer");
        };
        let ephemeral_vout = &ours[0].transparent_bundle().expect("shielding step").vout;
        assert_eq!(
            ephemeral_vout[ephemeral_vout.len() - 1].value(),
            tex.ephemeral_value().unwrap(),
            "ephemeral output value"
        );
    }

    #[tokio::test]
    async fn op_return_data_rides_the_final_transaction() {
        use crate::wallet::spend::op_return::OpReturnData;

        let mut wallet = wallet_with_orchard();
        let (_, address) = wallet
            .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
            .unwrap();
        let address = address.encode(&wallet.chain_type);
        let request = transaction_request_from_send_inputs(vec![(address.as_str(), 250_000, None)])
            .expect("valid send inputs form a request");

        let payload = b"=:ZEC.ZEC:thorchain-swap-memo".to_vec();
        let proposal = plan_transfer(
            &wallet,
            request,
            zip32::AccountId::ZERO,
            Some(OpReturnData::new(payload.clone()).unwrap()),
        )
        .expect("planner plans with OP_RETURN Data");

        // The build succeeding is itself the fee proof: the builder
        // recomputes the ZIP 317 fee including the null-data output and
        // refuses to build on any mismatch with the planned balance.
        let built = build_ours(&mut wallet, &proposal);

        let vout = &built[0]
            .transparent_bundle()
            .expect("the null-data output makes a transparent bundle")
            .vout;
        let null_data_output = vout
            .iter()
            .find(|out| out.value() == Zatoshis::ZERO)
            .expect("a zero-value output exists");
        let mut script = vec![];
        null_data_output
            .script_pubkey()
            .write(&mut script)
            .expect("scripts serialize");
        // Script serialization prefixes a compact-size length; then
        // OP_RETURN (0x6a), the push length, and the payload.
        assert_eq!(script[1], 0x6a, "OP_RETURN opcode");
        assert!(
            script
                .windows(payload.len())
                .any(|window| window == payload.as_slice()),
            "the payload is embedded in the script"
        );
    }
}

#[cfg(test)]
mod structural_shield {
    //! The shape-(c) structural gate: a shield's build must match the old
    //! path's on the same coins.

    use super::structural::{assert_structurally_equivalent, build_ours};
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;
    use crate::wallet::spend::plan::plan_shield;

    fn make_wallet() -> LightWallet {
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .transparent_coin(80_000)
            .transparent_coin(30_000)
            .build()
    }

    #[tokio::test]
    async fn shield_builds_structurally_equivalent() {
        let mut ours_wallet = make_wallet();
        let mut theirs_wallet = make_wallet();

        let proposal = plan_shield(&ours_wallet, zip32::AccountId::ZERO).expect("planner shields");
        let ours = build_ours(&mut ours_wallet, &proposal);

        let their_proposal = theirs_wallet
            .create_shield_proposal(zip32::AccountId::ZERO)
            .expect("zcb shields");
        let txids = theirs_wallet
            .calculate_transactions(their_proposal, zip32::AccountId::ZERO)
            .await
            .expect("zcb builds the shield");
        let theirs = theirs_wallet
            .wallet_transactions
            .get(txids.first())
            .expect("calculated tx is stored")
            .transaction();

        assert_structurally_equivalent(&ours[0], theirs);

        // Both consume both coins and pay no transparent output.
        let vin = ours[0]
            .transparent_bundle()
            .expect("a shield spends coins")
            .vin
            .len();
        assert_eq!(vin, 2, "both coins consumed");
        assert!(
            ours[0].transparent_bundle().unwrap().vout.is_empty(),
            "a shield pays no transparent outputs"
        );
    }
}
