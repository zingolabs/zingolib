use incrementalmerkletree::{Hashable, Position};
use zcash_primitives::{memo::Memo, merkle_tree::HashSer, transaction::TxId};

use super::{
    data::TransactionMetadata,
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
        output_index: u32,
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
    fn output_index(&self) -> &u32;
    fn output_index_mut(&mut self) -> &mut u32;
    fn pending_receipt(&self) -> bool {
        self.nullifier().is_none()
    }
    fn pending_spent(&self) -> &Option<(TxId, u32)>;
    fn pool() -> Pool;
    fn spent(&self) -> &Option<(TxId, u32)>;
    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)>;
    fn transaction_metadata_notes(wallet_transaction: &TransactionMetadata) -> &Vec<Self>;
    fn transaction_metadata_notes_mut(
        wallet_transaction: &mut TransactionMetadata,
    ) -> &mut Vec<Self>;
    fn pending_spent_mut(&mut self) -> &mut Option<(TxId, u32)>;
    ///Convenience function
    fn value(&self) -> u64 {
        Self::value_from_note(self.note())
    }
    fn value_from_note(note: &Self::Note) -> u64;
    fn witnessed_position(&self) -> &Option<Position>;
    fn witnessed_position_mut(&mut self) -> &mut Option<Position>;
}

pub struct SaplingNote {
    pub diversifier: zcash_primitives::sapling::Diversifier,
    pub note: zcash_primitives::sapling::Note,

    // The position of this note's value commitment in the global commitment tree
    // We need to create a witness to it, to spend
    pub(crate) witnessed_position: Option<Position>,

    // The note's index in its containing transaction
    pub(crate) output_index: u32,

    pub(super) nullifier: Option<zcash_primitives::sapling::Nullifier>,

    pub spent: Option<(TxId, u32)>, // If this note was confirmed spent. Todo: as related to unconfirmed spent, this is potential data incoherence

    // If this note was spent in a send, but has not yet been confirmed.
    // Contains the transaction id and height at which it was broadcast
    pub unconfirmed_spent: Option<(TxId, u32)>,
    pub memo: Option<Memo>,
    pub is_change: bool,

    // If the spending key is available in the wallet (i.e., whether to keep witness up-to-date) Todo should this data point really be here?
    pub have_spending_key: bool,
}

impl std::fmt::Debug for SaplingNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaplingNoteData")
            .field("diversifier", &self.diversifier)
            .field("note", &self.note)
            .field("nullifier", &self.nullifier)
            .field("spent", &self.spent)
            .field("unconfirmed_spent", &self.unconfirmed_spent)
            .field("memo", &self.memo)
            .field("diversifier", &self.diversifier)
            .field("note", &self.note)
            .field("nullifier", &self.nullifier)
            .field("spent", &self.spent)
            .field("unconfirmed_spent", &self.unconfirmed_spent)
            .field("memo", &self.memo)
            .field("is_change", &self.is_change)
            .finish_non_exhaustive()
    }
}

impl ShieldedNoteInterface for SaplingNote {
    type Diversifier = zcash_primitives::sapling::Diversifier;
    type Note = zcash_primitives::sapling::Note;
    type Node = zcash_primitives::sapling::Node;
    type Nullifier = zcash_primitives::sapling::Nullifier;

    fn diversifier(&self) -> &Self::Diversifier {
        &self.diversifier
    }

    fn nullifier_mut(&mut self) -> &mut Option<Self::Nullifier> {
        &mut self.nullifier
    }

    fn from_parts(
        diversifier: zcash_primitives::sapling::Diversifier,
        note: zcash_primitives::sapling::Note,
        witnessed_position: Option<Position>,
        nullifier: Option<zcash_primitives::sapling::Nullifier>,
        spent: Option<(TxId, u32)>,
        unconfirmed_spent: Option<(TxId, u32)>,
        memo: Option<Memo>,
        is_change: bool,
        have_spending_key: bool,
        output_index: u32,
    ) -> Self {
        Self {
            diversifier,
            note,
            witnessed_position,
            nullifier,
            spent,
            unconfirmed_spent,
            memo,
            is_change,
            have_spending_key,
            output_index,
        }
    }

    fn get_deprecated_serialized_view_key_buffer() -> Vec<u8> {
        vec![0u8; 169]
    }

    fn have_spending_key(&self) -> bool {
        self.have_spending_key
    }

    fn is_change(&self) -> bool {
        self.is_change
    }

    fn is_change_mut(&mut self) -> &mut bool {
        &mut self.is_change
    }

    fn memo(&self) -> &Option<Memo> {
        &self.memo
    }

    fn memo_mut(&mut self) -> &mut Option<Memo> {
        &mut self.memo
    }

    fn note(&self) -> &Self::Note {
        &self.note
    }

    fn nullifier(&self) -> Option<Self::Nullifier> {
        self.nullifier
    }

    fn pool() -> Pool {
        Pool::Sapling
    }

    fn spent(&self) -> &Option<(TxId, u32)> {
        &self.spent
    }

    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.spent
    }

    fn transaction_metadata_notes(wallet_transaction: &TransactionMetadata) -> &Vec<Self> {
        &wallet_transaction.sapling_notes
    }

    fn transaction_metadata_notes_mut(
        wallet_transaction: &mut TransactionMetadata,
    ) -> &mut Vec<Self> {
        &mut wallet_transaction.sapling_notes
    }

    fn pending_spent(&self) -> &Option<(TxId, u32)> {
        &self.unconfirmed_spent
    }

    fn pending_spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.unconfirmed_spent
    }

    fn value_from_note(note: &Self::Note) -> u64 {
        note.value().inner()
    }

    fn witnessed_position(&self) -> &Option<Position> {
        &self.witnessed_position
    }

    fn witnessed_position_mut(&mut self) -> &mut Option<Position> {
        &mut self.witnessed_position
    }

    fn output_index(&self) -> &u32 {
        &self.output_index
    }

    fn output_index_mut(&mut self) -> &mut u32 {
        &mut self.output_index
    }
}

#[derive(Debug)]
pub struct OrchardNote {
    pub diversifier: orchard::keys::Diversifier,
    pub note: orchard::note::Note,

    // The position of this note's value commitment in the global commitment tree
    // We need to create a witness to it, to spend
    pub witnessed_position: Option<Position>,

    // The note's index in its containing transaction
    pub(crate) output_index: u32,

    pub(super) nullifier: Option<orchard::note::Nullifier>,

    pub spent: Option<(TxId, u32)>, // If this note was confirmed spent. Todo: as related to unconfirmed spent, this is potential data incoherence

    // If this note was spent in a send, but has not yet been confirmed.
    // Contains the transaction id and height at which it was broadcast
    pub unconfirmed_spent: Option<(TxId, u32)>,
    pub memo: Option<Memo>,
    pub is_change: bool,

    // If the spending key is available in the wallet (i.e., whether to keep witness up-to-date)
    pub have_spending_key: bool,
}

impl ShieldedNoteInterface for OrchardNote {
    type Diversifier = orchard::keys::Diversifier;
    type Note = orchard::note::Note;
    type Node = MerkleHashOrchard;
    type Nullifier = orchard::note::Nullifier;

    fn diversifier(&self) -> &Self::Diversifier {
        &self.diversifier
    }

    fn nullifier_mut(&mut self) -> &mut Option<Self::Nullifier> {
        &mut self.nullifier
    }

    fn from_parts(
        diversifier: Self::Diversifier,
        note: Self::Note,
        witnessed_position: Option<Position>,
        nullifier: Option<Self::Nullifier>,
        spent: Option<(TxId, u32)>,
        unconfirmed_spent: Option<(TxId, u32)>,
        memo: Option<Memo>,
        is_change: bool,
        have_spending_key: bool,
        output_index: u32,
    ) -> Self {
        Self {
            diversifier,
            note,
            witnessed_position,
            nullifier,
            spent,
            unconfirmed_spent,
            memo,
            is_change,
            have_spending_key,
            output_index,
        }
    }

    fn get_deprecated_serialized_view_key_buffer() -> Vec<u8> {
        vec![0u8; 96]
    }

    fn have_spending_key(&self) -> bool {
        self.have_spending_key
    }
    fn is_change(&self) -> bool {
        self.is_change
    }

    fn is_change_mut(&mut self) -> &mut bool {
        &mut self.is_change
    }

    fn memo(&self) -> &Option<Memo> {
        &self.memo
    }

    fn memo_mut(&mut self) -> &mut Option<Memo> {
        &mut self.memo
    }

    fn note(&self) -> &Self::Note {
        &self.note
    }

    fn nullifier(&self) -> Option<Self::Nullifier> {
        self.nullifier
    }

    fn pool() -> Pool {
        Pool::Orchard
    }

    fn spent(&self) -> &Option<(TxId, u32)> {
        &self.spent
    }

    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.spent
    }

    fn transaction_metadata_notes(wallet_transaction: &TransactionMetadata) -> &Vec<Self> {
        &wallet_transaction.orchard_notes
    }

    fn transaction_metadata_notes_mut(
        wallet_transaction: &mut TransactionMetadata,
    ) -> &mut Vec<Self> {
        &mut wallet_transaction.orchard_notes
    }

    fn pending_spent(&self) -> &Option<(TxId, u32)> {
        &self.unconfirmed_spent
    }

    fn pending_spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.unconfirmed_spent
    }

    fn value_from_note(note: &Self::Note) -> u64 {
        note.value().inner()
    }

    fn witnessed_position(&self) -> &Option<Position> {
        &self.witnessed_position
    }
    fn witnessed_position_mut(&mut self) -> &mut Option<Position> {
        &mut self.witnessed_position
    }
    fn output_index(&self) -> &u32 {
        &self.output_index
    }

    fn output_index_mut(&mut self) -> &mut u32 {
        &mut self.output_index
    }
}
