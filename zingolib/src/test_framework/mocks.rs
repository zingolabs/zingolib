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

use rand::{rngs::OsRng, Rng};
use sapling_crypto::{
    note_encryption::PreparedIncomingViewingKey, zip32::ExtendedSpendingKey, PaymentAddress,
};

pub(crate) fn get_random_zaddr() -> (
    ExtendedSpendingKey,
    PreparedIncomingViewingKey,
    PaymentAddress,
) {
    let mut rng = OsRng;
    let mut seed = [0u8; 32];
    rng.fill(&mut seed);

    let extsk = ExtendedSpendingKey::master(&seed);
    let dfvk = extsk.to_diversifiable_full_viewing_key();
    let fvk = dfvk;
    let (_, addr) = fvk.default_address();

    (
        extsk,
        PreparedIncomingViewingKey::new(&fvk.fvk().vk.ivk()),
        addr,
    )
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

    impl LRZSaplingNoteBuilder {
        pub fn new() -> Self {
            LRZSaplingNoteBuilder {
                recipient: None,
                value: None,
                rseed: None,
            }
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
            let (_, _, address) = super::get_random_zaddr();
            Self::new()
                .recipient(address)
                .value(NoteValue::from_raw(1000000))
                .rseed(Rseed::AfterZip212([7; 32]))
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
