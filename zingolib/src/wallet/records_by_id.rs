use std::collections::HashMap;

use zcash_client_backend::data_api::InputSource;
use zcash_primitives::transaction::TxId;

use crate::error::ZingoLibError;

use crate::wallet::{data::TransactionRecord, notes::NoteRecordIdentifier};

#[derive(Debug)]
pub struct TransactionRecordsById(pub HashMap<TxId, TransactionRecord>);

impl std::ops::Deref for TransactionRecordsById {
    type Target = HashMap<TxId, TransactionRecord>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for TransactionRecordsById {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl TransactionRecordsById {
    // Associated function to create a TransactionRecordMap from a HashMap
    pub fn from_map(map: HashMap<TxId, TransactionRecord>) -> Self {
        TransactionRecordsById(map)
    }
}

impl InputSource for TransactionRecordsById {
    type Error = ZingoLibError;
    type AccountId = zcash_primitives::zip32::AccountId;
    type NoteRef = NoteRecordIdentifier;

    fn get_spendable_note(
        &self,
        _txid: &TxId,
        _protocol: zcash_client_backend::ShieldedProtocol,
        _index: u32,
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
        _account: Self::AccountId,
        _target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
        _sources: &[zcash_client_backend::ShieldedProtocol],
        _anchor_height: zcash_primitives::consensus::BlockHeight,
        _exclude: &[Self::NoteRef],
    ) -> Result<zcash_client_backend::data_api::SpendableNotes<Self::NoteRef>, Self::Error> {
        todo!()
    }
}
