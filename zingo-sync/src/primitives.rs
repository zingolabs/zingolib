//! Module for primitive structs associated with the sync engine

use std::sync::{Arc, RwLock};

use getset::{CopyGetters, Getters, MutGetters};

use zcash_client_backend::data_api::scanning::ScanRange;
use zcash_primitives::{block::BlockHash, consensus::BlockHeight, transaction::TxId};

/// Encapsulates the current state of sync
#[derive(Getters, MutGetters)]
#[getset(get = "pub")]
pub struct SyncState {
    scan_ranges: Arc<RwLock<Vec<ScanRange>>>,
}

impl SyncState {
    /// Create new SyncState
    pub fn new() -> Self {
        SyncState {
            scan_ranges: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

/// Output ID for a given pool type
#[derive(PartialEq, Eq, Hash, Clone, Copy, CopyGetters)]
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

/// Wallet compact block data
#[allow(dead_code)]
#[derive(Clone, CopyGetters)]
#[getset(get_copy = "pub")]
pub struct WalletCompactBlock {
    block_height: BlockHeight,
    block_hash: BlockHash,
    prev_hash: BlockHash,
    time: u32,
    #[getset(skip)]
    txids: Vec<TxId>,
    sapling_commitment_tree_size: u32,
    orchard_commitment_tree_size: u32,
}

impl WalletCompactBlock {
    pub fn txids(&self) -> &[TxId] {
        &self.txids
    }
}
