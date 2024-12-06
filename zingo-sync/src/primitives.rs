//! Module for primitive structs associated with the sync engine

use std::collections::{BTreeMap, BTreeSet};

use zcash_client_backend::data_api::scanning::{ScanPriority, ScanRange};
use zcash_keys::{address::UnifiedAddress, encoding::encode_payment_address};
use zcash_primitives::{
    block::BlockHash,
    consensus::{BlockHeight, NetworkConstants, Parameters},
    legacy::Script,
    memo::Memo,
    transaction::{components::amount::NonNegativeAmount, TxId},
};

use crate::{keys::transparent::TransparentAddressId, utils};

/// Block height and txid of relevant transactions that have yet to be scanned. These may be added due to spend
/// detections or transparent output discovery.
pub type Locator = (BlockHeight, TxId);

/// Encapsulates the current state of sync
pub struct SyncState {
    /// A vec of block ranges with scan priorities from wallet birthday to chain tip.
    /// In block height order with no overlaps or gaps.
    pub(crate) scan_ranges: Vec<ScanRange>,
    /// Block height and txid of known spends which are awaiting the scanning of the range it belongs to for transaction decryption.
    /// Locators for relevent transactions to the wallet.
    pub(crate) locators: BTreeSet<Locator>,
}

impl SyncState {
    /// Create new SyncState
    pub fn new() -> Self {
        SyncState {
            scan_ranges: Vec::new(),
            locators: BTreeSet::new(),
        }
    }

    /// Returns true if all scan ranges are scanned.
    pub(crate) fn scan_complete(&self) -> bool {
        self.scan_ranges
            .iter()
            .all(|scan_range| scan_range.priority() == ScanPriority::Scanned)
    }

    /// Returns the block height at which all blocks equal to and below this height are scanned.
    pub fn fully_scanned_height(&self) -> BlockHeight {
        if let Some(scan_range) = self
            .scan_ranges
            .iter()
            .find(|scan_range| scan_range.priority() != ScanPriority::Scanned)
        {
            scan_range.block_range().start - 1
        } else {
            self.scan_ranges
                .last()
                .expect("scan ranges always non-empty")
                .block_range()
                .end
        }
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

/// Output ID for a given pool type
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct OutputId {
    /// ID of associated transaction
    pub(crate) txid: TxId,
    /// Index of output within the transactions bundle of the given pool type.
    pub(crate) output_index: usize,
}

impl OutputId {
    /// Creates new OutputId from parts
    pub fn from_parts(txid: TxId, output_index: usize) -> Self {
        OutputId { txid, output_index }
    }
}

/// Binary tree map of nullifiers from transaction spends or actions
pub struct NullifierMap {
    pub(crate) sapling: BTreeMap<sapling_crypto::Nullifier, (BlockHeight, TxId)>,
    pub(crate) orchard: BTreeMap<orchard::note::Nullifier, (BlockHeight, TxId)>,
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

/// Binary tree map of out points (transparent spends)
#[derive(Debug)]
pub struct OutPointMap(BTreeMap<OutputId, Locator>);

impl OutPointMap {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn inner(&self) -> &BTreeMap<OutputId, Locator> {
        &self.0
    }

    pub fn inner_mut(&mut self) -> &mut BTreeMap<OutputId, Locator> {
        &mut self.0
    }
}

impl Default for OutPointMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Wallet block data
#[derive(Clone, Debug)]
pub struct WalletBlock {
    pub(crate) block_height: BlockHeight,
    pub(crate) block_hash: BlockHash,
    txids: Vec<TxId>,
    pub(crate) sapling_commitment_tree_size: u32,
    pub(crate) orchard_commitment_tree_size: u32,
}

impl WalletBlock {
    pub fn from_parts(
        block_height: BlockHeight,
        block_hash: BlockHash,
        txids: Vec<TxId>,
        sapling_commitment_tree_size: u32,
        orchard_commitment_tree_size: u32,
    ) -> Self {
        Self {
            block_height,
            block_hash,
            txids,
            sapling_commitment_tree_size,
            orchard_commitment_tree_size,
        }
    }

    pub fn txids(&self) -> &[TxId] {
        &self.txids
    }
}

/// Wallet transaction
pub struct WalletTransaction {
    pub(crate) transaction: zcash_primitives::transaction::Transaction,
    pub(crate) block_height: BlockHeight,
    pub(crate) sapling_notes: Vec<SaplingNote>,
    pub(crate) orchard_notes: Vec<OrchardNote>,
    outgoing_sapling_notes: Vec<OutgoingSaplingNote>,
    outgoing_orchard_notes: Vec<OutgoingOrchardNote>,
    transparent_coins: Vec<TransparentCoin>,
}

impl WalletTransaction {
    pub fn from_parts(
        transaction: zcash_primitives::transaction::Transaction,
        block_height: BlockHeight,
        sapling_notes: Vec<SaplingNote>,
        orchard_notes: Vec<OrchardNote>,
        outgoing_sapling_notes: Vec<OutgoingSaplingNote>,
        outgoing_orchard_notes: Vec<OutgoingOrchardNote>,
        transparent_coins: Vec<TransparentCoin>,
    ) -> Self {
        Self {
            transaction,
            block_height,
            sapling_notes,
            orchard_notes,
            outgoing_sapling_notes,
            outgoing_orchard_notes,
            transparent_coins,
        }
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

    pub fn transparent_coins(&self) -> &[TransparentCoin] {
        &self.transparent_coins
    }

    pub fn transparent_coins_mut(&mut self) -> Vec<&mut TransparentCoin> {
        self.transparent_coins.iter_mut().collect()
    }
}

impl std::fmt::Debug for WalletTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("WalletTransaction")
            .field("block_height", &self.block_height)
            .field("sapling_notes", &self.sapling_notes)
            .field("orchard_notes", &self.orchard_notes)
            .field("outgoing_sapling_notes", &self.outgoing_sapling_notes)
            .field("outgoing_orchard_notes", &self.outgoing_orchard_notes)
            .field("transparent_coins", &self.transparent_coins)
            .finish()
    }
}
pub type SaplingNote = WalletNote<sapling_crypto::Nullifier>;
pub type OrchardNote = WalletNote<orchard::note::Nullifier>;

/// Wallet note, shielded output with metadata relevant to the wallet
#[derive(Debug)]
pub struct WalletNote<Nf: Copy> {
    /// Derived nullifier
    pub(crate) nullifier: Option<Nf>, //TODO: syncing without nullfiier deriving key
    /// Memo
    pub(crate) memo: Memo,
    pub(crate) spending_transaction: Option<TxId>,
}

impl<Nf: Copy> WalletNote<Nf> {
    pub fn from_parts(
        nullifier: Option<Nf>,
        memo: Memo,
        spending_transaction: Option<TxId>,
    ) -> Self {
        Self {
            nullifier,
            memo,
            spending_transaction,
        }
    }
}

pub type OutgoingSaplingNote = OutgoingNote<sapling_crypto::Note>;
pub type OutgoingOrchardNote = OutgoingNote<orchard::Note>;

/// Note sent from this capability to a recipient
#[derive(Debug, Clone)]
pub struct OutgoingNote<N> {
    /// Decrypted note with recipient and value
    note: N,
    /// Recipient's full unified address from encoded memo
    pub(crate) recipient_ua: Option<UnifiedAddress>,
}

impl<N> OutgoingNote<N> {
    pub fn from_parts(note: N, recipient_ua: Option<UnifiedAddress>) -> Self {
        Self { note, recipient_ua }
    }
}

impl SyncOutgoingNotes for OutgoingNote<sapling_crypto::Note> {
    fn encoded_recipient<P>(&self, parameters: &P) -> String
    where
        P: Parameters + NetworkConstants,
    {
        encode_payment_address(
            parameters.hrp_sapling_payment_address(),
            &self.note.recipient(),
        )
    }
}

impl SyncOutgoingNotes for OutgoingNote<orchard::Note> {
    fn encoded_recipient<P>(&self, parameters: &P) -> String
    where
        P: Parameters + NetworkConstants,
    {
        utils::encode_orchard_receiver(parameters, &self.note.recipient()).unwrap()
    }
}

// TODO: condsider replacing with address enum instead of encoding to string
pub(crate) trait SyncOutgoingNotes {
    fn encoded_recipient<P>(&self, parameters: &P) -> String
    where
        P: Parameters + NetworkConstants;
}

///  Transparent coin (output) with metadata relevant to the wallet
#[derive(Debug)]
pub struct TransparentCoin {
    /// Output ID
    pub(crate) output_id: OutputId,
    /// Identifier for key used to derive address
    #[allow(dead_code)]
    key_id: TransparentAddressId,
    /// Encoded transparent address
    #[allow(dead_code)]
    address: String,
    /// Script
    #[allow(dead_code)]
    script: Script,
    /// Coin value
    #[allow(dead_code)]
    value: NonNegativeAmount,
    /// Spend status
    pub(crate) spending_transaction: Option<TxId>,
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
