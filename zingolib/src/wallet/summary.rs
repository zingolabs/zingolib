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
                let pools_sent_from = self.pools_sent_from(transaction)?;
                let ironwood_notes = transaction
                    .ironwood_notes()
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

                let outgoing_ironwood_notes = transaction
                    .outgoing_ironwood_notes()
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
                    pools_sent_from,
                    ironwood_notes,
                    orchard_notes,
                    sapling_notes,
                    transparent_coins,
                    outgoing_ironwood_notes,
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
                        .ironwood_notes
                        .iter()
                        .any(|note| note.memo.is_some())
                        || transaction
                            .orchard_notes
                            .iter()
                            .any(|note| note.memo.is_some())
                        || transaction
                            .sapling_notes
                            .iter()
                            .any(|note| note.memo.is_some())
                    {
                        let memos: Vec<String> = transaction
                            .ironwood_notes
                            .iter()
                            .filter_map(|note| note.memo.clone())
                            .chain(
                                transaction
                                    .orchard_notes
                                    .iter()
                                    .filter_map(|note| note.memo.clone()),
                            )
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
                            pools_sent_from: transaction.pools_sent_from.clone(),
                            pools_received: transaction.pools_received(),
                            memos,
                        });
                    }
                }
                TransactionKind::Sent(SendType::Shield) => {
                    // create 1 shielding value transfer for each pool shielded to
                    if !transaction.ironwood_notes.is_empty() {
                        let value: u64 = transaction
                            .ironwood_notes
                            .iter()
                            .map(|output| output.value)
                            .sum();
                        let memos: Vec<String> = transaction
                            .ironwood_notes
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
                            pools_sent_from: transaction.pools_sent_from.clone(),
                            pools_received: vec![PoolType::IRONWOOD],
                            memos,
                        });
                    }
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
                            pools_sent_from: transaction.pools_sent_from.clone(),
                            pools_received: vec![PoolType::ORCHARD],
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
                            pools_sent_from: transaction.pools_sent_from.clone(),
                            pools_received: vec![PoolType::SAPLING],
                            memos,
                        });
                    }
                }
                TransactionKind::Sent(SendType::SendToSelf) => {
                    // create 1 memo-to-self if a sending transaction receives any number of memos
                    // otherwise, create 1 send-to-self value transfer so every transaction creates at least 1 value transfer
                    // eventually we may replace send-to-self with a range of kinds such as deshield and migrate etc.
                    if transaction
                        .ironwood_notes
                        .iter()
                        .any(|note| note.memo.is_some())
                        || transaction
                            .orchard_notes
                            .iter()
                            .any(|note| note.memo.is_some())
                        || transaction
                            .sapling_notes
                            .iter()
                            .any(|note| note.memo.is_some())
                    {
                        let memos: Vec<String> = transaction
                            .ironwood_notes
                            .iter()
                            .filter_map(|note| note.memo.clone())
                            .chain(
                                transaction
                                    .orchard_notes
                                    .iter()
                                    .filter_map(|note| note.memo.clone()),
                            )
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
                            pools_sent_from: transaction.pools_sent_from.clone(),
                            pools_received: transaction.pools_received(),
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
                            pools_sent_from: transaction.pools_sent_from.clone(),
                            pools_received: transaction.pools_received(),
                            memos: Vec::new(),
                        });
                    }

                    // in the case Zennies For Zingo! is active
                    value_transfers.append(&mut self.create_send_value_transfers(&transaction)?);
                }
                TransactionKind::Received => {
                    // create 1 received value transfer for each pool received to
                    if !transaction.ironwood_notes.is_empty() {
                        let value: u64 = transaction
                            .ironwood_notes
                            .iter()
                            .map(|output| output.value)
                            .sum();
                        let memos: Vec<String> = transaction
                            .ironwood_notes
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
                            pools_sent_from: transaction.pools_sent_from.clone(),
                            pools_received: vec![PoolType::IRONWOOD],
                            memos,
                        });
                    }
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
                            pools_sent_from: transaction.pools_sent_from.clone(),
                            pools_received: vec![PoolType::ORCHARD],
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
                            pools_sent_from: transaction.pools_sent_from.clone(),
                            pools_received: vec![PoolType::SAPLING],
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
                            pools_sent_from: transaction.pools_sent_from.clone(),
                            pools_received: vec![PoolType::TRANSPARENT],
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
            .outgoing_ironwood_notes
            .iter()
            .map(|note| (note, PoolType::IRONWOOD))
            .chain(
                transaction
                    .outgoing_orchard_notes
                    .iter()
                    .map(|note| (note, PoolType::ORCHARD)),
            )
            .chain(
                transaction
                    .outgoing_sapling_notes
                    .iter()
                    .map(|note| (note, PoolType::SAPLING)),
            )
            .collect::<Vec<_>>();
        let outgoing_coins = &transaction.outgoing_transparent_coins;
        let mut addresses = HashSet::new();

        outgoing_notes.iter().try_for_each(|&(note, _)| {
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
            let outgoing_notes_to_address: Vec<(&OutgoingNoteSummary, PoolType)> = outgoing_notes
                .iter()
                .filter(|&&(note, _)| {
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
                .map(|&(note, _)| note.value)
                .chain(outgoing_coins_to_address.iter().map(|&coin| coin.value))
                .sum();
            let memos: Vec<String> = outgoing_notes_to_address
                .iter()
                .filter_map(|&(note, _)| note.memo.clone())
                .collect();
            let mut pools_received = Vec::new();
            if !outgoing_coins_to_address.is_empty() {
                pools_received.push(PoolType::TRANSPARENT);
            }
            for pool in [PoolType::SAPLING, PoolType::ORCHARD, PoolType::IRONWOOD] {
                if outgoing_notes_to_address
                    .iter()
                    .any(|&(_, note_pool)| note_pool == pool)
                {
                    pools_received.push(pool);
                }
            }
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
                pools_sent_from: transaction.pools_sent_from.clone(),
                pools_received,
                memos,
            });
        }

        Ok(value_transfers)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use pepper_sync::wallet::{
        IronwoodNote, OrchardNote, OutgoingIronwoodNote, OutputId, WalletTransaction,
    };
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

    /// Message semantics need no network: the ported tests in this module
    /// were libtonode integration tests (the first two ports alone cost
    /// 49s and 132s of LocalNet mining/syncing) whose assertions are pure
    /// summary/value-transfer derivation over wallet transaction records.
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
        WalletTransaction::new_for_test_with_ironwood_notes(
            txid,
            ConfirmationStatus::Confirmed(height.into()),
            memos
                .iter()
                .enumerate()
                .map(|(index, memo)| {
                    IronwoodNote::new_for_test(
                        OutputId::new(txid, index as u32),
                        zip32::AccountId::ZERO,
                        zip32::Scope::External,
                        OrchardCryptoNoteBuilder::default().build(),
                        Memo::from_str(memo).unwrap(),
                        None,
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
        WalletTransaction::new_for_test_with_ironwood_notes(
            txid,
            ConfirmationStatus::Confirmed(height.into()),
            vec![],
            vec![OutgoingIronwoodNote::new_for_test(
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
    /// derivation. The proposal/transmission pipeline the integration test
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
        let transaction = WalletTransaction::new_for_test_with_ironwood_notes(
            txid,
            ConfirmationStatus::Confirmed(10.into()),
            vec![],
            vec![
                // The send-to-self output: recipient is one of the wallet's
                // own orchard receivers.
                OutgoingIronwoodNote::new_for_test(
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
                OutgoingIronwoodNote::new_for_test(
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

    /// A send-to-self value transfer exposes the pools its value arrived
    /// into (here: ironwood), letting consumers label pool movements such
    /// as an orchard -> ironwood migration outside zingolib. The funding
    /// side (`pools_sent_from`) needs real spend links, so it is pinned by
    /// the chain-bound shield tests in libtonode instead.
    #[tokio::test]
    async fn send_to_self_exposes_pools_received() {
        use zcash_protocol::PoolType;

        let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);
        let own_orchard_receiver = *wallet
            .unified_addresses()
            .values()
            .next()
            .unwrap()
            .orchard()
            .unwrap();

        let txid = TxId::from_bytes([1; 32]);
        let transaction = WalletTransaction::new_for_test_with_ironwood_notes(
            txid,
            ConfirmationStatus::Confirmed(10.into()),
            // The self-received output, landing in the ironwood pool.
            vec![IronwoodNote::new_for_test(
                OutputId::new(txid, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::Internal,
                OrchardCryptoNoteBuilder::default().build(),
                Memo::Empty,
                None,
            )],
            // The outgoing view of the same output: recipient is one of
            // the wallet's own receivers, making the transaction a
            // send-to-self.
            vec![OutgoingIronwoodNote::new_for_test(
                OutputId::new(txid, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                OrchardCryptoNoteBuilder::default()
                    .recipient(own_orchard_receiver)
                    .build(),
                Memo::Empty,
                None,
            )],
        );
        wallet.wallet_transactions.insert(txid, transaction);

        let value_transfers = wallet.value_transfers(true).await.unwrap();

        let self_send = value_transfers
            .iter()
            .find(|vt| {
                vt.kind
                    == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                        SelfSendValueTransfer::Basic,
                    ))
            })
            .unwrap();
        assert_eq!(self_send.pools_received, [PoolType::IRONWOOD]);
        assert!(self_send.pools_sent_from.is_empty());
    }

    /// Migrated from libtonode
    /// `fast::spendable_balance_includes_notes_in_incomplete_shards`: the
    /// property is spendable-balance composition over wallet state — a
    /// confirmed, positioned note whose block has no completed orchard
    /// shard (sync state carries no orchard shard ranges, so the note lives
    /// in the trailing incomplete shard) still counts as spendable. The
    /// integration version only produced that condition incidentally via
    /// regtest's tiny tree; here it is constructed explicitly.
    /// (Lives here to share this module's record-fabrication rig;
    /// `spendable_balance` itself is defined in wallet/balance.rs.)
    #[test]
    fn spendable_balance_includes_notes_in_incomplete_shards() {
        use incrementalmerkletree::Position;
        use orchard::value::NoteValue;
        use pepper_sync::sync::{ScanPriority, ScanRange};
        use pepper_sync::wallet::SyncState;
        use zcash_protocol::consensus::BlockHeight;

        let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);

        // Birthday..tip fully scanned; no completed orchard shards.
        wallet.sync_state = SyncState::new_for_test(vec![ScanRange::from_parts(
            BlockHeight::from_u32(1)..BlockHeight::from_u32(21),
            ScanPriority::Scanned,
        )]);

        // The anchor height is capped by the sapling tree's newest
        // checkpoint (get_target_and_anchor_heights); give it one at the
        // tip, as sync would have.
        {
            use shardtree::store::{Checkpoint, ShardStore as _};
            wallet
                .shard_trees
                .sapling
                .store_mut()
                .add_checkpoint(BlockHeight::from_u32(20), Checkpoint::tree_empty())
                .unwrap();
        }

        let txid = TxId::from_bytes([1; 32]);
        wallet.wallet_transactions.insert(
            txid,
            WalletTransaction::new_for_test_with_ironwood_notes(
                txid,
                ConfirmationStatus::Confirmed(10.into()),
                vec![IronwoodNote::new_for_test(
                    OutputId::new(txid, 0),
                    zip32::AccountId::ZERO,
                    zip32::Scope::External,
                    OrchardCryptoNoteBuilder::default()
                        .value(NoteValue::from_raw(100_000))
                        .build(),
                    Memo::Empty,
                    Some(Position::from(0)),
                )],
                vec![],
            ),
        );

        assert_eq!(
            wallet
                .spendable_balance::<pepper_sync::wallet::IronwoodNote>(
                    zip32::AccountId::ZERO,
                    false
                )
                .unwrap()
                .into_u64(),
            100_000
        );
    }
    /// Migrated from libtonode `slow::sapling_to_sapling_scan_together`:
    /// transaction summaries order a sapling funding receive and its
    /// subsequent spend by height, with correct txids and values, and the
    /// spend's outgoing sapling notes carry the recipient and value. The
    /// original produced these records through a LocalNet round trip; here
    /// they are fabricated directly. (The original name hints at
    /// scan-batching, but its assertions only ever checked summary output;
    /// scan-batching coverage belongs to pepper-sync's scan layer.)
    #[tokio::test]
    async fn sapling_to_sapling_scan_together() {
        use pepper_sync::wallet::OutgoingSaplingNote;
        use sapling_crypto::value::NoteValue;
        use zcash_keys::encoding::encode_payment_address;
        use zcash_protocol::consensus::NetworkConstants as _;
        use zcash_protocol::consensus::Parameters as _;

        use crate::mocks::SaplingCryptoNoteBuilder;
        use crate::wallet::keys::unified::ReceiverSelection;

        let funding_value = 100_000;
        let spent_value = 20_000;

        let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);
        let network = wallet.chain_type();

        let mut external_wallet = regtest_wallet(seeds::ABANDON_ART_SEED);
        let (_, destination_ua) = external_wallet
            .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
            .unwrap();
        let destination = *destination_ua.sapling().unwrap();

        let funding_txid = TxId::from_bytes([1; 32]);
        let spent_txid = TxId::from_bytes([2; 32]);

        let mut funded_crypto_note = SaplingCryptoNoteBuilder::default();
        funded_crypto_note.value(NoteValue::from_raw(funding_value));
        // The spend linkage itself is not fabricated: summary derivation
        // validates a spent note against the spending transaction's actual
        // bundle nullifiers, and none of this test's assertions concern
        // spend status. (sapling_incoming_sapling_outgoing covers spend
        // status through views that read the wallet records directly.)
        let funded_note = pepper_sync::wallet::SaplingNote::new_for_test(
            OutputId::new(funding_txid, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            funded_crypto_note.build(),
            Memo::Empty,
            Some(incrementalmerkletree::Position::from(0)),
        );
        wallet.wallet_transactions.insert(
            funding_txid,
            WalletTransaction::new_for_test(funding_txid, ConfirmationStatus::Confirmed(5.into()))
                .with_sapling_notes_for_test(vec![funded_note]),
        );

        let mut outgoing_crypto_note = SaplingCryptoNoteBuilder::default();
        outgoing_crypto_note.recipient(destination);
        outgoing_crypto_note.value(NoteValue::from_raw(spent_value));
        let outgoing_note = OutgoingSaplingNote::new_for_test(
            OutputId::new(spent_txid, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            outgoing_crypto_note.build(),
            Memo::Empty,
            None,
        );
        wallet.wallet_transactions.insert(
            spent_txid,
            WalletTransaction::new_for_test(spent_txid, ConfirmationStatus::Confirmed(6.into()))
                .with_outgoing_sapling_notes_for_test(vec![outgoing_note]),
        );

        let transactions = wallet.transaction_summaries(false).await.unwrap().0;

        assert_eq!(transactions.first().unwrap().blockheight, 5.into());
        assert_eq!(transactions.first().unwrap().txid, funding_txid);
        assert_eq!(transactions.first().unwrap().value, funding_value);

        assert_eq!(transactions.get(1).unwrap().blockheight, 6.into());
        assert_eq!(transactions.get(1).unwrap().txid, spent_txid);
        assert_eq!(transactions.get(1).unwrap().value, spent_value);
        let expected_recipient = encode_payment_address(
            network.network_type().hrp_sapling_payment_address(),
            &destination,
        );
        assert!(
            transactions
                .get(1)
                .unwrap()
                .outgoing_sapling_notes
                .iter()
                .any(|note| note.recipient == expected_recipient)
        );
        assert!(
            transactions
                .get(1)
                .unwrap()
                .outgoing_sapling_notes
                .iter()
                .any(|note| note.value == spent_value)
        );
    }

    /// Migrated from libtonode `slow::sapling_incoming_sapling_outgoing`:
    /// balances and note/transaction views across the three states of a
    /// sapling note's life — received and confirmed, pending spent by a
    /// transmitted transaction, and spent by a confirmed transaction. The
    /// original walked a LocalNet chain through those states; here each
    /// state is fabricated and asserted directly.
    #[tokio::test]
    async fn sapling_incoming_sapling_outgoing() {
        use std::str::FromStr as _;

        use pepper_sync::wallet::{
            NoteInterface as _, OutgoingNoteInterface as _, OutgoingSaplingNote,
            OutputInterface as _, SaplingNote,
        };
        use sapling_crypto::value::NoteValue;

        use crate::lightclient::LightClient;
        use crate::mocks::SaplingCryptoNoteBuilder;
        use crate::wallet::keys::unified::ReceiverSelection;
        use crate::wallet::output::SpendStatus;

        let value = 100_000;
        let sent_value = 2_000;
        let outgoing_memo = "Outgoing Memo";

        let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);
        let (_, own_sapling_ua) = wallet
            .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
            .unwrap();
        let own_sapling_address = *own_sapling_ua.sapling().unwrap();

        let mut external_wallet = regtest_wallet(seeds::ABANDON_ART_SEED);
        let (_, external_ua) = external_wallet
            .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
            .unwrap();
        let external_sapling_address = *external_ua.sapling().unwrap();

        // State 1: a confirmed incoming sapling note on the wallet's own
        // sapling receiver.
        let funding_txid = TxId::from_bytes([1; 32]);
        let mut incoming_crypto_note = SaplingCryptoNoteBuilder::default();
        incoming_crypto_note.recipient(own_sapling_address);
        incoming_crypto_note.value(NoteValue::from_raw(value));
        let incoming_note = SaplingNote::new_for_test(
            OutputId::new(funding_txid, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            incoming_crypto_note.build(),
            Memo::Empty,
            Some(incrementalmerkletree::Position::from(0)),
        );
        wallet.wallet_transactions.insert(
            funding_txid,
            WalletTransaction::new_for_test(funding_txid, ConfirmationStatus::Confirmed(4.into()))
                .with_sapling_notes_for_test(vec![incoming_note]),
        );

        let client = LightClient::new_for_test(wallet).await;
        let balance = client
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert_eq!(balance.total_sapling_balance.unwrap().into_u64(), value);
        assert_eq!(balance.confirmed_sapling_balance.unwrap().into_u64(), value);
        assert_eq!(balance.unconfirmed_sapling_balance.unwrap().into_u64(), 0);
        {
            let wallet = client.wallet().read().await;
            let received_note = wallet
                .wallet_transactions
                .get(&funding_txid)
                .unwrap()
                .sapling_notes()
                .first()
                .unwrap();
            assert_eq!(received_note.value(), value);
            assert_eq!(received_note.note().recipient(), own_sapling_address);
        }

        // State 2: the note is pending spent by a transmitted transaction
        // carrying an outgoing note with a memo.
        let sent_txid = TxId::from_bytes([2; 32]);
        {
            let mut wallet = client.wallet().write().await;
            wallet
                .wallet_transactions
                .get_mut(&funding_txid)
                .unwrap()
                .sapling_notes_mut()
                .first_mut()
                .unwrap()
                .set_spending_transaction(Some(sent_txid));

            let mut outgoing_crypto_note = SaplingCryptoNoteBuilder::default();
            outgoing_crypto_note.recipient(external_sapling_address);
            outgoing_crypto_note.value(NoteValue::from_raw(sent_value));
            let outgoing_note = OutgoingSaplingNote::new_for_test(
                OutputId::new(sent_txid, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                outgoing_crypto_note.build(),
                Memo::from_str(outgoing_memo).unwrap(),
                None,
            );
            wallet.wallet_transactions.insert(
                sent_txid,
                WalletTransaction::new_for_test(
                    sent_txid,
                    ConfirmationStatus::Transmitted(5.into()),
                )
                .with_outgoing_sapling_notes_for_test(vec![outgoing_note]),
            );
        }
        {
            let wallet = client.wallet().read().await;
            let sapling_notes = wallet.note_summaries::<SaplingNote>(true);
            assert_eq!(wallet.wallet_outputs::<OrchardNote>().len(), 0);
            assert_eq!(
                sapling_notes
                    .iter()
                    .filter(|note| note.spend_status.is_confirmed_spent())
                    .count(),
                0
            );
            let pending_notes = sapling_notes
                .iter()
                .filter(|note| note.spend_status.is_pending_spent())
                .collect::<Vec<_>>();
            assert_eq!(pending_notes.len(), 1);
            let pending_sapling_note = pending_notes.first().unwrap();
            assert_eq!(pending_sapling_note.txid, funding_txid);
            if let SpendStatus::TransmittedSpent(txid) = pending_sapling_note.spend_status {
                assert_eq!(txid, sent_txid);
            } else {
                panic!("incorrect spend status!");
            }

            let sent_transaction = wallet.wallet_transactions.get(&sent_txid).unwrap();
            assert_eq!(wallet.wallet_transactions.len(), 2);
            assert_eq!(sent_transaction.total_value_sent(), sent_value);
            assert!(!sent_transaction.status().is_confirmed());
            assert_eq!(sent_transaction.status().get_height(), 5.into());

            let outgoing_sapling_note = sent_transaction
                .outgoing_sapling_notes()
                .iter()
                .find(|note| note.recipient() == external_sapling_address)
                .unwrap();
            if let Memo::Text(memo) = outgoing_sapling_note.memo() {
                assert_eq!(&String::from(memo.clone()), outgoing_memo);
            } else {
                panic!("no text memo");
            }
            assert_eq!(outgoing_sapling_note.value(), sent_value);
        }

        // State 3: the spending transaction confirms.
        {
            let mut wallet = client.wallet().write().await;
            wallet
                .wallet_transactions
                .get_mut(&sent_txid)
                .unwrap()
                .update_status(
                    ConfirmationStatus::Confirmed(5.into()),
                    crate::utils::now(),
                    false,
                );
        }
        {
            let wallet = client.wallet().read().await;
            let sent_transaction = wallet.wallet_transactions.get(&sent_txid).unwrap();
            assert!(sent_transaction.status().is_confirmed());
            assert_eq!(
                sent_transaction.status().get_confirmed_height().unwrap(),
                5.into()
            );
        }
    }
    /// Migrated from libtonode `slow::send_funds_to_all_pools`: per-pool
    /// balance aggregation over one confirmed note in each pool. The
    /// original asserted this balance check plus txid uniqueness across
    /// its transaction summaries; its live funding round trips are covered
    /// by the two surviving chain_generics fixtures (the pool matrix
    /// itself is now offline in `lightclient::propose::pool_matrix`).
    #[tokio::test]
    async fn send_funds_to_all_pools() {
        use crate::check_client_balances;
        use crate::lightclient::LightClient;
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

        let value = 100_000;
        let wallet = SyntheticWalletBuilder::new(seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(value)
            .ironwood_note(value)
            .sapling_note(value)
            .transparent_coin(value)
            .build();
        let client = LightClient::new_for_test(wallet).await;
        check_client_balances!(client, i: value o: value s: value t: value);
    }

    /// Migrated from libtonode `slow::by_address_finsight`: the
    /// memo-bytes-per-address summary accumulates outgoing memo lengths
    /// keyed by recipient address. Two 1-byte memos then a 4-byte memo.
    #[tokio::test]
    async fn by_address_finsight() {
        use crate::wallet::keys::unified::ReceiverSelection;

        let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);

        let mut external_wallet = regtest_wallet(seeds::ABANDON_ART_SEED);
        let (_, external_ua) = external_wallet
            .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
            .unwrap();
        let external_ua_encoded = external_ua.encode(&external_wallet.chain_type());

        for (txid_byte, height, memo) in [(1, 4, "1"), (2, 5, "1")] {
            let transaction = sent(txid_byte, height, &external_ua, memo);
            wallet
                .wallet_transactions
                .insert(transaction.txid(), transaction);
        }
        let memobytes = wallet.do_total_memobytes_to_address().await.unwrap();
        assert_eq!(
            json::JsonValue::from(memobytes)[&external_ua_encoded].pretty(4),
            "2".to_string()
        );

        let transaction = sent(3, 6, &external_ua, "aaaa");
        wallet
            .wallet_transactions
            .insert(transaction.txid(), transaction);
        let memobytes = wallet.do_total_memobytes_to_address().await.unwrap();
        assert_eq!(
            json::JsonValue::from(memobytes)[&external_ua_encoded].pretty(4),
            "6".to_string()
        );
    }

    /// Migrated from libtonode `fast::value_transfers`: a four-output
    /// memo'd receive aggregates into one value transfer carrying all
    /// four memos, and the derivation is idempotent and sort-stable
    /// (the descending ordering is exactly the reverse of ascending).
    #[tokio::test]
    async fn value_transfers_aggregation_and_ordering() {
        use incrementalmerkletree::Position;
        use orchard::value::NoteValue;
        use pepper_sync::wallet::OutputId;
        use std::str::FromStr as _;

        use crate::mocks::orchard_note::OrchardCryptoNoteBuilder;

        let mut wallet = regtest_wallet(seeds::HOSPITAL_MUSEUM_SEED);

        // One received transaction carrying four memo'd notes...
        let txid = TxId::from_bytes([1; 32]);
        let notes = (0..4u64)
            .map(|index| {
                IronwoodNote::new_for_test(
                    OutputId::new(txid, u32::try_from(index).unwrap()),
                    zip32::AccountId::ZERO,
                    zip32::Scope::External,
                    OrchardCryptoNoteBuilder::default()
                        .value(NoteValue::from_raw(5_000))
                        .build(),
                    Memo::from_str(&format!("Message #{}", index + 1)).unwrap(),
                    Some(Position::from(index)),
                )
            })
            .collect::<Vec<_>>();
        wallet.wallet_transactions.insert(
            txid,
            WalletTransaction::new_for_test_with_ironwood_notes(
                txid,
                ConfirmationStatus::Confirmed(4.into()),
                notes,
                vec![],
            ),
        );
        // ...plus two single-note receives at later heights so the
        // ordering assertions exercise a non-trivial sort.
        for (txid_byte, height) in [(2u8, 5u32), (3, 6)] {
            let txid = TxId::from_bytes([txid_byte; 32]);
            let note = IronwoodNote::new_for_test(
                OutputId::new(txid, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                OrchardCryptoNoteBuilder::default()
                    .value(NoteValue::from_raw(1_000 * u64::from(txid_byte)))
                    .build(),
                Memo::Empty,
                Some(Position::from(u64::from(txid_byte))),
            );
            wallet.wallet_transactions.insert(
                txid,
                WalletTransaction::new_for_test_with_ironwood_notes(
                    txid,
                    ConfirmationStatus::Confirmed(height.into()),
                    vec![note],
                    vec![],
                ),
            );
        }

        let value_transfers = wallet.value_transfers(true).await.unwrap();
        let value_transfers1 = wallet.value_transfers(true).await.unwrap();
        let value_transfers2 = wallet.value_transfers(true).await.unwrap();
        let mut value_transfers3 = wallet.value_transfers(false).await.unwrap();
        let mut value_transfers4 = wallet.value_transfers(false).await.unwrap();

        let four_memo_transfer = value_transfers
            .iter()
            .find(|transfer| transfer.txid == TxId::from_bytes([1; 32]))
            .unwrap();
        assert_eq!(four_memo_transfer.memos.len(), 4);

        value_transfers3.reverse();
        value_transfers4.reverse();

        assert_eq!(value_transfers, value_transfers1);
        assert_eq!(value_transfers, value_transfers2);
        assert_eq!(value_transfers, value_transfers3);
        assert_eq!(value_transfers, value_transfers4);
    }
}
