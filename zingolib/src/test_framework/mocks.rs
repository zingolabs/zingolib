//! Tools to facilitate mocks for testing
use zcash_primitives::transaction::TxId;

macro_rules! build_method {
    ($name:ident, $localtype:ty) => {
        pub fn $name(mut self, $name: $localtype) -> Self {
            self.$name = Some($name);
            self
        }
    };
}

// Transparent Note Mocker
use crate::wallet::notes::TransparentNote;
/// builds a mock transparent note after all pieces are supplied
pub struct TransparentNoteBuilder {
    address: Option<String>,
    txid: Option<TxId>,
    output_index: Option<u64>,
    script: Option<Vec<u8>>,
    value: Option<u64>,
    spent: Option<Option<(TxId, u32)>>,
    unconfirmed_spent: Option<Option<(TxId, u32)>>,
}
#[allow(dead_code)] //TODO:  fix this gross hack that I tossed in to silence the language-analyzer false positive
impl TransparentNoteBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    // Methods to set each field
    build_method!(address, String);
    build_method!(txid, TxId);
    build_method!(output_index, u64);
    build_method!(script, Vec<u8>);
    build_method!(value, u64);
    build_method!(spent, Option<(TxId, u32)>);
    build_method!(unconfirmed_spent, Option<(TxId, u32)>);

    // Build method
    pub fn build(self) -> TransparentNote {
        TransparentNote::from_parts(
            self.address.unwrap(),
            self.txid.unwrap(),
            self.output_index.unwrap(),
            self.script.unwrap(),
            self.value.unwrap(),
            self.spent.unwrap(),
            self.unconfirmed_spent.unwrap(),
        )
    }
}

impl Default for TransparentNoteBuilder {
    fn default() -> Self {
        TransparentNoteBuilder {
            address: Some("default_address".to_string()),
            txid: Some(TxId::from_bytes([0u8; 32])),
            output_index: Some(0),
            script: Some(vec![]),
            value: Some(0),
            spent: Some(None),
            unconfirmed_spent: Some(None),
        }
    }
}

// Sapling Note Mocker
pub mod sapling_note {

    use incrementalmerkletree::Position;
    use sapling_crypto::value::NoteValue;
    use sapling_crypto::PaymentAddress;
    use sapling_crypto::Rseed;
    use zcash_primitives::memo::Memo;
    use zcash_primitives::transaction::TxId;

    use crate::wallet::notes::SaplingNote;
    use crate::wallet::notes::ShieldedNoteInterface;
    use crate::wallet::traits::FromBytes;

    pub struct LRZSaplingNoteBuilder {
        recipient: Option<PaymentAddress>,
        value: Option<NoteValue>,
        rseed: Option<Rseed>,
    }
    #[allow(dead_code)] //TODO:  fix this gross hack that I tossed in to silence the language-analyzer false positive
    impl LRZSaplingNoteBuilder {
        pub fn new() -> Self {
            Self::default()
        }

        // Methods to set each field
        build_method!(recipient, PaymentAddress);
        build_method!(value, NoteValue);
        build_method!(rseed, Rseed);

        // Build method
        pub fn build(self) -> sapling_crypto::Note {
            sapling_crypto::Note::from_parts(
                self.recipient.unwrap(),
                self.value.unwrap(),
                self.rseed.unwrap(),
            )
        }
    }
    impl Default for LRZSaplingNoteBuilder {
        fn default() -> Self {
            LRZSaplingNoteBuilder {
                recipient: PaymentAddress::from_bytes(&[7; 43]),
                value: Some(NoteValue::from_raw(1000000)),
                rseed: Some(Rseed::AfterZip212([7; 32])),
            }
        }
    }
    /// builds a mock transparent note after all pieces are supplied
    pub struct SaplingNoteBuilder {
        diversifier: Option<sapling_crypto::Diversifier>,
        note: Option<sapling_crypto::Note>,
        witnessed_position: Option<Option<Position>>,
        output_index: Option<Option<u32>>,
        nullifier: Option<Option<sapling_crypto::Nullifier>>,
        spent: Option<Option<(TxId, u32)>>,
        unconfirmed_spent: Option<Option<(TxId, u32)>>,
        memo: Option<Option<Memo>>,
        is_change: Option<bool>,
        have_spending_key: Option<bool>,
    }

    #[allow(dead_code)] //TODO:  fix this gross hack that I tossed in to silence the language-analyzer false positive
    impl SaplingNoteBuilder {
        pub fn new() -> Self {
            Self::default()
        }

        // Methods to set each field
        build_method!(diversifier, sapling_crypto::Diversifier);
        build_method!(note, sapling_crypto::Note);
        build_method!(witnessed_position, Option<Position>);
        build_method!(output_index, Option<u32>);
        build_method!(nullifier, Option<sapling_crypto::Nullifier>);
        build_method!(spent, Option<(TxId, u32)>);
        build_method!(unconfirmed_spent, Option<(TxId, u32)>);
        build_method!(memo, Option<Memo>);
        build_method!(is_change, bool);
        build_method!(have_spending_key, bool);

        // Build method
        pub fn build(self) -> SaplingNote {
            SaplingNote::from_parts(
                self.diversifier.unwrap(),
                self.note.unwrap(),
                self.witnessed_position.unwrap(),
                self.nullifier.unwrap(),
                self.spent.unwrap(),
                self.unconfirmed_spent.unwrap(),
                self.memo.unwrap(),
                self.is_change.unwrap(),
                self.have_spending_key.unwrap(),
                self.output_index.unwrap(),
            )
        }
    }

    impl Default for SaplingNoteBuilder {
        fn default() -> Self {
            SaplingNoteBuilder {
                diversifier: Some(sapling_crypto::Diversifier([0; 11])),
                note: Some(LRZSaplingNoteBuilder::default().build()),
                witnessed_position: Some(Some(Position::from(0))),
                output_index: Some(Some(0)),
                nullifier: Some(Some(sapling_crypto::Nullifier::from_bytes([0; 32]))),
                spent: Some(None),
                unconfirmed_spent: Some(None),
                memo: Some(None),
                is_change: Some(false),
                have_spending_key: Some(true),
            }
        }
    }
}
