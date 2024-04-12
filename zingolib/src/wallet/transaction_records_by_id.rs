use std::collections::HashMap;

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;

use zcash_note_encryption::Domain;
use zcash_primitives::consensus::BlockHeight;

use zcash_primitives::transaction::TxId;

use crate::wallet::{
    data::TransactionRecord,
    notes::NoteInterface,
    traits::{DomainWalletExt, Recipient},
};

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
    // initializers

    // Associated function to create a TransactionRecordMap from a HashMap
    pub fn default() -> Self {
        TransactionRecordsById(HashMap::new())
    }
    pub fn from_map(map: HashMap<TxId, TransactionRecord>) -> Self {
        TransactionRecordsById(map)
    }

    // modify methods

    pub fn insert_transaction_record(&mut self, transaction_record: TransactionRecord) {
        self.insert(transaction_record.txid, transaction_record);
    }
    pub fn invalidate_all_transactions_after_or_at_height(&mut self, reorg_height: BlockHeight) {
        // First, collect txids that need to be removed
        let txids_to_remove = self
            .values()
            .filter_map(|transaction_metadata| {
                if transaction_metadata
                    .status
                    .is_confirmed_after_or_at(&reorg_height)
                    || transaction_metadata
                        .status
                        .is_broadcast_after_or_at(&reorg_height)
                // tODo: why dont we only remove confirmed transactions. unconfirmed transactions may still be valid in the mempool and may later confirm or expire.
                {
                    Some(transaction_metadata.txid)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        self.invalidate_txids(txids_to_remove);
    }

    /// this function invalidiates a vec of txids
    pub(crate) fn invalidate_txids(&mut self, txids_to_remove: Vec<TxId>) {
        for txid in &txids_to_remove {
            self.remove(txid);
        }

        // invalidate (roll back) any transparent spends in each invalidated tx
        self.values_mut().for_each(|transaction_metadata| {
            // Update UTXOs to roll back any spent utxos
            transaction_metadata
                .transparent_notes
                .iter_mut()
                .for_each(|utxo| {
                    if utxo.is_spent() && txids_to_remove.contains(&utxo.spent().unwrap().0) {
                        *utxo.spent_mut() = None;
                    }

                    if utxo.unconfirmed_spent.is_some()
                        && txids_to_remove.contains(&utxo.unconfirmed_spent.unwrap().0)
                    {
                        utxo.unconfirmed_spent = None;
                    }
                })
        });
        // roll back any sapling spends in each invalidated tx
        self.invalidate_domain_specific_txids::<SaplingDomain>(&txids_to_remove);
        // roll back any orchard spends in each invalidated tx
        self.invalidate_domain_specific_txids::<OrchardDomain>(&txids_to_remove);
    }

    pub(crate) fn invalidate_domain_specific_txids<D: DomainWalletExt>(
        &mut self,
        txids_to_remove: &[TxId],
    ) where
        <D as Domain>::Recipient: Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        self.values_mut().for_each(|transaction_metadata| {
            // Update notes to rollback any spent notes
            D::to_notes_vec_mut(transaction_metadata)
                .iter_mut()
                .for_each(|nd| {
                    // Mark note as unspent if the txid being removed spent it.
                    if nd.spent().is_some() && txids_to_remove.contains(&nd.spent().unwrap().0) {
                        *nd.spent_mut() = None;
                    }

                    // Remove unconfirmed spends too
                    if nd.pending_spent().is_some()
                        && txids_to_remove.contains(&nd.pending_spent().unwrap().0)
                    {
                        *nd.pending_spent_mut() = None;
                    }
                });
        });
    }
}

#[cfg(test)]
#[cfg(feature = "test-features")]
mod tests {
    use crate::wallet::{
        notes::{SaplingNote, TransparentNote},
        transaction_record::mocks::TransactionRecordBuilder,
    };

    use super::TransactionRecordsById;

    use zcash_primitives::consensus::BlockHeight;
    use zingo_status::confirmation_status::ConfirmationStatus::Confirmed;

    #[test]
    fn invalidated_note_is_deleted() {
        let mut transaction_record_early = TransactionRecordBuilder::default()
            .status(Confirmed(5.into()))
            .build();
        transaction_record_early
            .transparent_notes
            .push(TransparentNote::mock());

        let mut transaction_record_later = TransactionRecordBuilder::default()
            .status(Confirmed(15.into()))
            .build();
        transaction_record_later
            .sapling_notes
            .push(SaplingNote::mock());

        let mut transaction_records_by_id = TransactionRecordsById::default();
        transaction_records_by_id.insert_transaction_record(transaction_record_early);
        transaction_records_by_id.insert_transaction_record(transaction_record_later);

        let reorg_height: BlockHeight = 10.into();

        transaction_records_by_id.invalidate_all_transactions_after_or_at_height(reorg_height);

        assert_eq!(transaction_records_by_id.len(), 1);
    }
}
