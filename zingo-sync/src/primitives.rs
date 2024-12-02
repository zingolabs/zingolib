//! Module for primitive structs associated with the sync engine

use std::collections::BTreeMap;

use incrementalmerkletree::Position;
use zcash_client_backend::data_api::scanning::ScanRange;
use zcash_keys::{address::UnifiedAddress, encoding::encode_payment_address};
use zcash_primitives::{
    block::BlockHash,
    consensus::{BlockHeight, NetworkConstants, Parameters},
    memo::Memo,
    transaction::TxId,
};

use crate::{keys::KeyId, utils};

/// Encapsulates the current state of sync
pub struct SyncState {
    /// A vec of block ranges with scan priorities from wallet birthday to chain tip.
    /// In block height order with no overlaps or gaps.
    pub(crate) scan_ranges: Vec<ScanRange>,
    /// Block height and txid of known spends which are awaiting the scanning of the range it belongs to for transaction decryption.
    #[allow(dead_code)]
    pub(crate) spend_locations: Vec<(BlockHeight, TxId)>,
}

impl SyncState {
    /// Create new SyncState
    pub fn new() -> Self {
        SyncState {
            scan_ranges: Vec::new(),
            spend_locations: Vec::new(),
        }
    }

    pub fn fully_scanned(&self) -> bool {
        self.scan_ranges.iter().all(|scan_range| {
            matches!(
                scan_range.priority(),
                zcash_client_backend::data_api::scanning::ScanPriority::Scanned
            )
        })
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

/// Output ID for a given pool type
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
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

/// Wallet block data
#[derive(Debug)]
pub struct WalletBlock {
    pub(crate) block_height: BlockHeight,
    pub(crate) block_hash: BlockHash,
    prev_hash: BlockHash,
    time: u32,
    txids: Vec<TxId>,
    pub(crate) sapling_commitment_tree_size: u32,
    pub(crate) orchard_commitment_tree_size: u32,
}

impl WalletBlock {
    pub fn from_parts(
        block_height: BlockHeight,
        block_hash: BlockHash,
        prev_hash: BlockHash,
        time: u32,
        txids: Vec<TxId>,
        sapling_commitment_tree_size: u32,
        orchard_commitment_tree_size: u32,
    ) -> Self {
        Self {
            block_height,
            block_hash,
            prev_hash,
            time,
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
#[derive(Debug)]
pub struct WalletTransaction {
    transaction: zcash_primitives::transaction::Transaction,
    block_height: BlockHeight,
    sapling_notes: Vec<SaplingNote>,
    orchard_notes: Vec<OrchardNote>,
    outgoing_sapling_notes: Vec<OutgoingSaplingNote>,
    outgoing_orchard_notes: Vec<OutgoingOrchardNote>,
}

impl WalletTransaction {
    pub fn from_parts(
        transaction: zcash_primitives::transaction::Transaction,
        block_height: BlockHeight,
        sapling_notes: Vec<SaplingNote>,
        orchard_notes: Vec<OrchardNote>,
        outgoing_sapling_notes: Vec<OutgoingSaplingNote>,
        outgoing_orchard_notes: Vec<OutgoingOrchardNote>,
    ) -> Self {
        Self {
            transaction,
            block_height,
            sapling_notes,
            orchard_notes,
            outgoing_sapling_notes,
            outgoing_orchard_notes,
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
}

pub type SaplingNote = WalletNote<sapling_crypto::Note, sapling_crypto::Nullifier>;
pub type OrchardNote = WalletNote<orchard::Note, orchard::note::Nullifier>;

/// Wallet note, shielded output with metadata relevant to the wallet
#[derive(Debug)]
pub struct WalletNote<N, Nf: Copy> {
    /// Output ID
    output_id: OutputId,
    /// Identifier for key used to decrypt output
    key_id: KeyId,
    /// Decrypted note with recipient and value
    note: N,
    /// Derived nullifier
    nullifier: Option<Nf>, //TODO: syncing without nullfiier deriving key
    /// Commitment tree leaf position
    position: Position,
    /// Memo
    pub(crate) memo: Memo,
    pub(crate) spending_transaction: Option<TxId>,
}

impl<N, Nf: Copy> WalletNote<N, Nf> {
    pub fn from_parts(
        output_id: OutputId,
        key_id: KeyId,
        note: N,
        nullifier: Option<Nf>,
        position: Position,
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

pub type OutgoingSaplingNote = OutgoingNote<sapling_crypto::Note>;
pub type OutgoingOrchardNote = OutgoingNote<orchard::Note>;

/// Note sent from this capability to a recipient
#[derive(Debug, Clone)]
pub struct OutgoingNote<N> {
    /// Output ID
    output_id: OutputId,
    /// Identifier for key used to decrypt output
    key_id: KeyId,
    /// Decrypted note with recipient and value
    note: N,
    /// Memo
    memo: Memo,
    /// Recipient's full unified address from encoded memo
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
