//! Types and impls for displaying wallet transaction data to the user / consumer.

use std::{cmp::Ordering, collections::HashMap};

use pepper_sync::{
    keys::transparent::{self, TransparentScope},
    wallet::{
        KeyIdInterface, NoteInterface, OutgoingNoteInterface, OutputInterface as _, TransparentCoin,
    },
};
use zcash_primitives::{consensus::BlockHeight, memo::Memo, transaction::TxId};
use zcash_protocol::PoolType;
use zingo_status::confirmation_status::ConfirmationStatus;

use super::{
    LightWallet,
    data::{
        finsight::{
            TotalMemoBytesToAddress, TotalSendsToAddress, TotalValueToAddress, ValuesSentToAddress,
        },
        summaries::{
            BasicCoinSummary, BasicNoteSummary, OutgoingCoinSummary, OutgoingNoteSummary,
            SelfSendValueTransfer, SentValueTransfer, TransactionSummaries, TransactionSummary,
            TransactionSummaryBuilder, TransactionSummaryInterface, ValueTransfer,
            ValueTransferBuilder, ValueTransferKind, ValueTransfers,
        },
    },
    error::{KeyError, SummaryError},
    output::SpendStatus,
};

// TODO: move data::summaries and value transfer / transaction summary methods here

/// Scope enum with std::fmt::Display impl for use with summaries.
#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    External,
    Internal,
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Scope::External => "external",
                Scope::Internal => "internal",
            }
        )
    }
}

impl From<zip32::Scope> for Scope {
    fn from(value: zip32::Scope) -> Self {
        match value {
            zip32::Scope::External => Scope::External,
            zip32::Scope::Internal => Scope::Internal,
        }
    }
}

/// Note summary.
///
/// Intended for returning a standalone summary of all notes to the user / consumer outside the context of transactions.
#[allow(missing_docs)]
#[derive(Debug)]
pub struct NoteSummary {
    pub value: u64,
    pub status: ConfirmationStatus,
    pub block_height: BlockHeight,
    pub spend_status: SpendStatus,
    pub memo: Option<String>,
    pub time: u32,
    pub txid: TxId,
    pub output_index: u16,
    pub account_id: zip32::AccountId,
    pub scope: Scope,
}

impl std::fmt::Display for NoteSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let memo = self.memo.clone().unwrap_or_default();
        let time = if let Some(dt) = chrono::DateTime::from_timestamp(self.time as i64, 0) {
            format!("{}", dt)
        } else {
            "not available".to_string()
        };

        write!(
            f,
            "{{
                value: {}
                status: {} at block height {}
                spend status: {}
                memo: {}
                time: {}
                txid: {}
                output index: {}
                account id: {}
                scope: {}
            }}",
            self.value,
            self.status,
            self.block_height,
            self.spend_status,
            memo,
            time,
            self.txid,
            self.output_index,
            u32::from(self.account_id),
            self.scope
        )
    }
}

impl From<NoteSummary> for json::JsonValue {
    fn from(note: NoteSummary) -> Self {
        json::object! {
            "value" => note.value,
            "status" => format!("{} at block height {}", note.status, note.block_height),
            "spend_status" => note.spend_status.to_string(),
            "memo" => note.memo,
            "time" => note.time,
            "txid" => note.txid.to_string(),
            "output_index" => note.output_index,
            "account_id" => u32::from(note.account_id),
            "scope" => note.scope.to_string(),
        }
    }
}

/// A wrapper struct for implementing display and json on a vec of note summaries
#[derive(Debug)]
pub struct NoteSummaries(Vec<NoteSummary>);

impl NoteSummaries {
    /// Creates a new NoteSummaries
    pub fn new(note_summaries: Vec<NoteSummary>) -> Self {
        NoteSummaries(note_summaries)
    }
}

impl<'a> std::iter::IntoIterator for &'a NoteSummaries {
    type Item = &'a NoteSummary;
    type IntoIter = std::slice::Iter<'a, NoteSummary>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl std::ops::Deref for NoteSummaries {
    type Target = Vec<NoteSummary>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for NoteSummaries {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl std::ops::Index<usize> for NoteSummaries {
    type Output = NoteSummary; // The type of the value returned by the index

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index] // Forward the indexing operation to the underlying data structure
    }
}

impl std::fmt::Display for NoteSummaries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for value_transfer in &self.0 {
            write!(f, "\n{}", value_transfer)?;
        }
        Ok(())
    }
}

impl From<NoteSummaries> for json::JsonValue {
    fn from(note_summaries: NoteSummaries) -> Self {
        let note_summaries: Vec<json::JsonValue> = note_summaries
            .0
            .into_iter()
            .map(json::JsonValue::from)
            .collect();
        json::object! {
            "note_summaries" => note_summaries

        }
    }
}

/// Coin summary.
///
/// Intended for returning a standalone summary of all transparent coins to the user / consumer outside the context of
/// transactions.
#[allow(missing_docs)]
pub struct CoinSummary {
    pub value: u64,
    pub status: ConfirmationStatus,
    pub block_height: BlockHeight,
    pub spend_status: SpendStatus,
    pub time: u32,
    pub txid: TxId,
    pub output_index: u16,
    pub account_id: zip32::AccountId,
    pub scope: TransparentScope,
    pub address_index: u32,
}

impl std::fmt::Display for CoinSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let time = if let Some(dt) = chrono::DateTime::from_timestamp(self.time as i64, 0) {
            format!("{}", dt)
        } else {
            "not available".to_string()
        };

        write!(
            f,
            "{{
                value: {}
                status: {} at block height {}
                spend status: {}
                time: {}
                txid: {}
                output index: {}
                account id: {}
                scope: {}
                address_index: {}
            }}",
            self.value,
            self.status,
            self.block_height,
            self.spend_status,
            time,
            self.txid,
            self.output_index,
            u32::from(self.account_id),
            self.scope,
            self.address_index,
        )
    }
}

impl From<CoinSummary> for json::JsonValue {
    fn from(coin: CoinSummary) -> Self {
        json::object! {
            "value" => coin.value,
            "status" => format!("{} at block height {}", coin.status, coin.block_height),
            "spend_status" => coin.spend_status.to_string(),
            "time" => coin.time,
            "txid" => coin.txid.to_string(),
            "output_index" => coin.output_index,
            "account_id" => u32::from(coin.account_id),
            "scope" => coin.scope.to_string(),
            "address_index" => coin.address_index
        }
    }
}

/// TODO: doc comment
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TransactionKind {
    /// TODO: doc comment
    Sent(SendType),
    /// TODO: doc comment
    Received,
}

impl std::fmt::Display for TransactionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            TransactionKind::Received => write!(f, "received"),
            TransactionKind::Sent(SendType::Send) => write!(f, "sent"),
            TransactionKind::Sent(SendType::Shield) => write!(f, "shield"),
            TransactionKind::Sent(SendType::SendToSelf) => write!(f, "send-to-self"),
        }
    }
}

/// TODO: doc comment
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SendType {
    /// Transaction is sending funds to recipient other than the creator
    Send,
    /// Transaction is only sending funds from transparent pool to the creator's shielded pool
    Shield,
    /// Transaction is only sending funds to the creator's address(es) and is not a shield
    SendToSelf,
}

impl LightWallet {
    /// Provides a list of transaction summaries related to this wallet in order of blockheight
    pub async fn transaction_summaries(&self) -> Result<TransactionSummaries, SummaryError> {
        let mut transaction_summaries = self
            .wallet_transactions
            .values()
            .map(|transaction| {
                let kind = self.transaction_kind(transaction)?;
                let value = match kind {
                    TransactionKind::Received | TransactionKind::Sent(SendType::Shield) => {
                        transaction.total_value_received()
                    }
                    TransactionKind::Sent(SendType::Send)
                    | TransactionKind::Sent(SendType::SendToSelf) => transaction.total_value_sent(),
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
                            recipient: note.encoded_recipient(&self.network).expect("infallible"),
                            recipient_unified_address: note
                                .encoded_recipient_full_unified_address(&self.network),
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
                                            value: transparent_output.value.into_u64(),
                                            recipient: transparent::encode_address(
                                                &self.network,
                                                address,
                                            ),
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

                Ok(TransactionSummaryBuilder::new()
                    .txid(transaction.txid())
                    .datetime(transaction.datetime())
                    .blockheight(transaction.status().get_height())
                    .kind(kind)
                    .value(value)
                    .fee(fee)
                    .status(transaction.status())
                    .zec_price(None)
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

    /// Provides a list of value transfers related to this capability
    /// A value transfer is a group of all notes to a specific receiver in a transaction.
    pub async fn value_transfers(
        &self,
        sort_highest_to_lowest: bool,
    ) -> Result<ValueTransfers, SummaryError> {
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
                                .pool_received(Some(PoolType::ORCHARD.to_string()))
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
                                .pool_received(Some(PoolType::SAPLING.to_string()))
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
                                .pool_received(Some(PoolType::ORCHARD.to_string()))
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
                                .pool_received(Some(PoolType::SAPLING.to_string()))
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

        if sort_highest_to_lowest {
            value_transfers.reverse();
        }

        Ok(ValueTransfers::new(value_transfers))
    }

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

    /// Provides a list of ValueTransfers associated with the sender, or containing the string.
    pub async fn messages_containing(
        &self,
        filter: Option<&str>,
    ) -> Result<ValueTransfers, SummaryError> {
        let mut value_transfers = self.value_transfers(true).await?;
        value_transfers.reverse();

        // Filter out VTs where all memos are empty.
        value_transfers.retain(|vt| vt.memos().iter().any(|memo| !memo.is_empty()));

        match filter {
            Some(s) => {
                value_transfers.retain(|vt| {
                    if vt.memos().is_empty() {
                        return false;
                    }

                    if vt.recipient_address() == Some(s) {
                        true
                    } else {
                        for memo in vt.memos() {
                            if memo.contains(s) {
                                return true;
                            }
                        }
                        false
                    }
                });
            }
            None => value_transfers.retain(|vt| !vt.memos().is_empty()),
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
            if let ValueTransferKind::Sent(SentValueTransfer::Send) = value_transfer.kind() {
                let address = value_transfer
                    .recipient_address()
                    .expect("sent value transfer should always have a recipient_address")
                    .to_string();
                let bytes = value_transfer
                    .memos()
                    .iter()
                    .fold(0, |sum, m| sum + m.len());
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
            if let ValueTransferKind::Sent(SentValueTransfer::Send) = value_transfer.kind() {
                let address = value_transfer
                    .recipient_address()
                    .expect("sent value transfer should always have a recipient_address")
                    .to_string();
                amount_by_address
                    .entry(address)
                    .and_modify(|e: &mut Vec<u64>| e.push(value_transfer.value()))
                    .or_insert(vec![value_transfer.value()]);
            }
        }

        Ok(ValuesSentToAddress(amount_by_address))
    }

    /// Creates value transfers for all notes in a transaction that are sent to another
    /// recipient.  A value transfer is a group of all notes to a specific receiver in a transaction.
    /// The value transfer list is sorted by the output index of the notes.
    fn create_send_value_transfers(
        &self,
        transaction_summary: &TransactionSummary,
    ) -> Result<Vec<ValueTransfer>, KeyError> {
        let mut value_transfers: Vec<ValueTransfer> = Vec::new();
        let outgoing_notes = transaction_summary
            .outgoing_orchard_notes()
            .iter()
            .chain(transaction_summary.outgoing_sapling_notes().iter())
            .collect::<Vec<_>>();
        let outgoing_coins = transaction_summary.outgoing_transparent_coins();
        let mut addresses = HashMap::new();

        transaction_summary
            .outgoing_orchard_notes()
            .iter()
            .try_for_each(|note| {
                if self.is_wallet_address(&note.recipient)?.is_none() {
                    let encoded_address = note
                        .recipient_unified_address
                        .clone()
                        .unwrap_or(note.recipient.clone());
                    addresses.insert(encoded_address, note.output_index);
                }

                Ok::<(), KeyError>(())
            })?;
        transaction_summary
            .outgoing_sapling_notes()
            .iter()
            .try_for_each(|note| {
                // added scope check to circumvent sapling-crypto bug:
                // https://github.com/zcash/sapling-crypto/issues/160.
                if self.is_wallet_address(&note.recipient)?.is_none()
                    && note.scope != Scope::Internal
                {
                    let encoded_address = note
                        .recipient_unified_address
                        .clone()
                        .unwrap_or(note.recipient.clone());
                    addresses.insert(encoded_address, note.output_index);
                }

                Ok::<(), KeyError>(())
            })?;
        outgoing_coins.iter().try_for_each(|coin| {
            if self.is_wallet_address(&coin.recipient)?.is_none() {
                addresses.insert(coin.recipient.clone(), coin.output_index);
            }

            Ok::<(), KeyError>(())
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
}
