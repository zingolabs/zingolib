use std::collections::BTreeMap;

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::{data_api::InputSource, ShieldedProtocol};
use zcash_primitives::zip32::AccountId;

use crate::error::ZingoLibError;

use super::{NoteRecordReference, RecordBook};

impl InputSource for RecordBook<'_> {
    type Error = ZingoLibError;
    type AccountId = zcash_primitives::zip32::AccountId;
    type NoteRef = NoteRecordReference;

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
        let note_record_reference: <Self as InputSource>::NoteRef = NoteRecordReference {
            txid: *txid,
            shielded_protocol: protocol,
            index,
        };
        self.get_spendable_note_from_reference(note_record_reference)
    }

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<
        Vec<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        >,
        Self::Error,
    > {
        if account != AccountId::ZERO {
            return Err(ZingoLibError::UnknownError);
        }
        let vals_refs: BTreeMap<u64, NoteRecordReference> = BTreeMap::new();
        // for transaction_record in self.record_book.all_transactions.values() {
        //     if sources.contains(&ShieldedProtocol::Sapling) {
        //         noteset.extend(transaction_record.select_unspent_domain_notes::<SaplingDomain>());
        //     }
        //     match sources.contains(&ShieldedProtocol::Orchard) {
        //         true => {
        //             noteset
        //                 .extend(transaction_record.select_unspent_domain_notes::<OrchardDomain>());
        //         }
        //         false => (),
        //     }
        // }
        let mut noteset: Vec<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        > = Vec::new();
        Ok(noteset) //review! this is incorrect because it selects ALL the unspent notes, not just enough for the target value.
    }
}
