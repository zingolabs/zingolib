use std::collections::HashMap;

use zcash_primitives::transaction::TxId;

use super::transaction_record::TransactionRecord;

pub struct RecordBook<'a> {
    pub all_transactions: &'a HashMap<TxId, TransactionRecord>,
}
