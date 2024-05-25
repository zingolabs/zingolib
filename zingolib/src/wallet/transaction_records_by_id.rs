//! The lookup for transaction id indexed data.  Currently this provides the
//! transaction record.

use crate::wallet::{
    error::FeeError,
    notes::{
        interface::ShieldedNoteInterface,
        query::{OutputQuery, OutputSpendStatusQuery},
        OrchardNote, OutputInterface, SaplingNote,
    },
    traits::{DomainWalletExt, Recipient},
    transaction_record::TransactionRecord,
};
use std::collections::HashMap;

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;

use zcash_client_backend::wallet::NoteId;
use zcash_note_encryption::Domain;
use zcash_primitives::consensus::BlockHeight;

use zcash_primitives::transaction::TxId;

pub mod trait_inputsource;

/// A convenience wrapper, to impl behavior on.
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

/// Constructors
impl TransactionRecordsById {
    /// Constructs a new TransactionRecordsById with an empty map.
    pub fn new() -> Self {
        TransactionRecordsById(HashMap::new())
    }
    /// Constructs a TransactionRecordsById from a HashMap
    pub fn from_map(map: HashMap<TxId, TransactionRecord>) -> Self {
        TransactionRecordsById(map)
    }
}

/// Methods to query and modify the map.
impl TransactionRecordsById {
    /// Uses a query to select all notes across all transactions with specific properties and sum them
    pub fn query_sum_value(&self, include_notes: OutputQuery) -> u64 {
        self.0.iter().fold(0, |partial_sum, transaction_record| {
            partial_sum + transaction_record.1.query_sum_value(include_notes)
        })
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_received_spendable_note_from_identifier<D: DomainWalletExt>(
        &self,
        note_id: NoteId,
    ) -> Option<
        zcash_client_backend::wallet::ReceivedNote<
            NoteId,
            <D as zcash_note_encryption::Domain>::Note,
        >,
    >
    where
        <D as zcash_note_encryption::Domain>::Note: PartialEq + Clone,
        <D as zcash_note_encryption::Domain>::Recipient: super::traits::Recipient,
    {
        let transaction = self.get(note_id.txid());
        if note_id.protocol() == D::SHIELDED_PROTOCOL {
            transaction.and_then(|transaction_record| {
                D::WalletNote::transaction_record_to_outputs_vec(transaction_record)
                    .iter()
                    .find(|note| note.output_index() == &Some(note_id.output_index() as u32))
                    .and_then(|note| {
                        if note.spend_status_query(OutputSpendStatusQuery {
                            unspent: true,
                            pending_spent: false,
                            spent: false,
                        }) {
                            note.witnessed_position().map(|pos| {
                                zcash_client_backend::wallet::ReceivedNote::from_parts(
                                    note_id,
                                    transaction_record.txid,
                                    note_id.output_index(),
                                    note.note().clone(),
                                    zip32::Scope::External,
                                    pos,
                                )
                            })
                        } else {
                            None
                        }
                    })
            })
        } else {
            None
        }
    }
    /// Adds a TransactionRecord to the hashmap, using its TxId as a key.
    pub fn insert_transaction_record(&mut self, transaction_record: TransactionRecord) {
        self.insert(transaction_record.txid, transaction_record);
    }
    /// Invalidates all transactions from a given height including the block with block height `reorg_height`
    ///
    /// All information above a certain height is invalidated during a reorg.
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
                        .is_pending_after_or_at(&reorg_height)
                // TODO: why dont we only remove confirmed transactions. pending transactions may still be valid in the mempool and may later confirm or expire.
                {
                    Some(transaction_metadata.txid)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        self.invalidate_transactions(txids_to_remove);
    }
    /// Invalidiates a vec of txids by removing them and then all references to them.
    ///
    /// A transaction can be invalidated either by a reorg or if it was never confirmed by a miner.
    /// This is required in the case that a note was spent in a invalidated transaction.
    /// Takes a slice of txids corresponding to the invalidated transactions, searches all notes for being spent in one of those txids, and resets them to unspent.
    pub(crate) fn invalidate_transactions(&mut self, txids_to_remove: Vec<TxId>) {
        for txid in &txids_to_remove {
            self.remove(txid);
        }

        self.invalidate_transaction_specific_transparent_spends(&txids_to_remove);
        // roll back any sapling spends in each invalidated tx
        self.invalidate_transaction_specific_domain_spends::<SaplingDomain>(&txids_to_remove);
        // roll back any orchard spends in each invalidated tx
        self.invalidate_transaction_specific_domain_spends::<OrchardDomain>(&txids_to_remove);
    }
    /// Reverts any spent transparent notes in the given transactions to unspent.
    pub(crate) fn invalidate_transaction_specific_transparent_spends(
        &mut self,
        invalidated_txids: &[TxId],
    ) {
        self.values_mut().for_each(|transaction_metadata| {
            // Update UTXOs to roll back any spent utxos
            transaction_metadata
                .transparent_outputs
                .iter_mut()
                .for_each(|utxo| {
                    if utxo.is_spent() && invalidated_txids.contains(&utxo.spent().unwrap().0) {
                        *utxo.spent_mut() = None;
                    }

                    if utxo.pending_spent.is_some()
                        && invalidated_txids.contains(&utxo.pending_spent.unwrap().0)
                    {
                        utxo.pending_spent = None;
                    }
                })
        });
    }
    /// Reverts any spent shielded notes in the given transactions to unspent.
    pub(crate) fn invalidate_transaction_specific_domain_spends<D: DomainWalletExt>(
        &mut self,
        invalidated_txids: &[TxId],
    ) where
        <D as Domain>::Recipient: Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        self.values_mut().for_each(|transaction_metadata| {
            // Update notes to rollback any spent notes
            // Select only spent or pending_spent notes.
            D::WalletNote::transaction_record_to_outputs_vec_query_mut(
                transaction_metadata,
                OutputSpendStatusQuery {
                    unspent: false,
                    pending_spent: true,
                    spent: true,
                },
            )
            .iter_mut()
            .for_each(|nd| {
                // Mark note as unspent if the txid being removed spent it.
                if nd.spent().is_some() && invalidated_txids.contains(&nd.spent().unwrap().0) {
                    *nd.spent_mut() = None;
                }

                // Remove pending spends too
                if nd.pending_spent().is_some()
                    && invalidated_txids.contains(&nd.pending_spent().unwrap().0)
                {
                    *nd.pending_spent_mut() = None;
                }
            });
        });
    }

    /// Calculate the fee for a transaction in the wallet
    ///
    /// # Error
    ///
    /// Returns [`crate::wallet::error::FeeError::ReceivedTransaction`] if no spends or outgoing_tx_data were found
    /// in the wallet for this transaction, indicating this transaction was not created by this spend capability.
    /// Returns [`crate::wallet::error::FeeError::SpendNotFound`] if any shielded spends in the transaction are not
    /// found in the wallet, indicating that all shielded spends have not yet been synced. Also returns this error
    /// if the transaction record contains outgoing_tx_data but no spends are found.
    /// If a transparent spend has not yet been synced, the fee will be incorrect and return
    /// [`crate::wallet::error::FeeError::FeeUnderflow`] if an underflow occurs.
    /// The tracking of transparent spends will be improved on the next internal wallet version.
    pub fn calculate_transaction_fee(
        &self,
        transaction_record: &TransactionRecord,
    ) -> Result<u64, FeeError> {
        let sapling_spends = transaction_record
            .sapling_inputs
            .iter()
            .map(|nullifier| {
                self.values()
                    .flat_map(|transaction_record| transaction_record.sapling_notes())
                    .find(|&note| {
                        if let Some(nf) = note.nullifier() {
                            nf == *nullifier
                        } else {
                            false
                        }
                    })
                    .ok_or(FeeError::SpendNotFound)
            })
            .collect::<Result<Vec<&SaplingNote>, FeeError>>()?;
        let sapling_spend_value: u64 = sapling_spends.iter().map(|&note| note.value()).sum();

        let orchard_spends = transaction_record
            .orchard_inputs
            .iter()
            .map(|nullifier| {
                self.values()
                    .flat_map(|transaction_record| transaction_record.orchard_notes())
                    .find(|&note| {
                        if let Some(nf) = note.nullifier() {
                            nf == *nullifier
                        } else {
                            false
                        }
                    })
                    .ok_or(FeeError::SpendNotFound)
            })
            .collect::<Result<Vec<&OrchardNote>, FeeError>>()?;
        let orchard_spend_value: u64 = orchard_spends.iter().map(|&note| note.value()).sum();

        let total_spend_value = transaction_record.total_transparent_value_spent
            + sapling_spend_value
            + orchard_spend_value;

        if total_spend_value == 0 {
            if transaction_record.value_outgoing() == 0 {
                return Err(FeeError::ReceivedTransaction);
            } else {
                return Err(FeeError::SpendNotFound);
            }
        }

        let transparent_output_value: u64 = transaction_record
            .transparent_outputs()
            .iter()
            .map(|note| note.value())
            .sum();
        let sapling_output_value: u64 = transaction_record
            .sapling_notes()
            .iter()
            .map(|note| note.value())
            .sum();
        let orchard_output_value: u64 = transaction_record
            .orchard_notes()
            .iter()
            .map(|note| note.value())
            .sum();

        let total_output_value = transaction_record.value_outgoing()
            + transparent_output_value
            + sapling_output_value
            + orchard_output_value;

        if total_spend_value >= total_output_value {
            Ok(total_spend_value - total_output_value)
        } else {
            Err(FeeError::FeeUnderflow)
        }
    }

    /// Invalidates all those transactions which were broadcast but never 'confirmed' accepted by a miner.
    pub(crate) fn clear_expired_mempool(&mut self, latest_height: u64) {
        let cutoff = BlockHeight::from_u32(
            (latest_height.saturating_sub(zingoconfig::MAX_REORG as u64)) as u32,
        );

        let txids_to_remove = self
            .iter()
            .filter(|(_, transaction_metadata)| {
                transaction_metadata.status.is_pending_before(&cutoff)
            }) // this transaction was submitted to the mempool before the cutoff and has not been confirmed. we deduce that it has expired.
            .map(|(_, transaction_metadata)| transaction_metadata.txid)
            .collect::<Vec<_>>();

        txids_to_remove
            .iter()
            .for_each(|t| println!("Removing expired mempool tx {}", t));

        self.invalidate_transactions(txids_to_remove);
    }
    /// TODO: Add Doc Comment Here!
    pub fn total_funds_spent_in(&self, txid: &TxId) -> u64 {
        self.get(txid)
            .map(TransactionRecord::total_value_spent)
            .unwrap_or(0)
    }
    // Check this transaction to see if it is an outgoing transaction, and if it is, mark all received notes with non-textual memos in this
    // transaction as change. i.e., If any funds were spent in this transaction, all received notes without user-specified memos are change.
    //
    // TODO: When we start working on multi-sig, this could cause issues about hiding sends-to-self
    /// TODO: Add Doc Comment Here!
    pub fn check_notes_mark_change(&mut self, txid: &TxId) {
        //TODO: Incorrect with a 0-value fee somehow
        if self.total_funds_spent_in(txid) > 0 {
            if let Some(transaction_metadata) = self.get_mut(txid) {
                Self::mark_notes_as_change_for_pool(&mut transaction_metadata.sapling_notes);
                Self::mark_notes_as_change_for_pool(&mut transaction_metadata.orchard_notes);
            }
        }
    }
    fn mark_notes_as_change_for_pool<Note: crate::wallet::notes::ShieldedNoteInterface>(
        notes: &mut [Note],
    ) {
        notes.iter_mut().for_each(|n| {
            *n.is_change_mut() = match n.memo() {
                Some(zcash_primitives::memo::Memo::Text(_)) => false,
                Some(
                    zcash_primitives::memo::Memo::Empty
                    | zcash_primitives::memo::Memo::Arbitrary(_)
                    | zcash_primitives::memo::Memo::Future(_),
                )
                | None => true,
            }
        });
    }
    pub(crate) fn create_modify_get_transaction_metadata(
        &mut self,
        txid: &TxId,
        status: zingo_status::confirmation_status::ConfirmationStatus,
        datetime: u64,
    ) -> &'_ mut TransactionRecord {
        self.entry(*txid)
            // if we already have the transaction metadata, it may be newly confirmed. update confirmation_status
            .and_modify(|transaction_metadata| {
                transaction_metadata.status = status;
                transaction_metadata.datetime = datetime;
            })
            // if this transaction is new to our data, insert it
            .or_insert_with(|| TransactionRecord::new(status, datetime, txid))
    }

    /// TODO: Add Doc Comment Here!
    pub fn add_taddr_spent(
        &mut self,
        txid: TxId,
        status: zingo_status::confirmation_status::ConfirmationStatus,
        timestamp: u64,
        total_transparent_value_spent: u64,
    ) {
        let transaction_metadata =
            self.create_modify_get_transaction_metadata(&txid, status, timestamp);

        transaction_metadata.total_transparent_value_spent = total_transparent_value_spent;

        self.check_notes_mark_change(&txid);
    }

    /// TODO: Add Doc Comment Here!
    pub fn mark_txid_utxo_spent(
        &mut self,
        spent_txid: TxId,
        output_num: u32,
        source_txid: TxId,
        spending_tx_status: zingo_status::confirmation_status::ConfirmationStatus,
    ) -> u64 {
        // Find the UTXO
        let value = if let Some(utxo_transacion_metadata) = self.get_mut(&spent_txid) {
            if let Some(spent_utxo) = utxo_transacion_metadata
                .transparent_outputs
                .iter_mut()
                .find(|u| u.txid == spent_txid && u.output_index == output_num as u64)
            {
                if spending_tx_status.is_confirmed() {
                    // Mark this utxo as spent
                    *spent_utxo.spent_mut() =
                        Some((source_txid, spending_tx_status.get_height().into()));
                    spent_utxo.pending_spent = None;
                } else {
                    spent_utxo.pending_spent =
                        Some((source_txid, u32::from(spending_tx_status.get_height())));
                }

                spent_utxo.value
            } else {
                log::error!("Couldn't find UTXO that was spent");
                0
            }
        } else {
            log::error!("Couldn't find TxID that was spent!");
            0
        };

        // Return the value of the note that was spent.
        value
    }

    /// TODO: Add Doc Comment Here!
    #[allow(clippy::too_many_arguments)]
    pub fn add_new_taddr_output(
        &mut self,
        txid: TxId,
        taddr: String,
        status: zingo_status::confirmation_status::ConfirmationStatus,
        timestamp: u64,
        vout: &zcash_primitives::transaction::components::TxOut,
        output_num: u32,
    ) {
        // Read or create the current TxId
        let transaction_metadata =
            self.create_modify_get_transaction_metadata(&txid, status, timestamp);

        // Add this UTXO if it doesn't already exist
        if transaction_metadata
            .transparent_outputs
            .iter_mut()
            .any(|utxo| utxo.txid == txid && utxo.output_index == output_num as u64)
        {
            // If it already exists, it is likely an mempool tx, so update the height
        } else {
            transaction_metadata.transparent_outputs.push(
                crate::wallet::notes::TransparentOutput::from_parts(
                    taddr,
                    txid,
                    output_num as u64,
                    vout.script_pubkey.0.clone(),
                    u64::from(vout.value),
                    None,
                    None,
                ),
            );
        }
    }
    /// witness tree requirement:
    ///
    pub(crate) fn add_pending_note<D>(
        &mut self,
        txid: TxId,
        height: BlockHeight,
        timestamp: u64,
        note: D::Note,
        to: D::Recipient,
        output_index: usize,
    ) where
        D: DomainWalletExt,
        D::Note: PartialEq + Clone,
        D::Recipient: Recipient,
    {
        let status = zingo_status::confirmation_status::ConfirmationStatus::Pending(height);
        let transaction_record =
            self.create_modify_get_transaction_metadata(&txid, status, timestamp);

        match D::WalletNote::transaction_record_to_outputs_vec(transaction_record)
            .iter_mut()
            .find(|n| n.note() == &note)
        {
            None => {
                let nd = D::WalletNote::from_parts(
                    to.diversifier(),
                    note,
                    None,
                    None,
                    None,
                    None,
                    None,
                    // if this is change, we'll mark it later in check_notes_mark_change
                    false,
                    false,
                    Some(output_index as u32),
                );

                D::WalletNote::transaction_metadata_notes_mut(transaction_record).push(nd);
            }
            Some(_) => {}
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add_new_note<D: DomainWalletExt>(
        &mut self,
        txid: TxId,
        status: zingo_status::confirmation_status::ConfirmationStatus,
        timestamp: u64,
        note: <D::WalletNote as crate::wallet::notes::ShieldedNoteInterface>::Note,
        to: D::Recipient,
        have_spending_key: bool,
        nullifier: Option<
            <D::WalletNote as crate::wallet::notes::ShieldedNoteInterface>::Nullifier,
        >,
        output_index: u32,
        position: incrementalmerkletree::Position,
    ) where
        D::Note: PartialEq + Clone,
        D::Recipient: Recipient,
    {
        let transaction_metadata =
            self.create_modify_get_transaction_metadata(&txid, status, timestamp);

        let nd = D::WalletNote::from_parts(
            D::Recipient::diversifier(&to),
            note.clone(),
            Some(position),
            nullifier,
            None,
            None,
            None,
            // if this is change, we'll mark it later in check_notes_mark_change
            false,
            have_spending_key,
            Some(output_index),
        );
        match D::WalletNote::transaction_metadata_notes_mut(transaction_metadata)
            .iter_mut()
            .find(|n| n.note() == &note)
        {
            None => {
                D::WalletNote::transaction_metadata_notes_mut(transaction_metadata).push(nd);

                D::WalletNote::transaction_metadata_notes_mut(transaction_metadata)
                    .retain(|n| n.nullifier().is_some());
            }
            #[allow(unused_mut)]
            Some(mut n) => {
                // An overwrite should be safe here: TODO: test that confirms this
                *n = nd;
            }
        }
    }

    // Update the memo for a note if it already exists. If the note doesn't exist, then nothing happens.
    pub(crate) fn add_memo_to_note_metadata<Nd: crate::wallet::notes::ShieldedNoteInterface>(
        &mut self,
        txid: &TxId,
        note: Nd::Note,
        memo: zcash_primitives::memo::Memo,
    ) {
        if let Some(transaction_metadata) = self.get_mut(txid) {
            if let Some(n) = Nd::transaction_metadata_notes_mut(transaction_metadata)
                .iter_mut()
                .find(|n| n.note() == &note)
            {
                *n.memo_mut() = Some(memo);
            }
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn add_outgoing_metadata(
        &mut self,
        txid: &TxId,
        outgoing_metadata: Vec<crate::wallet::data::OutgoingTxData>,
    ) {
        // println!("        adding outgoing metadata to txid {}", txid);
        if let Some(transaction_metadata) = self.get_mut(txid) {
            transaction_metadata.outgoing_tx_data = outgoing_metadata
        } else {
            log::error!(
                "TxId {} should be present while adding metadata, but wasn't",
                txid
            );
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn set_price(&mut self, txid: &TxId, price: Option<f64>) {
        price.map(|p| self.get_mut(txid).map(|tx| tx.price = Some(p)));
    }
}

impl Default for TransactionRecordsById {
    /// Default constructor
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        mocks::{
            nullifier::{OrchardNullifierBuilder, SaplingNullifierBuilder},
            orchard_note::OrchardCryptoNoteBuilder,
            random_txid, SaplingCryptoNoteBuilder,
        },
        wallet::{
            data::mocks::OutgoingTxDataBuilder,
            notes::{
                orchard::mocks::OrchardNoteBuilder,
                query::{OutputPoolQuery, OutputQuery, OutputSpendStatusQuery},
                sapling::mocks::SaplingNoteBuilder,
                transparent::mocks::TransparentOutputBuilder,
                OutputInterface, SaplingNote,
            },
            transaction_record::mocks::{nine_note_transaction_record, TransactionRecordBuilder},
        },
    };

    use super::TransactionRecordsById;

    use sapling_crypto::note_encryption::SaplingDomain;
    use zcash_client_backend::{wallet::ReceivedNote, ShieldedProtocol};
    use zcash_primitives::consensus::BlockHeight;
    use zingo_status::confirmation_status::ConfirmationStatus::Confirmed;

    #[test]
    fn invalidate_all_transactions_after_or_at_height() {
        let transaction_record_later = TransactionRecordBuilder::default()
            .randomize_txid()
            .status(Confirmed(15.into()))
            .transparent_outputs(TransparentOutputBuilder::default())
            .build();
        let spending_txid = transaction_record_later.txid;

        let transaction_record_early = TransactionRecordBuilder::default()
            .randomize_txid()
            .status(Confirmed(5.into()))
            .transparent_outputs(
                TransparentOutputBuilder::default()
                    .spent(Some((spending_txid, 15)))
                    .clone(),
            )
            .sapling_notes(
                SaplingNoteBuilder::default()
                    .spent(Some((spending_txid, 15)))
                    .clone(),
            )
            .orchard_notes(
                OrchardNoteBuilder::default()
                    .spent(Some((spending_txid, 15)))
                    .clone(),
            )
            .sapling_notes(
                SaplingNoteBuilder::default()
                    .spent(Some((random_txid(), 15)))
                    .clone(),
            )
            .orchard_notes(OrchardNoteBuilder::default())
            .set_output_indexes()
            .build();

        let txid_containing_valid_note_with_invalid_spends = transaction_record_early.txid;

        let mut transaction_records_by_id = TransactionRecordsById::default();
        transaction_records_by_id.insert_transaction_record(transaction_record_early);
        transaction_records_by_id.insert_transaction_record(transaction_record_later);

        let reorg_height: BlockHeight = 10.into();

        transaction_records_by_id.invalidate_all_transactions_after_or_at_height(reorg_height);

        assert_eq!(transaction_records_by_id.len(), 1);
        //^ the deleted tx is not around
        let transaction_record_cvnwis = transaction_records_by_id
            .get(&txid_containing_valid_note_with_invalid_spends)
            .unwrap();

        let query_for_spentish_notes = OutputSpendStatusQuery {
            unspent: false,
            pending_spent: true,
            spent: true,
        };
        let spentish_notes_in_tx_cvnwis = transaction_record_cvnwis.query_for_ids(OutputQuery {
            spend_status: query_for_spentish_notes,
            pools: OutputPoolQuery::any(),
        });
        assert_eq!(spentish_notes_in_tx_cvnwis.len(), 1);
        // ^ so there is one spent note still in this transaction
        assert_ne!(
            SaplingNote::transaction_record_to_outputs_vec_query(
                transaction_record_cvnwis,
                query_for_spentish_notes
            )
            .first()
            .unwrap()
            .spent(),
            &Some((spending_txid, 15u32))
        );
        // ^ but it was not spent in the deleted txid
    }

    #[test]
    fn calculate_transaction_fee() {
        let mut sapling_nullifier_builder = SaplingNullifierBuilder::new();
        let mut orchard_nullifier_builder = OrchardNullifierBuilder::new();

        let sent_transaction_record = TransactionRecordBuilder::default()
            .status(Confirmed(15.into()))
            .sapling_inputs(sapling_nullifier_builder.assign_unique_nullifier().clone())
            .sapling_inputs(sapling_nullifier_builder.assign_unique_nullifier().clone())
            .orchard_inputs(orchard_nullifier_builder.assign_unique_nullifier().clone())
            .orchard_inputs(orchard_nullifier_builder.assign_unique_nullifier().clone())
            .transparent_outputs(TransparentOutputBuilder::default())
            .sapling_notes(SaplingNoteBuilder::default())
            .orchard_notes(OrchardNoteBuilder::default())
            .total_transparent_value_spent(30_000)
            .outgoing_tx_data(OutgoingTxDataBuilder::default())
            .build();
        let sent_txid = sent_transaction_record.txid;
        let first_sapling_nullifier = sent_transaction_record.sapling_inputs[0];
        let second_sapling_nullifier = sent_transaction_record.sapling_inputs[1];
        let first_orchard_nullifier = sent_transaction_record.orchard_inputs[0];
        let second_orchard_nullifier = sent_transaction_record.orchard_inputs[1];
        // t-note + s-note + o-note + outgoing_tx_data
        let expected_output_value: u64 = 100_000 + 200_000 + 800_000 + 50_000;

        let first_received_transaction_record = TransactionRecordBuilder::default()
            .randomize_txid()
            .status(Confirmed(5.into()))
            .sapling_notes(
                SaplingNoteBuilder::default()
                    .note(
                        SaplingCryptoNoteBuilder::default()
                            .value(sapling_crypto::value::NoteValue::from_raw(175_000))
                            .to_owned(),
                    )
                    .spent(Some((sent_txid, 15)))
                    .nullifier(Some(first_sapling_nullifier))
                    .to_owned(),
            )
            .sapling_notes(
                SaplingNoteBuilder::default()
                    .note(
                        SaplingCryptoNoteBuilder::default()
                            .value(sapling_crypto::value::NoteValue::from_raw(325_000))
                            .to_owned(),
                    )
                    .spent(Some((sent_txid, 15)))
                    .nullifier(Some(second_sapling_nullifier))
                    .to_owned(),
            )
            .orchard_notes(
                OrchardNoteBuilder::default()
                    .note(
                        OrchardCryptoNoteBuilder::default()
                            .value(orchard::value::NoteValue::from_raw(500_000))
                            .to_owned(),
                    )
                    .spent(Some((sent_txid, 15)))
                    .nullifier(Some(first_orchard_nullifier))
                    .to_owned(),
            )
            .transparent_outputs(TransparentOutputBuilder::default())
            .sapling_notes(
                SaplingNoteBuilder::default()
                    .spent(Some((random_txid(), 12)))
                    .to_owned(),
            )
            .orchard_notes(OrchardNoteBuilder::default())
            .set_output_indexes()
            .build();
        let second_received_transaction_record = TransactionRecordBuilder::default()
            .randomize_txid()
            .status(Confirmed(7.into()))
            .orchard_notes(
                OrchardNoteBuilder::default()
                    .note(
                        OrchardCryptoNoteBuilder::default()
                            .value(orchard::value::NoteValue::from_raw(200_000))
                            .to_owned(),
                    )
                    .spent(Some((sent_txid, 15)))
                    .nullifier(Some(second_orchard_nullifier))
                    .to_owned(),
            )
            .transparent_outputs(TransparentOutputBuilder::default())
            .sapling_notes(SaplingNoteBuilder::default().clone())
            .orchard_notes(
                OrchardNoteBuilder::default()
                    .spent(Some((random_txid(), 13)))
                    .to_owned(),
            )
            .set_output_indexes()
            .build();
        // s-note1 + s-note2 + o-note1 + o-note2 + sent_transaction.total_transparent_value_spent
        let expected_spend_value: u64 = 175_000 + 325_000 + 500_000 + 200_000 + 30_000;

        let mut transaction_records_by_id = TransactionRecordsById::default();
        transaction_records_by_id.insert_transaction_record(sent_transaction_record);
        transaction_records_by_id.insert_transaction_record(first_received_transaction_record);
        transaction_records_by_id.insert_transaction_record(second_received_transaction_record);

        let fee = transaction_records_by_id
            .calculate_transaction_fee(transaction_records_by_id.get(&sent_txid).unwrap())
            .unwrap();
        assert_eq!(expected_spend_value - expected_output_value, fee);
    }

    mod calculate_transaction_fee_errors {
        use crate::{
            mocks::{
                nullifier::{OrchardNullifierBuilder, SaplingNullifierBuilder},
                orchard_note::OrchardCryptoNoteBuilder,
                SaplingCryptoNoteBuilder,
            },
            wallet::{
                data::mocks::OutgoingTxDataBuilder,
                error::FeeError,
                notes::{
                    orchard::mocks::OrchardNoteBuilder, sapling::mocks::SaplingNoteBuilder,
                    transparent::mocks::TransparentOutputBuilder,
                },
                transaction_record::mocks::TransactionRecordBuilder,
                transaction_records_by_id::TransactionRecordsById,
            },
        };

        use zingo_status::confirmation_status::ConfirmationStatus::Confirmed;

        #[test]
        fn spend_not_found() {
            let mut sapling_nullifier_builder = SaplingNullifierBuilder::new();
            let mut orchard_nullifier_builder = OrchardNullifierBuilder::new();

            let sent_transaction_record = TransactionRecordBuilder::default()
                .status(Confirmed(15.into()))
                .sapling_inputs(sapling_nullifier_builder.assign_unique_nullifier().clone())
                .orchard_inputs(orchard_nullifier_builder.assign_unique_nullifier().clone())
                .outgoing_tx_data(OutgoingTxDataBuilder::default())
                .transparent_outputs(TransparentOutputBuilder::default())
                .sapling_notes(SaplingNoteBuilder::default())
                .orchard_notes(OrchardNoteBuilder::default())
                .build();
            let sent_txid = sent_transaction_record.txid;
            let sapling_nullifier = sent_transaction_record.sapling_inputs[0];

            let received_transaction_record = TransactionRecordBuilder::default()
                .randomize_txid()
                .status(Confirmed(5.into()))
                .sapling_notes(
                    SaplingNoteBuilder::default()
                        .note(
                            SaplingCryptoNoteBuilder::default()
                                .value(sapling_crypto::value::NoteValue::from_raw(175_000))
                                .to_owned(),
                        )
                        .spent(Some((sent_txid, 15)))
                        .nullifier(Some(sapling_nullifier))
                        .to_owned(),
                )
                .build();

            let mut transaction_records_by_id = TransactionRecordsById::default();
            transaction_records_by_id.insert_transaction_record(sent_transaction_record);
            transaction_records_by_id.insert_transaction_record(received_transaction_record);

            let fee = transaction_records_by_id
                .calculate_transaction_fee(transaction_records_by_id.get(&sent_txid).unwrap());
            assert!(matches!(fee, Err(FeeError::SpendNotFound)));
        }
        #[test]
        fn received_transaction() {
            let transaction_record = TransactionRecordBuilder::default()
                .status(Confirmed(15.into()))
                .transparent_outputs(TransparentOutputBuilder::default())
                .sapling_notes(SaplingNoteBuilder::default())
                .orchard_notes(OrchardNoteBuilder::default())
                .build();
            let sent_txid = transaction_record.txid;

            let mut transaction_records_by_id = TransactionRecordsById::default();
            transaction_records_by_id.insert_transaction_record(transaction_record);

            let fee = transaction_records_by_id
                .calculate_transaction_fee(transaction_records_by_id.get(&sent_txid).unwrap());
            assert!(matches!(fee, Err(FeeError::ReceivedTransaction)));
        }
        #[test]
        fn outgoing_tx_data_but_no_spends_found() {
            let transaction_record = TransactionRecordBuilder::default()
                .status(Confirmed(15.into()))
                .transparent_outputs(TransparentOutputBuilder::default())
                .sapling_notes(SaplingNoteBuilder::default())
                .orchard_notes(OrchardNoteBuilder::default())
                .outgoing_tx_data(OutgoingTxDataBuilder::default())
                .build();
            let sent_txid = transaction_record.txid;

            let mut transaction_records_by_id = TransactionRecordsById::default();
            transaction_records_by_id.insert_transaction_record(transaction_record);

            let fee = transaction_records_by_id
                .calculate_transaction_fee(transaction_records_by_id.get(&sent_txid).unwrap());
            assert!(matches!(fee, Err(FeeError::SpendNotFound)));
        }
        #[test]
        fn transparent_spends_not_fully_synced() {
            let transaction_record = TransactionRecordBuilder::default()
                .status(Confirmed(15.into()))
                .orchard_notes(
                    OrchardNoteBuilder::default()
                        .note(
                            OrchardCryptoNoteBuilder::default()
                                .value(orchard::value::NoteValue::from_raw(50_000))
                                .to_owned(),
                        )
                        .to_owned(),
                )
                .total_transparent_value_spent(20_000)
                .build();
            let sent_txid = transaction_record.txid;

            let mut transaction_records_by_id = TransactionRecordsById::default();
            transaction_records_by_id.insert_transaction_record(transaction_record);

            let fee = transaction_records_by_id
                .calculate_transaction_fee(transaction_records_by_id.get(&sent_txid).unwrap());
            assert!(matches!(fee, Err(FeeError::FeeUnderflow)));
        }
    }

    #[test]
    fn get_received_spendable_note_from_identifier() {
        let mut trbid = TransactionRecordsById::new();
        trbid.insert_transaction_record(nine_note_transaction_record(
            100_000_000,
            200_000_000,
            400_000_000,
            100_000_000,
            200_000_000,
            400_000_000,
            100_000_000,
            200_000_000,
            400_000_000,
        ));

        for i in 0..3 {
            let (txid, record) = trbid.0.iter().next().unwrap();

            let received_note = trbid.get_received_spendable_note_from_identifier::<SaplingDomain>(
                zcash_client_backend::wallet::NoteId::new(
                    *txid,
                    ShieldedProtocol::Sapling,
                    i as u16,
                ),
            );

            assert_eq!(
                if record.sapling_notes[i].spend_status_query(OutputSpendStatusQuery {
                    unspent: true,
                    pending_spent: false,
                    spent: false
                }) {
                    Some(zcash_client_backend::wallet::Note::Sapling(
                        record.sapling_notes[i].sapling_crypto_note.clone(),
                    ))
                } else {
                    None
                },
                received_note
                    .as_ref()
                    .map(ReceivedNote::note)
                    .cloned()
                    .map(zcash_client_backend::wallet::Note::Sapling),
            )
        }
    }
}
