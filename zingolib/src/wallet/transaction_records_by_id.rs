use std::collections::HashMap;

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::data_api::{InputSource, SpendableNotes};
use zcash_client_backend::wallet::ReceivedNote;
use zcash_client_backend::{PoolType, ShieldedProtocol};
use zcash_note_encryption::Domain;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::transaction::components::amount::NonNegativeAmount;
use zcash_primitives::transaction::TxId;
use zip32::AccountId;

use crate::{
    error::{ZingoLibError, ZingoLibResult},
    wallet::{
        data::TransactionRecord,
        notes::{NoteInterface, NoteRecordIdentifier},
        traits::{DomainWalletExt, Recipient},
    },
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
    pub fn new() -> Self {
        TransactionRecordsById(HashMap::new())
    }
    pub fn from_map(map: HashMap<TxId, TransactionRecord>) -> Self {
        TransactionRecordsById(map)
    }

    // modify methods

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

    // get methods

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

impl InputSource for TransactionRecordsById {
    type Error = ZingoLibError;
    type AccountId = zcash_primitives::zip32::AccountId;
    type NoteRef = NoteRecordIdentifier;

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
        let note_record_reference: <Self as InputSource>::NoteRef = NoteRecordIdentifier {
            txid: *txid,
            pool: PoolType::Shielded(protocol),
            index,
        };
        match protocol {
            ShieldedProtocol::Sapling => Ok(self
                .get_received_note_from_identifier::<SaplingDomain>(note_record_reference)
                .map(|note| {
                    note.map_note(|note_inner| {
                        zcash_client_backend::wallet::Note::Sapling(note_inner)
                    })
                })),
            ShieldedProtocol::Orchard => Ok(self
                .get_received_note_from_identifier::<OrchardDomain>(note_record_reference)
                .map(|note| {
                    note.map_note(|note_inner| {
                        zcash_client_backend::wallet::Note::Orchard(note_inner)
                    })
                })),
        }
    }

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<SpendableNotes<NoteRecordIdentifier>, ZingoLibError> {
        if account != AccountId::ZERO {
            return Err(ZingoLibError::Error(
                "we don't use non-zero accounts (yet?)".to_string(),
            ));
        }
        let mut sapling_note_noteref_pairs: Vec<(sapling_crypto::Note, NoteRecordIdentifier)> =
            Vec::new();
        let mut orchard_note_noteref_pairs: Vec<(orchard::Note, NoteRecordIdentifier)> = Vec::new();
        for transaction_record in self.values().filter(|transaction_record| {
            transaction_record
                .status
                .is_confirmed_before_or_at(&anchor_height)
        }) {
            if sources.contains(&ShieldedProtocol::Sapling) {
                sapling_note_noteref_pairs.extend(
                    transaction_record
                        .select_unspent_note_noteref_pairs_sapling()
                        .into_iter()
                        .filter(|note_ref_pair| !exclude.contains(&note_ref_pair.1)),
                );
            }
            if sources.contains(&ShieldedProtocol::Orchard) {
                orchard_note_noteref_pairs.extend(
                    transaction_record
                        .select_unspent_note_noteref_pairs_orchard()
                        .into_iter()
                        .filter(|note_ref_pair| !exclude.contains(&note_ref_pair.1)),
                );
            }
        }
        let mut sapling_notes =
            Vec::<ReceivedNote<NoteRecordIdentifier, sapling_crypto::Note>>::new();
        let mut orchard_notes = Vec::<ReceivedNote<NoteRecordIdentifier, orchard::Note>>::new();
        if let Some(missing_value_after_sapling) = sapling_note_noteref_pairs.into_iter().rev(/*biggest first*/).try_fold(
            Some(target_value),
            |rolling_target, (note, noteref)| match rolling_target {
                Some(targ) => {
                    sapling_notes.push(
                        self.get(&noteref.txid).and_then(|tr| tr.get_received_note::<SaplingDomain>(noteref.index))
                            .ok_or_else(|| ZingoLibError::Error("missing note".to_string()))?
                    );
                    Ok(targ
                        - NonNegativeAmount::from_u64(note.value().inner())
                            .map_err(|e| ZingoLibError::Error(e.to_string()))?)
                }
                None => Ok(None),
            },
        )? {
            if let Some(missing_value_after_orchard) = orchard_note_noteref_pairs.into_iter().rev(/*biggest first*/).try_fold(
            Some(missing_value_after_sapling),
            |rolling_target, (note, noteref)| match rolling_target {
                Some(targ) => {
                    orchard_notes.push(
                        self.get(&noteref.txid).and_then(|tr| tr.get_received_note::<OrchardDomain>(noteref.index))
                            .ok_or_else(|| ZingoLibError::Error("missing note".to_string()))?
                    );
                    Ok(targ
                        - NonNegativeAmount::from_u64(note.value().inner())
                            .map_err(|e| ZingoLibError::Error(e.to_string()))?)
                }
                None => Ok(None),
            },
        )? {
                return ZingoLibResult::Err(ZingoLibError::Error(format!(
                    "insufficient funds, short {}",
                    missing_value_after_orchard.into_u64()
                )));
            };
        };

        Ok(SpendableNotes::new(sapling_notes, orchard_notes))
    }
}
