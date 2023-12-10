use incrementalmerkletree::Position;
use zcash_note_encryption::Domain;
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

use crate::wallet::traits::{DomainWalletExt, Recipient, ShieldedNoteInterface};

use super::TransactionMetadataSet;

impl TransactionMetadataSet {
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
        <D as Domain>::Recipient: Recipient,
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
}
