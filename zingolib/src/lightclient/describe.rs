//! These functions can be called by consumer to learn about the LightClient.
use ::orchard::note_encryption::OrchardDomain;
use json::{object, JsonValue};
use sapling_crypto::note_encryption::SaplingDomain;
use std::{cmp::Ordering, collections::HashMap};
use tokio::runtime::Runtime;

use zcash_client_backend::{encoding::encode_payment_address, PoolType, ShieldedProtocol};
use zcash_primitives::{
    consensus::{BlockHeight, NetworkConstants},
    memo::Memo,
};

use crate::{
    config::margin_fee,
    wallet::data::summaries::{
        SelfSendValueTransfer, SentValueTransfer, TransactionSummaryInterface,
    },
};

use super::{AccountBackupInfo, LightClient, PoolBalances, UserBalances};
use crate::{
    error::ZingoLibError,
    wallet::{
        data::{
            finsight,
            summaries::{
                basic_transaction_summary_parts, DetailedTransactionSummaries,
                DetailedTransactionSummaryBuilder, TransactionSummaries, TransactionSummary,
                TransactionSummaryBuilder, ValueTransfer, ValueTransferBuilder, ValueTransferKind,
                ValueTransfers,
            },
            OutgoingTxData,
        },
        keys::address_from_pubkeyhash,
        notes::{query::OutputQuery, Output, OutputInterface},
        transaction_record::{SendType, TransactionKind},
        LightWallet,
    },
};

#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum ValueTransferRecordingError {
    #[error("Fee was not calculable because of error:  {0}")]
    FeeCalculationError(String), // TODO: revisit passed type
}
fn some_sum(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    a.xor(b).or_else(|| a.zip(b).map(|(v, u)| v + u))
}
impl LightClient {
    /// Uses a query to select all notes across all transactions with specific properties and sum them
    pub async fn query_sum_value(&self, include_notes: OutputQuery) -> u64 {
        self.wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .transaction_records_by_id
            .query_sum_value(include_notes)
    }

    /// TODO: Add Doc Comment Here!
    // todo use helpers
    pub async fn do_addresses(&self) -> JsonValue {
        let mut objectified_addresses = Vec::new();
        for address in self.wallet.wallet_capability().addresses().iter() {
            let encoded_ua = address.encode(&self.config.chain);
            let transparent = address
                .transparent()
                .map(|taddr| address_from_pubkeyhash(&self.config, *taddr));
            objectified_addresses.push(object! {
        "address" => encoded_ua,
        "receivers" => object!(
            "transparent" => transparent,
            "sapling" => address.sapling().map(|z_addr| encode_payment_address(self.config.chain.hrp_sapling_payment_address(), z_addr)),
            "orchard_exists" => address.orchard().is_some(),
            )
        })
        }
        JsonValue::Array(objectified_addresses)
    }

    /// TODO: Redefine the wallet balance functions as non-generics that take a
    /// PoolType variant as an argument, and iterate over a `Vec<Output>`
    pub async fn do_balance(&self) -> PoolBalances {
        let verified_sapling_balance = self.wallet.confirmed_balance::<SaplingDomain>().await;
        let unverified_sapling_balance = self.wallet.pending_balance::<SaplingDomain>().await;
        let spendable_sapling_balance = self.wallet.spendable_balance::<SaplingDomain>().await;
        let sapling_balance = some_sum(verified_sapling_balance, unverified_sapling_balance);

        let verified_orchard_balance = self.wallet.confirmed_balance::<OrchardDomain>().await;
        let unverified_orchard_balance = self.wallet.pending_balance::<OrchardDomain>().await;
        let spendable_orchard_balance = self.wallet.spendable_balance::<OrchardDomain>().await;
        let orchard_balance = some_sum(verified_orchard_balance, unverified_orchard_balance);
        PoolBalances {
            sapling_balance,
            verified_sapling_balance,
            spendable_sapling_balance,
            unverified_sapling_balance,

            orchard_balance,
            verified_orchard_balance,
            spendable_orchard_balance,
            unverified_orchard_balance,

            transparent_balance: self.wallet.get_transparent_balance().await,
        }
    }

    /// Returns the wallet balance, broken out into several figures that are expected to be meaningful to the user.
    /// # Parameters
    /// * `auto_shielding` - if true, UTXOs will be considered immature rather than spendable.
    #[allow(deprecated)]
    #[deprecated(note = "uses unstable deprecated functions")]
    pub async fn get_user_balances(
        &self,
        auto_shielding: bool,
    ) -> Result<UserBalances, ZingoLibError> {
        let mut balances = UserBalances {
            spendable: 0,
            immature_change: 0,
            minimum_fees: 0,
            immature_income: 0,
            dust: 0,
            incoming: 0,
            incoming_dust: 0,
        };

        // anchor height is the highest block height that contains income that are considered spendable.
        let anchor_height = self.wallet.get_anchor_height().await;

        self.wallet
            .transactions()
            .read()
            .await
            .transaction_records_by_id
            .iter()
            .for_each(|(_, tx)| {
                let mature = tx
                    .status
                    .is_confirmed_before_or_at(&BlockHeight::from_u32(anchor_height));
                let incoming = tx.is_incoming_transaction();

                let mut change = 0;
                let mut useful_value = 0;
                let mut dust_value = 0;
                let mut utxo_value = 0;
                let mut inbound_note_count_nodust = 0;
                let mut inbound_utxo_count_nodust = 0;
                let mut change_note_count = 0;

                tx.orchard_notes
                    .iter()
                    .filter(|n| n.spending_tx_status().is_none())
                    .for_each(|n| {
                        let value = n.orchard_crypto_note.value().inner();
                        if !incoming && n.is_change {
                            change += value;
                            change_note_count += 1;
                        } else if incoming {
                            if value > margin_fee() {
                                useful_value += value;
                                inbound_note_count_nodust += 1;
                            } else {
                                dust_value += value;
                            }
                        }
                    });

                tx.sapling_notes
                    .iter()
                    .filter(|n| n.spending_tx_status().is_none())
                    .for_each(|n| {
                        let value = n.sapling_crypto_note.value().inner();
                        if !incoming && n.is_change {
                            change += value;
                            change_note_count += 1;
                        } else if incoming {
                            if value > margin_fee() {
                                useful_value += value;
                                inbound_note_count_nodust += 1;
                            } else {
                                dust_value += value;
                            }
                        }
                    });

                tx.transparent_outputs
                    .iter()
                    .filter(|n| n.spending_tx_status().is_none())
                    .for_each(|n| {
                        // UTXOs are never 'change', as change would have been shielded.
                        if incoming {
                            if n.value > margin_fee() {
                                utxo_value += n.value;
                                inbound_utxo_count_nodust += 1;
                            } else {
                                dust_value += n.value;
                            }
                        }
                    });

                // The fee field only tracks mature income and change.
                balances.minimum_fees += change_note_count * margin_fee();
                if mature {
                    balances.minimum_fees += inbound_note_count_nodust * margin_fee();
                }

                // If auto-shielding, UTXOs are considered immature and do not fall into any of the buckets that
                // the fee balance covers.
                if !auto_shielding {
                    balances.minimum_fees += inbound_utxo_count_nodust * margin_fee();
                }

                if auto_shielding {
                    if !tx.status.is_confirmed() {
                        balances.incoming += utxo_value;
                    } else {
                        balances.immature_income += utxo_value;
                    }
                } else {
                    // UTXOs are spendable even without confirmations.
                    balances.spendable += utxo_value;
                }

                if mature {
                    // Spendable
                    balances.spendable += useful_value + change;
                    balances.dust += dust_value;
                } else if tx.status.is_confirmed() {
                    // Confirmed, but not yet spendable
                    balances.immature_income += useful_value;
                    balances.immature_change += change;
                    balances.dust += dust_value;
                } else {
                    // pending
                    balances.immature_change += change;
                    balances.incoming += useful_value;
                    balances.incoming_dust += dust_value;
                }
            });

        // Add the minimum fee for the receiving note,
        // but only if there exists notes to spend in the buckets that are covered by the minimum_fee.
        if balances.minimum_fees > 0 {
            balances.minimum_fees += margin_fee(); // The receiving note.
        }

        Ok(balances)
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_info(&self) -> String {
        match crate::grpc_connector::get_info(self.get_server_uri()).await {
            Ok(i) => {
                let o = object! {
                    "version" => i.version,
                    "git_commit" => i.git_commit,
                    "server_uri" => self.get_server_uri().to_string(),
                    "vendor" => i.vendor,
                    "taddr_support" => i.taddr_support,
                    "chain_name" => i.chain_name,
                    "sapling_activation_height" => i.sapling_activation_height,
                    "consensus_branch_id" => i.consensus_branch_id,
                    "latest_block_height" => i.block_height
                };
                o.pretty(2)
            }
            Err(e) => e,
        }
    }

    /// Provides a list of value transfers related to this capability
    /// A value transfer is a group of all notes to a specific receiver in a transaction.
    pub async fn value_transfers(&self) -> ValueTransfers {
        fn create_send_value_transfers(
            value_transfers: &mut Vec<ValueTransfer>,
            transaction_summary: &TransactionSummary,
        ) {
            let mut addresses =
                HashMap::with_capacity(transaction_summary.outgoing_tx_data().len());
            transaction_summary
                .outgoing_tx_data()
                .iter()
                .for_each(|outgoing_tx_data| {
                    let address = if let Some(ua) = outgoing_tx_data.recipient_ua.clone() {
                        ua
                    } else {
                        outgoing_tx_data.recipient_address.clone()
                    };
                    // hash map is used to create unique list of addresses as duplicates are not inserted twice
                    addresses.insert(address, outgoing_tx_data.output_index);
                });
            let mut addresses_vec = addresses.into_iter().collect::<Vec<_>>();
            addresses_vec.sort_by_key(|x| x.1);
            addresses_vec.iter().for_each(|(address, _output_index)| {
                let outgoing_data_to_address: Vec<OutgoingTxData> = transaction_summary
                    .outgoing_tx_data()
                    .iter()
                    .filter(|outgoing_tx_data| {
                        let query_address = if let Some(ua) = outgoing_tx_data.recipient_ua.clone()
                        {
                            ua
                        } else {
                            outgoing_tx_data.recipient_address.clone()
                        };
                        query_address == address.clone()
                    })
                    .cloned()
                    .collect();
                let value: u64 = outgoing_data_to_address
                    .iter()
                    .map(|outgoing_tx_data| outgoing_tx_data.value)
                    .sum();
                let memos: Vec<String> = outgoing_data_to_address
                    .iter()
                    .filter_map(|outgoing_tx_data| {
                        if let Memo::Text(memo_text) = outgoing_tx_data.memo.clone() {
                            Some(memo_text.to_string())
                        } else {
                            None
                        }
                    })
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
        }

        let mut value_transfers: Vec<ValueTransfer> = Vec::new();
        let transaction_summaries = self.transaction_summaries().await;

        for tx in transaction_summaries.iter() {
            match tx.kind() {
                TransactionKind::Sent(SendType::Send) => {
                    // create 1 sent value transfer for each non-self recipient address
                    // if recipient_ua is available it overrides recipient_address
                    create_send_value_transfers(&mut value_transfers, tx);

                    // create 1 memo-to-self if a sending transaction receives any number of memos
                    if tx.orchard_notes().iter().any(|note| note.memo().is_some())
                        || tx.sapling_notes().iter().any(|note| note.memo().is_some())
                    {
                        let memos: Vec<String> = tx
                            .orchard_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .chain(
                                tx.sapling_notes()
                                    .iter()
                                    .filter_map(|note| note.memo().map(|memo| memo.to_string())),
                            )
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
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
                    if !tx.orchard_notes().is_empty() {
                        let value: u64 =
                            tx.orchard_notes().iter().map(|output| output.value()).sum();
                        let memos: Vec<String> = tx
                            .orchard_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
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
                    if !tx.sapling_notes().is_empty() {
                        let value: u64 =
                            tx.sapling_notes().iter().map(|output| output.value()).sum();
                        let memos: Vec<String> = tx
                            .sapling_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
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
                    if tx.orchard_notes().iter().any(|note| note.memo().is_some())
                        || tx.sapling_notes().iter().any(|note| note.memo().is_some())
                    {
                        let memos: Vec<String> = tx
                            .orchard_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .chain(
                                tx.sapling_notes()
                                    .iter()
                                    .filter_map(|note| note.memo().map(|memo| memo.to_string())),
                            )
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
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
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
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
                    create_send_value_transfers(&mut value_transfers, tx);
                }
                TransactionKind::Received => {
                    // create 1 received value tansfer for each pool received to
                    if !tx.orchard_notes().is_empty() {
                        let value: u64 =
                            tx.orchard_notes().iter().map(|output| output.value()).sum();
                        let memos: Vec<String> = tx
                            .orchard_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
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
                    if !tx.sapling_notes().is_empty() {
                        let value: u64 =
                            tx.sapling_notes().iter().map(|output| output.value()).sum();
                        let memos: Vec<String> = tx
                            .sapling_notes()
                            .iter()
                            .filter_map(|note| note.memo().map(|memo| memo.to_string()))
                            .collect();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
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
                    if !tx.transparent_coins().is_empty() {
                        let value: u64 = tx
                            .transparent_coins()
                            .iter()
                            .map(|output| output.value())
                            .sum();
                        value_transfers.push(
                            ValueTransferBuilder::new()
                                .txid(tx.txid())
                                .datetime(tx.datetime())
                                .status(tx.status())
                                .blockheight(tx.blockheight())
                                .transaction_fee(tx.fee())
                                .zec_price(tx.zec_price())
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

        ValueTransfers(value_transfers)
    }

    /// TODO: doc comment
    pub async fn value_transfers_json_string(&self) -> String {
        json::JsonValue::from(self.value_transfers().await).pretty(2)
    }

    /// Provides a list of transaction summaries related to this wallet in order of blockheight
    pub async fn transaction_summaries(&self) -> TransactionSummaries {
        let transaction_map = self
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await;
        let transaction_records = &transaction_map.transaction_records_by_id;

        let mut transaction_summaries = transaction_records
            .values()
            .map(|tx| {
                let (kind, value, fee, orchard_notes, sapling_notes, transparent_coins) =
                    basic_transaction_summary_parts(tx, transaction_records, &self.config().chain);

                TransactionSummaryBuilder::new()
                    .txid(tx.txid)
                    .datetime(tx.datetime)
                    .blockheight(tx.status.get_height())
                    .kind(kind)
                    .value(value)
                    .fee(fee)
                    .status(tx.status)
                    .zec_price(tx.price)
                    .orchard_notes(orchard_notes)
                    .sapling_notes(sapling_notes)
                    .transparent_coins(transparent_coins)
                    .outgoing_tx_data(tx.outgoing_tx_data.clone())
                    .build()
                    .expect("all fields should be populated")
            })
            .collect::<Vec<_>>();
        transaction_summaries.sort_by(|sum1, sum2| {
            match sum1.blockheight().cmp(&sum2.blockheight()) {
                Ordering::Equal => {
                    let starts_with_tex = |summary: &TransactionSummary| {
                        summary.outgoing_tx_data().iter().any(|outgoing_txdata| {
                            outgoing_txdata.recipient_address.starts_with("tex")
                        })
                    };
                    match (starts_with_tex(sum1), starts_with_tex(sum2)) {
                        (true, false) => Ordering::Greater,
                        (false, true) => Ordering::Less,
                        (false, false) | (true, true) => Ordering::Equal,
                    }
                }
                otherwise => otherwise,
            }
        });

        TransactionSummaries::new(transaction_summaries)
    }

    /// TODO: doc comment
    pub async fn transaction_summaries_json_string(&self) -> String {
        json::JsonValue::from(self.transaction_summaries().await).pretty(2)
    }

    /// Provides a detailed list of transaction summaries related to this wallet in order of blockheight
    pub async fn detailed_transaction_summaries(&self) -> DetailedTransactionSummaries {
        let transaction_map = self
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await;
        let transaction_records = &transaction_map.transaction_records_by_id;

        let mut transaction_summaries = transaction_records
            .values()
            .map(|tx| {
                let (kind, value, fee, orchard_notes, sapling_notes, transparent_coins) =
                    basic_transaction_summary_parts(tx, transaction_records, &self.config().chain);
                let orchard_nullifiers: Vec<String> = tx
                    .spent_orchard_nullifiers
                    .iter()
                    .map(|nullifier| hex::encode(nullifier.to_bytes()))
                    .collect();
                let sapling_nullifiers: Vec<String> = tx
                    .spent_sapling_nullifiers
                    .iter()
                    .map(hex::encode)
                    .collect();

                DetailedTransactionSummaryBuilder::new()
                    .txid(tx.txid)
                    .datetime(tx.datetime)
                    .blockheight(tx.status.get_height())
                    .kind(kind)
                    .value(value)
                    .fee(fee)
                    .status(tx.status)
                    .zec_price(tx.price)
                    .orchard_notes(orchard_notes)
                    .sapling_notes(sapling_notes)
                    .transparent_coins(transparent_coins)
                    .outgoing_tx_data(tx.outgoing_tx_data.clone())
                    .orchard_nullifiers(orchard_nullifiers)
                    .sapling_nullifiers(sapling_nullifiers)
                    .build()
                    .expect("all fields should be populated")
            })
            .collect::<Vec<_>>();
        transaction_summaries.sort_by_key(|tx| tx.blockheight());

        DetailedTransactionSummaries::new(transaction_summaries)
    }

    /// TODO: doc comment
    pub async fn detailed_transaction_summaries_json_string(&self) -> String {
        json::JsonValue::from(self.detailed_transaction_summaries().await).pretty(2)
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_seed_phrase(&self) -> Result<AccountBackupInfo, &str> {
        match self.wallet.mnemonic() {
            Some(m) => Ok(AccountBackupInfo {
                seed_phrase: m.0.phrase().to_string(),
                birthday: self.wallet.get_birthday().await,
                account_index: m.1,
            }),
            None => Err("This wallet is watch-only or was created without a mnemonic."),
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn do_seed_phrase_sync(&self) -> Result<AccountBackupInfo, &str> {
        Runtime::new()
            .unwrap()
            .block_on(async move { self.do_seed_phrase().await })
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_total_memobytes_to_address(&self) -> finsight::TotalMemoBytesToAddress {
        let value_transfers = self.value_transfers().await.0;
        let mut memobytes_by_address = HashMap::new();
        for value_transfer in value_transfers {
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
        finsight::TotalMemoBytesToAddress(memobytes_by_address)
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_total_spends_to_address(&self) -> finsight::TotalSendsToAddress {
        let values_sent_to_addresses = self.value_transfer_by_to_address().await;
        let mut by_address_number_sends = HashMap::new();
        for key in values_sent_to_addresses.0.keys() {
            let number_sends = values_sent_to_addresses.0[key].len() as u64;
            by_address_number_sends.insert(key.clone(), number_sends);
        }
        finsight::TotalSendsToAddress(by_address_number_sends)
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_total_value_to_address(&self) -> finsight::TotalValueToAddress {
        let values_sent_to_addresses = self.value_transfer_by_to_address().await;
        let mut by_address_total = HashMap::new();
        for key in values_sent_to_addresses.0.keys() {
            let sum = values_sent_to_addresses.0[key].iter().sum();
            by_address_total.insert(key.clone(), sum);
        }
        finsight::TotalValueToAddress(by_address_total)
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_wallet_last_scanned_height(&self) -> JsonValue {
        json::JsonValue::from(self.wallet.last_synced_height().await)
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_server(&self) -> std::sync::RwLockReadGuard<http::Uri> {
        self.config.lightwalletd_uri.read().unwrap()
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_server_uri(&self) -> http::Uri {
        self.config.get_lightwalletd_uri()
    }

    async fn list_sapling_notes(
        &self,
        all_notes: bool,
        anchor_height: BlockHeight,
    ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
        let mut unspent_sapling_notes: Vec<JsonValue> = vec![];
        let mut pending_spent_sapling_notes: Vec<JsonValue> = vec![];
        let mut spent_sapling_notes: Vec<JsonValue> = vec![];
        // Collect Sapling notes
        self.wallet.transaction_context.transaction_metadata_set.read().await.transaction_records_by_id.iter()
            .flat_map( |(transaction_id, transaction_metadata)| {
                transaction_metadata.sapling_notes.iter().filter_map(move |note_metadata|
                    if !all_notes && note_metadata.spending_tx_status().is_some() {
                        None
                    } else {
                        let address = LightWallet::note_address::<sapling_crypto::note_encryption::SaplingDomain>(&self.config.chain, note_metadata, &self.wallet.wallet_capability());
                        let spendable = transaction_metadata.status.is_confirmed_after_or_at(&anchor_height) && note_metadata.spending_tx_status().is_none();

                        let created_block:u32 = transaction_metadata.status.get_height().into();
                        // this object should be created by the DomainOuput trait if this doesnt get deprecated
                        Some(object!{
                            "created_in_block"   => created_block,
                            "datetime"           => transaction_metadata.datetime,
                            "created_in_txid"    => format!("{}", transaction_id),
                            "value"              => note_metadata.sapling_crypto_note.value().inner(),
                            "pending"        => !transaction_metadata.status.is_confirmed(),
                            "address"            => address,
                            "spendable"          => spendable,
                            "spent"    => note_metadata.spending_tx_status().and_then(|(s_txid, status)| {if status.is_confirmed() {Some(format!("{}", s_txid))} else {None}}),
                            "pending_spent"    => note_metadata.spending_tx_status().and_then(|(s_txid, status)| {if !status.is_confirmed() {Some(format!("{}", s_txid))} else {None}}),
                            "spent_at_height"    => note_metadata.spending_tx_status().map(|(_, status)| u32::from(status.get_height())),
                        })
                    }
                )
            })
            .for_each( |note| {
                self.unspent_pending_spent(note, &mut unspent_sapling_notes, &mut pending_spent_sapling_notes, &mut spent_sapling_notes)
            });
        (
            unspent_sapling_notes,
            spent_sapling_notes,
            pending_spent_sapling_notes,
        )
    }

    async fn list_orchard_notes(
        &self,
        all_notes: bool,
        anchor_height: BlockHeight,
    ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
        let mut unspent_orchard_notes: Vec<JsonValue> = vec![];
        let mut pending_spent_orchard_notes: Vec<JsonValue> = vec![];
        let mut spent_orchard_notes: Vec<JsonValue> = vec![];
        self.wallet.transaction_context.transaction_metadata_set.read().await.transaction_records_by_id.iter()
            .flat_map( |(transaction_id, transaction_metadata)| {
                transaction_metadata.orchard_notes.iter().filter_map(move |note_metadata|
                    if !all_notes && note_metadata.is_spent_confirmed() {
                        None
                    } else {
                        let address = LightWallet::note_address::<OrchardDomain>(&self.config.chain, note_metadata, &self.wallet.wallet_capability());
                        let spendable = transaction_metadata.status.is_confirmed_after_or_at(&anchor_height) && note_metadata.spending_tx_status().is_none();

                        let created_block:u32 = transaction_metadata.status.get_height().into();
                        Some(object!{
                            "created_in_block"   => created_block,
                            "datetime"           => transaction_metadata.datetime,
                            "created_in_txid"    => format!("{}", transaction_id),
                            "value"              => note_metadata.orchard_crypto_note.value().inner(),
                            "pending"        => !transaction_metadata.status.is_confirmed(),
                            "address"            => address,
                            "spendable"          => spendable,
                            "spent"    => note_metadata.spending_tx_status().and_then(|(s_txid, status)| {if status.is_confirmed() {Some(format!("{}", s_txid))} else {None}}),
                            "pending_spent"    => note_metadata.spending_tx_status().and_then(|(s_txid, status)| {if !status.is_confirmed() {Some(format!("{}", s_txid))} else {None}}),
                            "spent_at_height"    => note_metadata.spending_tx_status().map(|(_, status)| u32::from(status.get_height())),
                        })
                    }
                )
            })
            .for_each( |note| {
                self.unspent_pending_spent(note, &mut unspent_orchard_notes, &mut pending_spent_orchard_notes, &mut spent_orchard_notes)
            });
        (
            unspent_orchard_notes,
            spent_orchard_notes,
            pending_spent_orchard_notes,
        )
    }

    async fn list_transparent_outputs(
        &self,
        all_notes: bool,
    ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
        let mut unspent_transparent_notes: Vec<JsonValue> = vec![];
        let mut pending_spent_transparent_note: Vec<JsonValue> = vec![];
        let mut spent_transparent_notes: Vec<JsonValue> = vec![];

        self.wallet.transaction_context.transaction_metadata_set.read().await.transaction_records_by_id.iter()
            .flat_map( |(transaction_id, transaction_record)| {
                transaction_record.transparent_outputs.iter().filter_map(move |utxo|
                    if !all_notes && utxo.is_spent_confirmed() {
                        None
                    } else {
                        let created_block:u32 = transaction_record.status.get_height().into();
                        let recipient = zcash_client_backend::address::Address::decode(&self.config.chain, &utxo.address);
                        let taddr = match recipient {
                        Some(zcash_client_backend::address::Address::Transparent(taddr)) => taddr,
                            _otherwise => panic!("Read invalid taddr from wallet-local Utxo, this should be impossible"),
                        };

                        let spendable = transaction_record.status.is_confirmed() && utxo.spending_tx_status().is_none();
                        Some(object!{
                            "created_in_block"   => created_block,
                            "datetime"           => transaction_record.datetime,
                            "created_in_txid"    => format!("{}", transaction_id),
                            "value"              => utxo.value,
                            "scriptkey"          => hex::encode(utxo.script.clone()),
                            "address"            => self.wallet.wallet_capability().get_ua_from_contained_transparent_receiver(&taddr).map(|ua| ua.encode(&self.config.chain)),
                            "spendable"          => spendable,
                            "spent"    => utxo.spending_tx_status().and_then(|(s_txid, status)| {if status.is_confirmed() {Some(format!("{}", s_txid))} else {None}}),
                            "pending_spent"    => utxo.spending_tx_status().and_then(|(s_txid, status)| {if !status.is_confirmed() {Some(format!("{}", s_txid))} else {None}}),
                            "spent_at_height"    => utxo.spending_tx_status().map(|(_, status)| u32::from(status.get_height())),
                        })
                    }
                )
            })
            .for_each( |note| {
                self.unspent_pending_spent(note, &mut unspent_transparent_notes, &mut pending_spent_transparent_note, &mut spent_transparent_notes)
            });

        (
            unspent_transparent_notes,
            spent_transparent_notes,
            pending_spent_transparent_note,
        )
    }

    /// Get all the outputs packed into an Output vector
    ///  This method will replace do_list_notes
    pub async fn list_outputs(&self) -> Vec<crate::wallet::notes::Output> {
        self.wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .transaction_records_by_id
            .0
            .values()
            .flat_map(Output::get_record_outputs)
            .collect()
    }

    /// Return a list of notes, if `all_notes` is false, then only return unspent notes
    ///  * TODO:  This fn does not handle failure it must be promoted to return a Result
    ///  * TODO:  The Err variant of the result must be a proper type
    ///  * TODO:  remove all_notes bool
    ///  * TODO:   This fn must (on success) return an Ok(Vec\<Notes\>) where Notes is a 3 variant enum....
    ///  * TODO:   type-associated to the variants of the enum must impl From\<Type\> for JsonValue
    ///  * TODO:  DEPRECATE in favor of list_outputs
    pub async fn do_list_notes(&self, all_notes: bool) -> JsonValue {
        let anchor_height = BlockHeight::from_u32(self.wallet.get_anchor_height().await);

        let (mut unspent_sapling_notes, mut spent_sapling_notes, mut pending_spent_sapling_notes) =
            self.list_sapling_notes(all_notes, anchor_height).await;
        let (mut unspent_orchard_notes, mut spent_orchard_notes, mut pending_spent_orchard_notes) =
            self.list_orchard_notes(all_notes, anchor_height).await;
        let (
            mut unspent_transparent_notes,
            mut spent_transparent_notes,
            mut pending_spent_transparent_notes,
        ) = self.list_transparent_outputs(all_notes).await;

        unspent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_spent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        unspent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_spent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        unspent_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_spent_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());

        let mut res = object! {
            "unspent_sapling_notes" => unspent_sapling_notes,
            "pending_sapling_notes" => pending_spent_sapling_notes,
            "unspent_orchard_notes" => unspent_orchard_notes,
            "pending_orchard_notes" => pending_spent_orchard_notes,
            "utxos"         => unspent_transparent_notes,
            "pending_utxos" => pending_spent_transparent_notes,
        };

        if all_notes {
            res["spent_sapling_notes"] = JsonValue::Array(spent_sapling_notes);
            res["spent_orchard_notes"] = JsonValue::Array(spent_orchard_notes);
            res["spent_utxos"] = JsonValue::Array(spent_transparent_notes);
        }

        res
    }

    async fn value_transfer_by_to_address(&self) -> finsight::ValuesSentToAddress {
        let value_transfers = self.value_transfers().await.0;
        let mut amount_by_address = HashMap::new();
        for value_transfer in value_transfers {
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
        finsight::ValuesSentToAddress(amount_by_address)
    }
}
