//! the subsection of TxMap that only applies to spending wallets

use std::collections::HashMap;

use zcash_primitives::transaction::{Transaction, TxId};

use crate::data::witness_trees::WitnessTrees;

/// the subsection of TxMap that only applies to spending wallets
pub(crate) struct SpendingData {
    witness_trees: WitnessTrees,
    cached_raw_transactions: HashMap<TxId, Vec<u8>>,
}

impl SpendingData {
    pub fn new(witness_trees: WitnessTrees) -> Self {
        SpendingData {
            witness_trees,
            cached_raw_transactions: HashMap::new(),
        }
    }
    pub fn witness_trees(&self) -> &WitnessTrees {
        &self.witness_trees
    }
    pub fn witness_trees_mut(&mut self) -> &mut WitnessTrees {
        &mut self.witness_trees
    }
    pub fn cached_transactions(&self) -> &HashMap<TxId, Vec<u8>> {
        &self.cached_raw_transactions
    }
    pub fn cached_transactions_mut(&mut self) -> &mut HashMap<TxId, Vec<u8>> {
        &mut self.cached_raw_transactions
    }
}
