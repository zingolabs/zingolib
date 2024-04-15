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

/// This block implements constructors.
impl Default for TransactionRecordsById {
    fn default() -> Self {
        Self::new()
    }
}

impl TransactionRecordsById {
    // New empty Map
    pub fn new() -> Self {
        TransactionRecordsById(HashMap::new())
    }
    // Associated function to create a TransactionRecordMap from a HashMap
    pub fn from_map(map: HashMap<TxId, TransactionRecord>) -> Self {
        TransactionRecordsById(map)
    }
}
/// This block implements methods to modify the map.
impl TransactionRecordsById {
    /// All information after a certain height is invalidated during a reorg.
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
    /// this function invalidiates a vec of txids by removing them and then all references to them.
    pub(crate) fn invalidate_txids(&mut self, txids_to_remove: Vec<TxId>) {
        for txid in &txids_to_remove {
            self.remove(txid);
        }

        self.invalidate_txid_specific_transparent_spends(&txids_to_remove);
        // roll back any sapling spends in each invalidated tx
        self.invalidate_txid_specific_domain_spends::<SaplingDomain>(&txids_to_remove);
        // roll back any orchard spends in each invalidated tx
        self.invalidate_txid_specific_domain_spends::<OrchardDomain>(&txids_to_remove);
    }
    /// any transparent note that is listed as being spent in one of these transactions will be marked as unspent.
    pub(crate) fn invalidate_txid_specific_transparent_spends(
        &mut self,
        invalidated_txids: &[TxId],
    ) {
        self.values_mut().for_each(|transaction_metadata| {
            // Update UTXOs to roll back any spent utxos
            transaction_metadata
                .transparent_notes
                .iter_mut()
                .for_each(|utxo| {
                    if utxo.is_spent() && invalidated_txids.contains(&utxo.spent().unwrap().0) {
                        *utxo.spent_mut() = None;
                    }

                    if utxo.unconfirmed_spent.is_some()
                        && invalidated_txids.contains(&utxo.unconfirmed_spent.unwrap().0)
                    {
                        utxo.unconfirmed_spent = None;
                    }
                })
        });
    }
    /// any shielded note of the specified domain that is listed as being spent in one of these transactions will be marked as unspent.
    pub(crate) fn invalidate_txid_specific_domain_spends<D: DomainWalletExt>(
        &mut self,
        invalidated_txids: &[TxId],
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
                    if nd.spent().is_some() && invalidated_txids.contains(&nd.spent().unwrap().0) {
                        *nd.spent_mut() = None;
                    }

                    // Remove unconfirmed spends too
                    if nd.pending_spent().is_some()
                        && invalidated_txids.contains(&nd.pending_spent().unwrap().0)
                    {
                        *nd.pending_spent_mut() = None;
                    }
                });
        });
    }
}
