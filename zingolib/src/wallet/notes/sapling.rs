//! TODO: Add Mod Description Here!
use incrementalmerkletree::Position;
use zcash_client_backend::{PoolType, ShieldedProtocol};
use zcash_primitives::{memo::Memo, transaction::TxId};
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::wallet::notes::interface::OutputConstructor;

use super::{
    super::data::TransactionRecord, query::OutputSpendStatusQuery, OutputInterface,
    ShieldedNoteInterface,
};

/// TODO: Add Doc Comment Here!
#[derive(Clone)]
pub struct SaplingNote {
    /// TODO: Add Doc Comment Here!
    pub diversifier: sapling_crypto::Diversifier,
    /// TODO: Add Doc Comment Here!
    pub sapling_crypto_note: sapling_crypto::Note,

    // The position of this note's value commitment in the global commitment tree
    // We need to create a witness to it, to spend
    pub(crate) witnessed_position: Option<Position>,

    // The note's index in its containing transaction
    pub(crate) output_index: Option<u32>,

    /// TODO: Add Doc Comment Here!
    pub nullifier: Option<sapling_crypto::Nullifier>,

    /// whether, where, and when it was spent
    spend: Option<(TxId, ConfirmationStatus)>,

    /// TODO: Add Doc Comment Here!
    pub memo: Option<Memo>,

    /// DEPRECATED
    pub is_change: bool,

    /// If the spending key is available in the wallet (i.e., whether to keep witness up-to-date) Todo should this data point really be here?
    pub have_spending_key: bool,
}

impl std::fmt::Debug for SaplingNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaplingNoteData")
            .field("diversifier", &self.diversifier)
            .field("memo", &self.memo)
            .field("note", &self.sapling_crypto_note)
            .field("nullifier", &self.nullifier)
            .field("spend", &self.spend)
            .field("is_change", &self.is_change)
            .finish_non_exhaustive()
    }
}

impl OutputInterface for SaplingNote {
    fn pool_type(&self) -> PoolType {
        PoolType::Shielded(ShieldedProtocol::Sapling)
    }

    fn value(&self) -> u64 {
        self.sapling_crypto_note.value().inner()
    }

    fn spending_tx_status(&self) -> &Option<(TxId, ConfirmationStatus)> {
        &self.spend
    }

    fn spending_tx_status_mut(&mut self) -> &mut Option<(TxId, ConfirmationStatus)> {
        &mut self.spend
    }
}
impl OutputConstructor for SaplingNote {
    fn get_record_outputs(transaction_record: &TransactionRecord) -> Vec<&Self> {
        transaction_record.sapling_notes.iter().collect()
    }
    fn get_record_query_matching_outputs(
        transaction_record: &TransactionRecord,
        spend_status_query: OutputSpendStatusQuery,
    ) -> Vec<&Self> {
        transaction_record
            .sapling_notes
            .iter()
            .filter(|output| output.spend_status_query(spend_status_query))
            .collect()
    }
    fn get_record_to_outputs_mut(transaction_record: &mut TransactionRecord) -> Vec<&mut Self> {
        transaction_record.sapling_notes.iter_mut().collect()
    }
    fn get_record_query_matching_outputs_mut(
        transaction_record: &mut TransactionRecord,
        spend_status_query: OutputSpendStatusQuery,
    ) -> Vec<&mut Self> {
        transaction_record
            .sapling_notes
            .iter_mut()
            .filter(|output| output.spend_status_query(spend_status_query))
            .collect()
    }
}

impl ShieldedNoteInterface for SaplingNote {
    type Diversifier = sapling_crypto::Diversifier;
    type Note = sapling_crypto::Note;
    type Node = sapling_crypto::Node;
    type Nullifier = sapling_crypto::Nullifier;

    fn diversifier(&self) -> &Self::Diversifier {
        &self.diversifier
    }

    fn nullifier_mut(&mut self) -> &mut Option<Self::Nullifier> {
        &mut self.nullifier
    }

    fn from_parts(
        diversifier: sapling_crypto::Diversifier,
        sapling_crypto_note: sapling_crypto::Note,
        witnessed_position: Option<Position>,
        nullifier: Option<sapling_crypto::Nullifier>,
        spend: Option<(TxId, ConfirmationStatus)>,
        memo: Option<Memo>,
        is_change: bool,
        have_spending_key: bool,
        output_index: Option<u32>,
    ) -> Self {
        Self {
            diversifier,
            sapling_crypto_note,
            witnessed_position,
            nullifier,
            spend,
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
        &self.sapling_crypto_note
    }

    fn nullifier(&self) -> Option<Self::Nullifier> {
        self.nullifier
    }

    fn pool() -> PoolType {
        PoolType::Shielded(ShieldedProtocol::Sapling)
    }

    fn transaction_metadata_notes(wallet_transaction: &TransactionRecord) -> &Vec<Self> {
        &wallet_transaction.sapling_notes
    }

    fn transaction_metadata_notes_mut(
        wallet_transaction: &mut TransactionRecord,
    ) -> &mut Vec<Self> {
        &mut wallet_transaction.sapling_notes
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

    fn output_index_mut(&mut self) -> &mut Option<u32> {
        &mut self.output_index
    }

    fn to_zcb_note(&self) -> zcash_client_backend::wallet::Note {
        zcash_client_backend::wallet::Note::Sapling(self.note().clone())
    }
}

#[cfg(any(test, feature = "test-elevation"))]
pub mod mocks {
    //! Mock version of the struct for testing
    use incrementalmerkletree::Position;
    use sapling_crypto::value::NoteValue;
    use zcash_primitives::{memo::Memo, transaction::TxId};
    use zingo_status::confirmation_status::ConfirmationStatus;

    use crate::{
        mocks::SaplingCryptoNoteBuilder,
        utils::build_method,
        wallet::{notes::ShieldedNoteInterface, traits::FromBytes},
    };

    use super::SaplingNote;

    /// to create a mock SaplingNote
    #[derive(Clone)]
    pub(crate) struct SaplingNoteBuilder {
        diversifier: Option<sapling_crypto::Diversifier>,
        note: Option<SaplingCryptoNoteBuilder>,
        witnessed_position: Option<Option<Position>>,
        pub output_index: Option<Option<u32>>,
        nullifier: Option<Option<sapling_crypto::Nullifier>>,
        spending_tx_status: Option<Option<(TxId, ConfirmationStatus)>>,
        memo: Option<Option<Memo>>,
        is_change: Option<bool>,
        have_spending_key: Option<bool>,
    }

    impl SaplingNoteBuilder {
        /// A builder, for a 'blank' note.
        /// Be aware that two notes generated with
        /// this function will be identical if built
        /// with no changes.
        #[allow(dead_code)]
        pub fn new() -> Self {
            Self::default()
        }

        // Methods to set each field
        build_method!(diversifier, sapling_crypto::Diversifier);
        build_method!(note, SaplingCryptoNoteBuilder);
        build_method!(witnessed_position, Option<Position>);
        build_method!(output_index, Option<u32>);
        build_method!(nullifier, Option<sapling_crypto::Nullifier>);
        build_method!(spending_tx_status, Option<(TxId, ConfirmationStatus)>);
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
        pub fn build(&self) -> SaplingNote {
            SaplingNote::from_parts(
                self.diversifier.unwrap(),
                self.note.clone().unwrap().build(),
                self.witnessed_position.unwrap(),
                self.nullifier.unwrap(),
                self.spending_tx_status.unwrap(),
                self.memo.clone().unwrap(),
                self.is_change.unwrap(),
                self.have_spending_key.unwrap(),
                self.output_index.unwrap(),
            )
        }
    }

    impl Default for SaplingNoteBuilder {
        fn default() -> Self {
            let mut builder = SaplingNoteBuilder {
                diversifier: None,
                note: None,
                witnessed_position: None,
                output_index: None,
                nullifier: None,
                spending_tx_status: None,
                memo: None,
                is_change: None,
                have_spending_key: None,
            };
            builder
                .diversifier(sapling_crypto::Diversifier([0; 11]))
                .note(crate::mocks::SaplingCryptoNoteBuilder::default())
                .witnessed_position(Some(Position::from(0)))
                .output_index(Some(0))
                .nullifier(Some(sapling_crypto::Nullifier::from_bytes([0; 32])))
                .spending_tx_status(None)
                .memo(None)
                .set_change(false)
                .have_spending_key(true);
            builder
        }
    }
}
