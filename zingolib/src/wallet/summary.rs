//! Types and impls for conveniently displaying information to the user or converting to JSON for interfacing with a larger stack.
use std::collections::HashSet;
/// A "snapshot" of the state of the items in the wallet at the time the summary was constructed.
/// Not to be used for internal logic in the system.
use std::{cmp::Ordering, collections::HashMap};

use zcash_protocol::PoolType;
use zcash_protocol::memo::Memo;

use pepper_sync::keys::transparent;
use pepper_sync::wallet::{
    KeyIdInterface, NoteInterface, OutgoingNoteInterface, OutputInterface, TransparentCoin,
};

use super::LightWallet;
use super::error::{KeyError, SummaryError};
use data::finsight::{
    TotalMemoBytesToAddress, TotalSendsToAddress, TotalValueToAddress, ValuesSentToAddress,
};
use data::{
    BasicCoinSummary, BasicNoteSummary, CoinSummary, NoteSummaries, NoteSummary,
    OutgoingCoinSummary, OutgoingNoteSummary, Scope, SelfSendValueTransfer, SendType,
    SentValueTransfer, TransactionKind, TransactionSummaries, TransactionSummary, ValueTransfer,
    ValueTransferKind, ValueTransfers,
};

pub mod data;

impl LightWallet {
    /// Returns summaries of all transactions in the wallet, sorted by confirmation status (confirmed first) and then
    /// blockheight (lowest first).
    ///
    /// `reverse_sort` will sort by unconfirmed first and then highest first.
    pub async fn transaction_summaries(
        &self,
        reverse_sort: bool,
    ) -> Result<TransactionSummaries, SummaryError> {
        let mut transaction_summaries = self
            .wallet_transactions
            .values()
            .map(|transaction| {
                let kind = self.transaction_kind(transaction)?;
                let value = match kind {
                    TransactionKind::Received | TransactionKind::Sent(SendType::Shield) => {
                        transaction.total_value_received()
                    }
                    TransactionKind::Sent(SendType::Send | SendType::SendToSelf) => {
                        transaction.total_value_sent()
                    }
                };
                let fee: Option<u64> = self
                    .calculate_transaction_fee(transaction)
                    .ok()
                    .map(zcash_protocol::value::Zatoshis::into_u64);
                let orchard_notes = transaction
                    .orchard_notes()
                    .iter()
                    .map(|output| {
                        let spend_status = self.output_spend_status(output);

                        let memo = if let Memo::Text(memo_text) = output.memo() {
                            Some(memo_text.to_string())
                        } else {
                            None
                        };

                        BasicNoteSummary::from_parts(
                            output.value(),
                            spend_status,
                            output.output_id().output_index(),
                            memo,
                        )
                    })
                    .collect::<Vec<_>>();
                let sapling_notes = transaction
                    .sapling_notes()
                    .iter()
                    .map(|output| {
                        let spend_status = self.output_spend_status(output);

                        let memo = if let Memo::Text(memo_text) = output.memo() {
                            Some(memo_text.to_string())
                        } else {
                            None
                        };

                        BasicNoteSummary::from_parts(
                            output.value(),
                            spend_status,
                            output.output_id().output_index(),
                            memo,
                        )
                    })
                    .collect::<Vec<_>>();
                let transparent_coins = transaction
                    .transparent_coins()
                    .iter()
                    .map(|output| {
                        let spend_status = self.output_spend_status(output);

                        BasicCoinSummary::from_parts(
                            output.value(),
                            spend_status,
                            output.output_id().output_index(),
                        )
                    })
                    .collect::<Vec<_>>();

                let outgoing_orchard_notes = transaction
                    .outgoing_orchard_notes()
                    .iter()
                    .map(|note| {
                        let memo = if let Memo::Text(memo_text) = note.memo() {
                            Some(memo_text.to_string())
                        } else {
                            None
                        };

                        Ok(OutgoingNoteSummary {
                            memo,
                            value: note.value(),
                            recipient: note
                                .encoded_recipient(&self.chain_type)
                                .map_err(zcash_address::ParseError::Unified)?,
                            recipient_unified_address: note
                                .encoded_recipient_full_unified_address(&self.chain_type),
                            output_index: note.output_id().output_index(),
                            account_id: note.key_id().account_id,
                            scope: Scope::from(note.key_id().scope),
                        })
                    })
                    .collect::<Result<Vec<_>, SummaryError>>()?;
                let outgoing_sapling_notes = transaction
                    .outgoing_sapling_notes()
                    .iter()
                    .map(|note| {
                        let memo = if let Memo::Text(memo_text) = note.memo() {
                            Some(memo_text.to_string())
                        } else {
                            None
                        };

                        OutgoingNoteSummary {
                            output_index: note.output_id().output_index(),
                            memo,
                            value: note.value(),
                            recipient: note
                                .encoded_recipient(&self.chain_type)
                                .expect("infallible"),
                            recipient_unified_address: note
                                .encoded_recipient_full_unified_address(&self.chain_type),
                            account_id: note.key_id().account_id,
                            scope: Scope::from(note.key_id().scope),
                        }
                    })
                    .collect::<Vec<_>>();
                let outgoing_transparent_coins = if kind == TransactionKind::Received {
                    Vec::new()
                } else {
                    transaction
                        .transaction()
                        .transparent_bundle()
                        .map_or(Vec::new(), |bundle| {
                            bundle
                                .vout
                                .iter()
                                .enumerate()
                                .filter_map(|(output_index, transparent_output)| {
                                    transparent_output.recipient_address().map(|address| {
                                        OutgoingCoinSummary {
                                            value: transparent_output.value().into_u64(),
                                            recipient: transparent::encode_address(
                                                &self.chain_type,
                                                address,
                                            ),
                                            output_index: output_index
                                                .try_into()
                                                .expect("output index should be valid u32"),
                                        }
                                    })
                                })
                                .collect()
                        })
                };

                // add price to transaction summary
                // takes price from the day of transaction's datetime. otherwise, current price.
                // TODO: historical prices currently unimplemented
                // let mut price = None;
                // for daily_price in self.price_list.daily_prices() {
                //     if daily_price.time > transaction.datetime() {
                //         assert!(daily_price.time - transaction.datetime() < 24 * 60 * 60);
                //         price = Some(daily_price.price_usd);
                //         break;
                //     }
                // }
                // if price.is_none() {
                //     price = self.price_list.current_price().and_then(|current_price| {
                //         if transaction.datetime() <= current_price.time
                //             && transaction.datetime() > current_price.time - 2 * 24 * 60 * 60
                //         // exchange APIs may start daily prices 2 days back
                //         {
                //             Some(current_price.price_usd)
                //         } else {
                //             None
                //         }
                //     });
                // }

                Ok(TransactionSummary {
                    txid: transaction.txid(),
                    datetime: transaction.datetime(),
                    status: transaction.status(),
                    blockheight: transaction.status().get_height(),
                    kind,
                    value,
                    fee,
                    zec_price: None,
                    orchard_notes,
                    sapling_notes,
                    transparent_coins,
                    outgoing_orchard_notes,
                    outgoing_sapling_notes,
                    outgoing_transparent_coins,
                })
            })
            .collect::<Result<Vec<_>, SummaryError>>()?;

        transaction_summaries.sort_by(|summary_a, summary_b| {
            match summary_a.status.cmp(&summary_b.status) {
                Ordering::Equal => {
                    // TODO: order tex transactions correctly by checking inputs / outputs are the wallet's refund addresses
                    summary_a.txid.cmp(&summary_b.txid)
                }
                otherwise => otherwise,
            }
        });

        if reverse_sort {
            transaction_summaries.reverse();
        }

        Ok(TransactionSummaries::new(transaction_summaries))
    }

    /// Provides a list of value transfers related to this capability.
    /// A value transfer is a group of all notes to a specific receiver in a transaction.
    pub async fn value_transfers(
        &self,
        sort_highest_to_lowest: bool,
    ) -> Result<ValueTransfers, SummaryError> {
        let mut value_transfers: Vec<ValueTransfer> = Vec::new();
        let transaction_summaries = self.transaction_summaries(sort_highest_to_lowest).await?.0;

        for transaction in transaction_summaries {
            match transaction.kind {
                TransactionKind::Sent(SendType::Send) => {
                    // create 1 sent value transfer for each non-self recipient address
                    // if recipient_ua is available it overrides recipient_address
                    value_transfers.append(&mut self.create_send_value_transfers(&transaction)?);

                    // create 1 memo-to-self if any number of memos are received in the sending transaction
                    if transaction
                        .orchard_notes
                        .iter()
                        .any(|note| note.memo.is_some())
                        || transaction
                            .sapling_notes
                            .iter()
                            .any(|note| note.memo.is_some())
                    {
                        let memos: Vec<String> = transaction
                            .orchard_notes
                            .iter()
                            .filter_map(|note| note.memo.clone())
                            .chain(
                                transaction
                                    .sapling_notes
                                    .iter()
                                    .filter_map(|note| note.memo.clone()),
                            )
                            .collect();
                        value_transfers.push(ValueTransfer {
                            txid: transaction.txid,
                            datetime: transaction.datetime,
                            status: transaction.status,
                            blockheight: transaction.blockheight,
                            transaction_fee: transaction.fee,
                            zec_price: transaction.zec_price,
                            kind: ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                SelfSendValueTransfer::MemoToSelf,
                            )),
                            value: 0,
                            recipient_address: None,
                            pool_received: None,
                            memos,
                        });
                    }
                }
                TransactionKind::Sent(SendType::Shield) => {
                    // create 1 shielding value transfer for each pool shielded to
                    if !transaction.orchard_notes.is_empty() {
                        let value: u64 = transaction
                            .orchard_notes
                            .iter()
                            .map(|output| output.value)
                            .sum();
                        let memos: Vec<String> = transaction
                            .orchard_notes
                            .iter()
                            .filter_map(|note| note.memo.clone())
                            .collect();
                        value_transfers.push(ValueTransfer {
                            txid: transaction.txid,
                            datetime: transaction.datetime,
                            status: transaction.status,
                            blockheight: transaction.blockheight,
                            transaction_fee: transaction.fee,
                            zec_price: transaction.zec_price,
                            kind: ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                SelfSendValueTransfer::Shield,
                            )),
                            value,
                            recipient_address: None,
                            pool_received: Some(PoolType::ORCHARD.to_string()),
                            memos,
                        });
                    }
                    if !transaction.sapling_notes.is_empty() {
                        let value: u64 = transaction
                            .sapling_notes
                            .iter()
                            .map(|output| output.value)
                            .sum();
                        let memos: Vec<String> = transaction
                            .sapling_notes
                            .iter()
                            .filter_map(|note| note.memo.clone())
                            .collect();
                        value_transfers.push(ValueTransfer {
                            txid: transaction.txid,
                            datetime: transaction.datetime,
                            status: transaction.status,
                            blockheight: transaction.blockheight,
                            transaction_fee: transaction.fee,
                            zec_price: transaction.zec_price,
                            kind: ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                SelfSendValueTransfer::Shield,
                            )),
                            value,
                            recipient_address: None,
                            pool_received: Some(PoolType::SAPLING.to_string()),
                            memos,
                        });
                    }
                }
                TransactionKind::Sent(SendType::SendToSelf) => {
                    // create 1 memo-to-self if a sending transaction receives any number of memos
                    // otherwise, create 1 send-to-self value transfer so every transaction creates at least 1 value transfer
                    // eventually we may replace send-to-self with a range of kinds such as deshield and migrate etc.
                    if transaction
                        .orchard_notes
                        .iter()
                        .any(|note| note.memo.is_some())
                        || transaction
                            .sapling_notes
                            .iter()
                            .any(|note| note.memo.is_some())
                    {
                        let memos: Vec<String> = transaction
                            .orchard_notes
                            .iter()
                            .filter_map(|note| note.memo.clone())
                            .chain(
                                transaction
                                    .sapling_notes
                                    .iter()
                                    .filter_map(|note| note.memo.clone()),
                            )
                            .collect();
                        value_transfers.push(ValueTransfer {
                            txid: transaction.txid,
                            datetime: transaction.datetime,
                            status: transaction.status,
                            blockheight: transaction.blockheight,
                            transaction_fee: transaction.fee,
                            zec_price: transaction.zec_price,
                            kind: ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                SelfSendValueTransfer::MemoToSelf,
                            )),
                            value: 0,
                            recipient_address: None,
                            pool_received: None,
                            memos,
                        });
                    } else {
                        value_transfers.push(ValueTransfer {
                            txid: transaction.txid,
                            datetime: transaction.datetime,
                            status: transaction.status,
                            blockheight: transaction.blockheight,
                            transaction_fee: transaction.fee,
                            zec_price: transaction.zec_price,
                            kind: ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                SelfSendValueTransfer::Basic,
                            )),
                            value: 0,
                            recipient_address: None,
                            pool_received: None,
                            memos: Vec::new(),
                        });
                    }

                    // in the case Zennies For Zingo! is active
                    value_transfers.append(&mut self.create_send_value_transfers(&transaction)?);
                }
                TransactionKind::Received => {
                    // create 1 received value transfer for each pool received to
                    if !transaction.orchard_notes.is_empty() {
                        let value: u64 = transaction
                            .orchard_notes
                            .iter()
                            .map(|output| output.value)
                            .sum();
                        let memos: Vec<String> = transaction
                            .orchard_notes
                            .iter()
                            .filter_map(|note| note.memo.clone())
                            .collect();
                        value_transfers.push(ValueTransfer {
                            txid: transaction.txid,
                            datetime: transaction.datetime,
                            status: transaction.status,
                            blockheight: transaction.blockheight,
                            transaction_fee: transaction.fee,
                            zec_price: transaction.zec_price,
                            kind: ValueTransferKind::Received,
                            value,
                            recipient_address: None,
                            pool_received: Some(PoolType::ORCHARD.to_string()),
                            memos,
                        });
                    }
                    if !transaction.sapling_notes.is_empty() {
                        let value: u64 = transaction
                            .sapling_notes
                            .iter()
                            .map(|output| output.value)
                            .sum();
                        let memos: Vec<String> = transaction
                            .sapling_notes
                            .iter()
                            .filter_map(|note| note.memo.clone())
                            .collect();
                        value_transfers.push(ValueTransfer {
                            txid: transaction.txid,
                            datetime: transaction.datetime,
                            status: transaction.status,
                            blockheight: transaction.blockheight,
                            transaction_fee: transaction.fee,
                            zec_price: transaction.zec_price,
                            kind: ValueTransferKind::Received,
                            value,
                            recipient_address: None,
                            pool_received: Some(PoolType::SAPLING.to_string()),
                            memos,
                        });
                    }
                    if !transaction.transparent_coins.is_empty() {
                        let value: u64 = transaction
                            .transparent_coins
                            .iter()
                            .map(|output| output.value)
                            .sum();
                        value_transfers.push(ValueTransfer {
                            txid: transaction.txid,
                            datetime: transaction.datetime,
                            status: transaction.status,
                            blockheight: transaction.blockheight,
                            transaction_fee: transaction.fee,
                            zec_price: transaction.zec_price,
                            kind: ValueTransferKind::Received,
                            value,
                            recipient_address: None,
                            pool_received: Some(PoolType::TRANSPARENT.to_string()),
                            memos: Vec::new(),
                        });
                    }
                }
            }
        }

        Ok(ValueTransfers::new(value_transfers))
    }

    #[must_use]
    pub fn note_summaries<N>(&self, include_spent_notes: bool) -> NoteSummaries
    where
        N: NoteInterface<KeyId = pepper_sync::keys::KeyId>,
    {
        let note_summaries = self
            .wallet_outputs::<N>()
            .into_iter()
            .filter(|&note| {
                if include_spent_notes {
                    true
                } else {
                    note.spending_transaction().is_none()
                }
            })
            .map(|note| {
                let memo = if let Memo::Text(memo_text) = note.memo() {
                    Some(memo_text.to_string())
                } else {
                    None
                };
                let transaction = self.output_transaction(note);

                NoteSummary {
                    value: note.value(),
                    status: transaction.status(),
                    block_height: transaction.status().get_height(),
                    spend_status: self.output_spend_status(note),
                    memo,
                    time: transaction.datetime(),
                    txid: note.output_id().txid(),
                    output_index: note.output_id().output_index(),
                    account_id: note.key_id().account_id,
                    scope: note.key_id().scope.into(),
                }
            })
            .collect();

        NoteSummaries::new(note_summaries)
    }

    #[must_use]
    pub fn coin_summaries(&self, include_spent_coins: bool) -> Vec<CoinSummary> {
        self.wallet_outputs::<TransparentCoin>()
            .into_iter()
            .filter(|&coin| {
                if include_spent_coins {
                    true
                } else {
                    coin.spending_transaction().is_none()
                }
            })
            .map(|coin| {
                let transaction = self.output_transaction(coin);

                CoinSummary {
                    value: coin.value(),
                    status: transaction.status(),
                    block_height: transaction.status().get_height(),
                    spend_status: self.output_spend_status(coin),
                    time: transaction.datetime(),
                    txid: coin.output_id().txid(),
                    output_index: coin.output_id().output_index(),
                    account_id: coin.key_id().account_id(),
                    scope: coin.key_id().scope(),
                    address_index: coin.key_id().address_index().index(),
                }
            })
            .collect()
    }

    /// Provides a list of `ValueTransfers` associated with the sender, or containing the string.
    pub async fn messages_containing(
        &self,
        filter: Option<&str>,
    ) -> Result<ValueTransfers, SummaryError> {
        let mut value_transfers = self.value_transfers(true).await?;
        value_transfers.reverse();

        // Filter out VTs where all memos are empty.
        value_transfers.retain(|vt| vt.memos.iter().any(|memo| !memo.is_empty()));

        match filter {
            Some(s) => {
                value_transfers.retain(|vt| {
                    if vt.memos.is_empty() {
                        return false;
                    }

                    if vt.recipient_address == Some(s.to_string()) {
                        true
                    } else {
                        for memo in &vt.memos {
                            if memo.contains(s) {
                                return true;
                            }
                        }
                        false
                    }
                });
            }
            None => value_transfers.retain(|vt| !vt.memos.is_empty()),
        }

        Ok(value_transfers)
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_total_memobytes_to_address(
        &self,
    ) -> Result<TotalMemoBytesToAddress, SummaryError> {
        let value_transfers = self.value_transfers(true).await?;
        let mut memobytes_by_address = HashMap::new();
        for value_transfer in &value_transfers {
            if let ValueTransferKind::Sent(SentValueTransfer::Send) = value_transfer.kind {
                let address = value_transfer
                    .recipient_address
                    .clone()
                    .expect("sent value transfer should always have a recipient_address");
                let bytes = value_transfer.memos.iter().fold(0, |sum, m| sum + m.len());
                memobytes_by_address
                    .entry(address)
                    .and_modify(|e| *e += bytes)
                    .or_insert(bytes);
            }
        }
        Ok(TotalMemoBytesToAddress(memobytes_by_address))
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_total_spends_to_address(&self) -> Result<TotalSendsToAddress, SummaryError> {
        let values_sent_to_addresses = self.value_transfer_by_to_address().await?;
        let mut by_address_number_sends = HashMap::new();
        for key in values_sent_to_addresses.0.keys() {
            let number_sends = values_sent_to_addresses.0[key].len() as u64;
            by_address_number_sends.insert(key.clone(), number_sends);
        }

        Ok(TotalSendsToAddress(by_address_number_sends))
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_total_value_to_address(&self) -> Result<TotalValueToAddress, SummaryError> {
        let values_sent_to_addresses = self.value_transfer_by_to_address().await?;
        let mut by_address_total = HashMap::new();
        for key in values_sent_to_addresses.0.keys() {
            let sum = values_sent_to_addresses.0[key].iter().sum();
            by_address_total.insert(key.clone(), sum);
        }

        Ok(TotalValueToAddress(by_address_total))
    }

    async fn value_transfer_by_to_address(&self) -> Result<ValuesSentToAddress, SummaryError> {
        let value_transfers = self.value_transfers(false).await?;
        let mut amount_by_address = HashMap::new();
        for value_transfer in &value_transfers {
            if let ValueTransferKind::Sent(SentValueTransfer::Send) = value_transfer.kind {
                let address = value_transfer
                    .recipient_address
                    .clone()
                    .expect("sent value transfer should always have a recipient_address");
                amount_by_address
                    .entry(address)
                    .and_modify(|e: &mut Vec<u64>| e.push(value_transfer.value))
                    .or_insert(vec![value_transfer.value]);
            }
        }

        Ok(ValuesSentToAddress(amount_by_address))
    }

    /// Creates value transfers for all notes in a transaction that are sent to another
    /// recipient.  A value transfer is a group of all notes to a specific receiver in a transaction.
    /// The value transfer list is sorted by the output index of the notes.
    fn create_send_value_transfers(
        &self,
        transaction: &TransactionSummary,
    ) -> Result<Vec<ValueTransfer>, KeyError> {
        let mut value_transfers: Vec<ValueTransfer> = Vec::new();
        let outgoing_notes = transaction
            .outgoing_orchard_notes
            .iter()
            .chain(transaction.outgoing_sapling_notes.iter())
            .collect::<Vec<_>>();
        let outgoing_coins = &transaction.outgoing_transparent_coins;
        let mut addresses = HashSet::new();

        outgoing_notes.iter().try_for_each(|&note| {
            if note.scope == Scope::External && self.is_wallet_address(&note.recipient)?.is_none() {
                let encoded_address = note
                    .recipient_unified_address
                    .clone()
                    .unwrap_or(note.recipient.clone());
                addresses.insert(encoded_address);
            }

            Ok::<(), KeyError>(())
        })?;
        outgoing_coins.iter().try_for_each(|coin| {
            if self.is_wallet_address(&coin.recipient)?.is_none() {
                addresses.insert(coin.recipient.clone());
            }

            Ok::<(), KeyError>(())
        })?;
        let mut addresses = addresses.into_iter().collect::<Vec<_>>();
        addresses.sort();
        for address in addresses {
            let outgoing_notes_to_address: Vec<&OutgoingNoteSummary> = outgoing_notes
                .iter()
                .filter(|&&note| {
                    let query_address = if let Some(ua) = note.recipient_unified_address.clone() {
                        ua
                    } else {
                        note.recipient.clone()
                    };
                    query_address == address
                })
                .copied()
                .collect();
            let outgoing_coins_to_address: Vec<&OutgoingCoinSummary> = outgoing_coins
                .iter()
                .filter(|&coin| coin.recipient.clone() == address)
                .collect();
            let value: u64 = outgoing_notes_to_address
                .iter()
                .map(|&note| note.value)
                .chain(outgoing_coins_to_address.iter().map(|&coin| coin.value))
                .sum();
            let memos: Vec<String> = outgoing_notes_to_address
                .iter()
                .filter_map(|&note| note.memo.clone())
                .collect();
            value_transfers.push(ValueTransfer {
                txid: transaction.txid,
                datetime: transaction.datetime,
                status: transaction.status,
                blockheight: transaction.blockheight,
                transaction_fee: transaction.fee,
                zec_price: transaction.zec_price,
                kind: ValueTransferKind::Sent(SentValueTransfer::Send),
                value,
                recipient_address: Some(address),
                pool_received: None,
                memos,
            });
        }

        Ok(value_transfers)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use pepper_sync::wallet::{OrchardNote, OutgoingOrchardNote, OutputId, WalletTransaction};
    use zcash_primitives::transaction::TxId;
    use zcash_protocol::memo::Memo;
    use zingo_common_components::protocol::ActivationHeights;
    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingo_test_vectors::seeds;

    use crate::ZENNIES_FOR_ZINGO_REGTEST_ADDRESS;
    use crate::config::{ChainType, WalletConfig};
    use crate::mocks::orchard_note::OrchardCryptoNoteBuilder;
    use crate::testutils::default_test_wallet_settings;
    use crate::wallet::LightWallet;
    use crate::wallet::keys::unified::ReceiverSelection;
    use crate::wallet::summary::data::{
        SelfSendValueTransfer, SentValueTransfer, ValueTransferKind,
    };

    /// Message semantics need no network: these tests were libtonode
    /// integration tests (49s and 132s of LocalNet mining/syncing) whose
    /// assertions are pure summary/value-transfer derivation over wallet
    /// transaction records.
    fn regtest_wallet(mnemonic_phrase: &str) -> LightWallet {
        LightWallet::new(
            ChainType::Regtest(ActivationHeights::default()),
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: mnemonic_phrase.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
        )
        .unwrap()
    }

    fn received(txid_byte: u8, height: u32, memos: &[&str]) -> WalletTransaction {
        let txid = TxId::from_bytes([txid_byte; 32]);
        WalletTransaction::new_for_test_with_orchard_notes(
            txid,
            ConfirmationStatus::Confirmed(height.into()),
            memos
                .iter()
                .enumerate()
                .map(|(index, memo)| {
                    OrchardNote::new_for_test(
                        OutputId::new(txid, index as u32),
                        zip32::AccountId::ZERO,
                        zip32::Scope::External,
                        OrchardCryptoNoteBuilder::default().build(),
                        Memo::from_str(memo).unwrap(),
                    )
                })
                .collect(),
            vec![],
        )
    }

    fn sent(
        txid_byte: u8,
        height: u32,
        recipient: &zcash_client_backend::address::UnifiedAddress,
        memo: &str,
    ) -> WalletTransaction {
        let txid = TxId::from_bytes([txid_byte; 32]);
        WalletTransaction::new_for_test_with_orchard_notes(
            txid,
            ConfirmationStatus::Confirmed(height.into()),
            vec![],
            vec![OutgoingOrchardNote::new_for_test(
                OutputId::new(txid, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                OrchardCryptoNoteBuilder::default().build(),
                Memo::from_str(memo).unwrap(),
                Some(recipient.clone()),
            )],
        )
    }

    /// Migrated from libtonode `fast::filter_empty_messages`.
    #[tokio::test]
    async fn filter_empty_messages() {
        let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);

        // Two received notes with empty memos: no messages.
        wallet
            .wallet_transactions
            .insert(TxId::from_bytes([1; 32]), received(1, 10, &["", ""]));
        assert_eq!(wallet.messages_containing(None).await.unwrap().len(), 0);

        // One real memo alongside an empty one: exactly one message.
        wallet
            .wallet_transactions
            .insert(TxId::from_bytes([2; 32]), received(2, 11, &["Hello", ""]));
        assert_eq!(wallet.messages_containing(None).await.unwrap().len(), 1);
    }

    /// Migrated from libtonode `fast::message_thread`.
    #[tokio::test]
    async fn message_thread() {
        // Alice is this wallet; Bob and Charlie are addresses of a foreign
        // wallet (different seed), exactly as the integration test used the
        // faucet's addresses.
        let mut alice_wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);
        let network = ChainType::Regtest(ActivationHeights::default());
        let alice = alice_wallet
            .unified_addresses()
            .values()
            .next()
            .unwrap()
            .encode(&network);

        let mut other_wallet = regtest_wallet(seeds::ABANDON_ART_SEED);
        let (_, bob) = other_wallet
            .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
            .unwrap();
        let (_, charlie) = other_wallet
            .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
            .unwrap();
        let bob_encoded = bob.encode(&network);
        let charlie_encoded = charlie.encode(&network);

        for transaction in [
            sent(1, 10, &bob, &format!("Alice->Bob #1\nReply to\n{alice}")),
            sent(2, 11, &bob, &format!("Alice->Bob #2\nReply to\n{alice}")),
            received(3, 12, &[&format!("Bob->Alice #2\nReply to\n{bob_encoded}")]),
            sent(
                4,
                13,
                &charlie,
                &format!("Alice->Charlie #2\nReply to\n{alice}"),
            ),
            received(
                5,
                14,
                &[&format!("Charlie->Alice #2\nReply to\n{charlie_encoded}")],
            ),
        ] {
            alice_wallet
                .wallet_transactions
                .insert(transaction.txid(), transaction);
        }

        let messages_bob = alice_wallet
            .messages_containing(Some(&bob_encoded))
            .await
            .unwrap();
        let messages_charlie = alice_wallet
            .messages_containing(Some(&charlie_encoded))
            .await
            .unwrap();
        let all_vts = alice_wallet.value_transfers(true).await.unwrap();
        let all_messages = alice_wallet.messages_containing(None).await.unwrap();

        assert_eq!(messages_bob.len(), 3);
        assert_eq!(messages_charlie.len(), 2);

        // ALL MESSAGES (first one should be the oldest one)
        assert!(
            all_messages
                .windows(2)
                .all(|pair| pair[0].blockheight <= pair[1].blockheight)
        );
        // ALL VTS (first one should be the most recent one)
        assert!(
            all_vts
                .windows(2)
                .all(|pair| pair[0].blockheight >= pair[1].blockheight)
        );
    }

    /// Migrated from libtonode `fast::create_send_to_self_with_zfz_active`:
    /// the assertions are value-transfer KIND classification (a self-send
    /// yields SendToSelf(Basic); the Zennies-for-Zingo output yields a
    /// Sent(Send) addressed to the ZFZ address), which is pure summary
    /// derivation. The proposal/broadcast pipeline the integration test
    /// drove incidentally remains covered by the chain-bound send tests.
    #[tokio::test]
    async fn create_send_to_self_with_zfz_active() {
        let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);
        let network = ChainType::Regtest(ActivationHeights::default());

        let own_orchard_receiver = *wallet
            .unified_addresses()
            .values()
            .next()
            .unwrap()
            .orchard()
            .unwrap();
        let zcash_client_backend::address::Address::Unified(zfz_unified_address) =
            zcash_client_backend::address::Address::decode(
                &network,
                ZENNIES_FOR_ZINGO_REGTEST_ADDRESS,
            )
            .unwrap()
        else {
            panic!("ZFZ address must be unified");
        };

        let txid = TxId::from_bytes([1; 32]);
        let transaction = WalletTransaction::new_for_test_with_orchard_notes(
            txid,
            ConfirmationStatus::Confirmed(10.into()),
            vec![],
            vec![
                // The send-to-self output: recipient is one of the wallet's
                // own orchard receivers.
                OutgoingOrchardNote::new_for_test(
                    OutputId::new(txid, 0),
                    zip32::AccountId::ZERO,
                    zip32::Scope::External,
                    OrchardCryptoNoteBuilder::default()
                        .recipient(own_orchard_receiver)
                        .build(),
                    Memo::Empty,
                    None,
                ),
                // The Zennies-for-Zingo output.
                OutgoingOrchardNote::new_for_test(
                    OutputId::new(txid, 1),
                    zip32::AccountId::ZERO,
                    zip32::Scope::External,
                    OrchardCryptoNoteBuilder::default().build(),
                    Memo::Empty,
                    Some(zfz_unified_address),
                ),
            ],
        );
        wallet.wallet_transactions.insert(txid, transaction);

        let value_transfers = wallet.value_transfers(true).await.unwrap();

        assert!(value_transfers.iter().any(|vt| vt.kind
            == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                SelfSendValueTransfer::Basic
            ))));
        assert!(value_transfers.iter().any(|vt| vt.kind
            == ValueTransferKind::Sent(SentValueTransfer::Send)
            && vt.recipient_address == Some(ZENNIES_FOR_ZINGO_REGTEST_ADDRESS.to_string())));
    }
}
