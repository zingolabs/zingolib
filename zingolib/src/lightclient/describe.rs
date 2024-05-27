//! These functions can be called by consumer to learn about the LightClient.
use json::{object, JsonValue};
use orchard::note_encryption::OrchardDomain;
use sapling_crypto::note_encryption::SaplingDomain;
use std::collections::HashMap;
use tokio::runtime::Runtime;

use zcash_address::ZcashAddress;
use zcash_client_backend::{encoding::encode_payment_address, PoolType, ShieldedProtocol};
use zcash_primitives::{
    consensus::{BlockHeight, NetworkConstants},
    memo::Memo,
    transaction::TxId,
};

use zingoconfig::margin_fee;

use super::{AccountBackupInfo, LightClient, PoolBalances, UserBalances};
use crate::{
    error::ZingoLibError,
    wallet::{
        data::{
            finsight, summaries::ValueTransfer, summaries::ValueTransferKind, OutgoingTxData,
            TransactionRecord,
        },
        keys::address_from_pubkeyhash,
        notes::{query::OutputQuery, OutputInterface},
        LightWallet,
    },
};

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

    /// TODO: Add Doc Comment Here!
    pub async fn do_balance(&self) -> PoolBalances {
        PoolBalances {
            sapling_balance: self
                .wallet
                .shielded_balance::<SaplingDomain>(None, &[])
                .await,
            verified_sapling_balance: self.wallet.verified_balance::<SaplingDomain>(None).await,
            spendable_sapling_balance: self.wallet.spendable_sapling_balance(None).await,
            unverified_sapling_balance: self.wallet.unverified_balance::<SaplingDomain>(None).await,

            orchard_balance: self
                .wallet
                .shielded_balance::<OrchardDomain>(None, &[])
                .await,
            verified_orchard_balance: self.wallet.verified_balance::<OrchardDomain>(None).await,
            spendable_orchard_balance: self.wallet.spendable_orchard_balance(None).await,
            unverified_orchard_balance: self.wallet.unverified_balance::<OrchardDomain>(None).await,

            transparent_balance: self.wallet.tbalance(None).await,
        }
    }

    /// Returns the wallet balance, broken out into several figures that are expected to be meaningful to the user.
    /// # Parameters
    /// * `auto_shielding` - if true, UTXOs will be considered immature rather than spendable.
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
                    .filter(|n| n.spent().is_none() && n.pending_spent.is_none())
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
                    .filter(|n| n.spent().is_none() && n.pending_spent.is_none())
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
                    .filter(|n| !n.is_spent() && n.pending_spent.is_none())
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

    /// TODO: Add Doc Comment Here!
    pub async fn list_txsummaries(&self) -> Vec<ValueTransfer> {
        let mut summaries: Vec<ValueTransfer> = Vec::new();
        let transaction_records_by_id = &self
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .transaction_records_by_id;

        for (txid, transaction_record) in transaction_records_by_id.iter() {
            LightClient::tx_summary_matcher(&mut summaries, *txid, transaction_record);

            if let Ok(tx_fee) =
                transaction_records_by_id.calculate_transaction_fee(transaction_record)
            {
                let (block_height, datetime, price, pending) = (
                    transaction_record.status.get_height(),
                    transaction_record.datetime,
                    transaction_record.price,
                    !transaction_record.status.is_confirmed(),
                );
                summaries.push(ValueTransfer {
                    block_height,
                    datetime,
                    kind: ValueTransferKind::Fee { amount: tx_fee },
                    memos: vec![],
                    price,
                    txid: *txid,
                    pending,
                });
            };
        }
        summaries.sort_by_key(|summary| summary.block_height);
        summaries
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
        let summaries = self.list_txsummaries().await;
        let mut memobytes_by_address = HashMap::new();
        for summary in summaries {
            use ValueTransferKind::*;
            match summary.kind {
                Sent { to_address, .. } => {
                    let address = to_address.encode();
                    let bytes = summary.memos.iter().fold(0, |sum, m| sum + m.len());
                    memobytes_by_address
                        .entry(address)
                        .and_modify(|e| *e += bytes)
                        .or_insert(bytes);
                }
                SendToSelf { .. } | Received { .. } | Fee { .. } => (),
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

    fn tx_summary_matcher(
        summaries: &mut Vec<ValueTransfer>,
        txid: TxId,
        transaction_md: &TransactionRecord,
    ) {
        let (block_height, datetime, price, pending) = (
            transaction_md.status.get_height(),
            transaction_md.datetime,
            transaction_md.price,
            !transaction_md.status.is_confirmed(),
        );
        match (
            transaction_md.is_outgoing_transaction(),
            transaction_md.is_incoming_transaction(),
        ) {
            // This transaction is entirely composed of what we consider
            // to be 'change'. We just make a Fee transfer and move on
            (false, false) => (),
            // All received funds were change, this is a normal send
            (true, false) => {
                for OutgoingTxData {
                    to_address,
                    value,
                    memo,
                    recipient_ua,
                } in &transaction_md.outgoing_tx_data
                {
                    if let Ok(to_address) =
                        ZcashAddress::try_from_encoded(recipient_ua.as_ref().unwrap_or(to_address))
                    {
                        let memos = if let Memo::Text(textmemo) = memo {
                            vec![textmemo.clone()]
                        } else {
                            vec![]
                        };
                        summaries.push(ValueTransfer {
                            block_height,
                            datetime,
                            kind: ValueTransferKind::Sent {
                                to_address,
                                amount: *value,
                            },
                            memos,
                            price,
                            txid,
                            pending,
                        });
                    }
                }
            }
            // No funds spent, this is a normal receipt
            (false, true) => {
                for received_transparent in transaction_md.transparent_outputs.iter() {
                    summaries.push(ValueTransfer {
                        block_height,
                        datetime,
                        kind: ValueTransferKind::Received {
                            pool_type: PoolType::Transparent,
                            amount: received_transparent.value,
                        },
                        memos: vec![],
                        price,
                        txid,
                        pending,
                    });
                }
                for received_sapling in transaction_md.sapling_notes.iter() {
                    let memos = if let Some(Memo::Text(textmemo)) = &received_sapling.memo {
                        vec![textmemo.clone()]
                    } else {
                        vec![]
                    };
                    summaries.push(ValueTransfer {
                        block_height,
                        datetime,
                        kind: ValueTransferKind::Received {
                            pool_type: PoolType::Shielded(ShieldedProtocol::Sapling),
                            amount: received_sapling.value(),
                        },
                        memos,
                        price,
                        txid,
                        pending,
                    });
                }
                for received_orchard in transaction_md.orchard_notes.iter() {
                    let memos = if let Some(Memo::Text(textmemo)) = &received_orchard.memo {
                        vec![textmemo.clone()]
                    } else {
                        vec![]
                    };
                    summaries.push(ValueTransfer {
                        block_height,
                        datetime,
                        kind: ValueTransferKind::Received {
                            pool_type: PoolType::Shielded(ShieldedProtocol::Orchard),
                            amount: received_orchard.value(),
                        },
                        memos,
                        price,
                        txid,
                        pending,
                    });
                }
            }
            // We spent funds, and received funds as non-change. This is most likely a send-to-self,
            // TODO: Figure out what kind of special-case handling we want for these
            (true, true) => {
                summaries.push(ValueTransfer {
                    block_height,
                    datetime,
                    kind: ValueTransferKind::SendToSelf,
                    memos: transaction_md
                        .sapling_notes
                        .iter()
                        .filter_map(|sapling_note| sapling_note.memo.clone())
                        .chain(
                            transaction_md
                                .orchard_notes
                                .iter()
                                .filter_map(|orchard_note| orchard_note.memo.clone()),
                        )
                        .filter_map(|memo| {
                            if let Memo::Text(text_memo) = memo {
                                Some(text_memo)
                            } else {
                                None
                            }
                        })
                        .collect(),
                    price,
                    txid,
                    pending,
                });
            }
        };
    }

    async fn list_sapling_notes(
        &self,
        all_notes: bool,
        anchor_height: BlockHeight,
    ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
        let mut unspent_sapling_notes: Vec<JsonValue> = vec![];
        let mut pending_sapling_notes: Vec<JsonValue> = vec![];
        let mut spent_sapling_notes: Vec<JsonValue> = vec![];
        // Collect Sapling notes
        self.wallet.transaction_context.transaction_metadata_set.read().await.transaction_records_by_id.iter()
            .flat_map( |(transaction_id, transaction_metadata)| {
                transaction_metadata.sapling_notes.iter().filter_map(move |note_metadata|
                    if !all_notes && note_metadata.spent.is_some() {
                        None
                    } else {
                        let address = LightWallet::note_address::<sapling_crypto::note_encryption::SaplingDomain>(&self.config.chain, note_metadata, &self.wallet.wallet_capability());
                        let spendable = transaction_metadata.status.is_confirmed_after_or_at(&anchor_height) && note_metadata.spent.is_none() && note_metadata.pending_spent.is_none();

                        let created_block:u32 = transaction_metadata.status.get_height().into();
                        Some(object!{
                            "created_in_block"   => created_block,
                            "datetime"           => transaction_metadata.datetime,
                            "created_in_txid"    => format!("{}", transaction_id.clone()),
                            "value"              => note_metadata.sapling_crypto_note.value().inner(),
                            "pending"        => !transaction_metadata.status.is_confirmed(),
                            "is_change"          => note_metadata.is_change,
                            "address"            => address,
                            "spendable"          => spendable,
                            "spent"              => note_metadata.spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                            "spent_at_height"    => note_metadata.spent.map(|(_, h)| h),
                            "pending_spent"  => note_metadata.pending_spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                        })
                    }
                )
            })
            .for_each( |note| {
                self.unspent_pending_spent(note, &mut unspent_sapling_notes, &mut pending_sapling_notes, &mut spent_sapling_notes)
            });
        (
            unspent_sapling_notes,
            spent_sapling_notes,
            pending_sapling_notes,
        )
    }

    async fn list_orchard_notes(
        &self,
        all_notes: bool,
        anchor_height: BlockHeight,
    ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
        let mut unspent_orchard_notes: Vec<JsonValue> = vec![];
        let mut pending_orchard_notes: Vec<JsonValue> = vec![];
        let mut spent_orchard_notes: Vec<JsonValue> = vec![];
        self.wallet.transaction_context.transaction_metadata_set.read().await.transaction_records_by_id.iter()
            .flat_map( |(transaction_id, transaction_metadata)| {
                transaction_metadata.orchard_notes.iter().filter_map(move |orch_note_metadata|
                    if !all_notes && orch_note_metadata.is_spent() {
                        None
                    } else {
                        let address = LightWallet::note_address::<orchard::note_encryption::OrchardDomain>(&self.config.chain, orch_note_metadata, &self.wallet.wallet_capability());
                        let spendable = transaction_metadata.status.is_confirmed_after_or_at(&anchor_height) && orch_note_metadata.spent.is_none() && orch_note_metadata.pending_spent.is_none();

                        let created_block:u32 = transaction_metadata.status.get_height().into();
                        Some(object!{
                            "created_in_block"   => created_block,
                            "datetime"           => transaction_metadata.datetime,
                            "created_in_txid"    => format!("{}", transaction_id),
                            "value"              => orch_note_metadata.orchard_crypto_note.value().inner(),
                            "pending"        => !transaction_metadata.status.is_confirmed(),
                            "is_change"          => orch_note_metadata.is_change,
                            "address"            => address,
                            "spendable"          => spendable,
                            "spent"              => orch_note_metadata.spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                            "spent_at_height"    => orch_note_metadata.spent.map(|(_, h)| h),
                            "pending_spent"  => orch_note_metadata.pending_spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                        })
                    }
                )
            })
            .for_each( |note| {
                self.unspent_pending_spent(note, &mut unspent_orchard_notes, &mut pending_orchard_notes, &mut spent_orchard_notes)
            });
        (
            unspent_orchard_notes,
            spent_orchard_notes,
            pending_orchard_notes,
        )
    }

    async fn list_transparent_notes(
        &self,
        all_notes: bool,
    ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
        let mut unspent_transparent_notes: Vec<JsonValue> = vec![];
        let mut pending_transparent_notes: Vec<JsonValue> = vec![];
        let mut spent_transparent_notes: Vec<JsonValue> = vec![];

        self.wallet.transaction_context.transaction_metadata_set.read().await.transaction_records_by_id.iter()
            .flat_map( |(transaction_id, wtx)| {
                wtx.transparent_outputs.iter().filter_map(move |utxo|
                    if !all_notes && utxo.is_spent() {
                        None
                    } else {
                        let created_block:u32 = wtx.status.get_height().into();
                        let recipient = zcash_client_backend::address::Address::decode(&self.config.chain, &utxo.address);
                        let taddr = match recipient {
                        Some(zcash_client_backend::address::Address::Transparent(taddr)) => taddr,
                            _otherwise => panic!("Read invalid taddr from wallet-local Utxo, this should be impossible"),
                        };

                        Some(object!{
                            "created_in_block"   => created_block,
                            "datetime"           => wtx.datetime,
                            "created_in_txid"    => format!("{}", transaction_id),
                            "value"              => utxo.value,
                            "scriptkey"          => hex::encode(utxo.script.clone()),
                            "is_change"          => false, // TODO: Identify notes as change if we send change to our own taddrs
                            "address"            => self.wallet.wallet_capability().get_ua_from_contained_transparent_receiver(&taddr).map(|ua| ua.encode(&self.config.chain)),
                            "spent"              => utxo.spent().map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                            "spent_at_height"    => utxo.spent().map(|(_, h)| h),
                            "pending_spent"  => utxo.pending_spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                        })
                    }
                )
            })
            .for_each( |note| {
                self.unspent_pending_spent(note, &mut unspent_transparent_notes, &mut pending_transparent_notes, &mut spent_transparent_notes)
            });

        (
            unspent_transparent_notes,
            spent_transparent_notes,
            pending_transparent_notes,
        )
    }

    /// Return a list of notes, if `all_notes` is false, then only return unspent notes
    ///  * TODO:  This fn does not handle failure it must be promoted to return a Result
    ///  * TODO:  The Err variant of the result must be a proper type
    ///  * TODO:  remove all_notes bool
    ///  * TODO:   This fn must (on success) return an Ok(Vec\<Notes\>) where Notes is a 3 variant enum....
    ///  * TODO:   type-associated to the variants of the enum must impl From\<Type\> for JsonValue
    pub async fn do_list_notes(&self, all_notes: bool) -> JsonValue {
        let anchor_height = BlockHeight::from_u32(self.wallet.get_anchor_height().await);

        let (mut unspent_sapling_notes, mut spent_sapling_notes, mut pending_sapling_notes) =
            self.list_sapling_notes(all_notes, anchor_height).await;
        let (mut unspent_orchard_notes, mut spent_orchard_notes, mut pending_orchard_notes) =
            self.list_orchard_notes(all_notes, anchor_height).await;
        let (
            mut unspent_transparent_notes,
            mut spent_transparent_notes,
            mut pending_transparent_notes,
        ) = self.list_transparent_notes(all_notes).await;

        unspent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        unspent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        unspent_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());

        let mut res = object! {
            "unspent_sapling_notes" => unspent_sapling_notes,
            "pending_sapling_notes" => pending_sapling_notes,
            "unspent_orchard_notes" => unspent_orchard_notes,
            "pending_orchard_notes" => pending_orchard_notes,
            "utxos"         => unspent_transparent_notes,
            "pending_utxos" => pending_transparent_notes,
        };

        if all_notes {
            res["spent_sapling_notes"] = JsonValue::Array(spent_sapling_notes);
            res["spent_orchard_notes"] = JsonValue::Array(spent_orchard_notes);
            res["spent_utxos"] = JsonValue::Array(spent_transparent_notes);
        }

        res
    }

    async fn value_transfer_by_to_address(&self) -> finsight::ValuesSentToAddress {
        let summaries = self.list_txsummaries().await;
        let mut amount_by_address = HashMap::new();
        for summary in summaries {
            use ValueTransferKind::*;
            match summary.kind {
                Sent { amount, to_address } => {
                    let address = to_address.encode();
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        amount_by_address.entry(address.clone())
                    {
                        e.insert(vec![amount]);
                    } else {
                        amount_by_address
                            .get_mut(&address)
                            .expect("a vec of u64")
                            .push(amount);
                    };
                }
                Fee { amount } => {
                    let fee_key = "fee".to_string();
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        amount_by_address.entry(fee_key.clone())
                    {
                        e.insert(vec![amount]);
                    } else {
                        amount_by_address
                            .get_mut(&fee_key)
                            .expect("a vec of u64.")
                            .push(amount);
                    };
                }
                SendToSelf { .. } | Received { .. } => (),
            }
        }
        finsight::ValuesSentToAddress(amount_by_address)
    }
}
