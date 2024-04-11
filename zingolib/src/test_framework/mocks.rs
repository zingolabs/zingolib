//! Tools to facilitate mocks for testing
//! This file contains mock implementations of lrz types for unit testing with.
use zcash_primitives::transaction::TxId;

macro_rules! build_method {
    ($name:ident, $localtype:ty) => {
        pub fn $name(mut self, $name: $localtype) -> Self {
            self.$name = Some($name);
            self
        }
    };
}
pub(crate) use build_method;

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
mod sapling_note {

    use sapling_crypto::value::NoteValue;
    use sapling_crypto::PaymentAddress;
    use sapling_crypto::Rseed;

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
}

pub(crate) fn mock_sapling_crypto_note() -> sapling_crypto::Note {
    sapling_note::LRZSaplingNoteBuilder::default().build()
}

pub(crate) fn mock_txid() -> TxId {
    zcash_primitives::transaction::TxId::from_bytes([7; 32])
}
