use std::collections::HashMap;

use zcash_client_backend::data_api::InputSource;
use zcash_primitives::transaction::TxId;

use crate::error::ZingoLibError;

use crate::wallet::{
    data::{TransactionRecord, WitnessTrees},
    notes::NoteRecordIdentifier,
};

#[derive(Debug)]
pub struct TransactionRecordMap(pub HashMap<TxId, TransactionRecord>);

impl std::ops::Deref for TransactionRecordMap {
    type Target = HashMap<TxId, TransactionRecord>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for TransactionRecordMap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl TransactionRecordMap {
    // Associated function to create a TransactionRecordMap from a HashMap
    pub fn from_map(map: HashMap<TxId, TransactionRecord>) -> Self {
        TransactionRecordMap(map)
    }
}

impl InputSource for TransactionRecordMap {
    type Error = ZingoLibError;
    type AccountId = zcash_primitives::zip32::AccountId;
    type NoteRef = NoteRecordIdentifier;

    fn get_spendable_note(
        &self,
        txid: &TxId,
        protocol: zcash_client_backend::ShieldedProtocol,
        index: u32,
    ) -> Result<
        Option<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        >,
        Self::Error,
    > {
        todo!()
    }

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<zcash_client_backend::data_api::SpendableNotes<Self::NoteRef>, Self::Error> {
        todo!()
    }
}
