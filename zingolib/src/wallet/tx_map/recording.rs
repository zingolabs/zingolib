//! contains associated methods for modifying and updating TxMap

use incrementalmerkletree::Position;
use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_note_encryption::Domain;
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::{
    error::{ZingoLibError, ZingoLibResult},
    wallet::{
        data::PoolNullifier,
        notes::interface::OutputConstructor,
        notes::OutputInterface,
        notes::ShieldedNoteInterface,
        traits::{self, DomainWalletExt, Nullifier, Recipient},
    },
};

/// Witness tree requiring methods, each method is noted with *HOW* it requires witness trees.
impl super::TxMap {
    /// During reorgs, we need to remove all txns at a given height, and all spends that refer to any removed txns.
    pub fn invalidate_all_transactions_after_or_at_height(&mut self, reorg_height: u64) {
        let reorg_height = BlockHeight::from_u32(reorg_height as u32);

        self.transaction_records_by_id
            .invalidate_all_transactions_after_or_at_height(reorg_height);

        if let Some(ref mut trees) = self.witness_trees_mut() {
            trees.truncate_to_checkpoint(reorg_height - 1);
        }
    }

    /// Records a TxId as having spent some nullifiers from the wallet.
    /// witness tree requirement:
    /// found_spent_nullifier_internal -> mark_note_as_spent -> remove_witness_mark
    #[allow(clippy::too_many_arguments)]
    pub fn found_spent_nullifier(
        &mut self,
        spending_txid: TxId,
        status: ConfirmationStatus,
        timestamp: Option<u32>,
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

    /// witness tree requirement:
    /// mark_note_as_spent -> remove_witness_mark
    #[allow(clippy::too_many_arguments)]
    fn found_spent_nullifier_internal<D: DomainWalletExt>(
        &mut self,
        spending_txid: TxId,
        status: ConfirmationStatus,
        timestamp: Option<u32>,
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
        let transaction_metadata = self
            .transaction_records_by_id
            .create_modify_get_transaction_record(&spending_txid, status, timestamp);

        if !<D::WalletNote as ShieldedNoteInterface>::Nullifier::get_nullifiers_spent_in_transaction(
            transaction_metadata,
        )
        .iter()
        .any(|nf| *nf == spent_nullifier)
        {
            transaction_metadata.add_spent_nullifier(spent_nullifier.into(), value)
        }

        Ok(())
    }

    // Will mark a note as having been spent at the supplied height and spent_txid.
    /// witness tree requirement:
    /// remove_witness_mark ~-> note_datum.witnessed_position
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
        if let Some(height) = status.get_confirmed_height() {
            // ie remove_witness_mark_sapling or _orchard
            self.remove_witness_mark::<D>(height, spending_txid, source_txid, output_index)?;
        }

        if let Some(transaction_spent_from) = self.transaction_records_by_id.get_mut(&source_txid) {
            if let Some(spent_note) =
                D::WalletNote::get_record_to_outputs_mut(transaction_spent_from)
                    .iter_mut()
                    .find(|note| note.nullifier() == Some(spent_nullifier))
            {
                *spent_note.spending_tx_status_mut() = Some((spending_txid, status));

                Ok(spent_note.value())
            } else {
                ZingoLibError::NoSuchNullifierInTx(spending_txid).handle()?
            }
        } else {
            ZingoLibError::NoSuchTxId(spending_txid).handle()?
        }
    }
}

// shardtree
impl crate::wallet::tx_map::TxMap {
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
        let transaction_record = self
            .transaction_records_by_id
            .get_mut(&source_txid)
            .expect("Txid should be present");

        if let Some(maybe_note) = D::WalletNote::get_record_to_outputs_mut(transaction_record)
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
                    // *note_datum.spending_tx_status_mut() = Some((txid, ConfirmationStatus::Confirmed(height)));
                    // TODO WTF why is this line here?
                    if let Some(position) = *note_datum.witnessed_position() {
                        if let Some(ref mut tree) =
                            D::transaction_metadata_set_to_shardtree_mut(self)
                        {
                            tree.remove_mark(position, Some(&(height - 1))).unwrap();
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
            if let Some(maybe_nnmd) = &mut D::WalletNote::get_record_to_outputs_mut(tmd)
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
