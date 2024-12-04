//! the subsection of TxMap that only applies to spending wallets

use zcash_primitives::{legacy::keys::EphemeralIvk, transaction::TxId};

use crate::data::witness_trees::WitnessTrees;

/// the subsection of TxMap that only applies to spending wallets
pub(crate) struct SpendingData {
    pub(crate) witness_trees: WitnessTrees,
    pub(crate) cached_raw_transactions: Vec<(TxId, Vec<u8>)>,
    pub(crate) rejection_ivk: EphemeralIvk,
}

impl SpendingData {
    pub fn new(witness_trees: WitnessTrees, rejection_ivk: EphemeralIvk) -> Self {
        SpendingData {
            witness_trees,
            cached_raw_transactions: Vec::new(),
            rejection_ivk,
        }
    }
}
