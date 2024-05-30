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
    transaction::components::{amount::NonNegativeAmount, TxOut},
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

/// Error type used by InputSource trait
#[derive(Debug, PartialEq, Error)]
pub enum InputSourceError {
    /// #[error("Note expected but not found: {0:?}")]
    #[error("Note expected but not found: {0:?}")]
    NoteCannotBeIdentified(NoteId),
    /// #[error("An output is this wallet is believed to contain {0:?} zec. That is more than exist. {0:?}")]
    #[error(
        "An output is this wallet is believed to contain {0:?} zec. That is more than exist. {0:?}"
    )]
    OutputTooBig((u64, BalanceError)),
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

    /// the trait method below is used as a TxMapAndMaybeTrees trait method by propose_transaction.
    /// this function is used inside a loop that calculates a fee and balances change
    /// this algorithm influences strategy for user fee minimization
    /// see [crate::lightclient::LightClient::create_send_proposal]
    /// TRAIT DOCUMENTATION
    /// Returns a list of spendable notes sufficient to cover the specified target value, if
    /// possible. Only spendable notes corresponding to the specified shielded protocol will
    /// be included.
    /// IMPLEMENTATION DETAILS
    /// account skipped because Zingo uses 1 account.
    /// the algorithm to pick notes is summarized below
    /// 1) first, sort all eligible notes by value
    /// 2) pick the smallest that overcomes the target value and return it
    /// 3) if you cant, add the biggest to the selection and return to step 2 to pick another
    fn select_spendable_notes(
        &self,
        _account: Self::AccountId,
        target_value: zcash_primitives::transaction::components::amount::NonNegativeAmount,
        sources: &[zcash_client_backend::ShieldedProtocol],
        anchor_height: zcash_primitives::consensus::BlockHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<SpendableNotes<Self::NoteRef>, Self::Error> {
        let mut unselected =
            self.get_spendable_note_ids_and_values(sources, anchor_height, exclude);

        unselected.sort_by_key(|(_id, value)| *value);

        let mut selected = vec![];

        let mut index_of_unselected = 0;
        loop {
            if unselected.is_empty() {
                // all notes are selected. we pass the max value onwards whether we have reached target or not
                break;
            }
            match unselected.get(index_of_unselected) {
                None => {
                    // the iterator went off the end of the vector without finding a note big enough to complete the transaction... add the biggest note and reset the iteraton
                    selected.push(unselected.pop().expect("nonempty"));
                    index_of_unselected = 0;
                    continue;
                }
                Some(smallest_unselected) => {
                    // selected a note to test if it has enough value to complete the transaction on its own
                    if smallest_unselected.1
                        >= target_value.into_u64()
                            - selected.iter().fold(0, |sum, (_id, value)| sum + *value)
                    {
                        selected.push(*smallest_unselected);
                        unselected.remove(index_of_unselected);
                        break;
                    } else {
                        // this note is not big enough. try the next
                        index_of_unselected += 1;
                    }
                }
            }
        }

        if selected.len() < 2 {
            // since we maxed out the target value with only one note, we have an option to grace a dust note.
            // we will simply rescue the smallest note
            unselected.reverse();
            if let Some(smallest_note) = unselected.pop() {
                selected.push(smallest_note);
            }
        }

        let mut selected_sapling = Vec::<ReceivedNote<NoteId, sapling_crypto::Note>>::new();
        let mut selected_orchard = Vec::<ReceivedNote<NoteId, orchard::Note>>::new();

        // transform each NoteId to a ReceivedNote
        selected.iter().try_for_each(|(id, _value)| {
            let opt_transaction_record = self.get(id.txid());
            let output_index = id.output_index() as u32;
            match id.protocol() {
                zcash_client_backend::ShieldedProtocol::Sapling => opt_transaction_record
                    .and_then(|transaction_record| {
                        transaction_record.get_received_note::<SaplingDomain>(output_index)
                    })
                    .map(|received_note| {
                        selected_sapling.push(received_note);
                    }),
                zcash_client_backend::ShieldedProtocol::Orchard => opt_transaction_record
                    .and_then(|transaction_record| {
                        transaction_record.get_received_note::<OrchardDomain>(output_index)
                    })
                    .map(|received_note| {
                        selected_orchard.push(received_note);
                    }),
            }
            .ok_or(InputSourceError::NoteCannotBeIdentified(*id))
        })?;

        Ok(SpendableNotes::new(selected_sapling, selected_orchard))
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
        data_api::InputSource as _, wallet::ReceivedNote, ShieldedProtocol,
    };
    use zcash_primitives::{
        consensus::BlockHeight, legacy::TransparentAddress,
        transaction::components::amount::NonNegativeAmount,
    };
    use zip32::AccountId;

    use crate::wallet::{
        notes::{
            query::OutputSpendStatusQuery, transparent::mocks::TransparentOutputBuilder,
            OutputInterface,
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
        fn select_spendable_notes( sapling_value in 0..10_000_000u32,
            orchard_value in 0..10_000_000u32,
            target_value in 0..10_000_000u32,
        ) {
            let mut transaction_records_by_id = TransactionRecordsById::new();
            transaction_records_by_id.insert_transaction_record(nine_note_transaction_record(
                1_000_000_u64,
                1_000_000_u64,
                1_000_000_u64,
                sapling_value as u64,
                1_000_000_u64,
                1_000_000_u64,
                orchard_value as u64,
                1_000_000_u64,
                1_000_000_u64,
            ));

            let target_amount = NonNegativeAmount::const_from_u64(target_value as u64);
            let anchor_height: BlockHeight = 10.into();
            let spendable_notes =
                zcash_client_backend::data_api::InputSource::select_spendable_notes(
                    &transaction_records_by_id,
                    AccountId::ZERO,
                    target_amount,
                    &[ShieldedProtocol::Sapling, ShieldedProtocol::Orchard],
                    anchor_height,
                    &[],
                ).unwrap();
            prop_assert_eq!(spendable_notes.sapling().len() + spendable_notes.orchard().len(), 2);
        }
        #[test]
        fn select_spendable_notes_2( sapling_value in 0..10_000_000u32,
            orchard_value in 0..10_000_000u32,
            target_value in 0..10_000_000u32,
        ) {
            let mut transaction_records_by_id = TransactionRecordsById::new();
            transaction_records_by_id.insert_transaction_record(nine_note_transaction_record(
                1_000_000_u64,
                1_000_000_u64,
                1_000_000_u64,
                sapling_value as u64,
                1_000_000_u64,
                1_000_000_u64,
                orchard_value as u64,
                1_000_000_u64,
                1_000_000_u64,
            ));

            let target_amount = NonNegativeAmount::const_from_u64(target_value as u64);
            let anchor_height: BlockHeight = 10.into();
            let spendable_notes =
                zcash_client_backend::data_api::InputSource::select_spendable_notes(
                    &transaction_records_by_id,
                    AccountId::ZERO,
                    target_amount,
                    &[ShieldedProtocol::Sapling, ShieldedProtocol::Orchard],
                    anchor_height,
                    &[],
                ).unwrap();
            let expected_len = if target_value > std::cmp::max(sapling_value, orchard_value) {2} else {1};
            prop_assert_eq!(spendable_notes.sapling().len() + spendable_notes.orchard().len(), expected_len);
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
