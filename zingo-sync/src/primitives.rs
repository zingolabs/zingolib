//! Module for primitive structs associated with the sync engine

use std::collections::BTreeMap;

use getset::{CopyGetters, Getters, MutGetters};

use incrementalmerkletree::Position;
use zcash_client_backend::data_api::scanning::ScanRange;
use zcash_keys::{address::UnifiedAddress, encoding::encode_payment_address};
use zcash_primitives::{
    block::BlockHash,
    consensus::{BlockHeight, NetworkConstants, Parameters},
    memo::Memo,
    transaction::TxId,
};

use crate::utils;

/// Encapsulates the current state of sync
#[derive(Debug, Getters, MutGetters)]
#[getset(get = "pub", get_mut = "pub")]
pub struct SyncState {
    scan_ranges: Vec<ScanRange>,
}

impl SyncState {
    /// Create new SyncState
    pub fn new() -> Self {
        SyncState {
            scan_ranges: Vec::new(),
        }
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

/// Output ID for a given pool type
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy, CopyGetters)]
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
#[derive(Debug, MutGetters)]
#[getset(get = "pub", get_mut = "pub")]
pub struct NullifierMap {
    sapling: BTreeMap<sapling_crypto::Nullifier, (BlockHeight, TxId)>,
    orchard: BTreeMap<orchard::note::Nullifier, (BlockHeight, TxId)>,
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
    sapling_commitment_tree_size: u32,
    orchard_commitment_tree_size: u32,
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
#[derive(Debug, CopyGetters)]
#[getset(get_copy = "pub")]
pub struct WalletTransaction {
    #[getset(get_copy = "pub")]
    txid: TxId,
    #[getset(get_copy = "pub")]
    block_height: BlockHeight,
    #[getset(skip)]
    sapling_notes: Vec<SaplingNote>,
    #[getset(skip)]
    orchard_notes: Vec<OrchardNote>,
}

impl WalletTransaction {
    pub fn from_parts(
        txid: TxId,
        block_height: BlockHeight,
        sapling_notes: Vec<SaplingNote>,
        orchard_notes: Vec<OrchardNote>,
    ) -> Self {
        Self {
            txid,
            block_height,
            sapling_notes,
            orchard_notes,
        }
    }

    pub fn sapling_notes(&self) -> &[SaplingNote] {
        &self.sapling_notes
    }

    pub fn orchard_notes(&self) -> &[OrchardNote] {
        &self.orchard_notes
    }
}

#[derive(Debug, Getters, CopyGetters)]
pub struct SaplingNote {
    #[getset(get_copy = "pub")]
    output_id: OutputId,
    #[getset(get = "pub")]
    note: sapling_crypto::Note,
    #[getset(get_copy = "pub")]
    nullifier: sapling_crypto::Nullifier, //TODO: make option and add handling for syncing without nullfiier deriving key
    #[getset(get_copy = "pub")]
    position: Position,
    #[getset(get = "pub")]
    memo: Memo,
}

impl SyncNote for SaplingNote {
    type WalletNote = Self;
    type ZcashNote = sapling_crypto::Note;
    type Nullifier = sapling_crypto::Nullifier;
    type Memo = Memo;

    fn from_parts(
        output_id: OutputId,
        note: Self::ZcashNote,
        nullifier: Self::Nullifier,
        position: Position,
        memo: Self::Memo,
    ) -> Self::WalletNote {
        Self {
            output_id,
            note,
            nullifier,
            position,
            memo,
        }
    }

    fn memo(&self) -> &Self::Memo {
        &self.memo
    }
}

#[derive(Debug, Getters, CopyGetters)]
pub struct OrchardNote {
    #[getset(get_copy = "pub")]
    output_id: OutputId,
    #[getset(get = "pub")]
    note: orchard::Note,
    #[getset(get_copy = "pub")]
    nullifier: orchard::note::Nullifier, //TODO: make option and add handling for syncing without nullfiier deriving key
    #[getset(get_copy = "pub")]
    position: Position,
    memo: Memo,
}

impl SyncNote for OrchardNote {
    type WalletNote = Self;
    type ZcashNote = orchard::Note;
    type Nullifier = orchard::note::Nullifier;
    type Memo = Memo;

    fn from_parts(
        output_id: OutputId,
        note: Self::ZcashNote,
        nullifier: Self::Nullifier,
        position: Position,
        memo: Self::Memo,
    ) -> Self::WalletNote {
        Self {
            output_id,
            note,
            nullifier,
            position,
            memo,
        }
    }

    fn memo(&self) -> &Self::Memo {
        &self.memo
    }
}

pub(crate) trait SyncNote {
    type WalletNote;
    type ZcashNote;
    type Nullifier: Copy;
    type Memo;

    fn from_parts(
        output_id: OutputId,
        note: Self::ZcashNote,
        nullifier: Self::Nullifier,
        position: Position,
        memo: Self::Memo,
    ) -> Self::WalletNote;

    fn memo(&self) -> &Self::Memo;
}

/// Note sent from this capability to a recipient
#[derive(Debug, Clone, Getters, CopyGetters, MutGetters)]
pub struct OutgoingNote<N> {
    /// Output ID
    #[getset(get_copy = "pub")]
    output_id: OutputId,
    /// Decrypted note with recipient and value
    #[getset(get = "pub")]
    note: N,
    /// Memo
    #[getset(get = "pub")]
    memo: Memo,
    /// Recipient's full unified address from encoded memo
    #[getset(get = "pub", get_mut = "pub")]
    recipient_ua: Option<UnifiedAddress>,
}

impl<N> OutgoingNote<N> {
    pub fn from_parts(
        output_id: OutputId,
        note: N,
        memo: Memo,
        recipient_ua: Option<UnifiedAddress>,
    ) -> Self {
        Self {
            output_id,
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

pub(crate) trait SyncOutgoingNotes {
    fn encoded_recipient<P>(&self, parameters: &P) -> String
    where
        P: Parameters + NetworkConstants;
}
