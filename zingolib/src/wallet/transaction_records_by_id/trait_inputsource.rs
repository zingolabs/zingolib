//! this mod implements InputSource on TransactionRecordsById

use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::{
    data_api::{InputSource, SpendableNotes},
    wallet::{ReceivedNote, WalletTransparentOutput},
    ShieldedProtocol,
};
use zcash_primitives::{
    legacy::Script,
    transaction::{
        components::{amount::NonNegativeAmount, TxOut},
        fees::zip317::MARGINAL_FEE,
    },
};

use crate::wallet::{
    notes::{query::OutputSpendStatusQuery, OutputInterface},
    transaction_records_by_id::TransactionRecordsById,
};

// error type
use std::fmt::Debug;
use thiserror::Error;

use zcash_client_backend::wallet::NoteId;
use zcash_primitives::transaction::components::amount::BalanceError;

/// TODO: Add Doc Comment Here!
#[derive(Debug, PartialEq, Error)]
pub enum InputSourceError {
    /// TODO: Add Doc Comment Here!
    #[error("Note expected but not found: {0:?}")]
    NoteCannotBeIdentified(NoteId),
    /// TODO: Add Doc Comment Here!
    #[error(
        "An output is this wallet is believed to contain {0:?} zec. That is more than exist. {0:?}"
    )]
    /// TODO: Add Doc Comment Here!
    OutputTooBig((u64, BalanceError)),
    /// TODO: Add Doc Comment Here!
    #[error("Cannot send. Fund shortfall: {0:?}")]
    Shortfall(u64),
}

/// A trait representing the capability to query a data store for unspent transaction outputs
/// belonging to a wallet.
impl InputSource for TransactionRecordsById {
    /// The type of errors produced by a wallet backend.
    /// IMPL: zingolib's error type. This could
    /// maybe more specific
    type Error = InputSourceError;
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
    type NoteRef = NoteId;

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
        let note_record_reference: <Self as InputSource>::NoteRef =
            NoteId::new(*txid, protocol, index as u16);
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
    /// IMPL: _account skipped because Zingo uses 1 account.
    /// IMPL: all notes beneath MARGINAL_FEE skipped as dust
    fn select_spendable_notes(
        &self,
        _account: Self::AccountId,
        target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<SpendableNotes<Self::NoteRef>, Self::Error> {
        let mut sapling_note_noteref_pairs: Vec<(sapling_crypto::Note, NoteId)> = Vec::new();
        let mut orchard_note_noteref_pairs: Vec<(orchard::Note, NoteId)> = Vec::new();
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

        // TOdo! this sort order does not maximize effective use of grace inputs.
        sapling_note_noteref_pairs.sort_by_key(|sapling_note| sapling_note.0.value().inner());
        sapling_note_noteref_pairs.reverse();
        orchard_note_noteref_pairs.sort_by_key(|orchard_note| orchard_note.0.value().inner());
        orchard_note_noteref_pairs.reverse();

        let mut sapling_notes = Vec::<ReceivedNote<NoteId, sapling_crypto::Note>>::new();
        let mut orchard_notes = Vec::<ReceivedNote<NoteId, orchard::Note>>::new();
        if let Some(missing_value_after_sapling) = sapling_note_noteref_pairs.into_iter().try_fold(
            Some(target_value),
            |rolling_target, (note, note_id)| match rolling_target {
                Some(targ) if targ == NonNegativeAmount::ZERO => Ok(None),
                Some(targ) if note.value().inner() <= MARGINAL_FEE.into_u64() => Ok(Some(targ)),
                Some(targ) => {
                    sapling_notes.push(
                        self.get(note_id.txid())
                            .and_then(|tr| {
                                tr.get_received_note::<SaplingDomain>(note_id.output_index() as u32)
                            })
                            .ok_or(InputSourceError::NoteCannotBeIdentified(note_id))?,
                    );
                    Ok(targ
                        - NonNegativeAmount::from_u64(note.value().inner()).map_err(|e| {
                            InputSourceError::OutputTooBig((note.value().inner(), e))
                        })?)
                }
                None => Ok(None),
            },
        )? {
            if let Some(missing_value_after_orchard) =
                orchard_note_noteref_pairs.into_iter().try_fold(
                    Some(missing_value_after_sapling),
                    |rolling_target, (note, note_id)| match rolling_target {
                        Some(targ) if targ == NonNegativeAmount::ZERO => Ok(None),
                        Some(targ) if note.value().inner() <= MARGINAL_FEE.into_u64() => {
                            Ok(Some(targ))
                        }
                        Some(targ) => {
                            orchard_notes.push(
                                self.get(note_id.txid())
                                    .and_then(|tr| {
                                        tr.get_received_note::<OrchardDomain>(
                                            note_id.output_index() as u32,
                                        )
                                    })
                                    .ok_or(InputSourceError::NoteCannotBeIdentified(note_id))?,
                            );
                            Ok(targ
                                - NonNegativeAmount::from_u64(note.value().inner()).map_err(
                                    |e| InputSourceError::OutputTooBig((note.value().inner(), e)),
                                )?)
                        }
                        None => Ok(None),
                    },
                )?
            {
                if missing_value_after_orchard != NonNegativeAmount::ZERO {
                    return Err(InputSourceError::Shortfall(
                        missing_value_after_orchard.into_u64(),
                    ));
                }
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
                .filter(|(_height, output)| {
                    output.spend_status_query(OutputSpendStatusQuery {
                        unspent: true,
                        pending_spent: false,
                        spent: false,
                    })
                })
        }) else {
            return Ok(None);
        };
        let value = NonNegativeAmount::from_u64(output.value)
            .map_err(|e| InputSourceError::OutputTooBig((output.value, e)))?;

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
    /// IMPL: Implemented and tested. address is unused, we select all outputs available to the wallet.
    /// IMPL: _address skipped because Zingo uses 1 account.
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
                    .filter(|output| {
                        output.spend_status_query(OutputSpendStatusQuery {
                            unspent: true,
                            pending_spent: false,
                            spent: false,
                        })
                    })
                    .filter_map(move |output| {
                        let value = match NonNegativeAmount::from_u64(output.value)
                            .map_err(|e| InputSourceError::OutputTooBig((output.value, e)))
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
        wallet::{NoteId, ReceivedNote},
        ShieldedProtocol,
    };
    use zcash_primitives::{
        consensus::BlockHeight, legacy::TransparentAddress,
        transaction::components::amount::NonNegativeAmount,
    };
    use zip32::AccountId;

    use crate::wallet::{
        notes::{
            orchard::mocks::OrchardNoteBuilder, query::OutputSpendStatusQuery,
            sapling::mocks::SaplingNoteBuilder, transparent::mocks::TransparentOutputBuilder,
            OutputInterface,
        },
        transaction_record::mocks::{
            nine_note_transaction_record, nine_note_transaction_record_default,
            TransactionRecordBuilder,
        },
        transaction_records_by_id::{trait_inputsource::InputSourceError, TransactionRecordsById},
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
                unspent_val as u64,
                spent_val as u64,
                unconf_spent_val as u64,
                unspent_val as u64,
                spent_val as u64,
                unconf_spent_val as u64,
                unspent_val as u64,
                spent_val as u64,
                unconf_spent_val as u64,
            ));

            let target_value = NonNegativeAmount::const_from_u64(20_000);
            let anchor_height: BlockHeight = 10.into();
            let spendable_notes: Result<SpendableNotes<NoteId>, InputSourceError> =
                zcash_client_backend::data_api::InputSource::select_spendable_notes(
                    &transaction_records_by_id,
                    AccountId::ZERO,
                    target_value,
                    &[ShieldedProtocol::Sapling, ShieldedProtocol::Orchard],
                    anchor_height,
                    &[],
                );
            if unspent_val >= 10_000 {
                prop_assert_eq!(
                    spendable_notes.unwrap().sapling().first().unwrap().note().value(),
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
            } else {
                let Err(notes) = spendable_notes else {
                    proptest::prop_assert!(false, "should fail to select enough value");
                    panic!();
                };
                assert_eq!(
                    notes,
                    InputSourceError::Shortfall(20_000 - (2 * unspent_val as u64))
                )
            }
        }

        #[test]
        fn select_spendable_notes_2(feebits in 0..5u64) {
            let mut transaction_records_by_id = TransactionRecordsById::new();

            let transaction_record = TransactionRecordBuilder::default()
                .sapling_notes(SaplingNoteBuilder::default().value(20_000).clone())
                .orchard_notes(OrchardNoteBuilder::default().value(20_000).clone())
                .set_output_indexes()
                .build();
            transaction_records_by_id.insert_transaction_record(transaction_record);

            let target_value = NonNegativeAmount::const_from_u64(feebits * 10_000);
            let anchor_height: BlockHeight = 10.into();
            let spendable_notes_result: Result<SpendableNotes<NoteId>, InputSourceError> =
                zcash_client_backend::data_api::InputSource::select_spendable_notes(
                    &transaction_records_by_id,
                    AccountId::ZERO,
                    target_value,
                    &[ShieldedProtocol::Sapling, ShieldedProtocol::Orchard],
                    anchor_height,
                    &[],
                );
            if feebits > 4 {
                let spendable_notes_error: InputSourceError = spendable_notes_result.map(|_sn| "expected Shortfall error").unwrap_err();
                prop_assert_eq!(spendable_notes_error, InputSourceError::Shortfall(10_000));
            } else {
                let spendable_notes = spendable_notes_result.unwrap();
                let expected_notes = ((feebits + 1) / 2) as usize;
                println!("sapling notes selected: {}", spendable_notes.sapling().len());
                println!("orchard notes selected: {}", spendable_notes.orchard().len());
                prop_assert_eq!(spendable_notes.sapling().len() + spendable_notes.orchard().len(), expected_notes);
            }
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
