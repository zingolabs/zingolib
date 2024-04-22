use incrementalmerkletree::Position;
use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_note_encryption::Domain;
use zcash_primitives::{
    consensus::BlockHeight,
    memo::Memo,
    transaction::{components::TxOut, TxId},
};
use zingo_status::confirmation_status::ConfirmationStatus;
use zingoconfig::MAX_REORG;

use log::error;

use crate::{
    error::{ZingoLibError, ZingoLibResult},
    wallet::{
        data::{OutgoingTxData, PoolNullifier, TransactionRecord},
        notes::NoteInterface,
        notes::ShieldedNoteInterface,
        traits::{self, DomainWalletExt, Nullifier, Recipient},
    },
};

use super::TxMapAndMaybeTrees;
impl TxMapAndMaybeTrees {
    /// During reorgs, we need to remove all txns at a given height, and all spends that refer to any removed txns.
    pub fn invalidate_all_transactions_after_or_at_height(&mut self, reorg_height: u64) {
        let reorg_height = BlockHeight::from_u32(reorg_height as u32);

        self.transaction_records_by_id
            .invalidate_all_transactions_after_or_at_height(reorg_height);

        if let Some(ref mut t) = self.witness_trees {
            t.witness_tree_sapling
                .truncate_removing_checkpoint(&(reorg_height - 1))
                .expect("Infallible");
            t.witness_tree_orchard
                .truncate_removing_checkpoint(&(reorg_height - 1))
                .expect("Infallible");
            t.add_checkpoint(reorg_height - 1);
        }
    }

    /// Invalidates all those transactions which were broadcast but never 'confirmed' accepted by a miner.
    pub(crate) fn clear_expired_mempool(&mut self, latest_height: u64) {
        let cutoff = BlockHeight::from_u32((latest_height.saturating_sub(MAX_REORG as u64)) as u32);

        let txids_to_remove = self
            .transaction_records_by_id
            .iter()
            .filter(|(_, transaction_metadata)| {
                transaction_metadata.status.is_broadcast_before(&cutoff)
            }) // this transaction was submitted to the mempool before the cutoff and has not been confirmed. we deduce that it has expired.
            .map(|(_, transaction_metadata)| transaction_metadata.txid)
            .collect::<Vec<_>>();

        txids_to_remove
            .iter()
            .for_each(|t| println!("Removing expired mempool tx {}", t));

        self.transaction_records_by_id
            .invalidate_transactions(txids_to_remove);
    }

    // Check this transaction to see if it is an outgoing transaction, and if it is, mark all received notes with non-textual memos in this
    // transaction as change. i.e., If any funds were spent in this transaction, all received notes without user-specified memos are change.
    //
    // TODO: When we start working on multi-sig, this could cause issues about hiding sends-to-self
    pub fn check_notes_mark_change(&mut self, txid: &TxId) {
        //TODO: Incorrect with a 0-value fee somehow
        if self.total_funds_spent_in(txid) > 0 {
            if let Some(transaction_metadata) = self.transaction_records_by_id.get_mut(txid) {
                Self::mark_notes_as_change_for_pool(&mut transaction_metadata.sapling_notes);
                Self::mark_notes_as_change_for_pool(&mut transaction_metadata.orchard_notes);
            }
        }
    }
    fn mark_notes_as_change_for_pool<Note: ShieldedNoteInterface>(notes: &mut [Note]) {
        notes.iter_mut().for_each(|n| {
            *n.is_change_mut() = match n.memo() {
                Some(Memo::Text(_)) => false,
                Some(Memo::Empty | Memo::Arbitrary(_) | Memo::Future(_)) | None => true,
            }
        });
    }

    fn create_modify_get_transaction_metadata(
        &mut self,
        txid: &TxId,
        status: ConfirmationStatus,
        datetime: u64,
    ) -> &'_ mut TransactionRecord {
        self.transaction_records_by_id
            .entry(*txid)
            // If we already have the transaction metadata, it may be newly confirmed. Update confirmation_status
            .and_modify(|transaction_metadata| {
                transaction_metadata.status = status;
                transaction_metadata.datetime = datetime;
            })
            // if this transaction is new to our data, insert it
            .or_insert_with(|| TransactionRecord::new(status, datetime, txid))
    }

    // Records a TxId as having spent some nullifiers from the wallet.
    #[allow(clippy::too_many_arguments)]
    pub fn found_spent_nullifier(
        &mut self,
        spending_txid: TxId,
        status: ConfirmationStatus,
        timestamp: u32,
        spent_nullifier: PoolNullifier,
        source_txid: TxId,
        output_index: Option<u32>,
    ) -> ZingoLibResult<()> {
        match spent_nullifier {
            PoolNullifier::Orchard(spent_nullifier) => self
                .found_spent_nullifier_internal::<OrchardDomain>(
                    spending_txid,
                    status,
                    timestamp,
                    spent_nullifier,
                    source_txid,
                    output_index,
                ),
            PoolNullifier::Sapling(spent_nullifier) => self
                .found_spent_nullifier_internal::<SaplingDomain>(
                    spending_txid,
                    status,
                    timestamp,
                    spent_nullifier,
                    source_txid,
                    output_index,
                ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn found_spent_nullifier_internal<D: DomainWalletExt>(
        &mut self,
        spending_txid: TxId,
        status: ConfirmationStatus,
        timestamp: u32,
        spent_nullifier: <D::WalletNote as ShieldedNoteInterface>::Nullifier,
        source_txid: TxId,
        output_index: Option<u32>,
    ) -> ZingoLibResult<()>
    where
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: traits::Recipient,
    {
        // Mark the source note as spent
        let value = self.mark_note_as_spent::<D>(
            spent_nullifier,
            spending_txid,
            status,
            source_txid,
            output_index,
        )?; // todo error handling

        // Record this Tx as having spent some funds
        let transaction_metadata =
            self.create_modify_get_transaction_metadata(&spending_txid, status, timestamp as u64);

        if !<D::WalletNote as ShieldedNoteInterface>::Nullifier::get_nullifiers_spent_in_transaction(
            transaction_metadata,
        )
        .iter()
        .any(|nf| *nf == spent_nullifier)
        {
            transaction_metadata.add_spent_nullifier(spent_nullifier.into(), value)
        }

        // Since this Txid has spent some funds, output notes in this Tx that are sent to us are actually change.
        self.check_notes_mark_change(&spending_txid);

        Ok(())
    }

    // Will mark a note as having been spent at the supplied height and spent_txid.
    pub fn mark_note_as_spent<D: DomainWalletExt>(
        &mut self,
        spent_nullifier: <D::WalletNote as ShieldedNoteInterface>::Nullifier,
        spending_txid: TxId,
        status: ConfirmationStatus,
        source_txid: TxId,
        output_index: Option<u32>,
    ) -> ZingoLibResult<u64>
    where
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: traits::Recipient,
    {
        Ok(if let Some(height) = status.get_confirmed_height() {
            // ie remove_witness_mark_sapling or _orchard
            self.remove_witness_mark::<D>(height, spending_txid, source_txid, output_index)?;
            if let Some(transaction_spent_from) =
                self.transaction_records_by_id.get_mut(&source_txid)
            {
                if let Some(confirmed_spent_note) = D::to_notes_vec_mut(transaction_spent_from)
                    .iter_mut()
                    .find(|note| note.nullifier() == Some(spent_nullifier))
                {
                    *confirmed_spent_note.spent_mut() = Some((spending_txid, height.into()));
                    *confirmed_spent_note.pending_spent_mut() = None;

                    confirmed_spent_note.value()
                } else {
                    ZingoLibError::NoSuchNullifierInTx(spending_txid).handle()?
                }
            } else {
                ZingoLibError::NoSuchTxId(spending_txid).handle()?
            }
        } else if let Some(height) = status.get_broadcast_height() {
            // Mark the unconfirmed_spent. Confirmed spends are already handled in update_notes
            if let Some(transaction_spent_from) =
                self.transaction_records_by_id.get_mut(&source_txid)
            {
                if let Some(unconfirmed_spent_note) = D::to_notes_vec_mut(transaction_spent_from)
                    .iter_mut()
                    .find(|note| note.nullifier() == Some(spent_nullifier))
                {
                    *unconfirmed_spent_note.pending_spent_mut() =
                        Some((spending_txid, u32::from(height)));
                    unconfirmed_spent_note.value()
                } else {
                    ZingoLibError::NoSuchNullifierInTx(spending_txid).handle()?
                }
            } else {
                ZingoLibError::NoSuchTxId(spending_txid).handle()?
            }
        } else {
            ZingoLibError::UnknownError.handle()?
        }) // todO add special error variant
    }

    pub fn add_taddr_spent(
        &mut self,
        txid: TxId,
        status: ConfirmationStatus,
        timestamp: u64,
        total_transparent_value_spent: u64,
    ) {
        let transaction_metadata =
            self.create_modify_get_transaction_metadata(&txid, status, timestamp);

        transaction_metadata.total_transparent_value_spent = total_transparent_value_spent;

        self.check_notes_mark_change(&txid);
    }

    pub fn mark_txid_utxo_spent(
        &mut self,
        spent_txid: TxId,
        output_num: u32,
        source_txid: TxId,
        spending_tx_status: ConfirmationStatus,
    ) -> u64 {
        // Find the UTXO
        let value = if let Some(utxo_transacion_metadata) =
            self.transaction_records_by_id.get_mut(&spent_txid)
        {
            if let Some(spent_utxo) = utxo_transacion_metadata
                .transparent_notes
                .iter_mut()
                .find(|u| u.txid == spent_txid && u.output_index == output_num as u64)
            {
                if spending_tx_status.is_confirmed() {
                    // Mark this utxo as spent
                    *spent_utxo.spent_mut() =
                        Some((source_txid, spending_tx_status.get_height().into()));
                    spent_utxo.unconfirmed_spent = None;
                } else {
                    spent_utxo.unconfirmed_spent =
                        Some((source_txid, u32::from(spending_tx_status.get_height())));
                }

                spent_utxo.value
            } else {
                error!("Couldn't find UTXO that was spent");
                0
            }
        } else {
            error!("Couldn't find TxID that was spent!");
            0
        };

        // Return the value of the note that was spent.
        value
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_new_taddr_output(
        &mut self,
        txid: TxId,
        taddr: String,
        status: ConfirmationStatus,
        timestamp: u64,
        vout: &TxOut,
        output_num: u32,
    ) {
        // Read or create the current TxId
        let transaction_metadata =
            self.create_modify_get_transaction_metadata(&txid, status, timestamp);

        // Add this UTXO if it doesn't already exist
        if transaction_metadata
            .transparent_notes
            .iter_mut()
            .any(|utxo| utxo.txid == txid && utxo.output_index == output_num as u64)
        {
            // If it already exists, it is likely an mempool tx, so update the height
        } else {
            transaction_metadata.transparent_notes.push(
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
        let status = ConfirmationStatus::Broadcast(height);
        let transaction_metadata =
            self.create_modify_get_transaction_metadata(&txid, status, timestamp);

        match D::to_notes_vec_mut(transaction_metadata)
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

                D::WalletNote::transaction_metadata_notes_mut(transaction_metadata).push(nd);
            }
            Some(_) => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn add_new_note<D: DomainWalletExt>(
        &mut self,
        txid: TxId,
        status: ConfirmationStatus,
        timestamp: u64,
        note: <D::WalletNote as ShieldedNoteInterface>::Note,
        to: D::Recipient,
        have_spending_key: bool,
        nullifier: Option<<D::WalletNote as ShieldedNoteInterface>::Nullifier>,
        output_index: u32,
        position: Position,
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
    pub(crate) fn add_memo_to_note_metadata<Nd: ShieldedNoteInterface>(
        &mut self,
        txid: &TxId,
        note: Nd::Note,
        memo: Memo,
    ) {
        if let Some(transaction_metadata) = self.transaction_records_by_id.get_mut(txid) {
            if let Some(n) = Nd::transaction_metadata_notes_mut(transaction_metadata)
                .iter_mut()
                .find(|n| n.note() == &note)
            {
                *n.memo_mut() = Some(memo);
            }
        }
    }

    pub fn add_outgoing_metadata(&mut self, txid: &TxId, outgoing_metadata: Vec<OutgoingTxData>) {
        // println!("        adding outgoing metadata to txid {}", txid);
        if let Some(transaction_metadata) = self.transaction_records_by_id.get_mut(txid) {
            transaction_metadata.outgoing_tx_data = outgoing_metadata
        } else {
            error!(
                "TxId {} should be present while adding metadata, but wasn't",
                txid
            );
        }
    }

    pub fn set_price(&mut self, txid: &TxId, price: Option<f64>) {
        price.map(|p| {
            self.transaction_records_by_id
                .get_mut(txid)
                .map(|tx| tx.price = Some(p))
        });
    }
}

// shardtree
impl TxMapAndMaybeTrees {
    /// A mark designates a leaf as non-ephemeral, mark removal causes
    /// the leaf to eventually transition to the ephemeral state
    pub fn remove_witness_mark<D>(
        &mut self,
        height: BlockHeight,
        txid: TxId,
        source_txid: TxId,
        output_index: Option<u32>,
    ) -> ZingoLibResult<()>
    where
        D: DomainWalletExt,
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: Recipient,
    {
        let transaction_metadata = self
            .transaction_records_by_id
            .get_mut(&source_txid)
            .expect("Txid should be present");

        if let Some(maybe_note) = D::to_notes_vec_mut(transaction_metadata)
            .iter_mut()
            .find_map(|nnmd| {
                if nnmd.output_index().is_some() != output_index.is_some() {
                    return Some(Err(ZingoLibError::MissingOutputIndex(txid)));
                }
                if *nnmd.output_index() == output_index {
                    Some(Ok(nnmd))
                } else {
                    None
                }
            })
        {
            match maybe_note {
                Ok(note_datum) => {
                    *note_datum.spent_mut() = Some((txid, height.into()));
                    if let Some(position) = *note_datum.witnessed_position() {
                        if let Some(ref mut tree) =
                            D::transaction_metadata_set_to_shardtree_mut(self)
                        {
                            tree.remove_mark(position, Some(&(height - BlockHeight::from(1))))
                                .unwrap();
                        }
                    } else {
                        todo!("Tried to mark note as spent with no position: FIX")
                    }
                }
                Err(_) => return Err(ZingoLibError::MissingOutputIndex(txid)),
            }
        } else {
            eprintln!("Could not remove node!")
        }
        Ok(())
    }

    pub(crate) fn mark_note_position<D: DomainWalletExt>(
        &mut self,
        txid: TxId,
        output_index: Option<u32>,
        position: Position,
        fvk: &D::Fvk,
    ) -> ZingoLibResult<()>
    where
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: Recipient,
    {
        if let Some(tmd) = self.transaction_records_by_id.get_mut(&txid) {
            if let Some(maybe_nnmd) = &mut D::to_notes_vec_mut(tmd).iter_mut().find_map(|nnmd| {
                if nnmd.output_index().is_some() != output_index.is_some() {
                    return Some(Err(ZingoLibError::MissingOutputIndex(txid)));
                }
                if *nnmd.output_index() == output_index {
                    Some(Ok(nnmd))
                } else {
                    None
                }
            }) {
                match maybe_nnmd {
                    Ok(nnmd) => {
                        *nnmd.witnessed_position_mut() = Some(position);
                        *nnmd.nullifier_mut() =
                            Some(D::get_nullifier_from_note_fvk_and_witness_position(
                                &nnmd.note().clone(),
                                fvk,
                                u64::from(position),
                            ));
                    }
                    Err(_) => return Err(ZingoLibError::MissingOutputIndex(txid)),
                }
            } else {
                println!("Could not update witness position");
            }
        } else {
            println!("Could not update witness position");
        }
        Ok(())
    }
}
