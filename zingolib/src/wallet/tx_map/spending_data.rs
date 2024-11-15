//! the subsection of TxMap that only applies to spending wallets

use getset::{Getters, MutGetters};

use zcash_primitives::{legacy::keys::EphemeralIvk, transaction::TxId};

use crate::data::witness_trees::WitnessTrees;

/// the subsection of TxMap that only applies to spending wallets
#[derive(Getters, MutGetters)]
pub(crate) struct SpendingData {
    #[getset(get = "pub(crate)", get_mut = "pub(crate)")]
    witness_trees: WitnessTrees,
    #[getset(get = "pub(crate)", get_mut = "pub(crate)")]
    cached_raw_transactions: Vec<(TxId, Vec<u8>)>,
    #[getset(get = "pub(crate)", get_mut = "pub(crate)")]
    rejection_ivk: EphemeralIvk,
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
