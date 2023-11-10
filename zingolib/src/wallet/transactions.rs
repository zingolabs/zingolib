use std::{
    collections::HashMap,
    io::{self, Read, Write},
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use incrementalmerkletree::Position;
use log::error;
use orchard;
use orchard::note_encryption::OrchardDomain;
use zcash_encoding::{Optional, Vector};
use zcash_note_encryption::Domain;
use zcash_primitives::{
    consensus::BlockHeight,
    memo::Memo,
    sapling::note_encryption::SaplingDomain,
    transaction::{components::TxOut, TxId},
};

use zingoconfig::{ChainType, MAX_REORG};

use crate::error::ZingoLibError;

use super::{
    confirmation_status::{self, ConfirmationStatus, SpendConfirmationStatus},
    data::{OutgoingTxData, PoolNullifier, TransactionMetadata, TransparentNote, WitnessTrees},
    keys::unified::WalletCapability,
    traits::{self, DomainWalletExt, NoteInterface, Nullifier, Recipient, ShieldedNoteInterface},
};

/// HashMap of all transactions in a wallet, keyed by txid.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TransactionMetadataSet {
    pub current: HashMap<TxId, TransactionMetadata>,
    pub(crate) some_txid_from_highest_wallet_block: Option<TxId>,
    pub witness_trees: Option<WitnessTrees>,
}

impl TransactionMetadataSet {
    pub fn serialized_version() -> u64 {
        22
    }

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

    pub fn read_old<R: Read>(
        mut reader: R,
        wallet_capability: &WalletCapability,
    ) -> io::Result<Self> {
        // Note, witness_trees will be Some(x) if the wallet has spend capability
        // so this check is a very un-ergonomic way of checking if the wallet
        // can spend.
        let mut witness_trees = wallet_capability.get_trees_witness_trees();
        let mut old_inc_witnesses = if witness_trees.is_some() {
            Some((Vec::new(), Vec::new()))
        } else {
            None
        };
        let txs = Vector::read_collected_mut(&mut reader, |r| {
            let mut txid_bytes = [0u8; 32];
            r.read_exact(&mut txid_bytes)?;

            Ok((
                TxId::from_bytes(txid_bytes),
                TransactionMetadata::read(r, (wallet_capability, old_inc_witnesses.as_mut()))
                    .unwrap(),
            ))
        })?;

        if let Some((mut old_sap_wits, mut old_orch_wits)) = old_inc_witnesses {
            old_sap_wits.sort_by(|(_w1, height1), (_w2, height2)| height1.cmp(height2));
            let sap_tree = &mut witness_trees.as_mut().unwrap().witness_tree_sapling;
            for (sap_wit, height) in old_sap_wits {
                sap_tree
                    .insert_witness_nodes(sap_wit, height - 1)
                    .expect("infallible");
                sap_tree.checkpoint(height).expect("infallible");
            }
            old_orch_wits.sort_by(|(_w1, height1), (_w2, height2)| height1.cmp(height2));
            let orch_tree = &mut witness_trees.as_mut().unwrap().witness_tree_orchard;
            for (orch_wit, height) in old_orch_wits {
                orch_tree
                    .insert_witness_nodes(orch_wit, height - 1)
                    .expect("infallible");
                orch_tree.checkpoint(height).expect("infallible");
            }
        }

        Ok(Self {
            current: txs,
            some_txid_from_highest_wallet_block: None,
            witness_trees,
        })
    }

    pub fn read<R: Read>(mut reader: R, wallet_capability: &WalletCapability) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;
        if version > Self::serialized_version() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Can't read wallettxns because of incorrect version",
            ));
        }

        let mut witness_trees = wallet_capability.get_trees_witness_trees();
        let mut old_inc_witnesses = if witness_trees.is_some() {
            Some((Vec::new(), Vec::new()))
        } else {
            None
        };
        let current: HashMap<_, _> = Vector::read_collected_mut(&mut reader, |r| {
            let mut txid_bytes = [0u8; 32];
            r.read_exact(&mut txid_bytes)?;

            Ok((
                TxId::from_bytes(txid_bytes),
                TransactionMetadata::read(r, (wallet_capability, old_inc_witnesses.as_mut()))?,
            ))
        })?;

        let some_txid_from_highest_wallet_block = current
            .values()
            .fold(None, |c: Option<(TxId, BlockHeight)>, w| {
                if c.is_none() || w.block_height > c.unwrap().1 {
                    Some((w.txid, w.block_height))
                } else {
                    c
                }
            })
            .map(|v| v.0);

        let _mempool: Vec<(TxId, TransactionMetadata)> = if version <= 20 {
            Vector::read_collected_mut(&mut reader, |r| {
                let mut txid_bytes = [0u8; 32];
                r.read_exact(&mut txid_bytes)?;
                let transaction_metadata =
                    TransactionMetadata::read(r, (wallet_capability, old_inc_witnesses.as_mut()))?;

                Ok((TxId::from_bytes(txid_bytes), transaction_metadata))
            })?
        } else {
            vec![]
        };

        if version >= 22 {
            witness_trees = Optional::read(reader, |r| WitnessTrees::read(r))?;
        } else if let Some((mut old_sap_wits, mut old_orch_wits)) = old_inc_witnesses {
            old_sap_wits.sort_by(|(_w1, height1), (_w2, height2)| height1.cmp(height2));
            let sap_tree = &mut witness_trees.as_mut().unwrap().witness_tree_sapling;
            for (sap_wit, height) in old_sap_wits {
                sap_tree
                    .insert_witness_nodes(sap_wit, height - 1)
                    .expect("infallible");
                sap_tree.checkpoint(height).expect("infallible");
            }
            old_orch_wits.sort_by(|(_w1, height1), (_w2, height2)| height1.cmp(height2));
            let orch_tree = &mut witness_trees.as_mut().unwrap().witness_tree_orchard;
            for (orch_wit, height) in old_orch_wits {
                orch_tree
                    .insert_witness_nodes(orch_wit, height - 1)
                    .expect("infallible");
                orch_tree.checkpoint(height).expect("infallible");
            }
        };

        Ok(Self {
            current,
            some_txid_from_highest_wallet_block,
            witness_trees,
        })
    }

    pub async fn write<W: Write>(&mut self, mut writer: W) -> io::Result<()> {
        // Write the version
        writer.write_u64::<LittleEndian>(Self::serialized_version())?;

        // The hashmap, write as a set of tuples. Store them sorted so that wallets are
        // deterministically saved
        {
            let mut transaction_metadatas = self
                .current
                .iter()
                .collect::<Vec<(&TxId, &TransactionMetadata)>>();
            // Don't write down metadata for transactions in the mempool, we'll rediscover
            // it on reload
            transaction_metadatas.retain(|metadata| metadata.1.confirmation_status.is_confirmed());
            transaction_metadatas.sort_by(|a, b| a.0.partial_cmp(b.0).unwrap());

            Vector::write(&mut writer, &transaction_metadatas, |w, (k, v)| {
                w.write_all(k.as_ref())?;
                v.write(w)
            })?;
        }

        Optional::write(writer, self.witness_trees.as_mut(), |w, t| t.write(w))
    }

    pub fn clear(&mut self) {
        self.current.clear();
        self.witness_trees.as_mut().map(WitnessTrees::clear);
    }

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
                    utxo.spend_status().erase_spent_in_txids(&txids_to_remove);
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
                .for_each(|note_datum| {
                    note_datum
                        .spend_status()
                        .erase_spent_in_txids(&txids_to_remove);
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
                    .confirmation_status
                    .is_confirmed_after_or_at(&reorg_height)
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

    /// This returns an _arbitrary_ txid from the latest block the wallet is aware of.
    pub fn get_some_txid_from_highest_wallet_block(&self) -> &'_ Option<TxId> {
        &self.some_txid_from_highest_wallet_block
    }

    // tODO why do we need to update notes post shardtree?
    pub fn get_notes_for_updating(&self, before_block: u64) -> Vec<(TxId, PoolNullifier, u32)> {
        let before_block = BlockHeight::from_u32(before_block as u32);

        self.current
            .iter()
            .filter(|(_, transaction_metadata)| transaction_metadata.confirmation_status.is_confirmed()) // Update only confirmed notes
            .flat_map(|(txid, transaction_metadata)| {
                // Fetch notes that are before the before_block.
                transaction_metadata
                    .sapling_notes
                    .iter()
                    .filter_map(move |sapling_note_description| {
                        if transaction_metadata.confirmation_status.is_confirmed_before_or_at(&before_block)
                            && sapling_note_description.have_spending_key
                            && sapling_note_description.spend_status.is_unspent()
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
                            if transaction_metadata.confirmation_status.is_confirmed_before_or_at(&before_block)
                                && orchard_note_description.have_spending_key
                                && orchard_note_description.spend_status.is_unspent()
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
                    .filter(|unspent_note_data| unspent_note_data.spend_status().is_unspent())
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

    pub(crate) fn clear_expired_mempool(&mut self, latest_height: u64) {
        let cutoff = BlockHeight::from_u32((latest_height.saturating_sub(MAX_REORG as u64)) as u32);

        let txids_to_remove = self
            .current
            .iter()
            .filter(|(_, transaction_metadata)| {
                transaction_metadata.confirmation_status.is_expired(&cutoff)
            })
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

    fn get_or_create_transaction_metadata(
        &mut self,
        txid: &TxId,
        confirmation_status: ConfirmationStatus,
        datetime: u64,
    ) -> &'_ mut TransactionMetadata {
        self.current
            .entry(*txid)
            // If we already have the transaction metadata, it may be newly confirmed. Update confirmation_status
            .and_modify(|transaction_metadata| {
                transaction_metadata.confirmation_status = confirmation_status;
                transaction_metadata.datetime = datetime;
            })
            // if this transaction is new to our data, insert it
            .or_insert_with(|| {
                self.some_txid_from_highest_wallet_block = Some(*txid); // TOdO IS this the highest wallet block?
                TransactionMetadata::new(confirmation_status, datetime, txid)
            })
    }

    pub fn set_price(&mut self, txid: &TxId, price: Option<f64>) {
        price.map(|p| self.current.get_mut(txid).map(|tx| tx.price = Some(p)));
    }

    // Will mark a note as having been spent at the supplied height and spent_txid.
    // Takes the nullifier of the spent note, the note's index in its containing transaction,
    // as well as the txid of its containing transaction. TODO: Only one of
    // `nullifier` and `(output_index, txid)` is needed, although we use the nullifier to
    // determine the domain.
    // toDO make domain-generic
    pub fn update_spend_status(
        &mut self,
        spent_nullifier: &PoolNullifier,
        spent_txid: TxId,
        output_index: u32,
        spend_status: SpendConfirmationStatus,
    ) -> Result<u64, String> {
        match spent_nullifier {
            PoolNullifier::Sapling(sapling_nullifier) => {
                if let Some(sapling_note_data) = self
                    .current
                    .get_mut(&spent_txid)
                    .expect("TXid should be a key in current.")
                    .sapling_notes
                    .iter_mut()
                    .find(|n| n.output_index == output_index)
                {
                    sapling_note_data.spend_status = spend_status;
                    Ok(sapling_note_data.note.value().inner())
                } else {
                    Err(format!(
                        "no such sapling nullifier '{:?}' found in transaction",
                        *sapling_nullifier
                    ))
                }
            }
            PoolNullifier::Orchard(orchard_nullifier) => {
                if let Some(orchard_note_data) = self
                    .current
                    .get_mut(&spent_txid)
                    .unwrap()
                    .orchard_notes
                    .iter_mut()
                    .find(|n| n.output_index == output_index)
                {
                    orchard_note_data.spend_status = spend_status;
                    Ok(orchard_note_data.note.value().inner())
                } else {
                    Err(format!(
                        "no such orchard nullifier '{:?}' found in transaction",
                        *orchard_nullifier
                    ))
                }
            }
        }
    }

    // Records a TxId as having spent some nullifiers from the wallet.
    #[allow(clippy::too_many_arguments)]
    pub async fn process_spend(
        &mut self,
        spending_txid: TxId,
        confirmation_status: ConfirmationStatus,
        timestamp: u32,
        spent_nullifier: PoolNullifier,
        value: u64,
        spent_txid: TxId,
        output_index: u32,
    ) {
        // Record this Tx as having spent some funds
        let spending_transaction_metadata = self.get_or_create_transaction_metadata(
            &spending_txid,
            confirmation_status,
            timestamp as u64,
        );

        // we may be able to add_spent_nullifier or we may not. in order to tell, we may have to check whether the nullifier exists somewhere. ToDo consider the case where we should not add_spent_nullifier.
        // ToDO what if the transaction never confirms? how do we unmark this nullifier? if we can figure it out, this
        match spent_nullifier {
            PoolNullifier::Orchard(spent_orchard_nullifier) => spending_transaction_metadata
                .add_unique_spent_nullifier(spent_orchard_nullifier.into(), value),
            PoolNullifier::Sapling(spent_sapling_nullifier) => spending_transaction_metadata
                .add_unique_spent_nullifier(spent_sapling_nullifier.into(), value),
        }

        // Since this Txid has spent some funds, output notes in this Tx that are sent to us are actually change.
        self.check_notes_mark_change(&spending_txid);

        //todo use enum's associated fn
        match confirmation_status {
            ConfirmationStatus::Local => {}
            ConfirmationStatus::InMempool => {
                self.update_spend_status(
                    &spent_nullifier,
                    spent_txid,
                    output_index,
                    SpendConfirmationStatus::PendingSpend(spending_txid),
                );
            }
            ConfirmationStatus::ConfirmedOnChain(confirmation_height) => {
                self.update_spend_status(
                    &spent_nullifier,
                    spent_txid,
                    output_index,
                    SpendConfirmationStatus::ConfirmedSpent(spending_txid, confirmation_height),
                );

                // ie remove_witness_mark_sapling or _orchard
                match spent_nullifier {
                    PoolNullifier::Orchard(_) => {
                        self.remove_witness_mark::<OrchardDomain>(
                            confirmation_height,
                            spending_txid,
                            spent_txid,
                            output_index,
                        );
                    }
                    PoolNullifier::Sapling(_) => {
                        self.remove_witness_mark::<SaplingDomain<ChainType>>(
                            confirmation_height,
                            spending_txid,
                            spent_txid,
                            output_index,
                        );
                    }
                }
            }
        }
    }

    pub fn add_taddr_spent(
        &mut self,
        txid: TxId,
        confirmation_status: ConfirmationStatus,
        timestamp: u64,
        total_transparent_value_spent: u64,
    ) {
        let transaction_metadata =
            self.get_or_create_transaction_metadata(&txid, confirmation_status, timestamp);
        transaction_metadata.total_transparent_value_spent = total_transparent_value_spent;

        self.check_notes_mark_change(&txid);
    }
    /// A mark designates a leaf as non-ephemeral, mark removal causes
    /// the leaf to eventually transition to the ephemeral state
    pub fn remove_witness_mark<D>(
        &mut self,
        height: BlockHeight,
        spending_txid: TxId,
        spent_txid: TxId,
        output_index: u32,
    ) where
        D: DomainWalletExt,
        <D as Domain>::Note: PartialEq + Clone,
        <D as Domain>::Recipient: traits::Recipient,
    {
        let transaction_metadata = self
            .current
            .get_mut(&spent_txid)
            .expect("Txid should be present");

        if let Some(note_datum) = D::to_notes_vec_mut(transaction_metadata)
            .iter_mut()
            .find(|n| *n.output_index() == output_index)
        {
            // *note_datum.spent_mut() = Some((spending_txid, height.into())); //this line deleted because this should be done elsewhere.
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

    pub fn update_transparent_spend_status(
        &mut self,
        spending_txid: TxId,
        output_num: u32,
        spent_txid: TxId,
        confirmation_status: ConfirmationStatus,
    ) -> Result<u64, ZingoLibError> {
        // Find the UTXO
        if let Some(utxo_transacion_metadata) = self.current.get_mut(&spending_txid) {
            if let Some(spent_utxo) = utxo_transacion_metadata
                .transparent_notes
                .iter_mut()
                .find(|u| u.txid == spending_txid && u.output_index == output_num as u64)
            {
                spent_utxo.spend_status = SpendConfirmationStatus::from_txid_and_confirmation(
                    spending_txid,
                    confirmation_status,
                );
                // Return the value of the note that was spent.
                Ok(spent_utxo.value)
            } else {
                error!("Couldn't find UTXO that was spent");
                Err(ZingoLibError::NoSuchNoteInTransaction)
            }
        } else {
            error!("Couldn't find TxID that was spent!");
            Err(ZingoLibError::NoSuchTransaction)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_new_taddr_output(
        &mut self,
        txid: TxId,
        taddr: String,
        confirmation_status: ConfirmationStatus,
        timestamp: u64,
        vout: &TxOut,
        output_num: u32,
    ) {
        // Read or create the current TxId
        let transaction_metadata =
            self.get_or_create_transaction_metadata(&txid, confirmation_status, timestamp);

        // have we recorded this UTXO?
        if let Some(utxo) = transaction_metadata
            .transparent_notes
            .iter_mut()
            .find(|utxo| utxo.txid == txid && utxo.output_index == output_num as u64)
        {
            // If it already exists, it is likely an mempool tx, so update the height
        } else {
            // Add this UTXO if it doesn't already exist
            transaction_metadata
                .transparent_notes
                .push(TransparentNote {
                    address: taddr,
                    txid,
                    output_index: output_num as u64,
                    script: vout.script_pubkey.0.clone(),
                    value: u64::try_from(vout.value).expect("Valid value for u64."),
                    spend_status: SpendConfirmationStatus::NoKnownSpends,
                });
        }
    }

    pub(crate) fn add_pending_note<D>(
        &mut self,
        txid: TxId,
        confirmation_status: ConfirmationStatus,
        timestamp: u64,
        note: D::Note,
        recipient: D::Recipient,
        output_index: usize,
    ) where
        D: DomainWalletExt,
        D::Note: PartialEq + Clone,
        D::Recipient: Recipient,
    {
        let transaction_metadata =
            self.get_or_create_transaction_metadata(&txid, confirmation_status, timestamp);

        match D::to_notes_vec_mut(transaction_metadata)
            .iter_mut()
            .find(|n| n.note() == &note)
        {
            None => {
                let nd = D::WalletNote::from_parts(
                    recipient.diversifier(),
                    note,
                    None,
                    None,
                    SpendConfirmationStatus::NoKnownSpends,
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
        confirmation_status: ConfirmationStatus,
        timestamp: u64,
        note: <D::WalletNote as ShieldedNoteInterface>::Note,
        recipient: D::Recipient,
        have_spending_key: bool,
        nullifier: Option<<D::WalletNote as ShieldedNoteInterface>::Nullifier>,
        output_index: u32,
        position: Position,
    ) where
        D::Note: PartialEq + Clone,
        D::Recipient: Recipient,
    {
        let transaction_metadata =
            self.get_or_create_transaction_metadata(&txid, confirmation_status, timestamp);

        // TODO review this
        let nd = D::WalletNote::from_parts(
            D::Recipient::diversifier(&recipient),
            note.clone(),
            Some(position),
            nullifier,
            SpendConfirmationStatus::NoKnownSpends,
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

    pub(crate) fn new_with_witness_trees() -> TransactionMetadataSet {
        Self {
            current: HashMap::default(),
            some_txid_from_highest_wallet_block: None,
            witness_trees: Some(WitnessTrees::default()),
        }
    }
    pub(crate) fn new_treeless() -> TransactionMetadataSet {
        Self {
            current: HashMap::default(),
            some_txid_from_highest_wallet_block: None,
            witness_trees: None,
        }
    }
}
