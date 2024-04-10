use std::collections::HashMap;

use zcash_primitives::transaction::TxId;

use crate::wallet::data::TransactionRecord;

#[derive(Debug)]
pub struct TransactionRecordMap(pub HashMap<TxId, TransactionRecord>);

impl std::ops::Deref for TransactionRecordMap {
    type Target = HashMap<TxId, TransactionRecord>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for TransactionRecordMap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl TransactionRecordMap {
    // Associated function to create a TransactionRecordMap from a HashMap
    pub fn from_map(map: HashMap<TxId, TransactionRecord>) -> Self {
        TransactionRecordMap(map)
    }
}
