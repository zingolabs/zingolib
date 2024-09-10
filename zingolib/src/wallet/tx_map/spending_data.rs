//! the subsection of TxMap that only applies to spending wallets

use crate::data::witness_trees::WitnessTrees;

/// the subsection of TxMap that only applies to spending wallets
pub(crate) struct SpendingData {
    witness_trees: WitnessTrees,
}

impl SpendingData {
    pub fn new(witness_trees: WitnessTrees) -> Self {
        SpendingData { witness_trees }
    }
    pub fn witness_trees(&self) -> &WitnessTrees {
        &self.witness_trees
    }
    pub fn witness_trees_mut(&mut self) -> &mut WitnessTrees {
        &mut self.witness_trees
    }
}
