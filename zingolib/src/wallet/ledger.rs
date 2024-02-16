use std::collections::HashMap;

use zcash_primitives::transaction::TxId;

use super::data::{TransactionRecord, WitnessTrees};

/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct ZingoLedger {
    pub current: HashMap<TxId, TransactionRecord>,
    pub witness_trees: Option<WitnessTrees>,
}

pub mod get;
pub mod recording;
pub mod serial_read_write;
pub mod walletread;
pub mod walletwrite;

impl ZingoLedger {
    pub(crate) fn new_with_witness_trees() -> ZingoLedger {
        Self {
            current: HashMap::default(),
            witness_trees: Some(WitnessTrees::default()),
        }
    }
    pub(crate) fn new_treeless() -> ZingoLedger {
        Self {
            current: HashMap::default(),
            witness_trees: None,
        }
    }
    pub fn clear(&mut self) {
        self.current.clear();
        self.witness_trees.as_mut().map(WitnessTrees::clear);
    }
}
