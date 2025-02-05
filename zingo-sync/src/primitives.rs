//! Module for primitive structs associated with the sync engine

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

use getset::{CopyGetters, Getters, MutGetters, Setters};

use incrementalmerkletree::Position;
use zcash_client_backend::data_api::scanning::{ScanPriority, ScanRange};
use zcash_keys::{address::UnifiedAddress, encoding::encode_payment_address};
use zcash_primitives::{
    block::BlockHash,
    consensus::{BlockHeight, NetworkConstants, Parameters},
    legacy::Script,
    memo::Memo,
    transaction::{components::amount::NonNegativeAmount, TxId},
};
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::{
    keys::{transparent::TransparentAddressId, KeyId},
    utils,
};

/// Block height and txid of relevant transactions that have yet to be scanned. These may be added due to transparent
/// output/spend discovery or for targetted rescan.
pub type Locator = (BlockHeight, TxId);

/// Initial sync state.
///
/// All fields will be reset when a new sync session starts.
#[derive(Debug, Clone, CopyGetters, Setters)]
#[getset(get_copy = "pub", set = "pub")]
pub struct InitialSyncState {
    /// One block above the fully scanned wallet height at start of sync session.
    sync_start_height: BlockHeight,
    /// The tree sizes of the fully scanned height and chain tip at start of sync session.
    sync_tree_boundaries: TreeBoundaries,
    /// Total number of blocks to scan.
    total_blocks_to_scan: u32,
    /// Total number of sapling outputs to scan.
    total_sapling_outputs_to_scan: u32,
    /// Total number of orchard outputs to scan.
    total_orchard_outputs_to_scan: u32,
}

impl InitialSyncState {
    /// Create new InitialSyncState
    pub fn new() -> Self {
        InitialSyncState {
            sync_start_height: 0.into(),
            sync_tree_boundaries: TreeBoundaries {
                sapling_initial_tree_size: 0,
                sapling_final_tree_size: 0,
                orchard_initial_tree_size: 0,
                orchard_final_tree_size: 0,
            },
            total_blocks_to_scan: 0,
            total_sapling_outputs_to_scan: 0,
            total_orchard_outputs_to_scan: 0,
        }
    }
}

impl Default for InitialSyncState {
    fn default() -> Self {
        Self::new()
    }
}

/// Encapsulates the current state of sync
#[derive(Debug, Clone, Getters, MutGetters, CopyGetters, Setters)]
#[getset(get = "pub", get_mut = "pub")]
pub struct SyncState {
    /// A vec of block ranges with scan priorities from wallet birthday to chain tip.
    /// In block height order with no overlaps or gaps.
    scan_ranges: Vec<ScanRange>,
    /// The block ranges that contain all sapling outputs of complete sapling shards.
    ///
    /// There is an edge case where a range may include two (or more) shards. However, this only occurs when the lower
    /// shards are already scanned so will cause no issues when punching in the higher scan priorites.
    sapling_shard_ranges: Vec<Range<BlockHeight>>,
    /// The block ranges that contain all orchard outputs of complete orchard shards.
    ///
    /// There is an edge case where a range may include two (or more) shards. However, this only occurs when the lower
    /// shards are already scanned so will cause no issues when punching in the higher scan priorites.
    orchard_shard_ranges: Vec<Range<BlockHeight>>,
    /// Locators for relevant transactions to the wallet.
    locators: BTreeSet<Locator>,
    /// Initial sync state.
    initial_sync_state: InitialSyncState,
}

impl SyncState {
    /// Create new SyncState
    pub fn new() -> Self {
        SyncState {
            scan_ranges: Vec::new(),
            sapling_shard_ranges: Vec::new(),
            orchard_shard_ranges: Vec::new(),
            locators: BTreeSet::new(),
            initial_sync_state: InitialSyncState::new(),
        }
    }

    /// Returns true if all scan ranges are scanned.
    pub(crate) fn scan_complete(&self) -> bool {
        self.scan_ranges()
            .iter()
            .all(|scan_range| scan_range.priority() == ScanPriority::Scanned)
    }

    /// Returns the block height at which all blocks equal to and below this height are scanned.
    ///
    /// Will panic if called before scan ranges are updated for the first time.
    pub fn fully_scanned_height(&self) -> BlockHeight {
        if let Some(scan_range) = self
            .scan_ranges()
            .iter()
            .find(|scan_range| scan_range.priority() != ScanPriority::Scanned)
        {
            scan_range.block_range().start - 1
        } else {
            self.scan_ranges()
                .last()
                .expect("scan ranges always non-empty")
                .block_range()
                .end
        }
    }

    /// Returns the highest block height that has been scanned.
    ///
    /// If no scan ranges have been scanned, returns the block below the wallet birthday.
    /// Will panic if called before scan ranges are updated for the first time.
    pub fn highest_scanned_height(&self) -> BlockHeight {
        if let Some(last_scanned_range) = self
            .scan_ranges()
            .iter()
            .filter(|scan_range| scan_range.priority() == ScanPriority::Scanned)
            .last()
        {
            last_scanned_range.block_range().end - 1
        } else {
            self.wallet_birthday()
                .expect("scan ranges always non-empty")
                - 1
        }
    }

    /// Returns the wallet birthday or `None` if `self.scan_ranges` is empty.
    ///
    /// If the wallet birthday is below the sapling activation height, returns the sapling activation height instead.
    pub fn wallet_birthday(&self) -> Option<BlockHeight> {
        self.scan_ranges()
            .first()
            .map(|range| range.block_range().start)
    }

    /// Returns the last known chain height to the wallet or `None` if `self.scan_ranges` is empty.
    pub fn wallet_height(&self) -> Option<BlockHeight> {
        self.scan_ranges()
            .last()
            .map(|range| range.block_range().end - 1)
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TreeBoundaries {
    pub sapling_initial_tree_size: u32,
    pub sapling_final_tree_size: u32,
    pub orchard_initial_tree_size: u32,
    pub orchard_final_tree_size: u32,
}

/// A snapshot of the current state of sync. Useful for displaying the status of sync to a user / consumer.
///
/// `percentage_outputs_scanned` is a much more accurate indicator of sync completion than `percentage_blocks_scanned`.
#[derive(Debug, Clone, Getters)]
pub struct SyncStatus {
    pub scan_ranges: Vec<ScanRange>,
    pub scanned_blocks: u32,
    pub unscanned_blocks: u32,
    pub percentage_blocks_scanned: f32,
    pub scanned_sapling_outputs: u32,
    pub unscanned_sapling_outputs: u32,
    pub scanned_orchard_outputs: u32,
    pub unscanned_orchard_outputs: u32,
    pub percentage_outputs_scanned: f32,
}

/// Output ID for a given pool type
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, CopyGetters)]
#[getset(get_copy = "pub")]
pub struct OutputId {
    /// ID of associated transaction
    txid: TxId,
    /// Index of output within the transactions bundle of the given pool type.
    output_index: usize,
}

impl OutputId {
    /// Creates new OutputId from parts
    pub fn from_parts(txid: TxId, output_index: usize) -> Self {
        OutputId { txid, output_index }
    }
}

/// Binary tree map of nullifiers from transaction spends or actions
#[derive(Debug, Getters, MutGetters)]
#[getset(get = "pub", get_mut = "pub")]
pub struct NullifierMap {
    sapling: BTreeMap<sapling_crypto::Nullifier, Locator>,
    orchard: BTreeMap<orchard::note::Nullifier, Locator>,
}

impl NullifierMap {
    pub fn new() -> Self {
        Self {
            sapling: BTreeMap::new(),
            orchard: BTreeMap::new(),
        }
    }
}

impl Default for NullifierMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Wallet block data
#[derive(Debug, Clone, CopyGetters)]
#[getset(get_copy = "pub")]
pub struct WalletBlock {
    block_height: BlockHeight,
    block_hash: BlockHash,
    prev_hash: BlockHash,
    time: u32,
    #[getset(skip)]
    txids: Vec<TxId>,
    tree_boundaries: TreeBoundaries,
    // TODO: optional price
}

impl WalletBlock {
    pub fn from_parts(
        block_height: BlockHeight,
        block_hash: BlockHash,
        prev_hash: BlockHash,
        time: u32,
        txids: Vec<TxId>,
        tree_boundaries: TreeBoundaries,
    ) -> Self {
        Self {
            block_height,
            block_hash,
            prev_hash,
            time,
            txids,
            tree_boundaries,
        }
    }

    pub fn txids(&self) -> &[TxId] {
        &self.txids
    }
}

/// Wallet transaction
#[derive(Getters, CopyGetters)]
pub struct WalletTransaction {
    #[getset(get_copy = "pub")]
    txid: TxId,
    #[getset(get = "pub")]
    transaction: zcash_primitives::transaction::Transaction,
    #[getset(get_copy = "pub")]
    status: ConfirmationStatus,
    #[getset(skip)]
    transparent_coins: Vec<TransparentCoin>,
    #[getset(skip)]
    sapling_notes: Vec<SaplingNote>,
    #[getset(skip)]
    orchard_notes: Vec<OrchardNote>,
    #[getset(skip)]
    outgoing_sapling_notes: Vec<OutgoingSaplingNote>,
    #[getset(skip)]
    outgoing_orchard_notes: Vec<OutgoingOrchardNote>,
}

impl WalletTransaction {
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        txid: TxId,
        transaction: zcash_primitives::transaction::Transaction,
        status: ConfirmationStatus,
        transparent_coins: Vec<TransparentCoin>,
        sapling_notes: Vec<SaplingNote>,
        orchard_notes: Vec<OrchardNote>,
        outgoing_sapling_notes: Vec<OutgoingSaplingNote>,
        outgoing_orchard_notes: Vec<OutgoingOrchardNote>,
    ) -> Self {
        Self {
            txid,
            transaction,
            status,
            sapling_notes,
            orchard_notes,
            outgoing_sapling_notes,
            outgoing_orchard_notes,
            transparent_coins,
        }
    }

    pub fn transparent_coins(&self) -> &[TransparentCoin] {
        &self.transparent_coins
    }

    pub fn transparent_coins_mut(&mut self) -> Vec<&mut TransparentCoin> {
        self.transparent_coins.iter_mut().collect()
    }
    pub fn sapling_notes(&self) -> &[SaplingNote] {
        &self.sapling_notes
    }

    pub fn sapling_notes_mut(&mut self) -> Vec<&mut SaplingNote> {
        self.sapling_notes.iter_mut().collect()
    }

    pub fn orchard_notes(&self) -> &[OrchardNote] {
        &self.orchard_notes
    }

    pub fn orchard_notes_mut(&mut self) -> Vec<&mut OrchardNote> {
        self.orchard_notes.iter_mut().collect()
    }

    pub fn outgoing_sapling_notes(&self) -> &[OutgoingSaplingNote] {
        &self.outgoing_sapling_notes
    }

    pub fn outgoing_orchard_notes(&self) -> &[OutgoingOrchardNote] {
        &self.outgoing_orchard_notes
    }
}

impl std::fmt::Debug for WalletTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("WalletTransaction")
            .field("confirmation_status", &self.status)
            .field("transparent_coins", &self.transparent_coins)
            .field("sapling_notes", &self.sapling_notes)
            .field("orchard_notes", &self.orchard_notes)
            .field("outgoing_sapling_notes", &self.outgoing_sapling_notes)
            .field("outgoing_orchard_notes", &self.outgoing_orchard_notes)
            .finish()
    }
}

/// Wallet note, shielded output with metadata relevant to the wallet
#[derive(Debug, Clone)]
pub struct WalletNote<N, Nf: Copy> {
    /// Output ID
    pub(crate) output_id: OutputId,
    /// Identifier for key used to decrypt output
    pub(crate) key_id: KeyId,
    /// Decrypted note with recipient and value
    pub(crate) note: N,
    /// Derived nullifier
    pub(crate) nullifier: Option<Nf>, //TODO: syncing without nullifier deriving key
    /// Commitment tree leaf position
    pub(crate) position: Option<Position>,
    /// Memo
    pub(crate) memo: Memo,
    /// Transaction this note was spent in.
    /// If `None`, note is not spent.
    pub(crate) spending_transaction: Option<TxId>,
}

impl<N, Nf: Copy> WalletNote<N, Nf> {
    pub fn from_parts(
        output_id: OutputId,
        key_id: KeyId,
        note: N,
        nullifier: Option<Nf>,
        position: Option<Position>,
        memo: Memo,
        spending_transaction: Option<TxId>,
    ) -> Self {
        Self {
            output_id,
            key_id,
            note,
            nullifier,
            position,
            memo,
            spending_transaction,
        }
    }
}

pub trait NoteInterface: Sized {
    type ZcashNote;
    type Nullifier: Copy;

    /// Output ID
    fn output_id(&self) -> OutputId;

    /// Identifier for key used to decrypt output
    fn key_id(&self) -> KeyId;

    /// Decrypted note with recipient and value
    fn note(&self) -> &Self::ZcashNote;

    /// Derived nullifier
    fn nullifier(&self) -> Option<Self::Nullifier>;

    /// Commitment tree leaf position
    fn position(&self) -> Option<Position>;

    /// Memo
    fn memo(&self) -> &Memo;

    /// Txid of transaction this note was spent in.
    /// If `None`, note is not spent.
    fn spending_transaction(&self) -> Option<TxId>;

    /// Note value.
    fn value(&self) -> u64;

    /// Notes within `transaction`.
    fn transaction_notes(transaction: &WalletTransaction) -> &[Self];
}

pub type SaplingNote = WalletNote<sapling_crypto::Note, sapling_crypto::Nullifier>;

impl NoteInterface for SaplingNote {
    type ZcashNote = sapling_crypto::Note;
    type Nullifier = sapling_crypto::Nullifier;

    fn output_id(&self) -> OutputId {
        self.output_id
    }

    fn key_id(&self) -> KeyId {
        self.key_id
    }

    fn note(&self) -> &Self::ZcashNote {
        &self.note
    }

    fn nullifier(&self) -> Option<sapling_crypto::Nullifier> {
        self.nullifier
    }

    fn position(&self) -> Option<Position> {
        self.position
    }

    fn memo(&self) -> &Memo {
        &self.memo
    }

    fn spending_transaction(&self) -> Option<TxId> {
        self.spending_transaction
    }

    fn value(&self) -> u64 {
        self.note.value().inner()
    }

    fn transaction_notes(transaction: &WalletTransaction) -> &[SaplingNote] {
        transaction.sapling_notes()
    }
}

pub type OrchardNote = WalletNote<orchard::Note, orchard::note::Nullifier>;

impl NoteInterface for OrchardNote {
    type ZcashNote = orchard::Note;
    type Nullifier = orchard::note::Nullifier;

    fn output_id(&self) -> OutputId {
        self.output_id
    }

    fn key_id(&self) -> KeyId {
        self.key_id
    }

    fn note(&self) -> &Self::ZcashNote {
        &self.note
    }

    fn nullifier(&self) -> Option<orchard::note::Nullifier> {
        self.nullifier
    }

    fn position(&self) -> Option<Position> {
        self.position
    }

    fn memo(&self) -> &Memo {
        &self.memo
    }

    fn spending_transaction(&self) -> Option<TxId> {
        self.spending_transaction
    }

    fn value(&self) -> u64 {
        self.note.value().inner()
    }

    fn transaction_notes(transaction: &WalletTransaction) -> &[OrchardNote] {
        transaction.orchard_notes()
    }
}

pub type OutgoingSaplingNote = OutgoingNote<sapling_crypto::Note>;
pub type OutgoingOrchardNote = OutgoingNote<orchard::Note>;

/// Note sent from this capability to a recipient
#[derive(Debug, Clone, Getters, CopyGetters, Setters)]
pub struct OutgoingNote<N> {
    /// Output ID
    #[getset(get_copy = "pub")]
    output_id: OutputId,
    /// Identifier for key used to decrypt output
    #[getset(get_copy = "pub")]
    key_id: KeyId,
    /// Decrypted note with recipient and value
    #[getset(get = "pub")]
    note: N,
    /// Memo
    #[getset(get = "pub")]
    memo: Memo,
    /// Recipient's full unified address from encoded memo
    #[getset(get = "pub", set = "pub")]
    recipient_ua: Option<UnifiedAddress>,
}

impl<N> OutgoingNote<N> {
    pub fn from_parts(
        output_id: OutputId,
        key_id: KeyId,
        note: N,
        memo: Memo,
        recipient_ua: Option<UnifiedAddress>,
    ) -> Self {
        Self {
            output_id,
            key_id,
            note,
            memo,
            recipient_ua,
        }
    }
}

impl SyncOutgoingNotes for OutgoingNote<sapling_crypto::Note> {
    fn encoded_recipient<P>(&self, parameters: &P) -> String
    where
        P: Parameters + NetworkConstants,
    {
        encode_payment_address(
            parameters.hrp_sapling_payment_address(),
            &self.note().recipient(),
        )
    }
}

impl SyncOutgoingNotes for OutgoingNote<orchard::Note> {
    fn encoded_recipient<P>(&self, parameters: &P) -> String
    where
        P: Parameters + NetworkConstants,
    {
        utils::encode_orchard_receiver(parameters, &self.note().recipient()).unwrap()
    }
}

// TODO: consider replacing with address enum instead of encoding to string
pub(crate) trait SyncOutgoingNotes {
    fn encoded_recipient<P>(&self, parameters: &P) -> String
    where
        P: Parameters + NetworkConstants;
}

///  Transparent coin (output) with metadata relevant to the wallet
#[derive(Debug, Clone, Getters, CopyGetters, Setters)]
pub struct TransparentCoin {
    /// Output ID
    #[getset(get_copy = "pub")]
    output_id: OutputId,
    /// Identifier for key used to derive address
    #[getset(get_copy = "pub")]
    key_id: TransparentAddressId,
    /// Encoded transparent address
    #[getset(get = "pub")]
    address: String,
    /// Script
    #[getset(get = "pub")]
    script: Script,
    /// Coin value
    #[getset(get_copy = "pub")]
    value: NonNegativeAmount,
    /// Spend status
    #[getset(get = "pub", set = "pub")]
    spending_transaction: Option<TxId>,
}

impl TransparentCoin {
    pub fn from_parts(
        output_id: OutputId,
        key_id: TransparentAddressId,
        address: String,
        script: Script,
        value: NonNegativeAmount,
        spending_transaction: Option<TxId>,
    ) -> Self {
        Self {
            output_id,
            key_id,
            address,
            script,
            value,
            spending_transaction,
        }
    }
}
