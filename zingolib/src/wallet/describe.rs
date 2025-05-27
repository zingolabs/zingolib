//! Wallet-State reporters as LightWallet methods.

use std::cmp::Ordering;
use std::collections::HashMap;

use zcash_primitives::legacy::TransparentAddress;
use zcash_primitives::memo::Memo;
use zcash_protocol::PoolType;
use zcash_protocol::ShieldedProtocol;

use super::data::summaries::BasicCoinSummary;
use super::data::summaries::BasicNoteSummary;
use super::data::summaries::OutgoingCoinSummary;
use super::data::summaries::OutgoingNoteSummary;
use super::data::summaries::SelfSendValueTransfer;
use super::data::summaries::SentValueTransfer;
use super::data::summaries::TransactionSummaries;
use super::data::summaries::TransactionSummary;
use super::data::summaries::TransactionSummaryBuilder;
use super::data::summaries::TransactionSummaryInterface as _;
use super::data::summaries::ValueTransfer;
use super::data::summaries::ValueTransferBuilder;
use super::data::summaries::ValueTransferKind;
use super::data::summaries::ValueTransfers;
use super::error::KeyError;
use super::error::SpendError;
use super::error::SummaryError;
use super::summary;
use super::summary::SendType;
use super::summary::TransactionKind;
use crate::config::ChainType;
use crate::config::ZENNIES_FOR_ZINGO_DONATION_ADDRESS;
use crate::config::ZENNIES_FOR_ZINGO_REGTEST_ADDRESS;
use crate::config::ZENNIES_FOR_ZINGO_TESTNET_ADDRESS;
use crate::wallet::LightWallet;
use pepper_sync::keys::decode_address;
use pepper_sync::keys::transparent;
use pepper_sync::keys::transparent::TransparentScope;
use pepper_sync::wallet::NoteInterface as _;
use pepper_sync::wallet::OrchardNote;
use pepper_sync::wallet::OutgoingNoteInterface;
use pepper_sync::wallet::OutputInterface;
use pepper_sync::wallet::SaplingNote;
use pepper_sync::wallet::TransparentCoin;
use pepper_sync::wallet::WalletTransaction;

impl LightWallet {
    /// Determine the kind of transaction from the wallet data.
    pub(crate) fn transaction_kind(
        &self,
        transaction: &WalletTransaction,
    ) -> Result<TransactionKind, SpendError> {
        let zfz_address = match self.network {
            ChainType::Mainnet => ZENNIES_FOR_ZINGO_DONATION_ADDRESS,
            ChainType::Testnet => ZENNIES_FOR_ZINGO_TESTNET_ADDRESS,
            ChainType::Regtest(_) => ZENNIES_FOR_ZINGO_REGTEST_ADDRESS,
        };

        let transparent_spends = self.find_spends::<TransparentCoin>(transaction, false)?;
        let sapling_spends = self.find_spends::<SaplingNote>(transaction, false)?;
        let orchard_spends = self.find_spends::<OrchardNote>(transaction, false)?;

        if transparent_spends.is_empty()
            && sapling_spends.is_empty()
            && orchard_spends.is_empty()
            && transaction.outgoing_sapling_notes().is_empty()
            && transaction.outgoing_orchard_notes().is_empty()
        {
            Ok(TransactionKind::Received)
        } else if !transparent_spends.is_empty()
            && sapling_spends.is_empty()
            && orchard_spends.is_empty()
            && transaction.outgoing_sapling_notes().is_empty()
            && transaction.outgoing_orchard_notes().is_empty()
            && (!transaction.orchard_notes().is_empty() || !transaction.sapling_notes().is_empty())
        {
            Ok(TransactionKind::Sent(SendType::Shield))
        } else if transaction
            .transaction()
            .transparent_bundle()
            .is_none_or(|bundle| bundle.vout.len() == transaction.transparent_coins().len())
            && transaction
                .outgoing_sapling_notes()
                .iter()
                .all(|outgoing_note| {
                    if let Some(full_address) = outgoing_note.recipient_full_unified_address() {
                        full_address.sapling().is_none_or(|address| {
                            self.is_sapling_external_send_to_self(address)
                                .expect("must have sapling view capability in this scope")
                                || outgoing_note.key_id().scope == zip32::Scope::Internal
                        }) || outgoing_note
                            .encoded_recipient_full_unified_address(&self.network)
                            .expect("should exist in this scope")
                            == *zfz_address
                    } else {
                        self.is_sapling_external_send_to_self(&outgoing_note.note().recipient())
                            .expect("must have sapling view capability in this scope")
                            || outgoing_note.key_id().scope == zip32::Scope::Internal
                    }
                })
            && transaction
                .outgoing_orchard_notes()
                .iter()
                .all(|outgoing_note| {
                    if let Some(full_address) = outgoing_note.recipient_full_unified_address() {
                        full_address.orchard().is_none_or(|address| {
                            self.is_orchard_send_to_self(address)
                                .expect("must have orchard view capability in this scope")
                        }) || outgoing_note
                            .encoded_recipient_full_unified_address(&self.network)
                            .expect("should exist in this scope")
                            == *zfz_address
                    } else {
                        self.is_orchard_send_to_self(&outgoing_note.note().recipient())
                            .expect("must have orchard view capability in this scope")
                    }
                })
        {
            Ok(TransactionKind::Sent(SendType::SendToSelf))
        } else {
            Ok(TransactionKind::Sent(SendType::Send))
        }
    }

    /// Provides a list of transaction summaries related to this wallet in order of blockheight
    // TODO: move to summary
    pub async fn transaction_summaries(&self) -> Result<TransactionSummaries, SummaryError> {
        let mut transaction_summaries = self
            .wallet_transactions
            .values()
            .map(|transaction| {
                let (
                    kind,
                    value,
                    fee,
                    orchard_notes,
                    sapling_notes,
                    transparent_coins,
                    outgoing_orchard_notes,
                    outgoing_sapling_notes,
                    outgoing_transparent_coins,
                    price,
                ) = self.basic_transaction_summary_parts(transaction)?;

                Ok(TransactionSummaryBuilder::new()
                    .txid(transaction.txid())
                    .datetime(transaction.datetime())
                    .blockheight(transaction.status().get_height())
                    .kind(kind)
                    .value(value)
                    .fee(fee)
                    .status(transaction.status())
                    .zec_price(price)
                    .orchard_notes(orchard_notes)
                    .sapling_notes(sapling_notes)
                    .transparent_coins(transparent_coins)
                    .outgoing_orchard_notes(outgoing_orchard_notes)
                    .outgoing_sapling_notes(outgoing_sapling_notes)
                    .outgoing_transparent_coins(outgoing_transparent_coins)
                    .build()
                    .expect("all fields should be populated"))
            })
            .collect::<Result<Vec<_>, SummaryError>>()?;

        transaction_summaries.sort_by(|summary_a, summary_b| {
            match summary_a.blockheight().cmp(&summary_b.blockheight()) {
                Ordering::Equal => {
                    // TODO: order tex transactions correctly by checking inputs / outputs are the wallet's refund addresses
                    summary_a.txid().cmp(&summary_b.txid())
                }
                otherwise => otherwise,
            }
        });

        Ok(TransactionSummaries::new(transaction_summaries))
    }

    /// TODO: doc comment
    // TODO: remove
    pub async fn transaction_summaries_json_string(&self) -> String {
        match self.transaction_summaries().await {
            Ok(transactions) => json::JsonValue::from(transactions).pretty(2),
            Err(e) => format!("Error: {e}"),
        }
    }

    // TODO: simplify type complexity
    // TODO: move to summary
    #[allow(clippy::type_complexity)]
    fn basic_transaction_summary_parts(
        &self,
        transaction: &WalletTransaction,
    ) -> Result<
        (
            TransactionKind,
            u64,
            Option<u64>,
            Vec<BasicNoteSummary>,
            Vec<BasicNoteSummary>,
            Vec<BasicCoinSummary>,
            Vec<OutgoingNoteSummary>,
            Vec<OutgoingNoteSummary>,
            Vec<OutgoingCoinSummary>,
            Option<f32>,
        ),
        SummaryError,
    > {
        let kind = self.transaction_kind(transaction)?;
        let value = match kind {
            TransactionKind::Received | TransactionKind::Sent(SendType::Shield) => {
                transaction.total_value_received()
            }
            TransactionKind::Sent(SendType::Send) | TransactionKind::Sent(SendType::SendToSelf) => {
                transaction.total_value_sent()
            }
        };
        let fee = self.calculate_transaction_fee(transaction).ok();
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
                    output.output_id().output_index() as u32,
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
                    output.output_id().output_index() as u32,
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
                    output.output_id().output_index() as u32,
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
                        .encoded_recipient(&self.network)
                        .map_err(zcash_address::ParseError::Unified)?,
                    recipient_unified_address: note
                        .encoded_recipient_full_unified_address(&self.network),
                    output_index: note.output_id().output_index(),
                    account_id: note.key_id().account_id,
                    scope: summary::Scope::from(note.key_id().scope),
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
                    recipient: note.encoded_recipient(&self.network).expect("infallible"),
                    recipient_unified_address: note
                        .encoded_recipient_full_unified_address(&self.network),
                    account_id: note.key_id().account_id,
                    scope: summary::Scope::from(note.key_id().scope),
                }
            })
            .collect::<Vec<_>>();
        let outgoing_transparent_coin = if kind == TransactionKind::Received {
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
                                    value: transparent_output.value.into_u64(),
                                    recipient: transparent::encode_address(&self.network, address),
                                    output_index: output_index as u16,
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

        Ok((
            kind,
            value,
            fee,
            orchard_notes,
            sapling_notes,
            transparent_coins,
            outgoing_orchard_notes,
            outgoing_sapling_notes,
            outgoing_transparent_coin,
            None,
        ))
    }

    /// Provides a list of value transfers related to this capability
    /// A value transfer is a group of all notes to a specific receiver in a transaction.
    // TODO: move to summary
    pub async fn value_transfers(&self) -> Result<ValueTransfers, SummaryError> {
        let mut value_transfers: Vec<ValueTransfer> = Vec::new();
        let transaction_summaries = self.transaction_summaries().await?.0;

        for transaction in transaction_summaries.iter() {
            match transaction.kind() {
                TransactionKind::Sent(SendType::Send) => {
                    // create 1 sent value transfer for each non-self recipient address
                    // if recipient_ua is available it overrides recipient_address
                    value_transfers.append(&mut self.create_send_value_transfers(transaction)?);

                    // create 1 memo-to-self if any number of memos are received in the sending transaction
                    if transaction
                        .orchard_notes()
                        .iter()
                        .any(|note| note.memo().is_some())
                        || transaction
                            .sapling_notes()
                            .iter()
                            .any(|note| note.memo().is_some())
                    {
                        let memos: Vec<String> = transaction
                            .orchard_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .chain(
                                transaction
                                    .sapling_notes()
                                    .iter()
                                    .filter_map(|note| note.memo().map(|memo| memo.to_string())),
                            )
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(transaction.txid())
                                .datetime(transaction.datetime())
                                .status(transaction.status())
                                .blockheight(transaction.blockheight())
                                .transaction_fee(transaction.fee())
                                .zec_price(transaction.zec_price())
                                .kind(ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                    SelfSendValueTransfer::MemoToSelf,
                                )))
                                .value(0)
                                .recipient_address(None)
                                .pool_received(None)
                                .memos(memos)
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }
                }
                TransactionKind::Sent(SendType::Shield) => {
                    // create 1 shielding value transfer for each pool shielded to
                    if !transaction.orchard_notes().is_empty() {
                        let value: u64 = transaction
                            .orchard_notes()
                            .iter()
                            .map(|output| output.value())
                            .sum();
                        let memos: Vec<String> = transaction
                            .orchard_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(transaction.txid())
                                .datetime(transaction.datetime())
                                .status(transaction.status())
                                .blockheight(transaction.blockheight())
                                .transaction_fee(transaction.fee())
                                .zec_price(transaction.zec_price())
                                .kind(ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                    SelfSendValueTransfer::Shield,
                                )))
                                .value(value)
                                .recipient_address(None)
                                .pool_received(Some(
                                    PoolType::Shielded(ShieldedProtocol::Orchard).to_string(),
                                ))
                                .memos(memos)
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }
                    if !transaction.sapling_notes().is_empty() {
                        let value: u64 = transaction
                            .sapling_notes()
                            .iter()
                            .map(|output| output.value())
                            .sum();
                        let memos: Vec<String> = transaction
                            .sapling_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(transaction.txid())
                                .datetime(transaction.datetime())
                                .status(transaction.status())
                                .blockheight(transaction.blockheight())
                                .transaction_fee(transaction.fee())
                                .zec_price(transaction.zec_price())
                                .kind(ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                    SelfSendValueTransfer::Shield,
                                )))
                                .value(value)
                                .recipient_address(None)
                                .pool_received(Some(
                                    PoolType::Shielded(ShieldedProtocol::Sapling).to_string(),
                                ))
                                .memos(memos)
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }
                }
                TransactionKind::Sent(SendType::SendToSelf) => {
                    // create 1 memo-to-self if a sending transaction receives any number of memos
                    // otherwise, create 1 send-to-self value transfer so every transaction creates at least 1 value transfer
                    // eventually we may replace send-to-self with a range of kinds such as deshield and migrate etc.
                    if transaction
                        .orchard_notes()
                        .iter()
                        .any(|note| note.memo().is_some())
                        || transaction
                            .sapling_notes()
                            .iter()
                            .any(|note| note.memo().is_some())
                    {
                        let memos: Vec<String> = transaction
                            .orchard_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .chain(
                                transaction
                                    .sapling_notes()
                                    .iter()
                                    .filter_map(|note| note.memo().map(|memo| memo.to_string())),
                            )
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(transaction.txid())
                                .datetime(transaction.datetime())
                                .status(transaction.status())
                                .blockheight(transaction.blockheight())
                                .transaction_fee(transaction.fee())
                                .zec_price(transaction.zec_price())
                                .kind(ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                    SelfSendValueTransfer::MemoToSelf,
                                )))
                                .value(0)
                                .recipient_address(None)
                                .pool_received(None)
                                .memos(memos)
                                .build()
                                .expect("all fields should be populated"),
                        );
                    } else {
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(transaction.txid())
                                .datetime(transaction.datetime())
                                .status(transaction.status())
                                .blockheight(transaction.blockheight())
                                .transaction_fee(transaction.fee())
                                .zec_price(transaction.zec_price())
                                .kind(ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                    SelfSendValueTransfer::Basic,
                                )))
                                .value(0)
                                .recipient_address(None)
                                .pool_received(None)
                                .memos(vec![])
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }

                    // in the case Zennies For Zingo! is active
                    value_transfers.append(&mut self.create_send_value_transfers(transaction)?);
                }
                TransactionKind::Received => {
                    // create 1 received value transfer for each pool received to
                    if !transaction.orchard_notes().is_empty() {
                        let value: u64 = transaction
                            .orchard_notes()
                            .iter()
                            .map(|output| output.value())
                            .sum();
                        let memos: Vec<String> = transaction
                            .orchard_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(transaction.txid())
                                .datetime(transaction.datetime())
                                .status(transaction.status())
                                .blockheight(transaction.blockheight())
                                .transaction_fee(transaction.fee())
                                .zec_price(transaction.zec_price())
                                .kind(ValueTransferKind::Received)
                                .value(value)
                                .recipient_address(None)
                                .pool_received(Some(
                                    PoolType::Shielded(ShieldedProtocol::Orchard).to_string(),
                                ))
                                .memos(memos)
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }
                    if !transaction.sapling_notes().is_empty() {
                        let value: u64 = transaction
                            .sapling_notes()
                            .iter()
                            .map(|output| output.value())
                            .sum();
                        let memos: Vec<String> = transaction
                            .sapling_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(transaction.txid())
                                .datetime(transaction.datetime())
                                .status(transaction.status())
                                .blockheight(transaction.blockheight())
                                .transaction_fee(transaction.fee())
                                .zec_price(transaction.zec_price())
                                .kind(ValueTransferKind::Received)
                                .value(value)
                                .recipient_address(None)
                                .pool_received(Some(
                                    PoolType::Shielded(ShieldedProtocol::Sapling).to_string(),
                                ))
                                .memos(memos)
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }
                    if !transaction.transparent_coins().is_empty() {
                        let value: u64 = transaction
                            .transparent_coins()
                            .iter()
                            .map(|output| output.value())
                            .sum();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(transaction.txid())
                                .datetime(transaction.datetime())
                                .status(transaction.status())
                                .blockheight(transaction.blockheight())
                                .transaction_fee(transaction.fee())
                                .zec_price(transaction.zec_price())
                                .kind(ValueTransferKind::Received)
                                .value(value)
                                .recipient_address(None)
                                .pool_received(Some(PoolType::Transparent.to_string()))
                                .memos(Vec::new())
                                .build()
                                .expect("all fields should be populated"),
                        );
                    }
                }
            };
        }

        Ok(ValueTransfers::new(value_transfers))
    }

    /// Provides a list of value transfers sorted
    /// A value transfer is a group of all notes to a specific receiver in a transaction.
    // TODO: move to summary
    pub async fn sorted_value_transfers(
        &self,
        newer_first: bool,
    ) -> Result<ValueTransfers, SummaryError> {
        let mut value_transfers = self.value_transfers().await?;
        if newer_first {
            value_transfers.reverse();
        }
        Ok(value_transfers)
    }

    /// TODO: doc comment
    // TODO: remove
    pub async fn value_transfers_json_string(&self) -> String {
        match self.sorted_value_transfers(true).await {
            Ok(value_transfers) => json::JsonValue::from(value_transfers).pretty(2),
            Err(e) => format!("Error: {e}"),
        }
    }

    /// Creates value transfers for all notes in a transaction that are sent to another
    /// recipient.  A value transfer is a group of all notes to a specific receiver in a transaction.
    /// The value transfer list is sorted by the output index of the notes.
    fn create_send_value_transfers(
        &self,
        transaction_summary: &TransactionSummary,
    ) -> std::io::Result<Vec<ValueTransfer>> {
        let mut value_transfers: Vec<ValueTransfer> = Vec::new();
        let outgoing_notes = transaction_summary
            .outgoing_orchard_notes()
            .iter()
            .chain(transaction_summary.outgoing_sapling_notes().iter())
            .collect::<Vec<_>>();
        let outgoing_coins = transaction_summary.outgoing_transparent_coins();
        let mut addresses = HashMap::new();

        outgoing_notes.iter().try_for_each(|&note| {
            let encoded_address = if let Some(ua) = note.recipient_unified_address.clone() {
                ua
            } else {
                note.recipient.clone()
            };

            if !self.is_send_to_self(&encoded_address, Some(&note.scope))? {
                // hash map is used to create unique list of addresses as duplicates are not inserted twice
                addresses.insert(encoded_address, note.output_index);
            }

            Ok::<(), std::io::Error>(())
        })?;
        outgoing_coins.iter().try_for_each(|coin| {
            if !self.is_send_to_self(&coin.recipient, None)? {
                // hash map is used to create unique list of addresses as duplicates are not inserted twice
                addresses.insert(coin.recipient.clone(), coin.output_index);
            }

            Ok::<(), std::io::Error>(())
        })?;
        let mut addresses_vec = addresses.into_iter().collect::<Vec<_>>();
        addresses_vec.sort_by_key(|(_address, output_index)| *output_index);
        addresses_vec.iter().for_each(|(address, _output_index)| {
            let outgoing_notes_to_address: Vec<&OutgoingNoteSummary> = outgoing_notes
                .iter()
                .filter(|&&note| {
                    let query_address = if let Some(ua) = note.recipient_unified_address.clone() {
                        ua
                    } else {
                        note.recipient.clone()
                    };
                    query_address == *address
                })
                .cloned()
                .collect();
            let outgoing_coins_to_address: Vec<&OutgoingCoinSummary> = outgoing_coins
                .iter()
                .filter(|&coin| coin.recipient.clone() == *address)
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
            value_transfers.push(
                ValueTransferBuilder::new()
                    .txid(transaction_summary.txid())
                    .datetime(transaction_summary.datetime())
                    .status(transaction_summary.status())
                    .blockheight(transaction_summary.blockheight())
                    .transaction_fee(transaction_summary.fee())
                    .zec_price(transaction_summary.zec_price())
                    .kind(ValueTransferKind::Sent(SentValueTransfer::Send))
                    .value(value)
                    .recipient_address(Some(address.clone()))
                    .pool_received(None)
                    .memos(memos)
                    .build()
                    .expect("all fields should be populated"),
            );
        });

        Ok(value_transfers)
    }

    /// Determines whether the `encoded_address` is derived by the wallet's keys.
    ///
    /// `note_scope` must be provided when the address could be decoded into a sapling address or a unified address
    /// with a sapling receiver. This is to circumvent a bug with sapling_crypto::zip32::DiversableFullViewingKey::decrpyt_diversifier
    /// https://github.com/zcash/sapling-crypto/issues/160.
    fn is_send_to_self(
        &self,
        encoded_address: &str,
        note_scope: Option<&summary::Scope>,
    ) -> std::io::Result<bool> {
        Ok(match decode_address(&self.network, encoded_address)? {
            zcash_keys::address::Address::Transparent(address) => {
                self.is_transparent_send_to_self(&address).is_some()
            }
            zcash_keys::address::Address::Sapling(address) => {
                self.is_sapling_external_send_to_self(&address)
                    .expect("should have sapling view capability in this scope")
                    || *note_scope
                        .expect("note scope must be provided for addresses with sapling receivers!")
                        == summary::Scope::Internal
            }
            zcash_keys::address::Address::Unified(address) => {
                address
                    .transparent()
                    .is_some_and(|addr| self.is_transparent_send_to_self(addr).is_some())
                    || address.sapling().is_some_and(|addr| {
                        self.is_sapling_external_send_to_self(addr)
                            .expect("should have sapling view capability in this scope")
                            || *note_scope.expect(
                                "note scope must be provided for addresses with sapling receivers!",
                            ) == summary::Scope::Internal
                    })
                    || address.orchard().is_some_and(|addr| {
                        self.is_orchard_send_to_self(addr)
                            .expect("should have sapling view capability in this scope")
                    })
            }
            zcash_keys::address::Address::Tex(_) => false,
        })
    }

    fn is_transparent_send_to_self(
        &self,
        address: &TransparentAddress,
    ) -> Option<TransparentScope> {
        let encoded_address = transparent::encode_address(&self.network, *address);

        self.transparent_addresses
            .iter()
            .find(|(_, wallet_address)| **wallet_address == encoded_address)
            .map(|(address_id, _)| address_id.scope())
    }

    /// Checks if the given `address` is derived from the wallet's sapling FVKs. External scope only.
    fn is_sapling_external_send_to_self(
        &self,
        address: &sapling_crypto::PaymentAddress,
    ) -> Result<bool, KeyError> {
        for unified_key in self.unified_key_store.values() {
            if sapling_crypto::zip32::DiversifiableFullViewingKey::try_from(unified_key)?
                .decrypt_diversifier(address)
                .is_some()
            {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Checks if the given `address` is derived from the wallet's orchard FVKs.
    fn is_orchard_send_to_self(&self, address: &orchard::Address) -> Result<bool, KeyError> {
        for unified_key in self.unified_key_store.values() {
            if orchard::keys::FullViewingKey::try_from(unified_key)?
                .scope_for_address(address)
                .is_some()
            {
                return Ok(true);
            }
        }

        Ok(false)
    }
}

#[cfg(any(test, feature = "test-elevation"))]
mod test {
    use zcash_protocol::{PoolType, ShieldedProtocol, consensus::NetworkConstants};

    use crate::wallet::LightWallet;

    /// these functions have clearer typing than
    /// the production functions using json that could be upgraded soon
    impl LightWallet {
        #[allow(clippy::result_unit_err)]
        /// gets a UnifiedAddress, the first of the wallet.
        /// zingolib includes derivations of further addresses.
        /// ZingoMobile uses one address.
        pub fn get_first_ua(&self) -> Result<zcash_keys::address::UnifiedAddress, ()> {
            Ok(self.unified_addresses.values().next().ok_or(())?.clone())
        }

        #[allow(clippy::result_unit_err)]
        /// UnifiedAddress type is not a string. to process it into a string requires chain date.
        pub fn encode_ua_as_pool(
            &self,
            ua: &zcash_keys::address::UnifiedAddress,
            pool: PoolType,
        ) -> Result<String, ()> {
            match pool {
                PoolType::Transparent => ua
                    .transparent()
                    .map(|taddr| {
                        // TODO: new crate for shared conversion, parsing and encoding
                        pepper_sync::keys::transparent::encode_address(&self.network, *taddr)
                    })
                    .ok_or(()),
                PoolType::Shielded(ShieldedProtocol::Sapling) => ua
                    .sapling()
                    .map(|z_addr| {
                        zcash_keys::encoding::encode_payment_address(
                            self.network.hrp_sapling_payment_address(),
                            z_addr,
                        )
                    })
                    .ok_or(()),
                PoolType::Shielded(ShieldedProtocol::Orchard) => Ok(ua.encode(&self.network)),
            }
        }

        #[allow(clippy::result_unit_err)]
        /// gets a string address for the wallet, based on pooltype
        pub fn get_first_address(&self, pool: PoolType) -> Result<String, ()> {
            match pool {
                PoolType::Transparent => {
                    Ok(self.transparent_addresses.values().next().unwrap().clone())
                }
                _ => {
                    let ua = self.get_first_ua()?;
                    self.encode_ua_as_pool(&ua, pool)
                }
            }
        }
    }

    // FIXME: zingo2 rewrite as an integration test
    // #[tokio::test]
    // async fn confirmed_balance_excluding_dust() {
    //     let wallet = LightWallet::new(
    //         ZingoConfigBuilder::default().create().chain,
    //         WalletBase::FreshEntropy,
    //         1.into(),
    //     )
    //     .unwrap();
    //     let confirmed_tx_record = TransactionRecordBuilder::default()
    //         .status(ConfirmationStatus::Confirmed(80.into()))
    //         .transparent_outputs(TransparentOutputBuilder::default())
    //         .sapling_notes(SaplingNoteBuilder::default())
    //         .sapling_notes(SaplingNoteBuilder::default())
    //         .sapling_notes(
    //             SaplingNoteBuilder::default()
    //                 .note(
    //                     SaplingCryptoNoteBuilder::default()
    //                         .value(sapling_crypto::value::NoteValue::from_raw(3_000))
    //                         .clone(),
    //                 )
    //                 .clone(),
    //         )
    //         .orchard_notes(OrchardNoteBuilder::default())
    //         .orchard_notes(OrchardNoteBuilder::default())
    //         .orchard_notes(
    //             OrchardNoteBuilder::default()
    //                 .note(
    //                     OrchardCryptoNoteBuilder::default()
    //                         .value(orchard::value::NoteValue::from_raw(5_001))
    //                         .clone(),
    //                 )
    //                 .clone(),
    //         )
    //         .orchard_notes(
    //             OrchardNoteBuilder::default()
    //                 .note(
    //                     OrchardCryptoNoteBuilder::default()
    //                         .value(orchard::value::NoteValue::from_raw(2_000))
    //                         .clone(),
    //                 )
    //                 .clone(),
    //         )
    //         .build();
    //     let mempool_tx_record = TransactionRecordBuilder::default()
    //         .status(ConfirmationStatus::Mempool(95.into()))
    //         .transparent_outputs(TransparentOutputBuilder::default())
    //         .sapling_notes(SaplingNoteBuilder::default())
    //         .orchard_notes(OrchardNoteBuilder::default())
    //         .build();
    //     {
    //         let mut tx_map = wallet
    //             .transaction_context
    //             .transaction_metadata_set
    //             .write()
    //             .await;
    //         tx_map
    //             .transaction_records_by_id
    //             .insert_transaction_record(confirmed_tx_record);
    //         tx_map
    //             .transaction_records_by_id
    //             .insert_transaction_record(mempool_tx_record);
    //     }

    //     assert_eq!(
    //         wallet.confirmed_balance_excluding_dust::<Sapling>().await,
    //         Some(400_000)
    //     );
    //     assert_eq!(
    //         wallet.confirmed_balance_excluding_dust::<Orchard>().await,
    //         Some(1_605_001)
    //     );
    // }
}
