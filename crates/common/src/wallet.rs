use std::{
    collections::BTreeMap, convert::Infallible, fmt::Debug, marker::PhantomData, ops::Range,
};

use incrementalmerkletree::Position;
use orchard::tree::MerkleHashOrchard;
use shardtree::{ShardTree, store::memory::MemoryShardStore};
use zcash_address::unified::ParseError;
use zcash_keys::{address::UnifiedAddress, encoding::encode_payment_address};
use zcash_primitives::{block::BlockHash, transaction::TxId};
use zcash_protocol::{
    PoolType, ShieldedPool,
    consensus::{self, BlockHeight},
    memo::Memo,
    value::Zatoshis,
};
use zcash_transparent::address::Script;
use zcash_transparent::bundle::OutPoint;

use crate::status::ConfirmationStatus;

use crate::{
    engine::{
        self, OutgoingNoteOps, ScanTargetOps, ScanTargetOrigin, ShardTreesOps, TransparentCoinOps,
        WalletBlockOps, WalletNoteOps, WalletTransactionOps, empty_shard_tree,
    },
    keys::{KeyId, transparent::TransparentAddressId},
    witness,
};

#[cfg(any(test, feature = "test-features"))]
use crate::engine::KeyIdOps;

#[cfg(feature = "serialization")]
pub mod serialization;

/// A block, or a transaction in it, that scanning must reach. A target names a block height and, when a particular
/// transaction is wanted, its txid. Two targets naming the same block height and txid are the same target.
///
/// The blocks around a target are prioritised for scanning: a narrow range by default, and the whole shard of each
/// pool the target's transaction was found to hold notes of, so those notes become spendable.
#[derive(Debug, Clone, Copy)]
pub struct ScanTarget {
    block_height: BlockHeight,
    txid: Option<TxId>,
    pools: ScanTargetPools,
    origin: ScanTargetOrigin,
}

// The shielded pools a scan target's transaction was found to hold notes of, as bits of a byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ScanTargetPools(u8);

impl ScanTargetPools {
    const ALL_POOLS: [ShieldedPool; 3] = [
        ShieldedPool::Sapling,
        ShieldedPool::Orchard,
        ShieldedPool::Ironwood,
    ];

    const fn bit(pool: ShieldedPool) -> u8 {
        match pool {
            ShieldedPool::Sapling => 1,
            ShieldedPool::Orchard => 1 << 1,
            ShieldedPool::Ironwood => 1 << 2,
        }
    }

    #[cfg(feature = "serialization")]
    pub(crate) fn from_bits(bits: u8) -> Option<Self> {
        let known = Self::ALL_POOLS
            .into_iter()
            .fold(0, |known, pool| known | Self::bit(pool));
        (bits & !known == 0).then_some(Self(bits))
    }

    #[cfg(feature = "serialization")]
    pub(crate) fn bits(self) -> u8 {
        self.0
    }

    pub(crate) fn with(self, pool: ShieldedPool) -> Self {
        Self(self.0 | Self::bit(pool))
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = ShieldedPool> {
        Self::ALL_POOLS
            .into_iter()
            .filter(move |&pool| self.0 & Self::bit(pool) != 0)
    }
}

impl ScanTarget {
    /// A target to add through `pepper_sync::sync::add_scan_targets`. The blocks around `block_height` are prioritised for
    /// scanning, and the transaction `txid` is scanned there when given.
    #[must_use]
    pub fn new(block_height: BlockHeight, txid: Option<TxId>) -> Self {
        Self {
            block_height,
            txid,
            pools: ScanTargetPools::default(),
            origin: ScanTargetOrigin::External,
        }
    }

    /// The block height of the target.
    #[must_use]
    pub fn block_height(&self) -> BlockHeight {
        self.block_height
    }

    /// The txid of the transaction to scan at the target's block height, when a particular one is wanted.
    #[must_use]
    pub fn txid(&self) -> Option<TxId> {
        self.txid
    }

    pub(crate) fn pools(&self) -> ScanTargetPools {
        self.pools
    }

    pub(crate) fn with_pool(self, pool: ShieldedPool) -> Self {
        Self {
            pools: self.pools.with(pool),
            ..self
        }
    }

    // The target that stands for both `self` and `other`, which name the same block height and txid: it holds the
    // pools of both, and it is internal where either is.
    pub(crate) fn merge(mut self, other: Self) -> Self {
        self.pools.0 |= other.pools.0;
        if other.origin == ScanTargetOrigin::Internal {
            self.origin = ScanTargetOrigin::Internal;
        }
        self
    }

    fn key(&self) -> (BlockHeight, Option<TxId>) {
        (self.block_height, self.txid)
    }
}

impl ScanTargetOps for ScanTarget {
    fn internal(block_height: BlockHeight, txid: TxId) -> Self {
        Self {
            block_height,
            txid: Some(txid),
            pools: ScanTargetPools::default(),
            origin: ScanTargetOrigin::Internal,
        }
    }

    fn origin(&self) -> ScanTargetOrigin {
        self.origin
    }
}

impl PartialEq for ScanTarget {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl Eq for ScanTarget {}

impl PartialOrd for ScanTarget {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScanTarget {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

/// Initial and final tree sizes.
#[derive(Debug, Clone, Copy)]
#[allow(missing_docs)]
pub struct TreeBounds {
    pub sapling_initial_tree_size: u32,
    pub sapling_final_tree_size: u32,
    pub orchard_initial_tree_size: u32,
    pub orchard_final_tree_size: u32,
    pub ironwood_initial_tree_size: u32,
    pub ironwood_final_tree_size: u32,
}

/// Output ID for a given pool type.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct OutputId {
    /// ID of associated transaction.
    txid: TxId,
    /// Index of output within the transactions bundle of the given pool type.
    output_index: u32,
}

impl OutputId {
    /// Creates new `OutputId` from parts.
    #[must_use]
    pub fn new(txid: TxId, output_index: u32) -> Self {
        OutputId { txid, output_index }
    }

    /// Transaction ID of output's associated transaction.
    #[must_use]
    pub fn txid(&self) -> TxId {
        self.txid
    }

    /// Index of output within the transactions bundle of the given pool type.
    #[must_use]
    pub fn output_index(&self) -> u32 {
        self.output_index
    }
}

impl std::fmt::Display for OutputId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{
                txid: {}
                output index: {}
            }}",
            self.txid, self.output_index
        )
    }
}

impl From<&OutPoint> for OutputId {
    fn from(value: &OutPoint) -> Self {
        OutputId::new(*value.txid(), value.n())
    }
}

impl From<OutputId> for OutPoint {
    fn from(value: OutputId) -> Self {
        OutPoint::new(value.txid.into(), value.output_index)
    }
}

/// Binary tree map of nullifiers from transaction spends or actions
#[derive(Debug)]
pub struct NullifierMap {
    /// Sapling nullifer map
    pub sapling: BTreeMap<sapling_crypto::Nullifier, ScanTarget>,
    /// Orchard nullifer map
    pub orchard: BTreeMap<orchard::note::Nullifier, ScanTarget>,
    /// Ironwood nullifer map
    pub ironwood: BTreeMap<orchard::note::Nullifier, ScanTarget>,
}

impl NullifierMap {
    /// Construct new nullifier map.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sapling: BTreeMap::new(),
            orchard: BTreeMap::new(),
            ironwood: BTreeMap::new(),
        }
    }

    /// Clear nullifier map.
    pub fn clear(&mut self) {
        self.sapling.clear();
        self.orchard.clear();
        self.ironwood.clear();
    }
}

impl Default for NullifierMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Wallet block data
#[derive(Debug, Clone)]
pub struct WalletBlock {
    pub(crate) block_height: BlockHeight,
    pub(crate) block_hash: BlockHash,
    pub(crate) prev_hash: BlockHash,
    pub(crate) time: u32,
    pub(crate) txids: Vec<TxId>,
    pub(crate) tree_bounds: TreeBounds,
}

impl WalletBlockOps for WalletBlock {
    fn from_parts(
        block_height: BlockHeight,
        block_hash: BlockHash,
        prev_hash: BlockHash,
        time: u32,
        txids: Vec<TxId>,
        tree_bounds: TreeBounds,
    ) -> Self {
        Self {
            block_height,
            block_hash,
            prev_hash,
            time,
            txids,
            tree_bounds,
        }
    }
}

impl WalletBlock {
    /// Block height.
    #[must_use]
    pub fn block_height(&self) -> BlockHeight {
        self.block_height
    }

    /// Block hash.
    #[must_use]
    pub fn block_hash(&self) -> BlockHash {
        self.block_hash
    }

    /// Previous block hash.
    #[must_use]
    pub fn prev_hash(&self) -> BlockHash {
        self.prev_hash
    }

    /// Time block was mined.
    #[must_use]
    pub fn time(&self) -> u32 {
        self.time
    }

    /// Transaction IDs of transactions in the block.
    #[must_use]
    pub fn txids(&self) -> &[TxId] {
        &self.txids
    }

    /// Tree size bounds
    #[must_use]
    pub fn tree_bounds(&self) -> TreeBounds {
        self.tree_bounds
    }
}

/// Wallet transaction
pub struct WalletTransaction {
    pub(crate) txid: TxId,
    pub(crate) status: ConfirmationStatus,
    pub(crate) transaction: zcash_primitives::transaction::Transaction,
    pub(crate) datetime: u32,
    pub(crate) transparent_coins: Vec<TransparentCoin>,
    pub(crate) sapling_notes: Vec<SaplingNote>,
    pub(crate) orchard_notes: Vec<OrchardNote>,
    pub(crate) ironwood_notes: Vec<IronwoodNote>,
    pub(crate) outgoing_sapling_notes: Vec<OutgoingSaplingNote>,
    pub(crate) outgoing_orchard_notes: Vec<OutgoingOrchardNote>,
    pub(crate) outgoing_ironwood_notes: Vec<OutgoingIronwoodNote>,
}

impl WalletTransactionOps for WalletTransaction {
    fn from_parts(
        txid: TxId,
        status: ConfirmationStatus,
        transaction: zcash_primitives::transaction::Transaction,
        datetime: u32,
        transparent_coins: Vec<TransparentCoin>,
        sapling_notes: Vec<SaplingNote>,
        orchard_notes: Vec<OrchardNote>,
        ironwood_notes: Vec<IronwoodNote>,
        outgoing_sapling_notes: Vec<OutgoingSaplingNote>,
        outgoing_orchard_notes: Vec<OutgoingOrchardNote>,
        outgoing_ironwood_notes: Vec<OutgoingIronwoodNote>,
    ) -> Self {
        Self {
            txid,
            status,
            transaction,
            datetime,
            transparent_coins,
            sapling_notes,
            orchard_notes,
            ironwood_notes,
            outgoing_sapling_notes,
            outgoing_orchard_notes,
            outgoing_ironwood_notes,
        }
    }

    /// - Replaces the refetch nullifier ranges of every received shielded note with `refetch_nullifier_ranges`.
    fn set_refetch_nullifier_ranges(&mut self, refetch_nullifier_ranges: &[Range<BlockHeight>]) {
        for note in self.sapling_notes.as_mut_slice() {
            note.refetch_nullifier_ranges = refetch_nullifier_ranges.to_vec();
        }
        for note in self.orchard_notes.as_mut_slice() {
            note.refetch_nullifier_ranges = refetch_nullifier_ranges.to_vec();
        }
        for note in self.ironwood_notes.as_mut_slice() {
            note.refetch_nullifier_ranges = refetch_nullifier_ranges.to_vec();
        }
    }
}

impl WalletTransaction {
    /// Transaction ID
    #[must_use]
    pub fn txid(&self) -> TxId {
        self.txid
    }

    /// Confirmation status
    #[must_use]
    pub fn status(&self) -> ConfirmationStatus {
        self.status
    }

    /// [`zcash_primitives::transaction::Transaction`]
    #[must_use]
    pub fn transaction(&self) -> &zcash_primitives::transaction::Transaction {
        &self.transaction
    }

    /// Datetime. In form of seconds since unix epoch.
    #[must_use]
    pub fn datetime(&self) -> u32 {
        self.datetime
    }

    /// Transparent coins
    #[must_use]
    pub fn transparent_coins(&self) -> &[TransparentCoin] {
        &self.transparent_coins
    }

    /// Transparent coins mutable
    pub fn transparent_coins_mut(&mut self) -> Vec<&mut TransparentCoin> {
        self.transparent_coins.iter_mut().collect()
    }

    /// Sapling notes
    #[must_use]
    pub fn sapling_notes(&self) -> &[SaplingNote] {
        &self.sapling_notes
    }

    /// Sapling notes mutable
    pub fn sapling_notes_mut(&mut self) -> Vec<&mut SaplingNote> {
        self.sapling_notes.iter_mut().collect()
    }

    /// Orchard notes
    #[must_use]
    pub fn orchard_notes(&self) -> &[OrchardNote] {
        &self.orchard_notes
    }

    /// Orchard notes mutable
    pub fn orchard_notes_mut(&mut self) -> Vec<&mut OrchardNote> {
        self.orchard_notes.iter_mut().collect()
    }

    /// Ironwood notes
    #[must_use]
    pub fn ironwood_notes(&self) -> &[IronwoodNote] {
        &self.ironwood_notes
    }

    /// Ironwood notes mutable
    pub fn ironwood_notes_mut(&mut self) -> Vec<&mut IronwoodNote> {
        self.ironwood_notes.iter_mut().collect()
    }

    /// Outgoing sapling notes
    #[must_use]
    pub fn outgoing_sapling_notes(&self) -> &[OutgoingSaplingNote] {
        &self.outgoing_sapling_notes
    }

    /// Outgoing orchard notes
    #[must_use]
    pub fn outgoing_orchard_notes(&self) -> &[OutgoingOrchardNote] {
        &self.outgoing_orchard_notes
    }

    /// Outgoing ironwood notes
    #[must_use]
    pub fn outgoing_ironwood_notes(&self) -> &[OutgoingIronwoodNote] {
        &self.outgoing_ironwood_notes
    }

    /// Returns nullifers from sapling bundle.
    /// Returns empty vec if bundle is `None`.
    pub fn sapling_nullifiers(&self) -> Vec<&sapling_crypto::Nullifier> {
        self.transaction
            .sapling_bundle()
            .map_or_else(Vec::new, |bundle| {
                bundle
                    .shielded_spends()
                    .iter()
                    .map(|spend| spend.nullifier())
                    .collect::<Vec<_>>()
            })
    }

    /// Returns nullifers from orchard bundle.
    /// Returns empty vec if bundle is `None`.
    pub fn orchard_nullifiers(&self) -> Vec<&orchard::note::Nullifier> {
        self.transaction
            .orchard_bundle()
            .map_or_else(Vec::new, |bundle| {
                bundle
                    .actions()
                    .iter()
                    .map(orchard::Action::nullifier)
                    .collect::<Vec<_>>()
            })
    }

    /// Returns nullifers from ironwood bundle.
    /// Returns empty vec if bundle is `None`.
    pub fn ironwood_nullifiers(&self) -> Vec<&orchard::note::Nullifier> {
        self.transaction
            .ironwood_bundle()
            .map_or_else(Vec::new, |bundle| {
                bundle
                    .actions()
                    .iter()
                    .map(orchard::Action::nullifier)
                    .collect::<Vec<_>>()
            })
    }

    /// Returns outpoints from transparent bundle.
    /// Returns empty vec if bundle is `None`.
    pub fn outpoints(&self) -> Vec<&OutPoint> {
        self.transaction
            .transparent_bundle()
            .map_or_else(Vec::new, |bundle| {
                bundle
                    .vin
                    .iter()
                    .map(zcash_transparent::bundle::TxIn::prevout)
                    .collect::<Vec<_>>()
            })
    }

    /// Updates transaction status if `status` is a valid update for the current transaction status.
    /// For example, if `status` is `Mempool` but the current transaction status is `Confirmed`, the status will remain
    /// unchanged.
    /// A `Failed` transaction may return to `Mempool` status, as being observed in the mempool is
    /// proof the transaction was not rejected or expired.
    /// `datetime` refers to the time in which the status was updated, or the time the block was mined when updating
    /// to `Confirmed` status.
    /// If `fail_confirmed` is set, a confirmed transaction can be set to failed (i.e. during re-org truncation).
    pub fn update_status(
        &mut self,
        status: ConfirmationStatus,
        datetime: u32,
        fail_confirmed: bool,
    ) {
        match status {
            ConfirmationStatus::Transmitted(_)
                if matches!(self.status(), ConfirmationStatus::Calculated(_)) =>
            {
                self.status = status;
                self.datetime = datetime;
            }
            ConfirmationStatus::Mempool(_)
                if matches!(
                    self.status(),
                    ConfirmationStatus::Calculated(_)
                        | ConfirmationStatus::Transmitted(_)
                        | ConfirmationStatus::Failed(_)
                ) =>
            {
                self.status = status;
                self.datetime = datetime;
            }
            ConfirmationStatus::Confirmed(_)
                if matches!(
                    self.status(),
                    ConfirmationStatus::Calculated(_)
                        | ConfirmationStatus::Transmitted(_)
                        | ConfirmationStatus::Mempool(_)
                ) =>
            {
                self.status = status;
                self.datetime = datetime;
            }
            ConfirmationStatus::Failed(_)
                if (fail_confirmed && !matches!(self.status(), ConfirmationStatus::Failed(_)))
                    || self.status().is_pending() =>
            {
                self.status = status;
                self.datetime = datetime;
            }
            _ => (),
        }
    }
}

#[cfg(any(test, feature = "test-features"))]
impl WalletTransaction {
    /// Creates a minimal `WalletTransaction` for testing purposes.
    ///
    /// Constructs a valid v5 transaction with empty bundles and the given `txid` and `status`.
    /// The transaction's expiry height is set to 0 (never expires).
    pub fn new_for_test(txid: TxId, status: ConfirmationStatus) -> Self {
        Self::new_for_test_with_expiry(txid, status, BlockHeight::from_u32(0))
    }

    /// Creates a minimal `WalletTransaction` for testing purposes, with the given expiry height.
    ///
    /// Constructs a valid v5 transaction with empty bundles and the given `txid` and `status`.
    pub fn new_for_test_with_expiry(
        txid: TxId,
        status: ConfirmationStatus,
        expiry_height: BlockHeight,
    ) -> Self {
        use zcash_primitives::transaction::{TransactionData, TxVersion};
        use zcash_protocol::consensus::BranchId;

        let transaction = TransactionData::from_parts(
            TxVersion::V6,
            BranchId::Nu6_3,
            0,
            expiry_height,
            None,
            None,
            None,
            None,
        )
        .freeze()
        .expect("empty v5 transaction should always be valid");

        Self {
            txid,
            status,
            transaction,
            datetime: 0,
            transparent_coins: Vec::new(),
            sapling_notes: Vec::new(),
            orchard_notes: Vec::new(),
            ironwood_notes: Vec::new(),
            outgoing_sapling_notes: Vec::new(),
            outgoing_orchard_notes: Vec::new(),
            outgoing_ironwood_notes: Vec::new(),
        }
    }

    /// As [`Self::new_for_test`], with received sapling notes attached.
    #[must_use]
    pub fn with_sapling_notes_for_test(mut self, sapling_notes: Vec<SaplingNote>) -> Self {
        self.sapling_notes = sapling_notes;
        self
    }

    /// As [`Self::new_for_test`], with outgoing sapling notes attached.
    #[must_use]
    pub fn with_outgoing_sapling_notes_for_test(
        mut self,
        outgoing_sapling_notes: Vec<OutgoingSaplingNote>,
    ) -> Self {
        self.outgoing_sapling_notes = outgoing_sapling_notes;
        self
    }

    /// As [`Self::new_for_test`], with received transparent coins attached.
    #[must_use]
    pub fn with_transparent_coins_for_test(
        mut self,
        transparent_coins: Vec<TransparentCoin>,
    ) -> Self {
        self.transparent_coins = transparent_coins;
        self
    }

    /// As [`Self::new_for_test`], with received and outgoing orchard notes
    /// attached, for tests exercising summary/value-transfer derivation
    /// without a chain.
    pub fn new_for_test_with_orchard_notes(
        txid: TxId,
        status: ConfirmationStatus,
        orchard_notes: Vec<OrchardNote>,
        outgoing_orchard_notes: Vec<OutgoingOrchardNote>,
    ) -> Self {
        let mut transaction = Self::new_for_test(txid, status);
        transaction.orchard_notes = orchard_notes;
        transaction.outgoing_orchard_notes = outgoing_orchard_notes;
        transaction
    }

    /// As [`Self::new_for_test`], with received and outgoing ironwood notes
    /// attached, for tests exercising summary/value-transfer derivation
    /// without a chain.
    pub fn new_for_test_with_ironwood_notes(
        txid: TxId,
        status: ConfirmationStatus,
        ironwood_notes: Vec<IronwoodNote>,
        outgoing_ironwood_notes: Vec<OutgoingIronwoodNote>,
    ) -> Self {
        let mut transaction = Self::new_for_test(txid, status);
        transaction.ironwood_notes = ironwood_notes;
        transaction.outgoing_ironwood_notes = outgoing_ironwood_notes;
        transaction
    }
}

#[cfg(any(test, feature = "test-features"))]
impl WalletBlock {
    /// - Zeroes the initial and final tree sizes of `pool`.
    pub fn clear_pool_tree_bounds_for_test(&mut self, pool: ShieldedPool) {
        let tree_bounds = &mut self.tree_bounds;
        match pool {
            ShieldedPool::Sapling => {
                tree_bounds.sapling_initial_tree_size = 0;
                tree_bounds.sapling_final_tree_size = 0;
            }
            ShieldedPool::Orchard => {
                tree_bounds.orchard_initial_tree_size = 0;
                tree_bounds.orchard_final_tree_size = 0;
            }
            ShieldedPool::Ironwood => {
                tree_bounds.ironwood_initial_tree_size = 0;
                tree_bounds.ironwood_final_tree_size = 0;
            }
        }
    }
}

#[cfg(any(test, feature = "test-features"))]
impl WalletTransaction {
    /// - Removes the received and outgoing notes of `pool`.
    pub fn clear_pool_notes_for_test(&mut self, pool: ShieldedPool) {
        match pool {
            ShieldedPool::Sapling => {
                self.sapling_notes.clear();
                self.outgoing_sapling_notes.clear();
            }
            ShieldedPool::Orchard => {
                self.orchard_notes.clear();
                self.outgoing_orchard_notes.clear();
            }
            ShieldedPool::Ironwood => {
                self.ironwood_notes.clear();
                self.outgoing_ironwood_notes.clear();
            }
        }
    }
}

#[cfg(any(test, feature = "test-features"))]
impl<N, Nf: Copy, P> WalletNote<N, Nf, P> {
    /// Creates a minimal received note for testing purposes.
    pub fn new_for_test(
        output_id: OutputId,
        account_id: zip32::AccountId,
        scope: zip32::Scope,
        note: N,
        memo: Memo,
        position: Option<Position>,
    ) -> Self {
        Self::from_parts(
            output_id,
            KeyId::from_parts(account_id, scope),
            note,
            None,
            position,
            memo,
        )
    }

    /// Attaches a nullifier, for tests exercising spend paths, since the
    /// spendable-note filter requires a known nullifier.
    #[must_use]
    pub fn with_nullifier_for_test(mut self, nullifier: Nf) -> Self {
        self.nullifier = Some(nullifier);
        self
    }
}

#[cfg(feature = "test-features")]
impl<N, P> OutgoingNote<N, P> {
    /// Creates a minimal outgoing note for testing purposes.
    pub fn new_for_test(
        output_id: OutputId,
        account_id: zip32::AccountId,
        scope: zip32::Scope,
        note: N,
        memo: Memo,
        recipient_full_unified_address: Option<UnifiedAddress>,
    ) -> Self {
        let mut outgoing_note =
            Self::from_parts(output_id, KeyId::from_parts(account_id, scope), note, memo);
        outgoing_note.set_recipient_full_unified_address(recipient_full_unified_address);
        outgoing_note
    }
}

#[cfg(feature = "serialization")]
impl WalletTransaction {
    /// Returns the total value sent to receivers, excluding value sent to the wallet's own addresses.
    #[must_use]
    pub fn total_value_sent(&self) -> u64 {
        let transparent_value_sent = self
            .transaction
            .transparent_bundle()
            .map_or(0, |bundle| {
                bundle
                    .vout
                    .iter()
                    .map(|output| output.value().into_u64())
                    .sum()
            })
            .saturating_sub(self.total_output_value::<TransparentCoin>());

        let sapling_value_sent =
            self.total_external_outgoing_note_value::<OutgoingSaplingNote, SaplingNote>();
        let orchard_value_sent =
            self.total_external_outgoing_note_value::<OutgoingOrchardNote, OrchardNote>();
        let ironwood_value_sent =
            self.total_external_outgoing_note_value::<OutgoingIronwoodNote, IronwoodNote>();

        transparent_value_sent + sapling_value_sent + orchard_value_sent + ironwood_value_sent
    }

    /// Returns total sum of all output values.
    #[must_use]
    pub fn total_value_received(&self) -> u64 {
        self.total_output_value::<TransparentCoin>()
            + self.total_output_value::<SaplingNote>()
            + self.total_output_value::<OrchardNote>()
            + self.total_output_value::<IronwoodNote>()
    }

    /// Returns total sum of output values for a given pool.
    #[must_use]
    pub fn total_output_value<Op: OutputInterface>(&self) -> u64 {
        Op::transaction_outputs(self)
            .iter()
            .map(OutputInterface::value)
            .sum()
    }

    /// Returns total sum of outgoing note values for a given shielded pool.
    #[must_use]
    pub fn total_outgoing_note_value<Op: OutgoingNoteInterface>(&self) -> u64 {
        Op::transaction_outgoing_notes(self)
            .iter()
            .map(OutgoingNoteInterface::value)
            .sum()
    }

    /// Returns total sum of outgoing note values for outputs that are not wallet-owned.
    #[must_use]
    pub fn total_external_outgoing_note_value<Outgoing, Incoming>(&self) -> u64
    where
        Outgoing: OutgoingNoteInterface,
        Incoming: OutputInterface,
    {
        Outgoing::transaction_outgoing_notes(self)
            .iter()
            .filter(|outgoing_note| {
                !Incoming::transaction_outputs(self)
                    .iter()
                    .any(|wallet_note| wallet_note.output_id() == outgoing_note.output_id())
            })
            .map(OutgoingNoteInterface::value)
            .sum()
    }
}

impl std::fmt::Debug for WalletTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("WalletTransaction")
            .field("txid", &self.txid)
            .field("confirmation_status", &self.status)
            .field("datetime", &self.datetime)
            .field("transparent_coins", &self.transparent_coins)
            .field("sapling_notes", &self.sapling_notes)
            .field("orchard_notes", &self.orchard_notes)
            .field("ironwood_notes", &self.ironwood_notes)
            .field("outgoing_sapling_notes", &self.outgoing_sapling_notes)
            .field("outgoing_orchard_notes", &self.outgoing_orchard_notes)
            .field("outgoing_ironwood_notes", &self.outgoing_ironwood_notes)
            .finish()
    }
}

/// Provides a common API for all key identifiers.
pub trait KeyIdInterface {
    /// Account ID.
    fn account_id(&self) -> zip32::AccountId;
}

/// Provides a common API for all output types.
pub trait OutputInterface: Sized {
    /// Identifier for key used to decrypt output.
    type KeyId: KeyIdInterface;
    /// Transaction input type associated with spend detection of output.
    type Input: Clone + Debug + PartialEq + Eq + PartialOrd + Ord;

    /// Output's associated pool type.
    const POOL_TYPE: PoolType;

    /// Output ID.
    fn output_id(&self) -> OutputId;

    /// Identifier for key used to decrypt output.
    fn key_id(&self) -> Self::KeyId;

    /// Transaction ID of transaction this output was spent.
    /// If `None`, output is not spent.
    fn spending_transaction(&self) -> Option<TxId>;

    /// Sets spending transaction.
    fn set_spending_transaction(&mut self, spending_transaction: Option<TxId>);

    /// Note value..
    // TODO: change to Zatoshis checked type
    fn value(&self) -> u64;

    /// Returns the type used to link with transaction inputs for spend detection.
    /// Returns `None` in the case the nullifier is not available for shielded outputs.
    ///
    /// Nullifier for shielded outputs.
    /// Outpoint for transparent outputs.
    fn spend_link(&self) -> Option<Self::Input>;

    /// Inputs within `transaction` used to detect an output's spend status.
    ///
    /// Nullifiers for shielded outputs.
    /// Out points for transparent outputs.
    fn transaction_inputs(transaction: &WalletTransaction) -> Vec<&Self::Input>;

    /// Outputs within `transaction`.
    fn transaction_outputs(transaction: &WalletTransaction) -> &[Self];
}

/// Provides a common API for all shielded output types.
pub trait NoteInterface: OutputInterface {
    /// Decrypted note type.
    type ZcashNote;
    /// Nullifier type.
    type Nullifier: Copy + Clone + PartialEq + Eq + PartialOrd + Ord;

    /// Note's associated shielded protocol.
    const SHIELDED_PROTOCOL: ShieldedPool;

    /// Decrypted note with recipient and value
    fn note(&self) -> &Self::ZcashNote;

    /// Derived nullifier
    fn nullifier(&self) -> Option<Self::Nullifier>;

    /// Commitment tree leaf position
    fn position(&self) -> Option<Position>;

    /// Memo
    fn memo(&self) -> &Memo;

    /// List of block ranges where the nullifiers must be re-fetched to guarantee the note has not been spent.
    /// These scan ranges were marked `ScannedWithoutMapping` or `RefetchingNullifiers` priority before this note was
    /// scanned, meaning the nullifiers were discarded due to memory constraints and will be re-fetched later in the
    /// sync process.
    fn refetch_nullifier_ranges(&self) -> &[Range<BlockHeight>];
}

///  Transparent coin (output) with metadata relevant to the wallet.
#[derive(Debug, Clone)]
pub struct TransparentCoin {
    /// Output ID.
    pub(crate) output_id: OutputId,
    /// Identifier for key used to derive address.
    pub(crate) key_id: TransparentAddressId,
    /// Encoded transparent address.
    pub(crate) address: String,
    /// Script.
    pub(crate) script: Script,
    /// Coin value.
    pub(crate) value: Zatoshis,
    /// Transaction ID of transaction this output was spent.
    /// If `None`, output is not spent.
    pub(crate) spending_transaction: Option<TxId>,
}

#[cfg(feature = "test-features")]
impl TransparentCoin {
    /// Creates a minimal received coin for testing purposes. The script
    /// must be the real locking script for `address`: spendable-output
    /// selection reconstructs the recipient from it and silently drops
    /// coins whose script does not parse to an address.
    pub fn new_for_test(
        output_id: OutputId,
        key_id: TransparentAddressId,
        address: String,
        script: Script,
        value: Zatoshis,
    ) -> Self {
        Self::from_parts(output_id, key_id, address, script, value)
    }
}

impl TransparentCoinOps for TransparentCoin {
    fn from_parts(
        output_id: OutputId,
        key_id: TransparentAddressId,
        address: String,
        script: Script,
        value: Zatoshis,
    ) -> Self {
        Self {
            output_id,
            key_id,
            address,
            script,
            value,
            spending_transaction: None,
        }
    }
}

impl TransparentCoin {
    /// Address received to.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Script.
    #[must_use]
    pub fn script(&self) -> &Script {
        &self.script
    }
}

impl OutputInterface for TransparentCoin {
    type KeyId = TransparentAddressId;
    type Input = OutPoint;

    const POOL_TYPE: PoolType = PoolType::Transparent;

    fn output_id(&self) -> OutputId {
        self.output_id
    }

    fn key_id(&self) -> Self::KeyId {
        self.key_id
    }

    fn spending_transaction(&self) -> Option<TxId> {
        self.spending_transaction
    }

    fn set_spending_transaction(&mut self, spending_transaction: Option<TxId>) {
        self.spending_transaction = spending_transaction;
    }

    fn value(&self) -> u64 {
        self.value.into_u64()
    }

    fn spend_link(&self) -> Option<Self::Input> {
        Some(self.output_id.into())
    }

    fn transaction_inputs(transaction: &WalletTransaction) -> Vec<&Self::Input> {
        transaction.outpoints()
    }

    fn transaction_outputs(transaction: &WalletTransaction) -> &[Self] {
        &transaction.transparent_coins
    }
}

/// The network upgrade at which a shielded pool begins to exist.
///
/// This is the workspace's single pool-to-upgrade mapping: upstream
/// `zcash_protocol` deliberately ships
/// `ShieldedPool` and `NetworkUpgrade` as unrelated enums, so the mapping
/// is defined exactly once, here. Every height clamp involving a pool's
/// existence derives from it. A second mapping anywhere in the workspace
/// is a defect.
#[must_use]
pub fn pool_upgrade(pool: ShieldedPool) -> consensus::NetworkUpgrade {
    match pool {
        ShieldedPool::Sapling => consensus::NetworkUpgrade::Sapling,
        ShieldedPool::Orchard => consensus::NetworkUpgrade::Nu5,
        ShieldedPool::Ironwood => consensus::NetworkUpgrade::Nu6_3,
    }
}

/// A pool's activation height: the block height at which the pool begins
/// to exist on the chain.
///
/// The only constructor is the derivation [`PoolActivation::of`], so a
/// value of this type carries its provenance: it came from the chain's
/// consensus parameters through [`pool_upgrade`], never from arithmetic a
/// call site invented. Seams that must not run without an activation floor
/// (the migration schedule planner, for one) demand this type in their
/// signatures, making floor omission unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolActivation(BlockHeight);

impl PoolActivation {
    /// Derives the pool's activation height from the chain's consensus
    /// parameters. `None` when the chain never activates the pool's
    /// upgrade, a real state (regtest schedules may defer an upgrade
    /// indefinitely), in which the pool simply cannot be scheduled
    /// against.
    pub fn of(
        consensus_parameters: &impl consensus::Parameters,
        pool: ShieldedPool,
    ) -> Option<Self> {
        consensus_parameters
            .activation_height(pool_upgrade(pool))
            .map(Self)
    }

    /// The activation height.
    #[must_use]
    pub fn height(&self) -> BlockHeight {
        self.0
    }

    /// The later of the activation and `height`: the effective floor for
    /// heights that must not precede the pool's existence (a wallet
    /// Birthday clamped to the pool's activation, the lower bound of a
    /// scan range). Birthday remains a wallet property throughout. The
    /// pool contributes only its activation height.
    #[must_use]
    pub fn max_with(&self, height: BlockHeight) -> BlockHeight {
        self.0.max(height)
    }

    /// A fabricated activation for tests exercising floor arithmetic
    /// without a full parameter set. Production code has no path here:
    /// the derivation constructor is the only other way in.
    #[cfg(any(test, feature = "test-features"))]
    #[must_use]
    pub fn new_for_test(height: BlockHeight) -> Self {
        Self(height)
    }
}

/// Marker for the Sapling pool, distinguishing note types whose inner data
/// is otherwise identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sapling;

/// Marker for the Orchard pool, distinguishing note types whose inner data
/// is otherwise identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Orchard;

/// Marker for the Ironwood pool. Ironwood reuses Orchard's note and
/// nullifier types (note version V3), so without the marker its wallet note
/// type would be the same type as the Orchard one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ironwood;

/// Wallet note, shielded output with metadata relevant to the wallet. `P` is
/// the pool marker ([`Sapling`], [`Orchard`] or [`Ironwood`]).
#[derive(Debug, Clone)]
pub struct WalletNote<N, Nf: Copy, P> {
    /// Output ID.
    pub(crate) output_id: OutputId,
    /// Identifier for key used to decrypt output.
    pub(crate) key_id: KeyId,
    /// Decrypted note with recipient and value.
    pub(crate) note: N,
    /// Derived nullifier.
    pub(crate) nullifier: Option<Nf>, //TODO: syncing without nullifier deriving key
    /// Commitment tree leaf position.
    pub(crate) position: Option<Position>,
    /// Memo.
    pub(crate) memo: Memo,
    /// Transaction ID of transaction this output was spent.
    /// If `None`, output is not spent.
    pub(crate) spending_transaction: Option<TxId>,
    /// List of block ranges where the nullifiers must be re-fetched to guarantee the note has not been spent.
    /// These scan ranges were marked `ScannedWithoutMapping` or `RefetchingNullifiers` priority before this note was
    /// scanned, meaning the nullifiers were discarded due to memory constraints and will be re-fetched later in the
    /// sync process.
    pub(crate) refetch_nullifier_ranges: Vec<Range<BlockHeight>>,
    /// Pool marker.
    pub(crate) marker: PhantomData<P>,
}

impl<N, Nf: Copy, P> WalletNoteOps<N, Nf> for WalletNote<N, Nf, P> {
    fn from_parts(
        output_id: OutputId,
        key_id: KeyId,
        note: N,
        nullifier: Option<Nf>,
        position: Option<Position>,
        memo: Memo,
    ) -> Self {
        Self {
            output_id,
            key_id,
            note,
            nullifier,
            position,
            memo,
            spending_transaction: None,
            refetch_nullifier_ranges: Vec::new(),
            marker: PhantomData,
        }
    }
}

/// Sapling note.
pub type SaplingNote = WalletNote<sapling_crypto::Note, sapling_crypto::Nullifier, Sapling>;

impl OutputInterface for SaplingNote {
    type KeyId = KeyId;
    type Input = sapling_crypto::Nullifier;

    const POOL_TYPE: PoolType = PoolType::Shielded(ShieldedPool::Sapling);

    fn output_id(&self) -> OutputId {
        self.output_id
    }

    fn key_id(&self) -> KeyId {
        self.key_id
    }

    fn spending_transaction(&self) -> Option<TxId> {
        self.spending_transaction
    }

    fn set_spending_transaction(&mut self, spending_transaction: Option<TxId>) {
        self.spending_transaction = spending_transaction;
    }

    fn value(&self) -> u64 {
        self.note.value().inner()
    }

    fn spend_link(&self) -> Option<Self::Input> {
        self.nullifier
    }

    fn transaction_inputs(transaction: &WalletTransaction) -> Vec<&Self::Input> {
        transaction.sapling_nullifiers()
    }

    fn transaction_outputs(transaction: &WalletTransaction) -> &[Self] {
        &transaction.sapling_notes
    }
}

impl NoteInterface for SaplingNote {
    type ZcashNote = sapling_crypto::Note;
    type Nullifier = Self::Input;

    const SHIELDED_PROTOCOL: ShieldedPool = ShieldedPool::Sapling;

    fn note(&self) -> &Self::ZcashNote {
        &self.note
    }

    fn nullifier(&self) -> Option<Self::Nullifier> {
        self.nullifier
    }

    fn position(&self) -> Option<Position> {
        self.position
    }

    fn memo(&self) -> &Memo {
        &self.memo
    }

    fn refetch_nullifier_ranges(&self) -> &[Range<BlockHeight>] {
        &self.refetch_nullifier_ranges
    }
}

/// Orchard note.
pub type OrchardNote = WalletNote<orchard::Note, orchard::note::Nullifier, Orchard>;

impl OutputInterface for OrchardNote {
    type KeyId = KeyId;
    type Input = orchard::note::Nullifier;

    const POOL_TYPE: PoolType = PoolType::Shielded(ShieldedPool::Orchard);

    fn output_id(&self) -> OutputId {
        self.output_id
    }

    fn key_id(&self) -> KeyId {
        self.key_id
    }

    fn spending_transaction(&self) -> Option<TxId> {
        self.spending_transaction
    }

    fn set_spending_transaction(&mut self, spending_transaction: Option<TxId>) {
        self.spending_transaction = spending_transaction;
    }

    fn value(&self) -> u64 {
        self.note.value().inner()
    }

    fn spend_link(&self) -> Option<Self::Input> {
        self.nullifier
    }

    fn transaction_inputs(transaction: &WalletTransaction) -> Vec<&Self::Input> {
        transaction.orchard_nullifiers()
    }

    fn transaction_outputs(transaction: &WalletTransaction) -> &[Self] {
        &transaction.orchard_notes
    }
}

impl NoteInterface for OrchardNote {
    type ZcashNote = orchard::Note;
    type Nullifier = Self::Input;

    const SHIELDED_PROTOCOL: ShieldedPool = ShieldedPool::Orchard;

    fn note(&self) -> &Self::ZcashNote {
        &self.note
    }

    fn nullifier(&self) -> Option<Self::Nullifier> {
        self.spend_link()
    }

    fn position(&self) -> Option<Position> {
        self.position
    }

    fn memo(&self) -> &Memo {
        &self.memo
    }

    fn refetch_nullifier_ranges(&self) -> &[Range<BlockHeight>] {
        &self.refetch_nullifier_ranges
    }
}

/// Ironwood note. Shares Orchard's note and nullifier types (version V3) but
/// belongs to the Ironwood pool with its own note commitment tree.
pub type IronwoodNote = WalletNote<orchard::Note, orchard::note::Nullifier, Ironwood>;

impl OutputInterface for IronwoodNote {
    type KeyId = KeyId;
    type Input = orchard::note::Nullifier;

    const POOL_TYPE: PoolType = PoolType::Shielded(ShieldedPool::Ironwood);

    fn output_id(&self) -> OutputId {
        self.output_id
    }

    fn key_id(&self) -> KeyId {
        self.key_id
    }

    fn spending_transaction(&self) -> Option<TxId> {
        self.spending_transaction
    }

    fn set_spending_transaction(&mut self, spending_transaction: Option<TxId>) {
        self.spending_transaction = spending_transaction;
    }

    fn value(&self) -> u64 {
        self.note.value().inner()
    }

    fn spend_link(&self) -> Option<Self::Input> {
        self.nullifier
    }

    fn transaction_inputs(transaction: &WalletTransaction) -> Vec<&Self::Input> {
        transaction.ironwood_nullifiers()
    }

    fn transaction_outputs(transaction: &WalletTransaction) -> &[Self] {
        &transaction.ironwood_notes
    }
}

impl NoteInterface for IronwoodNote {
    type ZcashNote = orchard::Note;
    type Nullifier = Self::Input;

    const SHIELDED_PROTOCOL: ShieldedPool = ShieldedPool::Ironwood;

    fn note(&self) -> &Self::ZcashNote {
        &self.note
    }

    fn nullifier(&self) -> Option<Self::Nullifier> {
        self.spend_link()
    }

    fn position(&self) -> Option<Position> {
        self.position
    }

    fn memo(&self) -> &Memo {
        &self.memo
    }

    fn refetch_nullifier_ranges(&self) -> &[Range<BlockHeight>] {
        &self.refetch_nullifier_ranges
    }
}

/// Provides a common API for all outgoing note types.
pub trait OutgoingNoteInterface: Sized {
    /// Decrypted note type.
    type ZcashNote;
    /// Address type.
    type Address: Clone + Copy + Debug + PartialEq + Eq;
    /// Encoding error
    type Error: Debug + std::error::Error;

    /// Note's associated shielded protocol.
    const SHIELDED_PROTOCOL: ShieldedPool;

    /// Output ID.
    fn output_id(&self) -> OutputId;

    /// Identifier for key used to decrypt outgoing note.
    fn key_id(&self) -> KeyId;

    /// Note value.
    fn value(&self) -> u64;

    /// Decrypted note with recipient and value.
    fn note(&self) -> &Self::ZcashNote;

    /// Memo.
    fn memo(&self) -> &Memo;

    /// Recipient address.
    fn recipient(&self) -> Self::Address;

    /// Recipient unified address as given by recipient and recorded in an encoded memo (all original receivers).
    fn recipient_full_unified_address(&self) -> Option<&UnifiedAddress>;

    /// Encoded recipient address recorded in note on chain (single receiver).
    fn encoded_recipient<P>(&self, parameters: &P) -> Result<String, Self::Error>
    where
        P: consensus::Parameters + consensus::NetworkConstants;

    /// Encoded recipient unified address as given by recipient and recorded in an encoded memo (all original receivers).
    fn encoded_recipient_full_unified_address<P>(&self, consensus_parameters: &P) -> Option<String>
    where
        P: consensus::Parameters + consensus::NetworkConstants;

    /// Outgoing notes within `transaction`.
    fn transaction_outgoing_notes(transaction: &WalletTransaction) -> &[Self];
}

/// Note sent from this capability to a recipient. `P` is the pool marker
/// ([`Sapling`], [`Orchard`] or [`Ironwood`]).
#[derive(Debug, Clone, PartialEq)]
pub struct OutgoingNote<N, P> {
    /// Output ID.
    pub(crate) output_id: OutputId,
    /// Identifier for key used to decrypt output.
    pub(crate) key_id: KeyId,
    /// Decrypted note with recipient and value.
    pub(crate) note: N,
    /// Memo.
    pub(crate) memo: Memo,
    /// Recipient's full unified address from encoded memo.
    pub(crate) recipient_full_unified_address: Option<UnifiedAddress>,
    /// Pool marker.
    pub(crate) marker: PhantomData<P>,
}

impl<N, P> OutgoingNoteOps<N> for OutgoingNote<N, P> {
    fn from_parts(output_id: OutputId, key_id: KeyId, note: N, memo: Memo) -> Self {
        Self {
            output_id,
            key_id,
            note,
            memo,
            recipient_full_unified_address: None,
            marker: PhantomData,
        }
    }

    /// - Replaces the recipient's full unified address.
    fn set_recipient_full_unified_address(
        &mut self,
        recipient_full_unified_address: Option<UnifiedAddress>,
    ) {
        self.recipient_full_unified_address = recipient_full_unified_address;
    }
}

/// Outgoing sapling note.
pub type OutgoingSaplingNote = OutgoingNote<sapling_crypto::Note, Sapling>;

impl OutgoingNoteInterface for OutgoingSaplingNote {
    type ZcashNote = sapling_crypto::Note;
    type Address = sapling_crypto::PaymentAddress;
    type Error = Infallible;

    const SHIELDED_PROTOCOL: ShieldedPool = ShieldedPool::Sapling;

    fn output_id(&self) -> OutputId {
        self.output_id
    }

    fn key_id(&self) -> KeyId {
        self.key_id
    }

    fn value(&self) -> u64 {
        self.note.value().inner()
    }

    fn note(&self) -> &Self::ZcashNote {
        &self.note
    }

    fn memo(&self) -> &Memo {
        &self.memo
    }

    fn recipient(&self) -> Self::Address {
        self.note.recipient()
    }

    fn recipient_full_unified_address(&self) -> Option<&UnifiedAddress> {
        self.recipient_full_unified_address.as_ref()
    }

    fn encoded_recipient<P>(&self, consensus_parameters: &P) -> Result<String, Self::Error>
    where
        P: consensus::Parameters + consensus::NetworkConstants,
    {
        Ok(encode_payment_address(
            zcash_address::constants::hrp_sapling_payment_address(
                consensus_parameters.network_type(),
            ),
            &self.note().recipient(),
        ))
    }

    fn encoded_recipient_full_unified_address<P>(&self, consensus_parameters: &P) -> Option<String>
    where
        P: consensus::Parameters + consensus::NetworkConstants,
    {
        self.recipient_full_unified_address
            .as_ref()
            .map(|unified_address| unified_address.encode(consensus_parameters))
    }

    fn transaction_outgoing_notes(transaction: &WalletTransaction) -> &[Self] {
        &transaction.outgoing_sapling_notes
    }
}

/// Outgoing orchard note.
pub type OutgoingOrchardNote = OutgoingNote<orchard::Note, Orchard>;

impl OutgoingNoteInterface for OutgoingOrchardNote {
    type ZcashNote = orchard::Note;
    type Address = orchard::Address;
    type Error = ParseError;

    const SHIELDED_PROTOCOL: ShieldedPool = ShieldedPool::Orchard;

    fn output_id(&self) -> OutputId {
        self.output_id
    }

    fn key_id(&self) -> KeyId {
        self.key_id
    }

    fn value(&self) -> u64 {
        self.note.value().inner()
    }

    fn note(&self) -> &Self::ZcashNote {
        &self.note
    }

    fn memo(&self) -> &Memo {
        &self.memo
    }

    fn recipient(&self) -> Self::Address {
        self.note.recipient()
    }

    fn recipient_full_unified_address(&self) -> Option<&UnifiedAddress> {
        self.recipient_full_unified_address.as_ref()
    }

    fn encoded_recipient<P>(&self, parameters: &P) -> Result<String, Self::Error>
    where
        P: consensus::Parameters + consensus::NetworkConstants,
    {
        engine::encode_orchard_receiver(parameters, &self.note().recipient())
    }

    fn encoded_recipient_full_unified_address<P>(&self, consensus_parameters: &P) -> Option<String>
    where
        P: consensus::Parameters + consensus::NetworkConstants,
    {
        self.recipient_full_unified_address
            .as_ref()
            .map(|unified_address| unified_address.encode(consensus_parameters))
    }

    fn transaction_outgoing_notes(transaction: &WalletTransaction) -> &[Self] {
        &transaction.outgoing_orchard_notes
    }
}

/// Outgoing ironwood note. Shares Orchard's note and receiver types.
pub type OutgoingIronwoodNote = OutgoingNote<orchard::Note, Ironwood>;

impl OutgoingNoteInterface for OutgoingIronwoodNote {
    type ZcashNote = orchard::Note;
    type Address = orchard::Address;
    type Error = ParseError;

    const SHIELDED_PROTOCOL: ShieldedPool = ShieldedPool::Ironwood;

    fn output_id(&self) -> OutputId {
        self.output_id
    }

    fn key_id(&self) -> KeyId {
        self.key_id
    }

    fn value(&self) -> u64 {
        self.note.value().inner()
    }

    fn note(&self) -> &Self::ZcashNote {
        &self.note
    }

    fn memo(&self) -> &Memo {
        &self.memo
    }

    fn recipient(&self) -> Self::Address {
        self.note.recipient()
    }

    fn recipient_full_unified_address(&self) -> Option<&UnifiedAddress> {
        self.recipient_full_unified_address.as_ref()
    }

    fn encoded_recipient<P>(&self, parameters: &P) -> Result<String, Self::Error>
    where
        P: consensus::Parameters + consensus::NetworkConstants,
    {
        engine::encode_orchard_receiver(parameters, &self.note().recipient())
    }

    fn encoded_recipient_full_unified_address<P>(&self, consensus_parameters: &P) -> Option<String>
    where
        P: consensus::Parameters + consensus::NetworkConstants,
    {
        self.recipient_full_unified_address
            .as_ref()
            .map(|unified_address| unified_address.encode(consensus_parameters))
    }

    fn transaction_outgoing_notes(transaction: &WalletTransaction) -> &[Self] {
        &transaction.outgoing_ironwood_notes
    }
}

/// The deepest chain reorganization the wallet tolerates, and the
/// repository's single source of truth for that depth. It mirrors the
/// validator's finalization boundary, zebra's
/// `zebra_chain::parameters::constants::MAX_BLOCK_REORG_HEIGHT` (1000 since
/// zebra 5.2.0), the depth below its tip at which zebra finalizes a block
/// and follows no fork beneath it. Zebra crates are not dependencies of
/// this workspace, so the value is pinned to its upstream by documentation
/// rather than import. If zebra ever moves its boundary, this constant is
/// the one place that follows it.
pub const MAX_REORG_ALLOWANCE: u32 = 1000;

/// The maximum number of checkpoints in the rolling window for re-org handling and chain tip anchor spends.
pub const SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE: u32 = MAX_REORG_ALLOWANCE + 1;

// TODO: allow consumer to define shard store. memory shard store has infallible error type but other may not so error
// handling will need to replace expects
/// Type alias for sapling memory shard store
pub type SaplingShardStore = MemoryShardStore<sapling_crypto::Node, BlockHeight>;

/// Type alias for orchard memory shard store
pub type OrchardShardStore = MemoryShardStore<MerkleHashOrchard, BlockHeight>;

/// Shard tree wallet data struct
#[derive(Debug)]
pub struct ShardTrees {
    /// Sapling shard tree
    pub sapling: ShardTree<
        SaplingShardStore,
        { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH },
        { witness::SHARD_HEIGHT },
    >,
    /// Orchard shard tree
    pub orchard: ShardTree<
        OrchardShardStore,
        { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
        { witness::SHARD_HEIGHT },
    >,
    /// Ironwood shard tree. Ironwood reuses the Orchard note commitment tree
    /// structure (same hash and depth), so it has the same shape as `orchard`.
    pub ironwood: ShardTree<
        OrchardShardStore,
        { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
        { witness::SHARD_HEIGHT },
    >,
}

impl ShardTrees {
    /// Create new `ShardTrees`
    #[must_use]
    pub fn new() -> Self {
        Self {
            sapling: empty_shard_tree(),
            orchard: empty_shard_tree(),
            ironwood: empty_shard_tree(),
        }
    }
}

impl ShardTreesOps for ShardTrees {
    /// Replaces one pool's shard tree with an empty tree, leaving the other
    /// pools alone.
    ///
    /// The empty tree is that pool's correct state at its own activation, so
    /// this is the rollback a pool needs when its recorded history cannot be
    /// trusted and no checkpoint survives to roll back to.
    fn clear_pool(&mut self, pool: ShieldedPool) {
        tracing::info!("Clearing {pool:?} shard tree.");
        match pool {
            ShieldedPool::Sapling => self.sapling = empty_shard_tree(),
            ShieldedPool::Orchard => self.orchard = empty_shard_tree(),
            ShieldedPool::Ironwood => self.ironwood = empty_shard_tree(),
        }
    }
}

impl Default for ShardTrees {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod test {
    use zcash_protocol::TxId;
    use zcash_protocol::consensus::BlockHeight;

    use super::WalletTransaction;
    use crate::status::ConfirmationStatus;

    fn transaction_with_status(status: ConfirmationStatus) -> WalletTransaction {
        WalletTransaction::new_for_test(TxId::from_bytes([0; 32]), status)
    }

    #[test]
    fn update_status_follows_transaction_lifecycle() {
        let mut transaction =
            transaction_with_status(ConfirmationStatus::Calculated(BlockHeight::from_u32(10)));

        transaction.update_status(
            ConfirmationStatus::Transmitted(BlockHeight::from_u32(10)),
            1,
            false,
        );
        assert!(matches!(
            transaction.status(),
            ConfirmationStatus::Transmitted(_)
        ));

        transaction.update_status(
            ConfirmationStatus::Mempool(BlockHeight::from_u32(11)),
            2,
            false,
        );
        assert!(matches!(
            transaction.status(),
            ConfirmationStatus::Mempool(_)
        ));

        transaction.update_status(
            ConfirmationStatus::Confirmed(BlockHeight::from_u32(11)),
            3,
            false,
        );
        assert!(matches!(
            transaction.status(),
            ConfirmationStatus::Confirmed(_)
        ));
    }

    #[test]
    fn update_status_failed_returns_to_mempool() {
        let mut transaction =
            transaction_with_status(ConfirmationStatus::Failed(BlockHeight::from_u32(10)));

        transaction.update_status(
            ConfirmationStatus::Mempool(BlockHeight::from_u32(11)),
            1,
            false,
        );
        assert!(matches!(
            transaction.status(),
            ConfirmationStatus::Mempool(_)
        ));
    }

    #[test]
    fn update_status_failed_is_not_directly_confirmable() {
        // confirming a failed transaction is handled by replacing the whole record during block
        // scanning, not by a status update.
        let mut transaction =
            transaction_with_status(ConfirmationStatus::Failed(BlockHeight::from_u32(10)));

        transaction.update_status(
            ConfirmationStatus::Confirmed(BlockHeight::from_u32(11)),
            1,
            false,
        );
        assert!(matches!(
            transaction.status(),
            ConfirmationStatus::Failed(_)
        ));
    }

    #[test]
    fn update_status_confirmed_is_terminal() {
        let mut transaction =
            transaction_with_status(ConfirmationStatus::Confirmed(BlockHeight::from_u32(10)));

        transaction.update_status(
            ConfirmationStatus::Mempool(BlockHeight::from_u32(11)),
            1,
            false,
        );
        assert!(matches!(
            transaction.status(),
            ConfirmationStatus::Confirmed(_)
        ));

        transaction.update_status(
            ConfirmationStatus::Failed(BlockHeight::from_u32(11)),
            2,
            false,
        );
        assert!(matches!(
            transaction.status(),
            ConfirmationStatus::Confirmed(_)
        ));
    }

    #[test]
    fn update_status_forces_confirmed_to_fail() {
        let mut transaction =
            transaction_with_status(ConfirmationStatus::Confirmed(BlockHeight::from_u32(10)));

        transaction.update_status(
            ConfirmationStatus::Failed(BlockHeight::from_u32(11)),
            2,
            true,
        );
        assert!(matches!(
            transaction.status(),
            ConfirmationStatus::Failed(_)
        ));
    }
}
