use std::collections::HashMap;

use zcash_primitives::transaction::TxId;

use super::data::{TransactionMetadata, WitnessTrees};

/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TransactionMetadataSet {
    pub current: HashMap<TxId, TransactionMetadata>,
    pub(crate) some_highest_txid: Option<TxId>,
    pub witness_trees: Option<WitnessTrees>,
}

pub mod get;
pub mod read_write;
pub mod recording;
pub mod shardtree;

impl TransactionMetadataSet {
    pub(crate) fn new_with_witness_trees() -> TransactionMetadataSet {
        Self {
            current: HashMap::default(),
            some_highest_txid: None,
            witness_trees: Some(WitnessTrees::default()),
        }
    }
    pub(crate) fn new_treeless() -> TransactionMetadataSet {
        Self {
            current: HashMap::default(),
            some_highest_txid: None,
            witness_trees: None,
        }
    }
    pub fn clear(&mut self) {
        self.current.clear();
        self.witness_trees.as_mut().map(WitnessTrees::clear);
    }
}
