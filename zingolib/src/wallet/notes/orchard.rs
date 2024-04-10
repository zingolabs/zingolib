use incrementalmerkletree::Position;
use zcash_primitives::{memo::Memo, transaction::TxId};

use super::{
    super::{data::TransactionRecord, Pool},
    NoteInterface, ShieldedNoteInterface,
};

#[derive(Debug)]
pub struct OrchardNote {
    pub diversifier: orchard::keys::Diversifier,
    pub note: orchard::note::Note,

    // The position of this note's value commitment in the global commitment tree
    // We need to create a witness to it, to spend
    pub witnessed_position: Option<Position>,

    // The note's index in its containing transaction
    pub(crate) output_index: Option<u32>,

    pub(crate) nullifier: Option<orchard::note::Nullifier>,

    pub spent: Option<(TxId, u32)>, // If this note was confirmed spent. Todo: as related to unconfirmed spent, this is potential data incoherence

    // If this note was spent in a send, but has not yet been confirmed.
    // Contains the transaction id and height at which it was broadcast
    pub unconfirmed_spent: Option<(TxId, u32)>,
    pub memo: Option<Memo>,
    pub is_change: bool,

    // If the spending key is available in the wallet (i.e., whether to keep witness up-to-date)
    pub have_spending_key: bool,
}

impl NoteInterface for OrchardNote {
    fn spent(&self) -> &Option<(TxId, u32)> {
        &self.spent
    }
    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.spent
    }
    fn pending_spent(&self) -> &Option<(TxId, u32)> {
        &self.unconfirmed_spent
    }
    fn pending_spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.unconfirmed_spent
    }
}

impl ShieldedNoteInterface for OrchardNote {
    type Diversifier = orchard::keys::Diversifier;
    type Note = orchard::note::Note;
    type Node = orchard::tree::MerkleHashOrchard;
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
        output_index: Option<u32>,
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

    fn transaction_metadata_notes(wallet_transaction: &TransactionRecord) -> &Vec<Self> {
        &wallet_transaction.orchard_notes
    }

    fn transaction_metadata_notes_mut(
        wallet_transaction: &mut TransactionRecord,
    ) -> &mut Vec<Self> {
        &mut wallet_transaction.orchard_notes
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
    fn output_index(&self) -> &Option<u32> {
        &self.output_index
    }
    fn to_zcb_note(&self) -> zcash_client_backend::wallet::Note {
        zcash_client_backend::wallet::Note::Orchard(*self.note())
    }
}
