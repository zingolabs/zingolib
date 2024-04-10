use std::collections::HashMap;

use crate::wallet::{
    data::WitnessTrees, spending_data::SpendingData,
    transaction_records_by_id::TransactionRecordsById,
};

/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TxMapAndMaybeSpendingData {
    pub transaction_records_by_id: TransactionRecordsById,
    spending_data: Option<SpendingData>,
}

pub mod get;
pub mod read_write;
pub mod recording;

impl TxMapAndMaybeSpendingData {
    pub(crate) fn new_spending() -> TxMapAndMaybeSpendingData {
        Self {
            transaction_records_by_id: TransactionRecordsById(HashMap::new()),
            spending_data: Some(SpendingData::default()),
        }
    }
    pub(crate) fn new_viewing() -> TxMapAndMaybeSpendingData {
        Self {
            transaction_records_by_id: TransactionRecordsById(HashMap::new()),
            spending_data: None,
        }
    }
    pub fn witness_trees(&self) -> Option<&WitnessTrees> {
        self.spending_data
            .as_ref()
            .map(|spending_data| spending_data.witness_trees())
    }
    pub(crate) fn witness_trees_mut(&mut self) -> Option<&mut WitnessTrees> {
        self.spending_data
            .as_mut()
            .map(|spending_data| spending_data.witness_trees_mut())
    }
    pub fn clear(&mut self) {
        self.transaction_records_by_id.clear();
        self.spending_data.as_mut().map(SpendingData::clear);
    }
}
