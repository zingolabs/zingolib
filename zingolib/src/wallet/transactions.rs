use std::collections::HashMap;

use zcash_primitives::transaction::TxId;

use super::data::{TransactionRecord, WitnessTrees};

pub struct TransactionRecordMap {
    pub map: HashMap<TxId, TransactionRecord>,
}

impl TransactionRecordMap {
    fn new_empty() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
    fn from_map(map: HashMap<TxId, TransactionRecord>) -> Self {
        Self { map }
    }
}

/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TxMapAndMaybeTrees {
    pub current: TransactionRecordMap,
    pub witness_trees: Option<WitnessTrees>,
}

pub mod get;
pub mod read_write;
pub mod recording;

impl TxMapAndMaybeTrees {
    pub(crate) fn new_with_witness_trees() -> TxMapAndMaybeTrees {
        Self {
            current: TransactionRecordMap::new_empty(),
            witness_trees: Some(WitnessTrees::default()),
        }
    }
    pub(crate) fn new_treeless() -> TxMapAndMaybeTrees {
        Self {
            current: TransactionRecordMap::new_empty(),
            witness_trees: None,
        }
    }
    pub fn clear(&mut self) {
        self.current.map.clear();
        self.witness_trees.as_mut().map(WitnessTrees::clear);
    }
}
