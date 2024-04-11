//! Tools to facilitate mocks for testing
//! This file contains mock implementations of lrz types for unit testing with.

macro_rules! build_method {
    ($name:ident, $localtype:ty) => {
        pub fn $name(mut self, $name: $localtype) -> Self {
            self.$name = Some($name);
            self
        }
    };
}
pub(crate) use build_method;

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

#[allow(dead_code)]
pub(crate) fn mock_sapling_crypto_note() -> sapling_crypto::Note {
    sapling_note::LRZSaplingNoteBuilder::default().build()
}

#[allow(dead_code)]
pub(crate) fn mock_txid() -> zcash_primitives::transaction::TxId {
    zcash_primitives::transaction::TxId::from_bytes([7; 32])
}
