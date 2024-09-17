//! the subsection of TxMap that only applies to spending wallets

use std::collections::HashMap;

use getset::{Getters, MutGetters};

use zcash_primitives::transaction::TxId;

use crate::data::witness_trees::WitnessTrees;

/// the subsection of TxMap that only applies to spending wallets
#[derive(Getters, MutGetters)]
pub(crate) struct SpendingData {
    #[getset(get = "pub(crate)", get_mut = "pub(crate)")]
    witness_trees: WitnessTrees,
    #[getset(get = "pub(crate)", get_mut = "pub(crate)")]
    cached_raw_transactions: HashMap<TxId, Vec<u8>>,
}

impl SpendingData {
    pub fn new(witness_trees: WitnessTrees) -> Self {
        SpendingData {
            witness_trees,
            cached_raw_transactions: HashMap::new(),
        }
    }
}
