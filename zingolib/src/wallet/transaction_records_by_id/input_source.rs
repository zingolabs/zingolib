use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::{
    data_api::{InputSource, SpendableNotes},
    wallet::{ReceivedNote, WalletTransparentOutput},
    ShieldedProtocol,
};
use zcash_primitives::{
    legacy::Script,
    transaction::components::{amount::NonNegativeAmount, TxOut},
};
use zip32::AccountId;

use crate::{
    error::{ZingoLibError, ZingoLibResult},
    wallet::{notes::ShNoteId, transaction_records_by_id::TransactionRecordsById},
};

impl InputSource for TransactionRecordsById {
    type Error = ZingoLibError;
    type AccountId = zcash_primitives::zip32::AccountId;
    type NoteRef = ShNoteId;

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
        let note_record_reference: <Self as InputSource>::NoteRef = ShNoteId {
            txid: *txid,
            shpool: protocol,
            index,
        };
        match protocol {
            ShieldedProtocol::Sapling => Ok(self
                .get_received_note_from_identifier::<SaplingDomain>(note_record_reference)
                .map(|note| {
                    note.map_note(|note_inner| {
                        zcash_client_backend::wallet::Note::Sapling(note_inner)
                    })
                })),
            ShieldedProtocol::Orchard => Ok(self
                .get_received_note_from_identifier::<OrchardDomain>(note_record_reference)
                .map(|note| {
                    note.map_note(|note_inner| {
                        zcash_client_backend::wallet::Note::Orchard(note_inner)
                    })
                })),
        }
    }

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<SpendableNotes<ShNoteId>, ZingoLibError> {
        if account != AccountId::ZERO {
            return Err(ZingoLibError::Error(
                "we don't use non-zero accounts (yet?)".to_string(),
            ));
        }
        let mut sapling_note_noteref_pairs: Vec<(sapling_crypto::Note, ShNoteId)> = Vec::new();
        let mut orchard_note_noteref_pairs: Vec<(orchard::Note, ShNoteId)> = Vec::new();
        for transaction_record in self.values().filter(|transaction_record| {
            transaction_record
                .status
                .is_confirmed_before_or_at(&anchor_height)
        }) {
            if sources.contains(&ShieldedProtocol::Sapling) {
                sapling_note_noteref_pairs.extend(
                    transaction_record
                        .select_unspent_shnotes_and_ids::<SaplingDomain>()
                        .into_iter()
                        .filter(|note_ref_pair| !exclude.contains(&note_ref_pair.1)),
                );
            }
            if sources.contains(&ShieldedProtocol::Orchard) {
                orchard_note_noteref_pairs.extend(
                    transaction_record
                        .select_unspent_shnotes_and_ids::<OrchardDomain>()
                        .into_iter()
                        .filter(|note_ref_pair| !exclude.contains(&note_ref_pair.1)),
                );
            }
        }
        let mut sapling_notes = Vec::<ReceivedNote<ShNoteId, sapling_crypto::Note>>::new();
        let mut orchard_notes = Vec::<ReceivedNote<ShNoteId, orchard::Note>>::new();
        if let Some(missing_value_after_sapling) = sapling_note_noteref_pairs.into_iter().rev(/*biggest first*/).try_fold(
            Some(target_value),
            |rolling_target, (note, noteref)| match rolling_target {
                Some(targ) => {
                    sapling_notes.push(
                        self.get(&noteref.txid).and_then(|tr| tr.get_received_note::<SaplingDomain>(noteref.index))
                            .ok_or_else(|| ZingoLibError::Error("missing note".to_string()))?
                    );
                    Ok(targ
                        - NonNegativeAmount::from_u64(note.value().inner())
                            .map_err(|e| ZingoLibError::Error(e.to_string()))?)
                }
                None => Ok(None),
            },
        )? {
            if let Some(missing_value_after_orchard) = orchard_note_noteref_pairs.into_iter().rev(/*biggest first*/).try_fold(
            Some(missing_value_after_sapling),
            |rolling_target, (note, noteref)| match rolling_target {
                Some(targ) => {
                    orchard_notes.push(
                        self.get(&noteref.txid).and_then(|tr| tr.get_received_note::<OrchardDomain>(noteref.index))
                            .ok_or_else(|| ZingoLibError::Error("missing note".to_string()))?
                    );
                    Ok(targ
                        - NonNegativeAmount::from_u64(note.value().inner())
                            .map_err(|e| ZingoLibError::Error(e.to_string()))?)
                }
                None => Ok(None),
            },
        )? {
                return ZingoLibResult::Err(ZingoLibError::Error(format!(
                    "insufficient funds, short {}",
                    missing_value_after_orchard.into_u64()
                )));
            };
        };

        Ok(SpendableNotes::new(sapling_notes, orchard_notes))
    }

    fn get_unspent_transparent_output(
        &self,
        outpoint: &zcash_primitives::transaction::components::OutPoint,
    ) -> Result<Option<zcash_client_backend::wallet::WalletTransparentOutput>, Self::Error> {
        let Some((height, tnote)) = self.values().find_map(|transaction_record| {
            transaction_record
                .transparent_outputs
                .iter()
                .find_map(|output| {
                    if &output.to_outpoint() == outpoint {
                        transaction_record
                            .status
                            .get_confirmed_height()
                            .map(|height| (height, output))
                    } else {
                        None
                    }
                })
        }) else {
            return Ok(None);
        };
        let value = NonNegativeAmount::from_u64(tnote.value)
            .map_err(|e| ZingoLibError::Error(e.to_string()))?;

        let script_pubkey = Script::read(tnote.script.as_slice())
            .map_err(|e| ZingoLibError::Error(e.to_string()))?;

        Ok(WalletTransparentOutput::from_parts(
            outpoint.clone(),
            TxOut {
                value,
                script_pubkey,
            },
            height,
        ))
    }
    fn get_unspent_transparent_outputs(
        &self,
        // I don't understand what this argument is for. Is the Trait's intent to only shield
        // utxos from one address at a time? Is this needed?
        _address: &zcash_primitives::legacy::TransparentAddress,
        max_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[zcash_primitives::transaction::components::OutPoint],
    ) -> Result<Vec<zcash_client_backend::wallet::WalletTransparentOutput>, Self::Error> {
        self.values()
            .filter_map(|transaction_record| {
                transaction_record
                    .status
                    .get_confirmed_height()
                    .map(|height| (transaction_record, height))
                    .filter(|(_, height)| height <= &max_height)
            })
            .flat_map(|(transaction_record, confirmed_height)| {
                transaction_record
                    .transparent_outputs
                    .iter()
                    .filter(|output| {
                        exclude
                            .iter()
                            .all(|excluded| excluded != &output.to_outpoint())
                    })
                    .filter_map(move |output| {
                        let value = match NonNegativeAmount::from_u64(output.value)
                            .map_err(|e| ZingoLibError::Error(e.to_string()))
                        {
                            Ok(v) => v,
                            Err(e) => return Some(Err(e)),
                        };

                        let script_pubkey = Script(output.script.clone());
                        Ok(WalletTransparentOutput::from_parts(
                            output.to_outpoint(),
                            TxOut {
                                value,
                                script_pubkey,
                            },
                            confirmed_height,
                        ))
                        .transpose()
                    })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use zcash_client_backend::{
        data_api::{InputSource as _, SpendableNotes},
        ShieldedProtocol,
    };
    use zcash_primitives::{
        consensus::BlockHeight, legacy::TransparentAddress,
        transaction::components::amount::NonNegativeAmount,
    };
    use zip32::AccountId;

    use crate::wallet::{
        notes::{
            sapling::mocks::SaplingNoteBuilder, transparent::mocks::TransparentOutputBuilder,
            ShNoteId,
        },
        transaction_record::mocks::TransactionRecordBuilder,
        transaction_records_by_id::TransactionRecordsById,
    };

    #[test]
    fn sapling_note_is_selected() {
        // WIP
        let mut transaction_record = TransactionRecordBuilder::default().build();
        transaction_record
            .sapling_notes
            .push(SaplingNoteBuilder::default().build());

        let mut transaction_records_by_id = TransactionRecordsById::new();
        transaction_records_by_id.insert(transaction_record.txid, transaction_record);

        let target_value = NonNegativeAmount::const_from_u64(20000);
        let anchor_height: BlockHeight = 10.into();
        let spendable_notes: SpendableNotes<ShNoteId> =
            zcash_client_backend::data_api::InputSource::select_spendable_notes(
                &transaction_records_by_id,
                AccountId::ZERO,
                target_value,
                &[ShieldedProtocol::Sapling, ShieldedProtocol::Orchard],
                anchor_height,
                &[],
            )
            .unwrap();
        assert_eq!(
            spendable_notes
                .sapling()
                .first()
                .unwrap()
                .note()
                .value()
                .inner(),
            // Default mock sapling note value
            1_000_000
        )
    }

    #[test]
    fn select_transparent_outputs() {
        let mut transaction_record = TransactionRecordBuilder::default().build();
        let transparent_output = TransparentOutputBuilder::default().build();
        transaction_record
            .transparent_outputs
            .push(transparent_output.clone());
        let record_height = transaction_record.status.get_confirmed_height();
        let mut transaction_records_by_id = TransactionRecordsById::new();
        transaction_records_by_id.insert_transaction_record(transaction_record);

        let selected_outputs = transaction_records_by_id
            .get_unspent_transparent_outputs(
                &TransparentAddress::ScriptHash([0; 20]),
                BlockHeight::from_u32(10),
                &[],
            )
            .unwrap();
        assert_eq!(
            selected_outputs.first().unwrap().outpoint(),
            &transparent_output.to_outpoint()
        );
        assert_eq!(
            selected_outputs.first().unwrap().txout().value.into_u64(),
            transparent_output.value
        );
        assert_eq!(
            selected_outputs.first().unwrap().txout().script_pubkey.0,
            transparent_output.script
        );
        assert_eq!(
            Some(selected_outputs.first().unwrap().height()),
            record_height
        )
    }
}
