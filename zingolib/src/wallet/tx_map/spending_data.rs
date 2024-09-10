//! the subsection of TxMap that only applies to spending wallets

use std::collections::HashMap;

use zcash_primitives::transaction::{Transaction, TxId};

use crate::data::witness_trees::WitnessTrees;

/// the subsection of TxMap that only applies to spending wallets
pub(crate) struct SpendingData {
    witness_trees: WitnessTrees,
    cached_transactions: HashMap<TxId, Transaction>,
}

impl SpendingData {
    pub fn new(witness_trees: WitnessTrees) -> Self {
        SpendingData {
            witness_trees,
            cached_transactions: HashMap::new(),
        }
    }
    pub fn witness_trees(&self) -> &WitnessTrees {
        &self.witness_trees
    }
    pub fn witness_trees_mut(&mut self) -> &mut WitnessTrees {
        &mut self.witness_trees
    }
}
