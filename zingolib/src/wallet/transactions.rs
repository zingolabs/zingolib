use std::collections::HashMap;

use zcash_primitives::transaction::TxId;

use super::data::{TransactionRecord, WitnessTrees};

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
    pub fn get_received_note_from_identifier(
        &self,
        note_record_reference: super::notes::NoteRecordIdentifier,
    ) -> Option<
        zcash_client_backend::wallet::ReceivedNote<
            super::notes::NoteRecordIdentifier,
            zcash_client_backend::wallet::Note,
        >,
    > {
        let transaction = self.get(&note_record_reference.txid);
        transaction.and_then(|transaction_record| match note_record_reference.pool {
            zcash_client_backend::PoolType::Transparent => None, // explain implication
            zcash_client_backend::PoolType::Shielded(domain) => match domain {
                zcash_client_backend::ShieldedProtocol::Sapling => {
                    transaction_record
                        .get_received_note::<sapling_crypto::note_encryption::SaplingDomain>(
                            note_record_reference.index,
                        )
                }
                zcash_client_backend::ShieldedProtocol::Orchard => {
                    transaction_record.get_received_note::<orchard::note_encryption::OrchardDomain>(
                        note_record_reference.index,
                    )
                }
            },
        })
    }
}
#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use crate::wallet::transaction_record::TransactionRecord;

    use super::TransactionRecordMap;

    #[test]
    fn trm_get_received_note_from_identifier() {
        let single_transparent_trans_record =
            crate::test_framework::create_transaction_record_with_one_tnote();
        let txid = single_transparent_trans_record.txid;
        let hashmap = HashMap::from([(txid, single_transparent_trans_record)]);
        let trm = TransactionRecordMap::from_map(hashmap);
        let identifier = crate::test_framework::create_note_record_id();
        let note = trm.get_received_note_from_identifier(identifier);
        assert_eq!(note, "satan");
    }
}
/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TxMapAndMaybeTrees {
    pub current: TransactionRecordMap,
    pub witness_trees: Option<WitnessTrees>,
}

pub mod get;
pub mod read_write;
pub mod recording;

impl TxMapAndMaybeTrees {
    pub(crate) fn new_with_witness_trees() -> TxMapAndMaybeTrees {
        Self {
            current: TransactionRecordMap(HashMap::new()),
            witness_trees: Some(WitnessTrees::default()),
        }
    }
    pub(crate) fn new_treeless() -> TxMapAndMaybeTrees {
        Self {
            current: TransactionRecordMap(HashMap::new()),
            witness_trees: None,
        }
    }
    pub fn clear(&mut self) {
        self.current.clear();
        self.witness_trees.as_mut().map(WitnessTrees::clear);
    }
}
