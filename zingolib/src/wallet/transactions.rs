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

impl zcash_client_backend::data_api::InputSource for TransactionRecordMap {
    type Error = crate::error::ZingoLibError;
    type AccountId = zcash_primitives::zip32::AccountId;
    type NoteRef = super::notes::NoteRecordIdentifier;

    fn get_spendable_note(
        &self,
        txid: &zcash_primitives::transaction::TxId,
        protocol: zcash_client_backend::ShieldedProtocol,
        index: u32,
    ) -> Result<
        Option<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        >,
        Self::Error,
    > {
        let note_record_reference: <Self as zcash_client_backend::data_api::InputSource>::NoteRef =
            super::notes::NoteRecordIdentifier {
                txid: *txid,
                pool: zcash_client_backend::PoolType::Shielded(protocol),
                index,
            };
        Ok(self.get_shielded_received_note_from_identifier(note_record_reference))
    }

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<
        zcash_client_backend::data_api::SpendableNotes<super::notes::NoteRecordIdentifier>,
        crate::error::ZingoLibError,
    > {
        todo!()
    }
}
impl TransactionRecordMap {
    // Associated function to create a TransactionRecordMap from a HashMap
    pub fn from_map(map: HashMap<TxId, TransactionRecord>) -> Self {
        TransactionRecordMap(map)
    }
    pub fn get_shielded_received_note_from_identifier(
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

    use super::TransactionRecordMap;

    #[test]
    fn trm_get_received_note_from_identifier() {
        let (txid, tnote) = crate::test_framework::create_empty_txid_and_tnote();
        let single_transparent_trans_record =
            crate::test_framework::create_transaction_record_with_one_tnote(txid, tnote);
        let hashmap = HashMap::from([(txid, single_transparent_trans_record)]);
        let trm = TransactionRecordMap::from_map(hashmap);
        let identifier = crate::test_framework::create_note_record_id();
        let note = trm.get_shielded_received_note_from_identifier(identifier);
        assert!(note.is_none());
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
