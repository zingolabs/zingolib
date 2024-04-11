use std::collections::HashMap;

use zcash_primitives::transaction::TxId;

use crate::wallet::data::TransactionRecord;

#[derive(Debug)]
pub struct TransactionRecordsById(pub HashMap<TxId, TransactionRecord>);

impl std::ops::Deref for TransactionRecordsById {
    type Target = HashMap<TxId, TransactionRecord>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for TransactionRecordsById {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl TransactionRecordsById {
    // Associated function to create a TransactionRecordMap from a HashMap
    pub fn new() -> Self {
        TransactionRecordsById(HashMap::new())
    }
    pub fn from_map(map: HashMap<TxId, TransactionRecord>) -> Self {
        TransactionRecordsById(map)
    }
}
