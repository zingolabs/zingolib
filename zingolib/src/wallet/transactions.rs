use std::collections::HashMap;

use incrementalmerkletree::Position;
use log::error;
use orchard;
use orchard::note_encryption::OrchardDomain;
use zcash_note_encryption::Domain;
use zcash_primitives::{
    consensus::BlockHeight,
    memo::Memo,
    sapling::note_encryption::SaplingDomain,
    transaction::{components::TxOut, TxId},
};

use zingo_status::confirmation_status::ConfirmationStatus;
use zingoconfig::{ChainType, MAX_REORG};

use crate::error::{ZingoLibError, ZingoLibResult};

use super::{
    data::{OutgoingTxData, PoolNullifier, TransactionMetadata, TransparentNote, WitnessTrees},
    traits::{self, DomainWalletExt, Nullifier, Recipient, ShieldedNoteInterface},
};

/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TransactionMetadataSet {
    pub current: HashMap<TxId, TransactionMetadata>,
    pub(crate) some_highest_txid: Option<TxId>,
    pub witness_trees: Option<WitnessTrees>,
}

pub mod read_write;

impl TransactionMetadataSet {
    pub fn clear(&mut self) {
        self.current.clear();
        self.witness_trees.as_mut().map(WitnessTrees::clear);
    }

    // transaction handling

    pub fn remove_txids(&mut self, txids_to_remove: Vec<TxId>) {
        for txid in &txids_to_remove {
            self.current.remove(txid);
        }
        self.current.values_mut().for_each(|transaction_metadata| {
            // Update UTXOs to rollback any spent utxos
            transaction_metadata
                .transparent_notes
                .iter_mut()
                .for_each(|utxo| {
                    if utxo.spent.is_some() && txids_to_remove.contains(&utxo.spent.unwrap()) {
                        utxo.spent = None;
                        utxo.spent_at_height = None;
                    }

                    if utxo.unconfirmed_spent.is_some()
                        && txids_to_remove.contains(&utxo.unconfirmed_spent.unwrap().0)
                    {
                        utxo.unconfirmed_spent = None;
                    }
                })
        });
        self.remove_domain_specific_txids::<SaplingDomain<ChainType>>(&txids_to_remove);
        self.remove_domain_specific_txids::<OrchardDomain>(&txids_to_remove);
    }

    fn remove_domain_specific_txids<D: DomainWalletExt>(&mut self, txids_to_remove: &[TxId])
    where
        <D as Domain>::Recipient: Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        self.current.values_mut().for_each(|transaction_metadata| {
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

    // During reorgs, we need to remove all txns at a given height, and all spends that refer to any removed txns.
    pub fn remove_txns_at_height(&mut self, reorg_height: u64) {
        let reorg_height = BlockHeight::from_u32(reorg_height as u32);

        // First, collect txids that need to be removed
        let txids_to_remove = self
            .current
            .values()
            .filter_map(|transaction_metadata| {
                if transaction_metadata
                    .status
                    .is_confirmed_after_or_at(&reorg_height)
                    || transaction_metadata
                        .status
                        .is_broadcast_unconfirmed_after(&reorg_height)
                {
                    Some(transaction_metadata.txid)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        self.remove_txids(txids_to_remove);
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

    pub(crate) fn clear_expired_mempool(&mut self, latest_height: u64) {
        let cutoff = BlockHeight::from_u32((latest_height.saturating_sub(MAX_REORG as u64)) as u32);

        let txids_to_remove = self
            .current
            .iter()
            .filter(|(_, transaction_metadata)| transaction_metadata.status.is_expired(&cutoff))
            .map(|(_, transaction_metadata)| transaction_metadata.txid)
            .collect::<Vec<_>>();

        txids_to_remove
            .iter()
            .for_each(|t| println!("Removing expired mempool tx {}", t));

        self.remove_txids(txids_to_remove);
    }

    // Check this transaction to see if it is an outgoing transaction, and if it is, mark all received notes with non-textual memos in this
    // transction as change. i.e., If any funds were spent in this transaction, all received notes without user-specified memos are change.
    //
    // TODO: When we start working on multi-sig, this could cause issues about hiding sends-to-self
    pub fn check_notes_mark_change(&mut self, txid: &TxId) {
        //TODO: Incorrect with a 0-value fee somehow
        if self.total_funds_spent_in(txid) > 0 {
            if let Some(transaction_metadata) = self.current.get_mut(txid) {
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
    ) -> &'_ mut TransactionMetadata {
        self.current
            .entry(*txid)
            // If we already have the transaction metadata, it may be newly confirmed. Update confirmation_status
            .and_modify(|transaction_metadata| {
                transaction_metadata.status = status;
                transaction_metadata.datetime = datetime;
            })
            // if this transaction is new to our data, insert it
            .or_insert_with(|| {
                self.some_highest_txid = Some(*txid); // TOdO IS this the highest wallet block?
                TransactionMetadata::new(status, datetime, txid)
            })
    }

    // Records a TxId as having spent some nullifiers from the wallet.
    #[allow(clippy::too_many_arguments)]
    pub fn add_new_spent(
        &mut self,
        txid: TxId,
        status: ConfirmationStatus,
        timestamp: u32,
        spent_nullifier: PoolNullifier,
        value: u64,
        source_txid: TxId,
        output_index: u32,
    ) {
        match spent_nullifier {
            PoolNullifier::Orchard(spent_nullifier) => self
                .add_new_spent_internal::<OrchardDomain>(
                    txid,
                    status,
                    timestamp,
                    spent_nullifier,
                    value,
                    source_txid,
                    output_index,
                ),
            PoolNullifier::Sapling(spent_nullifier) => self
                .add_new_spent_internal::<SaplingDomain<ChainType>>(
                    txid,
                    status,
                    timestamp,
                    spent_nullifier,
                    value,
                    source_txid,
                    output_index,
                ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_new_spent_internal<D: DomainWalletExt>(
        &mut self,
        txid: TxId,
        status: ConfirmationStatus,
        timestamp: u32,
        spent_nullifier: <D::WalletNote as ShieldedNoteInterface>::Nullifier,
        value: u64,
        source_txid: TxId,
        output_index: u32,
    ) where
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: traits::Recipient,
    {
        // Record this Tx as having spent some funds
        let transaction_metadata =
            self.create_modify_get_transaction_metadata(&txid, status, timestamp as u64);

        if !<D::WalletNote as ShieldedNoteInterface>::Nullifier::get_nullifiers_spent_in_transaction(
            transaction_metadata,
        )
        .iter()
        .any(|nf| *nf == spent_nullifier)
        {
            transaction_metadata.add_spent_nullifier(spent_nullifier.into(), value)
        }

        // Since this Txid has spent some funds, output notes in this Tx that are sent to us are actually change.
        self.check_notes_mark_change(&txid);

        // Mark the source note as spent
        if let Some(height) = status.get_confirmed_height() {
            // ie remove_witness_mark_sapling or _orchard
            self.remove_witness_mark::<D>(height, txid, source_txid, output_index);
        } else if let Some(height) = status.get_broadcast_unconfirmed_height() {
            // Mark the unconfirmed_spent. Confirmed spends are already handled in update_notes
            if let Some(transaction_spent_from) = self.current.get_mut(&source_txid) {
                if let Some(unconfirmed_spent_note) = D::to_notes_vec_mut(transaction_spent_from)
                    .iter_mut()
                    .find(|note| note.nullifier() == Some(spent_nullifier))
                {
                    *unconfirmed_spent_note.pending_spent_mut() = Some((txid, u32::from(height)));
                }
            }
        }
    }

    // Will mark a note as having been spent at the supplied height and spent_txid.
    // Takes the nullifier of the spent note, the note's index in its containing transaction,
    // as well as the txid of its containing transaction. tODO: make generic
    pub fn process_spent_note(
        &mut self,
        txid: TxId,
        spent_nullifier: &PoolNullifier,
        spent_txid: &TxId,
        spent_at_height: BlockHeight,
        output_index: u32,
    ) -> ZingoLibResult<u64> {
        match self.current.get_mut(&txid) {
            None => ZingoLibError::NoSuchTxId(txid).print_and_pass_error(),
            Some(transaction_metadata) => match spent_nullifier {
                PoolNullifier::Sapling(_sapling_nullifier) => {
                    if let Some(sapling_note_data) = transaction_metadata
                        .sapling_notes
                        .iter_mut()
                        .find(|n| n.output_index == output_index)
                    {
                        sapling_note_data.spent = Some((*spent_txid, spent_at_height.into()));
                        sapling_note_data.unconfirmed_spent = None;
                        Ok(sapling_note_data.note.value().inner())
                    } else {
                        ZingoLibError::NoSuchSaplingOutputInTxId(txid, output_index)
                            .print_and_pass_error()
                    }
                }
                PoolNullifier::Orchard(_orchard_nullifier) => {
                    if let Some(orchard_note_data) = transaction_metadata
                        .orchard_notes
                        .iter_mut()
                        .find(|n| n.output_index == output_index)
                    {
                        orchard_note_data.spent = Some((*spent_txid, spent_at_height.into()));
                        orchard_note_data.unconfirmed_spent = None;
                        Ok(orchard_note_data.note.value().inner())
                    } else {
                        ZingoLibError::NoSuchOrchardOutputInTxId(txid, output_index)
                            .print_and_pass_error()
                    }
                }
            },
        }
    }

    pub fn add_taddr_spent(
        &mut self,
        txid: TxId,
        height: BlockHeight,
        unconfirmed: bool,
        timestamp: u64,
        total_transparent_value_spent: u64,
    ) {
        let status = ConfirmationStatus::from_blockheight_and_unconfirmed_bool(height, unconfirmed);
        let transaction_metadata =
            self.create_modify_get_transaction_metadata(&txid, status, timestamp);
        // Todo yeesh
        transaction_metadata.total_transparent_value_spent = total_transparent_value_spent;

        self.check_notes_mark_change(&txid);
    }

    pub fn mark_txid_utxo_spent(
        &mut self,
        spent_txid: TxId,
        output_num: u32,
        source_txid: TxId,
        source_height: u32,
        unconfirmed: bool,
    ) -> u64 {
        // Find the UTXO
        let value = if let Some(utxo_transacion_metadata) = self.current.get_mut(&spent_txid) {
            if let Some(spent_utxo) = utxo_transacion_metadata
                .transparent_notes
                .iter_mut()
                .find(|u| u.txid == spent_txid && u.output_index == output_num as u64)
            {
                if unconfirmed {
                    spent_utxo.unconfirmed_spent = Some((source_txid, source_height));
                } else {
                    // Mark this one as spent
                    spent_utxo.spent = Some(source_txid);
                    spent_utxo.spent_at_height = Some(source_height as i32);
                    spent_utxo.unconfirmed_spent = None;
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
        height: u32,
        unconfirmed: bool,
        timestamp: u64,
        vout: &TxOut,
        output_num: u32,
    ) {
        let blockheight = BlockHeight::from(height);
        let status =
            ConfirmationStatus::from_blockheight_and_unconfirmed_bool(blockheight, unconfirmed);
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
            transaction_metadata
                .transparent_notes
                .push(TransparentNote {
                    address: taddr,
                    txid,
                    output_index: output_num as u64,
                    script: vout.script_pubkey.0.clone(),
                    value: u64::try_from(vout.value).expect("Valid value for u64."),
                    spent_at_height: None,
                    spent: None,
                    unconfirmed_spent: None,
                });
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
        let status = ConfirmationStatus::Broadcast(Some(height));
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
                    output_index as u32,
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
        height: BlockHeight,
        unconfirmed: bool,
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
        let status = ConfirmationStatus::from_blockheight_and_unconfirmed_bool(height, unconfirmed);
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
            output_index,
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
            Some(n) => {
                // An overwrite should be safe here: TODO: test that confirms this
                *n = nd;
            }
        }
    }

    // get data

    pub fn get_notes_for_updating(&self, before_block: u64) -> Vec<(TxId, PoolNullifier, u32)> {
        let before_block = BlockHeight::from_u32(before_block as u32);

        self.current
            .iter()
            .filter(|(_, transaction_metadata)| transaction_metadata.status.is_confirmed()) // Update only confirmed notes
            .flat_map(|(txid, transaction_metadata)| {
                // Fetch notes that are before the before_block.
                transaction_metadata
                    .sapling_notes
                    .iter()
                    .filter_map(move |sapling_note_description| {
                        if transaction_metadata.status.is_confirmed_before_or_at(&before_block)
                            && sapling_note_description.have_spending_key
                            && sapling_note_description.spent.is_none()
                        {
                            Some((
                                *txid,
                                PoolNullifier::Sapling(
                                    sapling_note_description.nullifier.unwrap_or_else(|| {
                                        todo!("Do something about note even with missing nullifier")
                                    }),
                                ),
                                sapling_note_description.output_index
                            ))
                        } else {
                            None
                        }
                    })
                    .chain(transaction_metadata.orchard_notes.iter().filter_map(
                        move |orchard_note_description| {
                            if transaction_metadata.status.is_confirmed_before_or_at(&before_block)
                                && orchard_note_description.have_spending_key
                                && orchard_note_description.spent.is_none()
                            {
                                Some((
                                    *txid,
                                    PoolNullifier::Orchard(orchard_note_description.nullifier.unwrap_or_else(|| {
                                        todo!("Do something about note even with missing nullifier")
                                    }))
                                    , orchard_note_description.output_index
,                                ))
                            } else {
                                None
                            }
                        },
                    ))
            })
            .collect()
    }

    pub fn total_funds_spent_in(&self, txid: &TxId) -> u64 {
        self.current
            .get(txid)
            .map(TransactionMetadata::total_value_spent)
            .unwrap_or(0)
    }

    pub fn get_nullifier_value_txid_outputindex_of_unspent_notes<D: DomainWalletExt>(
        &self,
    ) -> Vec<(
        <<D as DomainWalletExt>::WalletNote as ShieldedNoteInterface>::Nullifier,
        u64,
        TxId,
        u32,
    )>
    where
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: traits::Recipient,
    {
        self.current
            .iter()
            .flat_map(|(_, transaction_metadata)| {
                D::to_notes_vec(transaction_metadata)
                    .iter()
                    .filter(|unspent_note_data| unspent_note_data.spent().is_none())
                    .filter_map(move |unspent_note_data| {
                        unspent_note_data.nullifier().map(|unspent_nullifier| {
                            (
                                unspent_nullifier,
                                unspent_note_data.value(),
                                transaction_metadata.txid,
                                *unspent_note_data.output_index(),
                            )
                        })
                    })
            })
            .collect()
    }

    // shardtree

    /// A mark designates a leaf as non-ephemeral, mark removal causes
    /// the leaf to eventually transition to the ephemeral state
    pub fn remove_witness_mark<D>(
        &mut self,
        height: BlockHeight,
        txid: TxId,
        source_txid: TxId,
        output_index: u32,
    ) where
        D: DomainWalletExt,
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: traits::Recipient,
    {
        let transaction_metadata = self
            .current
            .get_mut(&source_txid)
            .expect("Txid should be present");

        if let Some(note_datum) = D::to_notes_vec_mut(transaction_metadata)
            .iter_mut()
            .find(|n| *n.output_index() == output_index)
        {
            *note_datum.spent_mut() = Some((txid, height.into()));
            if let Some(position) = *note_datum.witnessed_position() {
                if let Some(ref mut tree) = D::transaction_metadata_set_to_shardtree_mut(self) {
                    tree.remove_mark(position, Some(&(height - BlockHeight::from(1))))
                        .unwrap();
                }
            } else {
                todo!("Tried to mark note as spent with no position: FIX")
            }
        } else {
            eprintln!("Could not remove node!")
        }
    }

    pub(crate) fn mark_note_position<D: DomainWalletExt>(
        &mut self,
        txid: TxId,
        output_index: u32,
        position: Position,
        fvk: &D::Fvk,
    ) where
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: Recipient,
    {
        if let Some(tmd) = self.current.get_mut(&txid) {
            if let Some(nnmd) = &mut D::to_notes_vec_mut(tmd)
                .iter_mut()
                .find(|nnmd| *nnmd.output_index() == output_index)
            {
                *nnmd.witnessed_position_mut() = Some(position);
                *nnmd.nullifier_mut() = Some(D::get_nullifier_from_note_fvk_and_witness_position(
                    &nnmd.note().clone(),
                    fvk,
                    u64::from(position),
                ));
            } else {
                println!("Could not update witness position");
            }
        } else {
            println!("Could not update witness position");
        }
    }

    pub(crate) fn new_with_witness_trees() -> TransactionMetadataSet {
        Self {
            current: HashMap::default(),
            some_highest_txid: None,
            witness_trees: Some(WitnessTrees::default()),
        }
    }
    pub(crate) fn new_treeless() -> TransactionMetadataSet {
        Self {
            current: HashMap::default(),
            some_highest_txid: None,
            witness_trees: None,
        }
    }

    // misc

    // Update the memo for a note if it already exists. If the note doesn't exist, then nothing happens.
    pub(crate) fn add_memo_to_note_metadata<Nd: ShieldedNoteInterface>(
        &mut self,
        txid: &TxId,
        note: Nd::Note,
        memo: Memo,
    ) {
        if let Some(transaction_metadata) = self.current.get_mut(txid) {
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
        if let Some(transaction_metadata) = self.current.get_mut(txid) {
            transaction_metadata.outgoing_tx_data = outgoing_metadata
        } else {
            error!(
                "TxId {} should be present while adding metadata, but wasn't",
                txid
            );
        }
    }

    /// This returns an _arbitrary_ confirmed txid from the latest block the wallet is aware of.
    pub fn get_some_txid_from_highest_wallet_block(&self) -> &'_ Option<TxId> {
        &self.some_highest_txid
    }
    pub fn set_price(&mut self, txid: &TxId, price: Option<f64>) {
        price.map(|p| self.current.get_mut(txid).map(|tx| tx.price = Some(p)));
    }
}

#[cfg(feature = "lightclient-deprecated")]
impl TransactionMetadataSet {
    pub fn get_fee_by_txid(&self, txid: &TxId) -> u64 {
        match self
            .current
            .get(txid)
            .expect("To have the requested txid")
            .get_transaction_fee()
        {
            Ok(tx_fee) => tx_fee,
            Err(e) => panic!("{:?} for txid {}", e, txid,),
        }
    }
}
