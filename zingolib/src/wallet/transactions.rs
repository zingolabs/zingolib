use std::collections::HashMap;

use zcash_client_backend::PoolType;
use zcash_primitives::transaction::TxId;

use crate::wallet::{
    data::WitnessTrees, notes::NoteRecordIdentifier, traits::DomainWalletExt,
    transaction_records_by_id::TransactionRecordsById,
};

use super::data::TransactionRecord;

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
    pub fn get_received_note_from_identifier<D: DomainWalletExt>(
        &self,
        note_record_reference: NoteRecordIdentifier,
    ) -> Option<
        zcash_client_backend::wallet::ReceivedNote<
            NoteRecordIdentifier,
            <D as zcash_note_encryption::Domain>::Note,
        >,
    >
    where
        <D as zcash_note_encryption::Domain>::Note: PartialEq + Clone,
        <D as zcash_note_encryption::Domain>::Recipient: super::traits::Recipient,
    {
        let transaction = self.get(&note_record_reference.txid);
        transaction.and_then(|transaction_record| {
            if note_record_reference.pool == PoolType::Shielded(D::protocol()) {
                transaction_record.get_received_note::<D>(note_record_reference.index)
            } else {
                None
            }
        })
    }
}
/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TxMapAndMaybeTrees {
    pub transaction_records_by_id: TransactionRecordsById,
    witness_trees: Option<WitnessTrees>,
}

pub mod get;
pub mod read_write;
pub mod recording;
mod trait_inputsource;

impl TxMapAndMaybeTrees {
    pub(crate) fn new_with_witness_trees() -> TxMapAndMaybeTrees {
        Self {
            transaction_records_by_id: TransactionRecordsById(HashMap::new()),
            witness_trees: Some(WitnessTrees::default()),
        }
    }
    pub(crate) fn new_treeless() -> TxMapAndMaybeTrees {
        Self {
            transaction_records_by_id: TransactionRecordsById(HashMap::new()),
            witness_trees: None,
        }
    }
    pub fn witness_trees(&self) -> Option<&WitnessTrees> {
        self.witness_trees.as_ref()
    }
    pub(crate) fn witness_trees_mut(&mut self) -> Option<&mut WitnessTrees> {
        self.witness_trees.as_mut()
    }
    pub fn clear(&mut self) {
        self.transaction_records_by_id.clear();
        self.witness_trees.as_mut().map(WitnessTrees::clear);
    }
}
