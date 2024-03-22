use std::collections::BTreeMap;

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::{data_api::InputSource, ShieldedProtocol};
use zcash_primitives::{transaction::components::amount::NonNegativeAmount, zip32::AccountId};

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
            return Err(ZingoLibError::Error(
                "we don't use non-zero accounts (yet?)".to_string(),
            ));
        }
        let vals_refs: BTreeMap<u64, NoteRecordReference> = BTreeMap::new();
        let mut noteset: Vec<
            zcash_client_backend::wallet::ReceivedNote<
                Self::NoteRef,
                zcash_client_backend::wallet::Note,
            >,
        > = Vec::new();
        let mut value_selected = NonNegativeAmount::ZERO;
        for transaction_record in self.all_transactions.values() {
            if sources.contains(&ShieldedProtocol::Sapling) {
                noteset.extend(transaction_record.select_unspent_domain_notes::<SaplingDomain>());
                value_selected = (value_selected
                    + NonNegativeAmount::from_u64(transaction_record.total_sapling_value_spent)
                        .map_err(|e| ZingoLibError::Error("Balance overflow".to_string()))?)
                .ok_or(ZingoLibError::Error("Balance overflow".to_string()))?;
            }
            match sources.contains(&ShieldedProtocol::Orchard) {
                true => {
                    noteset
                        .extend(transaction_record.select_unspent_domain_notes::<OrchardDomain>());
                    value_selected = (value_selected
                        + NonNegativeAmount::from_u64(
                            transaction_record.total_orchard_value_spent,
                        )
                        .map_err(|e| ZingoLibError::Error("Balance overflow".to_string()))?)
                    .ok_or(ZingoLibError::Error("Balance overflow".to_string()))?;
                }
                false => (),
            }
            //review! select notes in some sort of sane, non-arbitrary order, instead of
            //adding all notes from a random? transaction and the stopping if we have enough
            if value_selected >= target_value {
                break;
            }
        }
        Ok(noteset) //review! this is incorrect because it selects more notes than needed if they're
                    // in the same transaction, and has no rhyme or reason for what notes it selects
    }
}
