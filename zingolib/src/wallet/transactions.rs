use std::{
    collections::HashMap,
    io::{self, Read, Write},
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use log::error;
use orchard::{
    keys::FullViewingKey as OrchardFullViewingKey,
    note::{Note as OrchardNote, Nullifier as OrchardNullifier},
    note_encryption::OrchardDomain,
    tree::MerkleHashOrchard,
};
use zcash_encoding::Vector;
use zcash_note_encryption::Domain;
use zcash_primitives::{
    consensus::BlockHeight,
    memo::Memo,
    merkle_tree::IncrementalWitness,
    sapling::{
        keys::DiversifiableFullViewingKey as SaplingFvk, note_encryption::SaplingDomain,
        Node as SaplingNode, Note as SaplingNote, Nullifier as SaplingNullifier, PaymentAddress,
    },
    transaction::{components::TxOut, TxId},
};

use zingoconfig::{ChainType, MAX_REORG};

use super::{
    data::{
        OutgoingTxMetadata, PoolNullifier, ReceivedOrchardNoteAndMetadata,
        ReceivedSaplingNoteAndMetadata, TransactionMetadata, Utxo, WitnessCache,
    },
    traits::{DomainWalletExt, FromBytes, Nullifier, ReceivedNoteAndMetadata, Recipient},
};

/// List of all transactions in a wallet.
/// Note that the parent is expected to hold a RwLock, so we will assume that all accesses to
/// this struct are threadsafe/locked properly.
pub struct TransactionMetadataSet {
    pub(crate) current: HashMap<TxId, TransactionMetadata>,
    pub(crate) some_txid_from_highest_wallet_block: Option<TxId>,
}

impl TransactionMetadataSet {
    pub fn serialized_version() -> u64 {
        return 21;
    }

    pub fn new() -> Self {
        Self {
            current: HashMap::new(),
            some_txid_from_highest_wallet_block: None,
        }
    }

    pub fn read_old<R: Read>(mut reader: R) -> io::Result<Self> {
        let txs_tuples = Vector::read(&mut reader, |r| {
            let mut txid_bytes = [0u8; 32];
            r.read_exact(&mut txid_bytes)?;

            Ok((
                TxId::from_bytes(txid_bytes),
                TransactionMetadata::read(r).unwrap(),
            ))
        })?;

        let txs = txs_tuples
            .into_iter()
            .collect::<HashMap<TxId, TransactionMetadata>>();

        Ok(Self {
            current: txs,
            some_txid_from_highest_wallet_block: None,
        })
    }

    pub fn read<R: Read>(mut reader: R) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;
        if version > Self::serialized_version() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Can't read wallettxns because of incorrect version",
            ));
        }

        let txs_tuples = Vector::read(&mut reader, |r| {
            let mut txid_bytes = [0u8; 32];
            r.read_exact(&mut txid_bytes)?;

            Ok((
                TxId::from_bytes(txid_bytes),
                TransactionMetadata::read(r).unwrap(),
            ))
        })?;

        let current = txs_tuples
            .into_iter()
            .collect::<HashMap<TxId, TransactionMetadata>>();
        let some_txid_from_highest_wallet_block = current
            .values()
            .fold(None, |c: Option<(TxId, BlockHeight)>, w| {
                if c.is_none() || w.block_height > c.unwrap().1 {
                    Some((w.txid.clone(), w.block_height))
                } else {
                    c
                }
            })
            .map(|v| v.0);

        let _mempool = if version <= 20 {
            Vector::read(&mut reader, |r| {
                let mut txid_bytes = [0u8; 32];
                r.read_exact(&mut txid_bytes)?;
                let transaction_metadata = TransactionMetadata::read(r)?;

                Ok((TxId::from_bytes(txid_bytes), transaction_metadata))
            })?
            .into_iter()
            .collect()
        } else {
            vec![]
        };

        Ok(Self {
            current,
            some_txid_from_highest_wallet_block,
        })
    }

    pub fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
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
            transaction_metadatas.retain(|metadata| !metadata.1.unconfirmed);
            transaction_metadatas.sort_by(|a, b| a.0.partial_cmp(b.0).unwrap());

            Vector::write(&mut writer, &transaction_metadatas, |w, (k, v)| {
                w.write_all(k.as_ref())?;
                v.write(w)
            })?;
        }

        Ok(())
    }

    pub fn clear(&mut self) {
        self.current.clear();
    }

    pub fn remove_txids(&mut self, txids_to_remove: Vec<TxId>) {
        for txid in &txids_to_remove {
            self.current.remove(&txid);
        }
        self.current.values_mut().for_each(|transaction_metadata| {
            // Update UTXOs to rollback any spent utxos
            transaction_metadata.utxos.iter_mut().for_each(|utxo| {
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

    fn remove_domain_specific_txids<D: DomainWalletExt>(&mut self, txids_to_remove: &Vec<TxId>)
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
                    if nd.unconfirmed_spent().is_some()
                        && txids_to_remove.contains(&nd.unconfirmed_spent().unwrap().0)
                    {
                        *nd.unconfirmed_spent_mut() = None;
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
                if transaction_metadata.block_height >= reorg_height {
                    Some(transaction_metadata.txid.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        self.remove_txids(txids_to_remove);

        // Of the notes that still remain, unroll the witness.
        // Trim all witnesses for the invalidated blocks
        for tx in self.current.values_mut() {
            // We only want to trim the witness for "existing" notes, i.e., notes that were created before the block that is being removed
            if tx.block_height < reorg_height {
                for nd in tx.sapling_notes.iter_mut() {
                    // The latest witness is at the last() position, so just pop() it.
                    // We should be checking if there is a witness at all, but if there is none, it is an
                    // empty vector, for which pop() is a no-op.
                    let _discard = nd.witnesses.pop(u64::from(reorg_height));
                }
                for nd in tx.orchard_notes.iter_mut() {
                    let _discard = nd.witnesses.pop(u64::from(reorg_height));
                }
            }
        }
    }

    /// This returns an _arbitrary_ txid from the latest block the wallet is aware of.
    pub fn get_some_txid_from_highest_wallet_block(&self) -> &'_ Option<TxId> {
        &self.some_txid_from_highest_wallet_block
    }

    pub fn get_notes_for_updating(&self, before_block: u64) -> Vec<(TxId, PoolNullifier)> {
        let before_block = BlockHeight::from_u32(before_block as u32);

        self.current
            .iter()
            .filter(|(_, transaction_metadata)| !transaction_metadata.unconfirmed) // Update only confirmed notes
            .flat_map(|(txid, transaction_metadata)| {
                // Fetch notes that are before the before_block.
                transaction_metadata
                    .sapling_notes
                    .iter()
                    .filter_map(move |sapling_note_description| {
                        if transaction_metadata.block_height <= before_block
                            && sapling_note_description.have_spending_key
                            && sapling_note_description.witnesses.len() > 0
                            && sapling_note_description.spent.is_none()
                        {
                            Some((
                                txid.clone(),
                                PoolNullifier::Sapling(sapling_note_description.nullifier.clone()),
                            ))
                        } else {
                            None
                        }
                    })
                    .chain(transaction_metadata.orchard_notes.iter().filter_map(
                        move |orchard_note_description| {
                            if transaction_metadata.block_height <= before_block
                                && orchard_note_description.have_spending_key
                                && orchard_note_description.witnesses.len() > 0
                                && orchard_note_description.spent.is_none()
                            {
                                Some((
                                    txid.clone(),
                                    PoolNullifier::Orchard(
                                        orchard_note_description.nullifier.clone(),
                                    ),
                                ))
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
            .get(&txid)
            .map(TransactionMetadata::total_value_spent)
            .unwrap_or(0)
    }

    pub fn get_nullifiers_of_unspent_sapling_notes(&self) -> Vec<(SaplingNullifier, u64, TxId)> {
        self.current
            .iter()
            .flat_map(|(_, transaction_metadata)| {
                transaction_metadata
                    .sapling_notes
                    .iter()
                    .filter(|nd| nd.spent.is_none())
                    .map(move |nd| {
                        (
                            nd.nullifier.clone(),
                            nd.note.value,
                            transaction_metadata.txid.clone(),
                        )
                    })
            })
            .collect()
    }

    pub fn get_nullifiers_of_unspent_orchard_notes(&self) -> Vec<(OrchardNullifier, u64, TxId)> {
        self.current
            .iter()
            .flat_map(|(_, transaction_metadata)| {
                transaction_metadata
                    .orchard_notes
                    .iter()
                    .filter(|nd| nd.spent.is_none())
                    .map(move |nd| {
                        (
                            nd.nullifier.clone(),
                            nd.note.value().inner(),
                            transaction_metadata.txid.clone(),
                        )
                    })
            })
            .collect()
    }

    pub(crate) fn get_sapling_note_witnesses(
        &self,
        txid: &TxId,
        nullifier: &SaplingNullifier,
    ) -> Option<(WitnessCache<SaplingNode>, BlockHeight)> {
        self.current.get(txid).map(|transaction_metadata| {
            transaction_metadata
                .sapling_notes
                .iter()
                .find(|nd| nd.nullifier == *nullifier)
                .map(|nd| (nd.witnesses.clone(), transaction_metadata.block_height))
        })?
    }

    pub(crate) fn get_orchard_note_witnesses(
        &self,
        txid: &TxId,
        nullifier: &OrchardNullifier,
    ) -> Option<(WitnessCache<MerkleHashOrchard>, BlockHeight)> {
        self.current.get(txid).map(|transaction_metadata| {
            transaction_metadata
                .orchard_notes
                .iter()
                .find(|nd| nd.nullifier == *nullifier)
                .map(|nd| (nd.witnesses.clone(), transaction_metadata.block_height))
        })?
    }

    pub(crate) fn set_sapling_note_witnesses(
        &mut self,
        txid: &TxId,
        nullifier: &SaplingNullifier,
        witnesses: WitnessCache<SaplingNode>,
    ) {
        self.current
            .get_mut(txid)
            .unwrap()
            .sapling_notes
            .iter_mut()
            .find(|nd| nd.nullifier == *nullifier)
            .unwrap()
            .witnesses = witnesses;
    }

    pub(crate) fn set_orchard_note_witnesses(
        &mut self,
        txid: &TxId,
        nullifier: &OrchardNullifier,
        witnesses: WitnessCache<MerkleHashOrchard>,
    ) {
        self.current
            .get_mut(txid)
            .unwrap()
            .orchard_notes
            .iter_mut()
            .find(|nd| nd.nullifier == *nullifier)
            .unwrap()
            .witnesses = witnesses;
    }

    pub(crate) fn clear_old_witnesses(&mut self, latest_height: u64) {
        self.clear_old_domain_specific_witnesses::<OrchardDomain>(latest_height);
        self.clear_old_domain_specific_witnesses::<SaplingDomain<zingoconfig::ChainType>>(
            latest_height,
        );
    }

    fn clear_old_domain_specific_witnesses<D: DomainWalletExt>(&mut self, latest_height: u64)
    where
        <D as Domain>::Note: Clone + PartialEq,
        <D as Domain>::Recipient: Recipient,
    {
        let cutoff = (latest_height.saturating_sub(MAX_REORG as u64)) as u32;

        self.current
            .iter_mut()
            .for_each(|(_, transaction_metadata)| {
                D::to_notes_vec_mut(transaction_metadata)
                    .iter_mut()
                    .filter(|note_data| {
                        !note_data.witnesses().is_empty()
                            && note_data.is_spent()
                            && note_data.spent().unwrap().1 < cutoff
                    })
                    .for_each(|note_data| note_data.witnesses_mut().clear());
            });
    }

    pub(crate) fn clear_expired_mempool(&mut self, latest_height: u64) {
        let cutoff = BlockHeight::from_u32((latest_height.saturating_sub(MAX_REORG as u64)) as u32);

        let txids_to_remove = self
            .current
            .iter()
            .filter(|(_, transaction_metadata)| {
                transaction_metadata.unconfirmed && transaction_metadata.block_height < cutoff
            })
            .map(|(_, transaction_metadata)| transaction_metadata.txid.clone())
            .collect::<Vec<_>>();

        txids_to_remove
            .iter()
            .for_each(|t| println!("Removing expired mempool tx {}", t));

        self.remove_txids(txids_to_remove);
    }

    // Will mark the nullifier of the given txid as spent. Returns the amount of the nullifier
    pub fn mark_txid_nf_spent(
        &mut self,
        txid: TxId,
        nullifier: &PoolNullifier,
        spent_txid: &TxId,
        spent_at_height: BlockHeight,
    ) -> u64 {
        match nullifier {
            PoolNullifier::Sapling(nf) => {
                let mut note_data = self
                    .current
                    .get_mut(&txid)
                    .unwrap()
                    .sapling_notes
                    .iter_mut()
                    .find(|n| n.nullifier == *nf)
                    .unwrap();
                note_data.spent = Some((spent_txid.clone(), spent_at_height.into()));
                note_data.unconfirmed_spent = None;
                note_data.note.value
            }
            PoolNullifier::Orchard(nf) => {
                let mut note_data = self
                    .current
                    .get_mut(&txid)
                    .unwrap()
                    .orchard_notes
                    .iter_mut()
                    .find(|n| n.nullifier == *nf)
                    .unwrap();
                note_data.spent = Some((spent_txid.clone(), spent_at_height.into()));
                note_data.unconfirmed_spent = None;
                note_data.note.value().inner()
            }
        }
    }

    // Check this transaction to see if it is an outgoing transaction, and if it is, mark all recieved notes in this
    // transction as change. i.e., If any funds were spent in this transaction, all recieved notes are change notes.
    pub fn check_notes_mark_change(&mut self, txid: &TxId) {
        if self.total_funds_spent_in(txid) > 0 {
            self.current.get_mut(txid).map(|transaction_metadata| {
                transaction_metadata.sapling_notes.iter_mut().for_each(|n| {
                    n.is_change = true;
                });
                transaction_metadata.orchard_notes.iter_mut().for_each(|n| {
                    n.is_change = true;
                })
            });
        }
    }

    fn get_or_create_transaction_metadata(
        &mut self,
        txid: &TxId,
        height: BlockHeight,
        unconfirmed: bool,
        datetime: u64,
    ) -> &'_ mut TransactionMetadata {
        if !self.current.contains_key(&txid) {
            self.current.insert(
                txid.clone(),
                TransactionMetadata::new(BlockHeight::from(height), datetime, &txid, unconfirmed),
            );
            self.some_txid_from_highest_wallet_block = Some(txid.clone());
        }
        let transaction_metadata = self.current.get_mut(&txid).expect("Txid should be present");

        // Make sure the unconfirmed status matches
        if transaction_metadata.unconfirmed != unconfirmed {
            transaction_metadata.unconfirmed = unconfirmed;
            transaction_metadata.block_height = height;
            transaction_metadata.datetime = datetime;
        }

        transaction_metadata
    }

    pub fn set_price(&mut self, txid: &TxId, price: Option<f64>) {
        price.map(|p| self.current.get_mut(txid).map(|tx| tx.zec_price = Some(p)));
    }

    // Records a TxId as having spent some nullifiers from the wallet.
    pub fn add_new_spent(
        &mut self,
        txid: TxId,
        height: BlockHeight,
        unconfirmed: bool,
        timestamp: u32,
        nullifier: PoolNullifier,
        value: u64,
        source_txid: TxId,
    ) {
        match nullifier {
            PoolNullifier::Orchard(nullifier) => self
                .add_new_spent_internal::<ReceivedOrchardNoteAndMetadata>(
                    txid,
                    height,
                    unconfirmed,
                    timestamp,
                    nullifier,
                    value,
                    source_txid,
                ),
            PoolNullifier::Sapling(nullifier) => self
                .add_new_spent_internal::<ReceivedSaplingNoteAndMetadata>(
                    txid,
                    height,
                    unconfirmed,
                    timestamp,
                    nullifier,
                    value,
                    source_txid,
                ),
        }
    }

    fn add_new_spent_internal<NnMd: ReceivedNoteAndMetadata>(
        &mut self,
        txid: TxId,
        height: BlockHeight,
        unconfirmed: bool,
        timestamp: u32,
        nullifier: NnMd::Nullifier,
        value: u64,
        source_txid: TxId,
    ) {
        // Record this Tx as having spent some funds
        let transaction_metadata = self.get_or_create_transaction_metadata(
            &txid,
            BlockHeight::from(height),
            unconfirmed,
            timestamp as u64,
        );

        // Mark the height correctly, in case this was previously a mempool or unconfirmed tx.
        transaction_metadata.block_height = height;
        if NnMd::Nullifier::get_nullifiers_spent_in_transaction(transaction_metadata)
            .iter()
            .find(|nf| **nf == nullifier)
            .is_none()
        {
            transaction_metadata.add_spent_nullifier(nullifier.into(), value)
        }

        // Since this Txid has spent some funds, output notes in this Tx that are sent to us are actually change.
        self.check_notes_mark_change(&txid);

        // Mark the source note as spent
        if !unconfirmed {
            let transaction_metadata = self
                .current
                .get_mut(&source_txid)
                .expect("Txid should be present");

            NnMd::transaction_metadata_notes_mut(transaction_metadata)
                .iter_mut()
                .find(|n| n.nullifier() == nullifier)
                .map(|nd| {
                    // Record the spent height
                    *nd.spent_mut() = Some((txid, height.into()));
                });
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
        let transaction_metadata = self.get_or_create_transaction_metadata(
            &txid,
            BlockHeight::from(height),
            unconfirmed,
            timestamp,
        );
        transaction_metadata.total_transparent_value_spent = total_transparent_value_spent;

        self.check_notes_mark_change(&txid);
    }

    pub fn mark_txid_utxo_spent(
        &mut self,
        spent_txid: TxId,
        output_num: u32,
        source_txid: TxId,
        source_height: u32,
    ) -> u64 {
        // Find the UTXO
        let value = if let Some(utxo_transacion_metadata) = self.current.get_mut(&spent_txid) {
            if let Some(spent_utxo) = utxo_transacion_metadata
                .utxos
                .iter_mut()
                .find(|u| u.txid == spent_txid && u.output_index == output_num as u64)
            {
                // Mark this one as spent
                spent_utxo.spent = Some(source_txid.clone());
                spent_utxo.spent_at_height = Some(source_height as i32);
                spent_utxo.unconfirmed_spent = None;

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
        // Read or create the current TxId
        let transaction_metadata = self.get_or_create_transaction_metadata(
            &txid,
            BlockHeight::from(height),
            unconfirmed,
            timestamp,
        );

        // Add this UTXO if it doesn't already exist
        if let Some(utxo) = transaction_metadata
            .utxos
            .iter_mut()
            .find(|utxo| utxo.txid == txid && utxo.output_index == output_num as u64)
        {
            // If it already exists, it is likely an mempool tx, so update the height
            utxo.height = height as i32
        } else {
            transaction_metadata.utxos.push(Utxo {
                address: taddr,
                txid: txid.clone(),
                output_index: output_num as u64,
                script: vout.script_pubkey.0.clone(),
                value: vout.value.into(),
                height: height as i32,
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
        fvk: &D::Fvk,
    ) where
        D: DomainWalletExt,
        D::Note: PartialEq + Clone,
        D::Recipient: Recipient,
    {
        // Check if this is a change note
        let is_change = self.total_funds_spent_in(&txid) > 0;

        let transaction_metadata = self.get_or_create_transaction_metadata(
            &txid,
            BlockHeight::from(height),
            true,
            timestamp,
        );
        // Update the block height, in case this was a mempool or unconfirmed tx.
        transaction_metadata.block_height = height;

        match D::to_notes_vec_mut(transaction_metadata)
            .iter_mut()
            .find(|n| n.note() == &note)
        {
            None => {
                let nd = D::WalletNote::from_parts(
                    <D::WalletNote as ReceivedNoteAndMetadata>::Fvk::clone(fvk),
                    to.diversifier(),
                    note,
                    WitnessCache::empty(),
                    <D::WalletNote as ReceivedNoteAndMetadata>::Nullifier::from_bytes([0u8; 32]),
                    None,
                    None,
                    None,
                    is_change,
                    false,
                );

                D::WalletNote::transaction_metadata_notes_mut(transaction_metadata).push(nd);
            }
            Some(_) => {}
        }
    }

    pub fn add_new_sapling_note(
        &mut self,
        txid: TxId,
        height: BlockHeight,
        unconfirmed: bool,
        timestamp: u64,
        note: SaplingNote,
        to: PaymentAddress,
        fvk: &SaplingFvk,
        have_spending_key: bool,
        witness: IncrementalWitness<SaplingNode>,
    ) {
        self.add_new_note::<SaplingDomain<zingoconfig::ChainType>>(
            txid,
            height,
            unconfirmed,
            timestamp,
            note,
            to,
            fvk,
            have_spending_key,
            witness,
        )
    }
    pub fn add_new_orchard_note(
        &mut self,
        txid: TxId,
        height: BlockHeight,
        unconfirmed: bool,
        timestamp: u64,
        note: OrchardNote,
        to: orchard::Address,
        fvk: &OrchardFullViewingKey,
        have_spending_key: bool,
        witness: IncrementalWitness<MerkleHashOrchard>,
    ) {
        self.add_new_note::<OrchardDomain>(
            txid,
            height,
            unconfirmed,
            timestamp,
            note,
            to,
            fvk,
            have_spending_key,
            witness,
        )
    }

    pub(crate) fn add_new_note<D: DomainWalletExt>(
        &mut self,
        txid: TxId,
        height: BlockHeight,
        unconfirmed: bool,
        timestamp: u64,
        note: <D::WalletNote as ReceivedNoteAndMetadata>::Note,
        to: D::Recipient,
        fvk: &<D::WalletNote as ReceivedNoteAndMetadata>::Fvk,
        have_spending_key: bool,
        witness: IncrementalWitness<<D::WalletNote as ReceivedNoteAndMetadata>::Node>,
    ) where
        D::Note: PartialEq + Clone,
        D::Recipient: Recipient,
    {
        // Check if this is a change note
        let is_change = self.total_funds_spent_in(&txid) > 0;

        let transaction_metadata = self.get_or_create_transaction_metadata(
            &txid,
            BlockHeight::from(height),
            unconfirmed,
            timestamp,
        );
        // Update the block height, in case this was a mempool or unconfirmed tx.
        transaction_metadata.block_height = height;

        let nullifier = D::WalletNote::get_nullifier_from_note_fvk_and_witness_position(
            &note,
            &fvk,
            witness.position() as u64,
        );
        let witnesses = if have_spending_key {
            WitnessCache::new(vec![witness], u64::from(height))
        } else {
            WitnessCache::empty()
        };

        match D::WalletNote::transaction_metadata_notes_mut(transaction_metadata)
            .iter_mut()
            .find(|n| n.nullifier() == nullifier)
        {
            None => {
                let nd = D::WalletNote::from_parts(
                    fvk.clone(),
                    D::Recipient::diversifier(&to),
                    note,
                    witnesses,
                    nullifier,
                    None,
                    None,
                    None,
                    is_change,
                    have_spending_key,
                );

                D::WalletNote::transaction_metadata_notes_mut(transaction_metadata).push(nd);

                // Also remove any pending notes.
                use super::traits::ToBytes;
                D::WalletNote::transaction_metadata_notes_mut(transaction_metadata)
                    .retain(|n| n.nullifier().to_bytes() != [0u8; 32]);
            }
            Some(n) => {
                // If this note already exists, then just reset the witnesses, because we'll start scanning the witnesses
                // again after this.
                // This is likely to happen if the previous wallet wasn't synced properly or was aborted in the middle of a sync,
                // and has some dangling witnesses
                *n.witnesses_mut() = witnesses;
            }
        }
    }

    // Update the memo for a note if it already exists. If the note doesn't exist, then nothing happens.
    pub(crate) fn add_memo_to_note_metadata<Nd: ReceivedNoteAndMetadata>(
        &mut self,
        txid: &TxId,
        note: Nd::Note,
        memo: Memo,
    ) {
        self.current.get_mut(txid).map(|transaction_metadata| {
            Nd::transaction_metadata_notes_mut(transaction_metadata)
                .iter_mut()
                .find(|n| n.note() == &note)
                .map(|n| *n.memo_mut() = Some(memo));
        });
    }

    pub fn add_outgoing_metadata(
        &mut self,
        txid: &TxId,
        outgoing_metadata: Vec<OutgoingTxMetadata>,
    ) {
        if let Some(transaction_metadata) = self.current.get_mut(txid) {
            // This is n^2 search, but this is likely very small struct, limited by the protocol, so...
            let new_omd: Vec<_> = outgoing_metadata
                .into_iter()
                .filter(|om| {
                    transaction_metadata
                        .outgoing_metadata
                        .iter()
                        .find(|o| **o == *om)
                        .is_none()
                })
                .collect();

            transaction_metadata.outgoing_metadata.extend(new_omd);
        } else {
            error!(
                "TxId {} should be present while adding metadata, but wasn't",
                txid
            );
        }
    }
}
