//! The lookup for transaction id indexed data.  Currently this provides the
//! transaction record.

use crate::wallet::notes::{interface::OutputConstructor, TransparentOutput};
use crate::wallet::{
    error::FeeError,
    notes::{
        interface::ShieldedNoteInterface,
        query::{OutputQuery, OutputSpendStatusQuery},
        OrchardNote, OutputInterface, SaplingNote,
    },
    traits::{DomainWalletExt, Recipient},
    transaction_record::{SendType, TransactionKind, TransactionRecord},
};
use std::collections::HashMap;

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;

use zcash_client_backend::wallet::NoteId;
use zcash_note_encryption::Domain;
use zcash_primitives::consensus::BlockHeight;
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::config::{
    ChainType, ZENNIES_FOR_ZINGO_DONATION_ADDRESS, ZENNIES_FOR_ZINGO_REGTEST_ADDRESS,
    ZENNIES_FOR_ZINGO_TESTNET_ADDRESS,
};
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
        self.0.iter().fold(0, |partial_sum, (_id, record)| {
            partial_sum + record.query_sum_value(include_notes)
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
                D::WalletNote::get_record_outputs(transaction_record)
                    .iter()
                    .find(|note| note.output_index() == &Some(note_id.output_index() as u32))
                    .and_then(|note| {
                        if note.spend_status_query(OutputSpendStatusQuery::only_unspent()) {
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
    /// Invalidates all transactions from a given height including the block with block height `reorg_height`.
    ///
    /// All information above a certain height is invalidated during a reorg.
    pub fn invalidate_all_transactions_after_or_at_height(&mut self, reorg_height: BlockHeight) {
        // First, collect txids that need to be removed
        let txids_to_remove = self
            .values()
            .filter_map(|transaction_metadata| {
                // doesnt matter the status: if it happen after a reorg, eliminate it
                if transaction_metadata.status.get_height() >= reorg_height
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
                    // Mark utxo as unspent if the txid being removed spent it.
                    if utxo
                        .spending_tx_status()
                        .filter(|(txid, _status)| invalidated_txids.contains(txid))
                        .is_some()
                    {
                        *utxo.spending_tx_status_mut() = None;
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
            D::WalletNote::get_record_query_matching_outputs_mut(
                transaction_metadata,
                OutputSpendStatusQuery::spentish(),
            )
            .iter_mut()
            .for_each(|note| {
                // Mark note as unspent if the txid being removed spent it.
                if note
                    .spending_tx_status()
                    .filter(|(txid, _status)| invalidated_txids.contains(txid))
                    .is_some()
                {
                    *note.spending_tx_status_mut() = None;
                }
            });
        });
    }

    /// Finds orchard note with given nullifier and updates its spend status
    /// Currently only used for updating through pending statuses
    /// For marking spent see [`crate::wallet::tx_map::TxMap::mark_note_as_spent`]i
    // TODO: verify there is logic to mark pending notes back to unspent during invalidation
    fn update_orchard_note_spend_status(
        &mut self,
        nullifier: &orchard::note::Nullifier,
        spend_status: Option<(TxId, ConfirmationStatus)>,
    ) {
        let source_txid = self
            .values()
            .find(|tx| {
                tx.orchard_notes()
                    .iter()
                    .flat_map(|note| note.nullifier)
                    .any(|nf| nf == *nullifier)
            })
            .map(|tx| tx.txid);

        if let Some(txid) = source_txid {
            let source_tx = self.get_mut(&txid).expect("transaction should exist");
            *source_tx
                .orchard_notes
                .iter_mut()
                .find(|note| {
                    if let Some(nf) = note.nullifier() {
                        nf == *nullifier
                    } else {
                        false
                    }
                })
                .expect("spend must exist")
                .spending_tx_status_mut() = spend_status;
        }
    }
    /// Finds sapling note with given nullifier and updates its spend status
    /// Currently only used for updating through pending statuses
    /// For marking spent see [`crate::wallet::tx_map::TxMap::mark_note_as_spent`]i
    // TODO: verify there is logic to mark pending notes back to unspent during invalidation
    fn update_sapling_note_spend_status(
        &mut self,
        nullifier: &sapling_crypto::Nullifier,
        spend_status: Option<(TxId, ConfirmationStatus)>,
    ) {
        let source_txid = self
            .values()
            .find(|tx| {
                tx.sapling_notes()
                    .iter()
                    .flat_map(|note| note.nullifier)
                    .any(|nf| nf == *nullifier)
            })
            .map(|tx| tx.txid);

        if let Some(txid) = source_txid {
            let source_tx = self.get_mut(&txid).expect("transaction should exist");
            *source_tx
                .sapling_notes
                .iter_mut()
                .find(|note| {
                    if let Some(nf) = note.nullifier() {
                        nf == *nullifier
                    } else {
                        false
                    }
                })
                .expect("spend must exist")
                .spending_tx_status_mut() = spend_status;
        }
    }

    /// Updates notes spent in spending transaction to the given spend status
    ///
    /// Panics if spending transaction doesn't exist in wallet data, intended to be called after `scan_full_tx`
    pub(crate) fn update_note_spend_statuses(
        &mut self,
        spending_txid: TxId,
        spend_status: Option<(TxId, ConfirmationStatus)>,
    ) {
        if let Some(spending_tx) = self.get(&spending_txid) {
            let orchard_nullifiers = spending_tx.spent_orchard_nullifiers.clone();
            let sapling_nullifiers = spending_tx.spent_sapling_nullifiers.clone();

            orchard_nullifiers
                .iter()
                .for_each(|nf| self.update_orchard_note_spend_status(nf, spend_status));
            sapling_nullifiers
                .iter()
                .for_each(|nf| self.update_sapling_note_spend_status(nf, spend_status));
        }
    }

    fn find_sapling_spend(&self, nullifier: &sapling_crypto::Nullifier) -> Option<&SaplingNote> {
        self.values()
            .flat_map(|wallet_transaction_record| wallet_transaction_record.sapling_notes())
            .find(|&note| {
                if let Some(nf) = note.nullifier() {
                    nf == *nullifier
                } else {
                    false
                }
            })
    }
    fn find_orchard_spend(&self, nullifier: &orchard::note::Nullifier) -> Option<&OrchardNote> {
        self.values()
            .flat_map(|wallet_transaction_record| wallet_transaction_record.orchard_notes())
            .find(|&note| {
                if let Some(nf) = note.nullifier() {
                    nf == *nullifier
                } else {
                    false
                }
            })
    }
    // returns all sapling notes spent in a given transaction.
    // will fail if a spend is not found when `fail_on_miss` is set to true.
    fn get_sapling_notes_spent_in_tx(
        &self,
        query_record: &TransactionRecord,
        fail_on_miss: bool,
    ) -> Result<Vec<&SaplingNote>, FeeError> {
        let mut sapling_spends: Vec<&SaplingNote> =
            Vec::with_capacity(query_record.spent_sapling_nullifiers.len());

        for nullifier in query_record.spent_sapling_nullifiers() {
            if let Some(spend) = self.find_sapling_spend(nullifier) {
                sapling_spends.push(spend);
            } else if fail_on_miss {
                return Err(FeeError::SaplingSpendNotFound(*nullifier));
            }
        }
        Ok(sapling_spends)
    }
    // returns all orchard notes spent in a given transaction.
    // will fail if a spend is not found when `fail_on_miss` is set to true.
    fn get_orchard_notes_spent_in_tx(
        &self,
        query_record: &TransactionRecord,
        fail_on_miss: bool,
    ) -> Result<Vec<&OrchardNote>, FeeError> {
        let mut orchard_spends: Vec<&OrchardNote> =
            Vec::with_capacity(query_record.spent_orchard_nullifiers.len());

        for nullifier in query_record.spent_orchard_nullifiers() {
            if let Some(spend) = self.find_orchard_spend(nullifier) {
                orchard_spends.push(spend);
            } else if fail_on_miss {
                return Err(FeeError::OrchardSpendNotFound(*nullifier));
            }
        }
        Ok(orchard_spends)
    }

    // returns total sum of spends for a given transaction.
    // will fail if a spend is not found in the wallet
    fn total_value_input_to_transaction(
        &self,
        query_record: &TransactionRecord,
    ) -> Result<u64, FeeError> {
        let transparent_spends = self.get_transparent_coins_spent_in_tx(query_record);
        let sapling_spends = self.get_sapling_notes_spent_in_tx(query_record, true)?;
        let orchard_spends = self.get_orchard_notes_spent_in_tx(query_record, true)?;

        if sapling_spends.is_empty() && orchard_spends.is_empty() && transparent_spends.is_empty() {
            if query_record.outgoing_tx_data.is_empty() {
                return Err(FeeError::ReceivedTransaction);
            } else {
                return Err(FeeError::OutgoingWithoutSpends(
                    query_record.outgoing_tx_data.to_vec(),
                ));
            }
        }

        let transparent_spend_value = transparent_spends
            .iter()
            .map(|&coin| coin.value())
            .sum::<u64>();
        let sapling_spend_value = sapling_spends.iter().map(|&note| note.value()).sum::<u64>();
        let orchard_spend_value = orchard_spends.iter().map(|&note| note.value()).sum::<u64>();

        Ok(transparent_spend_value + sapling_spend_value + orchard_spend_value)
    }

    fn get_all_transparent_outputs(&self) -> Vec<&TransparentOutput> {
        self.values()
            .flat_map(|record| record.transparent_outputs())
            .collect()
    }
    /// Because this method needs access to all outputs to query their
    /// "spent" txid it is a method of the TransactionRecordsById
    /// It's theoretically possible to create a 0-input transaction, but I
    /// don't know if it's allowed in protocol.  For the moment I conservatively
    /// assume that a 0-input transaction is unexpected behavior.
    /// A transaction created by another capability, using only shielded inputs,
    /// will also be ZeroInputTransaction.
    fn get_transparent_coins_spent_in_tx(
        &self,
        query_record: &TransactionRecord,
    ) -> Vec<&TransparentOutput> {
        self.get_all_transparent_outputs()
            .into_iter()
            .filter(|o| {
                (*o.spending_tx_status()).map_or(false, |(txid, _)| txid == query_record.txid)
            })
            .collect()
    }
    /// Calculate the fee for a transaction in the wallet
    ///
    /// # Error
    ///
    /// Returns [`crate::wallet::error::FeeError::ReceivedTransaction`] if no spends or outgoing_tx_data were found
    /// in the wallet for this transaction, indicating this transaction was not created by this spend capability.
    /// Returns
    /// [`crate::wallet::error::FeeError::SaplingSpendNotFound`]
    /// OR
    /// [`crate::wallet::error::FeeError::OrchardSpendNotFound`]
    /// if any shielded spends in the transaction are not
    /// found in the wallet, indicating that all shielded spends have not yet been synced.
    /// Also returns this error
    /// if the transaction record contains outgoing_tx_data but no spends are found.
    /// If a transparent spend has not yet been synced, the fee will be incorrect and return
    /// [`crate::wallet::error::FeeError::FeeUnderflow`] if an underflow occurs.
    /// The tracking of transparent spends will be improved on the next internal wallet version.
    pub fn calculate_transaction_fee(
        &self,
        query_record: &TransactionRecord,
    ) -> Result<u64, FeeError> {
        let input_value = self.total_value_input_to_transaction(query_record)?;
        let explicit_output_value = query_record.total_value_output_to_explicit_receivers();

        if input_value >= explicit_output_value {
            Ok(input_value - explicit_output_value)
        } else {
            Err(FeeError::FeeUnderflow {
                input_value,
                explicit_output_value,
            })
        }
    }

    /// Invalidates all those transactions which were broadcast but never 'confirmed' accepted by a miner.
    pub(crate) fn clear_expired_mempool(&mut self, latest_height: u64) {
        let cutoff = BlockHeight::from_u32(
            (latest_height.saturating_sub(crate::config::MAX_REORG as u64)) as u32,
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

    /// Note this method is INCORRECT in the case of a 0-value, 0-fee transaction from the
    /// Creating Capability.  Such a transaction would violate ZIP317, but could exist in
    /// the Zcash protocol
    ///  TODO:   Test and handle 0-value, 0-fee transaction
    pub(crate) fn transaction_kind(
        &self,
        query_record: &TransactionRecord,
        chain: &ChainType,
    ) -> TransactionKind {
        let zfz_address = match chain {
            ChainType::Mainnet => ZENNIES_FOR_ZINGO_DONATION_ADDRESS,
            ChainType::Testnet => ZENNIES_FOR_ZINGO_TESTNET_ADDRESS,
            ChainType::Regtest(_) => ZENNIES_FOR_ZINGO_REGTEST_ADDRESS,
        };

        let transparent_spends = self.get_transparent_coins_spent_in_tx(query_record);
        let sapling_spends = self
            .get_sapling_notes_spent_in_tx(query_record, false)
            .expect("cannot fail. fail_on_miss is set false");
        let orchard_spends = self
            .get_orchard_notes_spent_in_tx(query_record, false)
            .expect("cannot fail. fail_on_miss is set false");

        if transparent_spends.is_empty()
            && sapling_spends.is_empty()
            && orchard_spends.is_empty()
            && query_record.outgoing_tx_data.is_empty()
        {
            // no spends and no outgoing tx data
            TransactionKind::Received
        } else if !transparent_spends.is_empty()
            && sapling_spends.is_empty()
            && orchard_spends.is_empty()
            && query_record.outgoing_tx_data.is_empty()
            && (!query_record.orchard_notes().is_empty() | !query_record.sapling_notes().is_empty())
        {
            // only transparent spends, no outgoing tx data and notes received
            // TODO: this could be improved by checking outputs recipient addr against the wallet addrs
            TransactionKind::Sent(SendType::Shield)
        } else if query_record.outgoing_tx_data.is_empty()
            || (query_record.outgoing_tx_data.len() == 1
                && query_record.outgoing_tx_data.iter().any(|otd| {
                    otd.recipient_address == *zfz_address
                        || otd.recipient_ua == Some(zfz_address.to_string())
                }))
        {
            // not Received, this capability created this transaction
            // not Shield, notes were spent
            // no outgoing tx data, with the exception of ONLY a Zennies For Zingo! donation
            TransactionKind::Sent(SendType::SendToSelf)
        } else {
            TransactionKind::Sent(SendType::Send)
        }
    }

    /// TODO: Add Doc Comment Here!
    #[allow(deprecated)]
    #[deprecated(note = "uses unstable deprecated functions")]
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
    #[allow(deprecated)]
    #[deprecated(note = "uses unstable deprecated functions")]
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
    pub(crate) fn create_modify_get_transaction_record(
        &mut self,
        txid: &TxId,
        status: ConfirmationStatus,
        datetime: Option<u32>,
    ) -> &'_ mut TransactionRecord {
        // check if there is already a confirmed transaction with the same txid
        let existing_tx_confirmed = if let Some(existing_tx) = self.get(txid) {
            existing_tx.status.is_confirmed()
        } else {
            false
        };

        // if datetime is None, take the datetime value from existing transaction in the wallet
        let datetime = datetime.unwrap_or_else(|| {
            self.get(txid).expect(
            "datetime should only be None when re-scanning a tx that already exists in the wallet",
                )
                .datetime as u32
        });

        // prevent confirmed transaction from being overwritten by pending transaction
        if existing_tx_confirmed && !status.is_confirmed() {
            self.get_mut(txid)
                .expect("previous check proves this tx exists")
        } else {
            self.entry(*txid)
                // if we already have the transaction metadata, it may be newly confirmed. update confirmation_status
                .and_modify(|transaction_metadata| {
                    transaction_metadata.status = status;
                    transaction_metadata.datetime = datetime as u64;
                })
                // if this transaction is new to our data, insert it
                .or_insert_with(|| TransactionRecord::new(status, datetime as u64, txid))
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn add_taddr_spent(
        &mut self,
        txid: TxId,
        status: ConfirmationStatus,
        timestamp: Option<u32>,
        total_transparent_value_spent: u64,
    ) {
        let transaction_metadata =
            self.create_modify_get_transaction_record(&txid, status, timestamp);

        transaction_metadata.total_transparent_value_spent = total_transparent_value_spent;
    }

    /// TODO: Add Doc Comment Here!
    pub fn mark_txid_utxo_spent(
        &mut self,
        spent_txid: TxId,
        output_num: u32,
        source_txid: TxId,
        spending_tx_status: ConfirmationStatus,
    ) -> u64 {
        // Find the UTXO
        let value = if let Some(utxo_transacion_metadata) = self.get_mut(&spent_txid) {
            if let Some(spent_utxo) = utxo_transacion_metadata
                .transparent_outputs
                .iter_mut()
                .find(|u| u.txid == spent_txid && u.output_index == output_num as u64)
            {
                // Mark this utxo as spent
                *spent_utxo.spending_tx_status_mut() = Some((source_txid, spending_tx_status));

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
        status: ConfirmationStatus,
        timestamp: Option<u32>,
        vout: &zcash_primitives::transaction::components::TxOut,
        output_num: u32,
    ) {
        // Read or create the current TxId
        let transaction_metadata =
            self.create_modify_get_transaction_record(&txid, status, timestamp);

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
                ),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add_new_note<D: DomainWalletExt>(
        &mut self,
        txid: TxId,
        status: ConfirmationStatus,
        timestamp: Option<u32>,
        note: <D::WalletNote as crate::wallet::notes::ShieldedNoteInterface>::Note,
        to: D::Recipient,
        have_spending_key: bool,
        nullifier: Option<
            <D::WalletNote as crate::wallet::notes::ShieldedNoteInterface>::Nullifier,
        >,
        output_index: u32,
        position: incrementalmerkletree::Position,
    ) {
        let transaction_metadata =
            self.create_modify_get_transaction_record(&txid, status, timestamp);

        let nd = D::WalletNote::from_parts(
            D::Recipient::diversifier(&to),
            note.clone(),
            Some(position),
            nullifier,
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

    /// get a list of spendable NoteIds with associated note values
    #[allow(clippy::type_complexity)]
    pub(crate) fn get_spendable_note_ids_and_values(
        &self,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[NoteId],
    ) -> Result<Vec<(NoteId, u64)>, Vec<(TxId, BlockHeight)>> {
        let mut missing_output_index = vec![];
        let ok = self
            .values()
            .flat_map(|transaction_record| {
                if transaction_record
                    .status
                    .is_confirmed_before_or_at(&anchor_height)
                {
                    if let Ok(notes_from_tx) =
                        transaction_record.get_spendable_note_ids_and_values(sources, exclude)
                    {
                        notes_from_tx
                    } else {
                        missing_output_index.push((
                            transaction_record.txid,
                            transaction_record.status.get_height(),
                        ));
                        vec![]
                    }
                } else {
                    vec![]
                }
            })
            .collect();
        if missing_output_index.is_empty() {
            Ok(ok)
        } else {
            Err(missing_output_index)
        }
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
                orchard::mocks::OrchardNoteBuilder, query::OutputSpendStatusQuery,
                sapling::mocks::SaplingNoteBuilder, transparent::mocks::TransparentOutputBuilder,
                Output, OutputInterface,
            },
            transaction_record::mocks::{nine_note_transaction_record, TransactionRecordBuilder},
        },
    };

    use super::TransactionRecordsById;

    use sapling_crypto::note_encryption::SaplingDomain;
    use zcash_client_backend::{wallet::ReceivedNote, ShieldedProtocol};
    use zcash_primitives::{consensus::BlockHeight, transaction::TxId};
    use zingo_status::confirmation_status::ConfirmationStatus::{self, Confirmed};

    #[test]
    fn invalidate_all_transactions_after_or_at_height() {
        let transaction_record_later = TransactionRecordBuilder::default()
            .randomize_txid()
            .status(Confirmed(15.into()))
            .transparent_outputs(TransparentOutputBuilder::default())
            .build();
        let spending_txid = transaction_record_later.txid;

        let spend_in_known_tx = Some((spending_txid, Confirmed(15.into())));

        let transaction_record_early = TransactionRecordBuilder::default()
            .randomize_txid()
            .status(Confirmed(5.into()))
            .transparent_outputs(
                TransparentOutputBuilder::default()
                    .spending_tx_status(spend_in_known_tx)
                    .clone(),
            )
            .sapling_notes(
                SaplingNoteBuilder::default()
                    .spending_tx_status(spend_in_known_tx)
                    .clone(),
            )
            .orchard_notes(
                OrchardNoteBuilder::default()
                    .spending_tx_status(spend_in_known_tx)
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

        let query_for_spentish_notes = OutputSpendStatusQuery::spentish();
        let spentish_sapling_notes_in_tx_cvnwis = Output::get_all_outputs_with_status(
            transaction_record_cvnwis,
            query_for_spentish_notes,
        );
        assert_eq!(spentish_sapling_notes_in_tx_cvnwis.len(), 0);
    }

    // TODO: move this into an associated fn of TransparentOutputBuilder
    fn spent_transparent_output_builder(
        amount: u64,
        sent: (TxId, ConfirmationStatus),
    ) -> TransparentOutputBuilder {
        TransparentOutputBuilder::default()
            .value(amount)
            .spending_tx_status(Some(sent))
            .to_owned()
    }

    fn spent_sapling_note_builder(
        amount: u64,
        sent: (TxId, ConfirmationStatus),
        sapling_nullifier: &sapling_crypto::Nullifier,
    ) -> SaplingNoteBuilder {
        SaplingNoteBuilder::default()
            .note(
                SaplingCryptoNoteBuilder::default()
                    .value(sapling_crypto::value::NoteValue::from_raw(amount))
                    .to_owned(),
            )
            .spending_tx_status(Some(sent))
            .nullifier(Some(*sapling_nullifier))
            .to_owned()
    }
    fn spent_orchard_note_builder(
        amount: u64,
        sent: (TxId, ConfirmationStatus),
        orchard_nullifier: &orchard::note::Nullifier,
    ) -> OrchardNoteBuilder {
        OrchardNoteBuilder::default()
            .note(
                OrchardCryptoNoteBuilder::default()
                    .value(orchard::value::NoteValue::from_raw(amount))
                    .to_owned(),
            )
            .spending_tx_status(Some(sent))
            .nullifier(Some(*orchard_nullifier))
            .to_owned()
    }
    #[test]
    fn calculate_transaction_fee() {
        let mut sapling_nullifier_builder = SaplingNullifierBuilder::new();
        let mut orchard_nullifier_builder = OrchardNullifierBuilder::new();

        let sent_transaction_record = TransactionRecordBuilder::default()
            .status(Confirmed(15.into()))
            .spent_sapling_nullifiers(sapling_nullifier_builder.assign_unique_nullifier().clone())
            .spent_sapling_nullifiers(sapling_nullifier_builder.assign_unique_nullifier().clone())
            .spent_orchard_nullifiers(orchard_nullifier_builder.assign_unique_nullifier().clone())
            .spent_orchard_nullifiers(orchard_nullifier_builder.assign_unique_nullifier().clone())
            .transparent_outputs(TransparentOutputBuilder::default()) // value 100_000
            .sapling_notes(SaplingNoteBuilder::default()) // value 200_000
            .orchard_notes(OrchardNoteBuilder::default()) // value 800_000
            .outgoing_tx_data(OutgoingTxDataBuilder::default()) // value 50_000
            .build();
        let sent_txid = sent_transaction_record.txid;
        let first_sapling_nullifier = sent_transaction_record.spent_sapling_nullifiers[0];
        let second_sapling_nullifier = sent_transaction_record.spent_sapling_nullifiers[1];
        let first_orchard_nullifier = sent_transaction_record.spent_orchard_nullifiers[0];
        let second_orchard_nullifier = sent_transaction_record.spent_orchard_nullifiers[1];
        // t-note + s-note + o-note + outgoing_tx_data
        let expected_output_value: u64 = 100_000 + 200_000 + 800_000 + 50_000; // 1_150_000

        let spent_in_sent_txid = (sent_txid, Confirmed(15.into()));
        let first_received_transaction_record = TransactionRecordBuilder::default()
            .randomize_txid()
            .status(Confirmed(5.into()))
            .sapling_notes(spent_sapling_note_builder(
                175_000,
                spent_in_sent_txid,
                &first_sapling_nullifier,
            ))
            .sapling_notes(spent_sapling_note_builder(
                325_000,
                spent_in_sent_txid,
                &second_sapling_nullifier,
            ))
            .orchard_notes(spent_orchard_note_builder(
                500_000,
                spent_in_sent_txid,
                &first_orchard_nullifier,
            ))
            .transparent_outputs(spent_transparent_output_builder(30_000, spent_in_sent_txid)) // 100_000
            .sapling_notes(
                SaplingNoteBuilder::default()
                    .spending_tx_status(Some((random_txid(), Confirmed(12.into()))))
                    .to_owned(),
            )
            .orchard_notes(OrchardNoteBuilder::default()) // 800_000
            .set_output_indexes()
            .build();
        let second_received_transaction_record = TransactionRecordBuilder::default()
            .randomize_txid()
            .status(Confirmed(7.into()))
            .orchard_notes(spent_orchard_note_builder(
                200_000,
                spent_in_sent_txid,
                &second_orchard_nullifier,
            ))
            .transparent_outputs(TransparentOutputBuilder::default())
            .sapling_notes(SaplingNoteBuilder::default().clone())
            .orchard_notes(
                OrchardNoteBuilder::default()
                    .spending_tx_status(Some((random_txid(), Confirmed(13.into()))))
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
                transaction_records_by_id::{
                    tests::spent_transparent_output_builder, TransactionRecordsById,
                },
            },
        };

        use zingo_status::confirmation_status::ConfirmationStatus::Confirmed;

        #[test]
        fn spend_not_found() {
            let mut sapling_nullifier_builder = SaplingNullifierBuilder::new();
            let mut orchard_nullifier_builder = OrchardNullifierBuilder::new();

            let sent_transaction_record = TransactionRecordBuilder::default()
                .status(Confirmed(15.into()))
                .spent_sapling_nullifiers(
                    sapling_nullifier_builder.assign_unique_nullifier().clone(),
                )
                .spent_orchard_nullifiers(
                    orchard_nullifier_builder.assign_unique_nullifier().clone(),
                )
                .outgoing_tx_data(OutgoingTxDataBuilder::default())
                .transparent_outputs(TransparentOutputBuilder::default())
                .sapling_notes(SaplingNoteBuilder::default())
                .orchard_notes(OrchardNoteBuilder::default())
                .build();
            let sent_txid = sent_transaction_record.txid;
            let sapling_nullifier = sent_transaction_record.spent_sapling_nullifiers[0];

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
                        .spending_tx_status(Some((sent_txid, Confirmed(15.into()))))
                        .nullifier(Some(sapling_nullifier))
                        .to_owned(),
                )
                .build();

            let mut transaction_records_by_id = TransactionRecordsById::default();
            transaction_records_by_id.insert_transaction_record(sent_transaction_record);
            transaction_records_by_id.insert_transaction_record(received_transaction_record);

            let fee = transaction_records_by_id
                .calculate_transaction_fee(transaction_records_by_id.get(&sent_txid).unwrap());
            assert!(matches!(fee, Err(FeeError::OrchardSpendNotFound(_))));
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
            assert!(matches!(fee, Err(FeeError::OutgoingWithoutSpends(_))));
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
                .build();
            let sent_txid = transaction_record.txid;
            let spent_in_sent_txid = (sent_txid, Confirmed(15.into()));
            let transparent_funding_tx = TransactionRecordBuilder::default()
                .randomize_txid()
                .status(Confirmed(7.into()))
                .transparent_outputs(spent_transparent_output_builder(20_000, spent_in_sent_txid))
                .set_output_indexes()
                .build();

            let mut transaction_records_by_id = TransactionRecordsById::default();
            transaction_records_by_id.insert_transaction_record(transaction_record);
            transaction_records_by_id.insert_transaction_record(transparent_funding_tx);

            let fee = transaction_records_by_id
                .calculate_transaction_fee(transaction_records_by_id.get(&sent_txid).unwrap());
            assert!(matches!(
                fee,
                Err(FeeError::FeeUnderflow {
                    input_value: _,
                    explicit_output_value: _,
                })
            ));
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
                if record.sapling_notes[i]
                    .spend_status_query(OutputSpendStatusQuery::only_unspent())
                {
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
