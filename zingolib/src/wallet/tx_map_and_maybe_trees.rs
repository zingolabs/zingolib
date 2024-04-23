//! This mod should be called tx_map_and_maybe_trees.rs. it contains
//! struct TxMapAndMaybeTrees
//! implementations for TxMapAndMaybeTrees
//! associated types for TxMapAndMaybeTrees that have no relevance elsewhere.

use crate::wallet::{data::WitnessTrees, transaction_records_by_id::TransactionRecordsById};

/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TxMapAndMaybeTrees {
    pub transaction_records_by_id: TransactionRecordsById,
    witness_trees: Option<WitnessTrees>,
}

pub mod get;
pub mod read_write;
pub mod recording;

impl TxMapAndMaybeTrees {
    pub(crate) fn new_with_witness_trees() -> TxMapAndMaybeTrees {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            witness_trees: Some(WitnessTrees::default()),
        }
    }
    pub(crate) fn new_treeless() -> TxMapAndMaybeTrees {
        Self {
            transaction_records_by_id: TransactionRecordsById::new(),
            witness_trees: None,
        }
    }
    pub fn witness_trees(&self) -> Option<&WitnessTrees> {
        self.witness_trees.as_ref()
    }
    pub(crate) fn witness_trees_mut(&mut self) -> Option<&mut WitnessTrees> {
        self.witness_trees.as_mut()
    }
    pub fn clear(&mut self) {
        self.transaction_records_by_id.clear();
        self.witness_trees.as_mut().map(WitnessTrees::clear);
    }
}

pub mod trait_walletread;
