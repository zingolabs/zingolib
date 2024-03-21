use std::collections::HashMap;

use zcash_client_backend::ShieldedProtocol;
use zcash_primitives::transaction::TxId;

use super::transaction_record::TransactionRecord;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NoteRecordReference {
    pub txid: TxId,
    pub sh_pr: ShieldedProtocol,
    pub index: u32,
}

pub struct TransparentRecordRef {
    txid: TxId,
    index: u32,
}

pub struct RecordBook<'a> {
    pub all_transactions: &'a HashMap<TxId, TransactionRecord>,
}
