use std::{collections::BTreeMap, fmt::format, ops::Add};

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::{data_api::InputSource, ShieldedProtocol};
use zcash_primitives::{transaction::components::amount::NonNegativeAmount, zip32::AccountId};

use crate::{
    error::{ZingoLibError, ZingoLibResult},
    wallet::transaction_record,
};

use super::{NoteRecordIdentifier, RecordBook};

impl InputSource for RecordBook<'_> {
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
            shielded_protocol: protocol,
            index,
        };
        Ok(self.get_spendable_note_from_identifier(note_record_reference))
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
        let mut value_ref_pairs: BTreeMap<u64, NoteRecordIdentifier> = BTreeMap::new();
        for transaction_record in self
            .remote_transactions
            .values()
            .filter(|transaction_record| {
                transaction_record
                    .status
                    .is_confirmed_before_or_at(&anchor_height)
            })
        {
            if sources.contains(&ShieldedProtocol::Sapling) {
                value_ref_pairs.extend(
                    transaction_record
                        .select_value_ref_pairs_sapling()
                        .into_iter()
                        .filter(|value_ref_pair| !exclude.contains(&value_ref_pair.1)),
                );
            }
            if sources.contains(&ShieldedProtocol::Orchard) {
                value_ref_pairs.extend(
                    transaction_record
                        .select_value_ref_pairs_orchard()
                        .into_iter()
                        .filter(|value_ref_pair| !exclude.contains(&value_ref_pair.1)),
                );
            }
        }
        // let mut noteset: Vec<
        //     zcash_client_backend::wallet::ReceivedNote<
        //         Self::NoteRef,
        //         zcash_client_backend::wallet::Note,
        //     >,
        // > = Vec::new();
        // if let Some(missing_value) = value_ref_pairs.into_iter().rev(/*biggest first*/).try_fold(
        //     Some(target_value),
        //     |rolling_target, (val, noteref)| match rolling_target {
        //         Some(targ) => {
        //             noteset.push(
        //                 self.get_spendable_note_from_identifier(noteref)
        //                     .ok_or(ZingoLibError::Error("missing note".to_string()))?,
        //             );
        //             Ok(targ
        //                 - NonNegativeAmount::from_u64(val)
        //                     .map_err(|e| ZingoLibError::Error(e.to_string()))?)
        //         }
        //         None => Ok(None),
        //     },
        // )? {
        //     return ZingoLibResult::Err(ZingoLibError::Error(format!(
        //         "Insufficient sendable balance, need {} more zats",
        //         missing_value.into_u64()
        //     )));
        // };
        let noteset = value_ref_pairs
            .into_iter()
            .filter_map(|(_, identifier)| self.get_spendable_note_from_identifier(identifier))
            .collect();
        Ok(noteset) //review! this is incorrect because it selects more notes than needed if they're
                    // in the same transaction, and has no rhyme or reason for what notes it selects
    }
}
