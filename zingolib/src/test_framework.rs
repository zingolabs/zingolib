use crate::wallet::traits::FromBytes;
pub(crate) mod macros;

use incrementalmerkletree::Position;
use zcash_primitives::{memo::Memo, transaction::TxId};

macro_rules! build_method {
    ($name:ident, $localtype:ty) => {
        pub fn $name(&mut self, $name: $localtype) -> &mut Self {
            self.$name = Some($name);
            self
        }
    };
}
use crate::wallet::notes::{ShieldedNoteInterface, TransparentNote};
pub struct TransparentNoteBuilder {
    address: Option<String>,
    txid: Option<TxId>,
    output_index: Option<u64>,
    script: Option<Vec<u8>>,
    value: Option<u64>,
    spent_at_height: Option<Option<i32>>,
    spent: Option<Option<TxId>>,
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
    build_method!(spent_at_height, Option<i32>); //  TODO:  WHY IS THIS AN i32?!
    build_method!(spent, Option<TxId>);
    build_method!(unconfirmed_spent, Option<(TxId, u32)>);

    // Build method
    pub fn build(self) -> TransparentNote {
        TransparentNote {
            address: self.address.unwrap(),
            txid: self.txid.unwrap(),
            output_index: self.output_index.unwrap(),
            script: self.script.unwrap(),
            value: self.value.unwrap(),
            spent_at_height: self.spent_at_height.unwrap(),
            spent: self.spent.unwrap(),
            unconfirmed_spent: self.unconfirmed_spent.unwrap(),
        }
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
            spent_at_height: Some(None),
            spent: Some(None),
            unconfirmed_spent: Some(None),
        }
    }
}

pub struct ShieldedNoteBuilder<T: ShieldedNoteInterface> {
    diversifier: Option<T::Diversifier>,
    note: Option<T::Note>,
    position_of_commitment_to_witness: Option<Option<Position>>,
    nullifier: Option<Option<T::Nullifier>>,
    spent: Option<Option<(TxId, u32)>>,
    unconfirmed_spent: Option<Option<(TxId, u32)>>,
    memo: Option<Option<Memo>>,
    is_change: Option<bool>,
    have_spending_key: Option<bool>,
    output_index: Option<Option<u32>>,
}

impl<T: ShieldedNoteInterface> ShieldedNoteBuilder<T> {
    pub fn new() -> Self {
        Self::default()
    }
    // Methods to set each field
    build_method!(diversifier, T::Diversifier);
    build_method!(note, T::Note);
    build_method!(position_of_commitment_to_witness, Option<Position>);
    build_method!(nullifier, Option<T::Nullifier>);
    build_method!(spent, Option<(TxId, u32)>);
    build_method!(unconfirmed_spent, Option<(TxId, u32)>);
    build_method!(memo, Option<Memo>);
    build_method!(is_change, bool);
    build_method!(have_spending_key, bool);
    build_method!(output_index, Option<u32>);

    // Build method
    pub fn build(self) -> T {
        ShieldedNoteInterface::from_parts(
            self.diversifier.unwrap(),
            self.note.unwrap(),
            self.position_of_commitment_to_witness.unwrap(),
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

impl<T: ShieldedNoteInterface> Default for ShieldedNoteBuilder<T> {
    fn default() -> Self {
        Self {
            diversifier: Some(T::Diversifier::from_bytes([0; 11])),
            note: None,
            position_of_commitment_to_witness: Some(None),
            nullifier: Some(None),
            spent: Some(None),
            unconfirmed_spent: Some(None),
            memo: Some(None),
            is_change: Some(false),
            have_spending_key: Some(false),
            output_index: Some(None),
        }
    }
}
