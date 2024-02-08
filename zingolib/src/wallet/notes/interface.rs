


use incrementalmerkletree::{Hashable, Position};
use zcash_primitives::{
    memo::Memo,
    merkle_tree::HashSer,
    transaction::{TxId},
};

use super::super::{
    data::TransactionRecord,
    keys::unified::WalletCapability,
    traits::{FromBytes, FromCommitment, Nullifier, ReadableWriteable, ToBytes},
    Pool,
};

///   All zingolib::wallet::traits::Notes are NoteInterface
///   NoteInterface provides...
pub trait ShieldedNoteInterface: Sized {
    type Diversifier: Copy + FromBytes<11> + ToBytes<11>;

    type Note: PartialEq
        + for<'a> ReadableWriteable<(Self::Diversifier, &'a WalletCapability)>
        + Clone;
    type Node: Hashable + HashSer + FromCommitment + Send + Clone + PartialEq + Eq;
    type Nullifier: Nullifier;

    fn diversifier(&self) -> &Self::Diversifier;
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
    fn get_deprecated_serialized_view_key_buffer() -> Vec<u8>;
    fn have_spending_key(&self) -> bool;
    fn is_change(&self) -> bool;
    fn is_change_mut(&mut self) -> &mut bool;
    fn is_spent(&self) -> bool {
        Self::spent(self).is_some()
    }
    fn memo(&self) -> &Option<Memo>;
    fn memo_mut(&mut self) -> &mut Option<Memo>;
    fn note(&self) -> &Self::Note;
    fn nullifier(&self) -> Option<Self::Nullifier>;
    fn nullifier_mut(&mut self) -> &mut Option<Self::Nullifier>;
    fn output_index(&self) -> &Option<u32>;
    fn pending_receipt(&self) -> bool {
        self.nullifier().is_none()
    }
    fn pending_spent(&self) -> &Option<(TxId, u32)>;
    fn pool() -> Pool;
    fn spent(&self) -> &Option<(TxId, u32)>;
    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)>;
    fn transaction_metadata_notes(wallet_transaction: &TransactionRecord) -> &Vec<Self>;
    fn transaction_metadata_notes_mut(wallet_transaction: &mut TransactionRecord)
        -> &mut Vec<Self>;
    fn pending_spent_mut(&mut self) -> &mut Option<(TxId, u32)>;
    ///Convenience function
    fn value(&self) -> u64 {
        Self::value_from_note(self.note())
    }
    fn value_from_note(note: &Self::Note) -> u64;
    fn witnessed_position(&self) -> &Option<Position>;
    fn witnessed_position_mut(&mut self) -> &mut Option<Position>;
}
