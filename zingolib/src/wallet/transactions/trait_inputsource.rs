use std::collections::BTreeMap;

use zcash_client_backend::{
    data_api::{InputSource, SpendableNotes},
    PoolType, ShieldedProtocol,
};
use zcash_primitives::{transaction::components::amount::NonNegativeAmount, zip32::AccountId};

use crate::error::{ZingoLibError, ZingoLibResult};

use super::{NoteRecordIdentifier, TransactionRecordMap};

impl InputSource for TransactionRecordMap {
    type Error = ZingoLibError;
    type AccountId = zcash_primitives::zip32::AccountId;
    type NoteRef = NoteRecordIdentifier;

    fn get_spendable_note(
        &self,
        txid: &zcash_primitives::transaction::TxId,
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
        let note_record_reference: <Self as InputSource>::NoteRef = NoteRecordIdentifier {
            txid: *txid,
            pool: PoolType::Shielded(protocol),
            index,
        };
        Ok(self.get_received_note_from_identifier(note_record_reference))
    }

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<SpendableNotes<NoteRecordIdentifier>, ZingoLibError> {
        todo!()
    }
}
