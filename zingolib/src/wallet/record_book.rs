use std::collections::HashMap;

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::ShieldedProtocol;
use zcash_primitives::transaction::TxId;

use crate::error::ZingoLibError;

use super::transaction_record::TransactionRecord;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NoteRecordReference {
    pub txid: TxId,
    pub shielded_protocol: ShieldedProtocol,
    pub index: u32,
}

pub struct TransparentRecordRef {
    txid: TxId,
    index: u32,
}

pub struct RecordBook<'a> {
    pub all_transactions: &'a HashMap<TxId, TransactionRecord>,
}

impl RecordBook<'_> {
    pub fn get_spendable_note_from_reference(
        &self,
        note_record_reference: NoteRecordReference,
    ) -> Option<
        zcash_client_backend::wallet::ReceivedNote<
            NoteRecordReference,
            zcash_client_backend::wallet::Note,
        >,
    > {
        let transaction = self.all_transactions.get(&note_record_reference.txid);
        transaction
            .map(
                |transaction_record| match note_record_reference.shielded_protocol {
                    zcash_client_backend::ShieldedProtocol::Sapling => {
                        transaction_record
                            .get_received_note::<SaplingDomain>(note_record_reference.index)
                    }
                    zcash_client_backend::ShieldedProtocol::Orchard => {
                        transaction_record
                            .get_received_note::<OrchardDomain>(note_record_reference.index)
                    }
                },
            )
            .flatten()
    }
}

pub mod trait_inputsource;
