//! TODO: Add Mod Description Here!
use incrementalmerkletree::{Hashable, Position};
use zcash_primitives::{memo::Memo, merkle_tree::HashSer, transaction::TxId};

use super::super::{
    data::TransactionRecord,
    keys::unified::WalletCapability,
    traits::{FromBytes, FromCommitment, Nullifier, ReadableWriteable, ToBytes},
    Pool,
};

/// TODO: Add Doc Comment Here!
pub trait NoteInterface: Sized {
    /// TODO: Add Doc Comment Here!
    fn spent(&self) -> &Option<(TxId, u32)>;

    /// TODO: Add Doc Comment Here!
    fn is_spent(&self) -> bool {
        Self::spent(self).is_some()
    }

    /// TODO: Add Doc Comment Here!
    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)>;

    /// TODO: Add Doc Comment Here!
    fn pending_spent(&self) -> &Option<(TxId, u32)>;

    /// TODO: Add Doc Comment Here!
    fn pending_spent_mut(&mut self) -> &mut Option<(TxId, u32)>;
    fn is_pending_spent(&self) -> bool {
        Self::pending_spent(self).is_some()
    }
    fn is_spent_or_pending_spent(&self) -> bool {
        self.is_spent() || self.is_pending_spent()
    }
}

///   ShieldedNotes are either part of a Sapling or Orchard Pool
pub trait ShieldedNoteInterface: NoteInterface + Sized {
    /// TODO: Add Doc Comment Here!
    type Diversifier: Copy + FromBytes<11> + ToBytes<11>;
    /// TODO: Add Doc Comment Here!
    type Note: PartialEq
        + for<'a> ReadableWriteable<(Self::Diversifier, &'a WalletCapability)>
        + Clone;
    /// TODO: Add Doc Comment Here!
    type Node: Hashable + HashSer + FromCommitment + Send + Clone + PartialEq + Eq;
    /// TODO: Add Doc Comment Here!
    type Nullifier: Nullifier;

    /// TODO: Add Doc Comment Here!
    fn diversifier(&self) -> &Self::Diversifier;

    /// TODO: Add Doc Comment Here!
    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        diversifier: Self::Diversifier,
        note: Self::Note,
        position_of_commitment_to_witness: Option<Position>,
        nullifier: Option<Self::Nullifier>,
        spent: Option<(TxId, u32)>,
        unconfirmed_spent: Option<(TxId, u32)>,
        memo: Option<Memo>,
        is_change: bool,
        have_spending_key: bool,
        output_index: Option<u32>,
    ) -> Self;

    /// TODO: Add Doc Comment Here!
    fn get_deprecated_serialized_view_key_buffer() -> Vec<u8>;

    /// TODO: Add Doc Comment Here!
    fn have_spending_key(&self) -> bool;

    /// TODO: Add Doc Comment Here!
    fn is_change(&self) -> bool;

    /// TODO: Add Doc Comment Here!
    fn is_change_mut(&mut self) -> &mut bool;

    /// TODO: Add Doc Comment Here!
    fn memo(&self) -> &Option<Memo>;

    /// TODO: Add Doc Comment Here!
    fn memo_mut(&mut self) -> &mut Option<Memo>;

    /// TODO: Add Doc Comment Here!
    fn note(&self) -> &Self::Note;

    /// TODO: Add Doc Comment Here!
    fn nullifier(&self) -> Option<Self::Nullifier>;

    /// TODO: Add Doc Comment Here!
    fn nullifier_mut(&mut self) -> &mut Option<Self::Nullifier>;

    /// TODO: Add Doc Comment Here!
    fn output_index(&self) -> &Option<u32>;

    /// TODO: Add Doc Comment Here!
    fn pending_receipt(&self) -> bool {
        self.nullifier().is_none()
    }

    /// TODO: Add Doc Comment Here!
    fn pool() -> Pool;

    /// TODO: Add Doc Comment Here!
    fn transaction_metadata_notes(wallet_transaction: &TransactionRecord) -> &Vec<Self>;

    /// TODO: Add Doc Comment Here!
    fn transaction_metadata_notes_mut(wallet_transaction: &mut TransactionRecord)
        -> &mut Vec<Self>;

    ///Convenience function
    fn value(&self) -> u64 {
        Self::value_from_note(self.note())
    }

    /// TODO: Add Doc Comment Here!
    fn value_from_note(note: &Self::Note) -> u64;

    /// TODO: Add Doc Comment Here!
    fn witnessed_position(&self) -> &Option<Position>;

    /// TODO: Add Doc Comment Here!
    fn witnessed_position_mut(&mut self) -> &mut Option<Position>;

    /// TODO: Add Doc Comment Here!
    fn to_zcb_note(&self) -> zcash_client_backend::wallet::Note;
}
