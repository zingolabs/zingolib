//! TODO: Add Mod Description Here!
use incrementalmerkletree::Position;
use zcash_client_backend::{PoolType, ShieldedProtocol};
use zcash_primitives::{memo::Memo, transaction::TxId};

use super::{
    super::data::TransactionRecord, query::OutputSpendStatusQuery, OutputInterface,
    ShieldedNoteInterface,
};

/// TODO: Add Doc Comment Here!
#[derive(Debug)]
pub struct OrchardNote {
    /// TODO: Add Doc Comment Here!
    pub diversifier: orchard::keys::Diversifier,
    /// TODO: Add Doc Comment Here!
    pub orchard_crypto_note: orchard::note::Note,

    /// The position of this note's value commitment in the global commitment tree
    /// We need to create a witness to it, to spend
    pub witnessed_position: Option<Position>,

    /// The note's index in its containing transaction
    pub(crate) output_index: Option<u32>,

    pub(crate) nullifier: Option<orchard::note::Nullifier>,

    /// If this note was confirmed spent
    pub spent: Option<(TxId, u32)>, // Todo: as related to pending spent, this is potential data incoherence

    /// If this note was spent in a send, but has not yet been confirmed.
    /// Contains the transaction id and height at which it was broadcast
    pub pending_spent: Option<(TxId, u32)>,

    /// TODO: Add Doc Comment Here!
    pub memo: Option<Memo>,

    /// TODO: Add Doc Comment Here!
    pub is_change: bool,

    /// If the spending key is available in the wallet (i.e., whether to keep witness up-to-date)
    pub have_spending_key: bool,
}

impl OutputInterface for OrchardNote {
    fn pool_type(&self) -> PoolType {
        PoolType::Shielded(ShieldedProtocol::Orchard)
    }

    fn value(&self) -> u64 {
        self.orchard_crypto_note.value().inner()
    }

    fn spent(&self) -> &Option<(TxId, u32)> {
        &self.spent
    }

    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.spent
    }

    fn pending_spent(&self) -> &Option<(TxId, u32)> {
        &self.pending_spent
    }

    fn pending_spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.pending_spent
    }

    fn transaction_record_to_outputs_vec(transaction_record: &TransactionRecord) -> Vec<&Self> {
        transaction_record.orchard_notes.iter().collect()
    }
    fn transaction_record_to_outputs_vec_query(
        transaction_record: &TransactionRecord,
        spend_status_query: OutputSpendStatusQuery,
    ) -> Vec<&Self> {
        transaction_record
            .orchard_notes
            .iter()
            .filter(|output| output.spend_status_query(spend_status_query))
            .collect()
    }
    fn transaction_record_to_outputs_vec_mut(
        transaction_record: &mut TransactionRecord,
    ) -> Vec<&mut Self> {
        transaction_record.orchard_notes.iter_mut().collect()
    }
    fn transaction_record_to_outputs_vec_query_mut(
        transaction_record: &mut TransactionRecord,
        spend_status_query: OutputSpendStatusQuery,
    ) -> Vec<&mut Self> {
        transaction_record
            .orchard_notes
            .iter_mut()
            .filter(|output| output.spend_status_query(spend_status_query))
            .collect()
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
        orchard_crypto_note: Self::Note,
        witnessed_position: Option<Position>,
        nullifier: Option<Self::Nullifier>,
        spent: Option<(TxId, u32)>,
        pending_spent: Option<(TxId, u32)>,
        memo: Option<Memo>,
        is_change: bool,
        have_spending_key: bool,
        output_index: Option<u32>,
    ) -> Self {
        Self {
            diversifier,
            orchard_crypto_note,
            witnessed_position,
            nullifier,
            spent,
            pending_spent,
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
        &self.orchard_crypto_note
    }

    fn nullifier(&self) -> Option<Self::Nullifier> {
        self.nullifier
    }

    fn pool() -> PoolType {
        PoolType::Shielded(ShieldedProtocol::Orchard)
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

#[cfg(test)]
pub mod mocks {
    //! Mock version of the struct for testing
    use incrementalmerkletree::Position;
    use orchard::{keys::Diversifier, note::Nullifier, value::NoteValue};
    use zcash_primitives::{memo::Memo, transaction::TxId};

    use crate::{
        mocks::{build_method, orchard_note::OrchardCryptoNoteBuilder},
        wallet::notes::ShieldedNoteInterface,
    };

    use super::OrchardNote;

    /// to create a mock SaplingNote
    #[derive(Clone)]
    pub(crate) struct OrchardNoteBuilder {
        diversifier: Option<Diversifier>,
        note: Option<OrchardCryptoNoteBuilder>,
        witnessed_position: Option<Option<Position>>,
        pub output_index: Option<Option<u32>>,
        nullifier: Option<Option<Nullifier>>,
        spent: Option<Option<(TxId, u32)>>,
        pending_spent: Option<Option<(TxId, u32)>>,
        memo: Option<Option<Memo>>,
        is_change: Option<bool>,
        have_spending_key: Option<bool>,
    }

    #[allow(dead_code)] //TODO:  fix this gross hack that I tossed in to silence the language-analyzer false positive
    impl OrchardNoteBuilder {
        /// blank builder
        pub fn new() -> Self {
            OrchardNoteBuilder {
                diversifier: None,
                note: None,
                witnessed_position: None,
                output_index: None,
                nullifier: None,
                spent: None,
                pending_spent: None,
                memo: None,
                is_change: None,
                have_spending_key: None,
            }
        }

        // Methods to set each field
        build_method!(diversifier, Diversifier);
        build_method!(note, OrchardCryptoNoteBuilder);
        build_method!(witnessed_position, Option<Position>);
        build_method!(output_index, Option<u32>);
        build_method!(nullifier, Option<Nullifier>);
        build_method!(spent, Option<(TxId, u32)>);
        build_method!(pending_spent, Option<(TxId, u32)>);
        build_method!(memo, Option<Memo>);
        #[doc = "Set the is_change field of the builder."]
        pub fn set_change(&mut self, is_change: bool) -> &mut Self {
            self.is_change = Some(is_change);
            self
        }
        build_method!(have_spending_key, bool);
        pub fn value(&mut self, value: u64) -> &mut Self {
            self.note
                .as_mut()
                .unwrap()
                .value(NoteValue::from_raw(value));
            self
        }

        /// builds a mock SaplingNote after all pieces are supplied
        pub fn build(self) -> OrchardNote {
            OrchardNote::from_parts(
                self.diversifier.unwrap(),
                self.note.unwrap().build(),
                self.witnessed_position.unwrap(),
                self.nullifier.unwrap(),
                self.spent.unwrap(),
                self.pending_spent.unwrap(),
                self.memo.unwrap(),
                self.is_change.unwrap(),
                self.have_spending_key.unwrap(),
                self.output_index.unwrap(),
            )
        }
    }

    impl Default for OrchardNoteBuilder {
        fn default() -> Self {
            let mut builder = OrchardNoteBuilder::new();
            builder
                .diversifier(Diversifier::from_bytes([0; 11]))
                .note(OrchardCryptoNoteBuilder::default())
                .witnessed_position(Some(Position::from(0)))
                .output_index(Some(0))
                .nullifier(Some(Nullifier::from_bytes(&[0u8; 32]).unwrap()))
                .spent(None)
                .pending_spent(None)
                .memo(None)
                .set_change(false)
                .have_spending_key(true);
            builder
        }
    }
}
