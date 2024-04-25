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

/// A trait representing the capability to query a data store for unspent transaction outputs
/// belonging to a wallet.
impl InputSource for TransactionRecordsById {
    /// The type of errors produced by a wallet backend.
    /// IMPL: zingolib's error type. This could
    /// maybe more specific
    type Error = ZingoLibError;
    /// Backend-specific account identifier.
    ///
    /// An account identifier corresponds to at most a single unified spending key's worth of spend
    /// authority, such that both received notes and change spendable by that spending authority
    /// will be interpreted as belonging to that account. This might be a database identifier type
    /// or a UUID.
    /// IMPL: We only use this as much as we are
    /// forced to by the interface, zingo does
    /// not support multiple accounts at present
    type AccountId = zcash_primitives::zip32::AccountId;
    /// Backend-specific note identifier.
    ///
    /// For example, this might be a database identifier type or a UUID.
    /// IMPL: We identify notes by
    /// txid, domain, and index
    type NoteRef = ShNoteId;

    /// Fetches a spendable note by indexing into a transaction's shielded outputs for the
    /// specified shielded protocol.
    ///
    /// Returns `Ok(None)` if the note is not known to belong to the wallet or if the note
    /// is not spendable.
    /// IMPL: implemented and tested
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
                .get_received_spendable_note_from_identifier::<SaplingDomain>(note_record_reference)
                .map(|note| {
                    note.map_note(|note_inner| {
                        zcash_client_backend::wallet::Note::Sapling(note_inner)
                    })
                })),
            ShieldedProtocol::Orchard => Ok(self
                .get_received_spendable_note_from_identifier::<OrchardDomain>(note_record_reference)
                .map(|note| {
                    note.map_note(|note_inner| {
                        zcash_client_backend::wallet::Note::Orchard(note_inner)
                    })
                })),
        }
    }

    /// Returns a list of spendable notes sufficient to cover the specified target value, if
    /// possible. Only spendable notes corresponding to the specified shielded protocol will
    /// be included.
    /// IMPL: implemented and tested
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
        if let Some(missing_value_after_sapling) = sapling_note_noteref_pairs.into_iter().try_fold(
            Some(target_value),
            |rolling_target, (note, noteref)| match rolling_target {
                Some(targ) => {
                    sapling_notes.push(
                        self.get(&noteref.txid)
                            .and_then(|tr| tr.get_received_note::<SaplingDomain>(noteref.index))
                            .ok_or_else(|| ZingoLibError::Error("missing note".to_string()))?,
                    );
                    Ok(targ
                        - NonNegativeAmount::from_u64(note.value().inner())
                            .map_err(|e| ZingoLibError::Error(e.to_string()))?)
                }
                None => Ok(None),
            },
        )? {
            if let Some(missing_value_after_orchard) =
                orchard_note_noteref_pairs.into_iter().try_fold(
                    Some(missing_value_after_sapling),
                    |rolling_target, (note, noteref)| match rolling_target {
                        Some(targ) => {
                            orchard_notes.push(
                                self.get(&noteref.txid)
                                    .and_then(|tr| {
                                        tr.get_received_note::<OrchardDomain>(noteref.index)
                                    })
                                    .ok_or_else(|| {
                                        ZingoLibError::Error("missing note".to_string())
                                    })?,
                            );
                            Ok(targ
                                - NonNegativeAmount::from_u64(note.value().inner())
                                    .map_err(|e| ZingoLibError::Error(e.to_string()))?)
                        }
                        None => Ok(None),
                    },
                )?
            {
                return ZingoLibResult::Err(ZingoLibError::Error(format!(
                    "insufficient funds, short {}",
                    missing_value_after_orchard.into_u64()
                )));
            };
        };

        Ok(SpendableNotes::new(sapling_notes, orchard_notes))
    }

    /// Fetches a spendable transparent output.
    ///
    /// Returns `Ok(None)` if the UTXO is not known to belong to the wallet or is not
    /// spendable.
    /// IMPL: Implemented and tested
    fn get_unspent_transparent_output(
        &self,
        outpoint: &zcash_primitives::transaction::components::OutPoint,
    ) -> Result<Option<zcash_client_backend::wallet::WalletTransparentOutput>, Self::Error> {
        let Some((height, output)) = self.values().find_map(|transaction_record| {
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
        let value = NonNegativeAmount::from_u64(output.value)
            .map_err(|e| ZingoLibError::Error(e.to_string()))?;

        let script_pubkey = Script(output.script.clone());

        Ok(WalletTransparentOutput::from_parts(
            outpoint.clone(),
            TxOut {
                value,
                script_pubkey,
            },
            height,
        ))
    }
    /// Returns a list of unspent transparent UTXOs that appear in the chain at heights up to and
    /// including `max_height`.
    /// IMPL: Implemented and tested. address is unused, we select all outputs
    /// available to the wallet.
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
    use proptest::{prop_assert_eq, proptest};
    use zcash_client_backend::{
        data_api::{InputSource as _, SpendableNotes},
        wallet::ReceivedNote,
        ShieldedProtocol,
    };
    use zcash_primitives::{
        consensus::BlockHeight, legacy::TransparentAddress,
        transaction::components::amount::NonNegativeAmount,
    };
    use zip32::AccountId;

    use crate::wallet::{
        notes::{
            query::OutputSpendStatusQuery, transparent::mocks::TransparentOutputBuilder,
            OutputInterface as _, ShNoteId,
        },
        transaction_record::mocks::{
            nine_note_transaction_record, nine_note_transaction_record_default,
        },
        transaction_records_by_id::TransactionRecordsById,
    };

    #[test]
    fn get_spendable_note() {
        let mut transaction_records_by_id = TransactionRecordsById::new();
        transaction_records_by_id.insert_transaction_record(nine_note_transaction_record(
            100_000_000,
            200_000_000,
            400_000_000,
            100_000_000,
            200_000_000,
            400_000_000,
            100_000_000,
            200_000_000,
            400_000_000,
        ));

        let (txid, record) = transaction_records_by_id.0.iter().next().unwrap();

        for i in 0..3 {
            let single_note = transaction_records_by_id
                .get_spendable_note(txid, ShieldedProtocol::Sapling, i as u32)
                .unwrap();
            assert_eq!(
                if record.sapling_notes[i].spend_status_query(OutputSpendStatusQuery {
                    unspent: true,
                    pending_spent: false,
                    spent: false
                }) {
                    Some(zcash_client_backend::wallet::Note::Sapling(
                        record.sapling_notes[i].sapling_crypto_note.clone(),
                    ))
                } else {
                    None
                }
                .as_ref(),
                single_note.as_ref().map(ReceivedNote::note)
            )
        }
    }

    proptest! {
        #[test]
        fn select_spendable_notes( spent_val in 0..10_000_000i32,
            unspent_val in 0..10_000_000i32,
            unconf_spent_val in 0..10_000_000i32,
        ) {
            let mut transaction_records_by_id = TransactionRecordsById::new();
            transaction_records_by_id.insert_transaction_record(nine_note_transaction_record(
                spent_val as u64,
                unspent_val as u64,
                unconf_spent_val as u64,
                spent_val as u64,
                unspent_val as u64,
                unconf_spent_val as u64,
                spent_val as u64,
                unspent_val as u64,
                unconf_spent_val as u64,
            ));

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
            prop_assert_eq!(
                spendable_notes.sapling().first().unwrap().note().value(),
                transaction_records_by_id
                    .values()
                    .next()
                    .unwrap()
                    .sapling_notes
                    .iter()
                    .find(|note| {
                        note.spend_status_query(OutputSpendStatusQuery {
                            unspent: true,
                            pending_spent: false,
                            spent: false,
                        })
                    })
                    .unwrap()
                    .sapling_crypto_note
                    .value()
            )
        }
    }

    #[test]
    fn get_unspent_transparent_output() {
        let mut transaction_records_by_id = TransactionRecordsById::new();

        let transaction_record = nine_note_transaction_record_default();

        transaction_records_by_id.insert_transaction_record(transaction_record);

        let transparent_output = transaction_records_by_id
            .0
            .values()
            .next()
            .unwrap()
            .transparent_outputs
            .first()
            .unwrap();
        let record_height = transaction_records_by_id
            .0
            .values()
            .next()
            .unwrap()
            .status
            .get_confirmed_height();

        let wto = transaction_records_by_id
            .get_unspent_transparent_output(
                &TransparentOutputBuilder::default().build().to_outpoint(),
            )
            .unwrap()
            .unwrap();

        assert_eq!(wto.outpoint(), &transparent_output.to_outpoint());
        assert_eq!(wto.txout().value.into_u64(), transparent_output.value);
        assert_eq!(wto.txout().script_pubkey.0, transparent_output.script);
        assert_eq!(Some(wto.height()), record_height)
    }

    #[test]
    fn get_unspent_transparent_outputs() {
        let mut transaction_records_by_id = TransactionRecordsById::new();
        transaction_records_by_id.insert_transaction_record(nine_note_transaction_record_default());

        let transparent_output = transaction_records_by_id
            .0
            .values()
            .next()
            .unwrap()
            .transparent_outputs
            .first()
            .unwrap();
        let record_height = transaction_records_by_id
            .0
            .values()
            .next()
            .unwrap()
            .status
            .get_confirmed_height();

        let selected_outputs = transaction_records_by_id
            .get_unspent_transparent_outputs(
                &TransparentAddress::ScriptHash([0; 20]),
                BlockHeight::from_u32(10),
                &[],
            )
            .unwrap();
        assert_eq!(selected_outputs.len(), 1);
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
