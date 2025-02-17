//! These functions can be called by consumer to learn about the LightClient.
use json::{object, JsonValue};
use pepper_sync::wallet::{OrchardNote, SaplingNote, TransparentCoin};
use std::collections::HashMap;
use tokio::runtime::Runtime;

use crate::{
    lightclient::{AccountBackupInfo, LightClient, PoolBalances},
    wallet::data::{
        finsight,
        summaries::{SentValueTransfer, TransactionSummaries, ValueTransferKind, ValueTransfers},
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
pub enum UAReceivers {
    Orchard,
    Shielded,
    All,
}
impl LightClient {
    /// Wrapper for [crate::wallet::LightWallet::do_addresses].
    pub async fn do_addresses(&self, subset: UAReceivers) -> JsonValue {
        self.wallet.lock().await.do_addresses(subset).await
    }

    /// TODO: Redefine the wallet balance functions as non-generics that take a
    /// PoolType variant as an argument, and iterate over a `Vec<Output>`
    pub async fn do_balance(&self) -> PoolBalances {
        let wallet = self.wallet.lock().await;

        let transparent_balance = wallet.confirmed_balance::<TransparentCoin>().await;

        let verified_sapling_balance = wallet.confirmed_balance::<SaplingNote>().await;
        let unverified_sapling_balance = wallet.pending_balance::<SaplingNote>().await;
        let spendable_sapling_balance = wallet.spendable_balance::<SaplingNote>().await;
        let sapling_balance = some_sum(verified_sapling_balance, unverified_sapling_balance);

        let verified_orchard_balance = wallet.confirmed_balance::<OrchardNote>().await;
        let unverified_orchard_balance = wallet.pending_balance::<OrchardNote>().await;
        let spendable_orchard_balance = wallet.spendable_balance::<OrchardNote>().await;
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

            transparent_balance,
        }
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

    /// Provides a list of ValueTransfers associated with the sender, or containing the string.
    pub async fn messages_containing(&self, filter: Option<&str>) -> ValueTransfers {
        let mut value_transfers = self.sorted_value_transfers(true).await;
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

        value_transfers
    }

    /// Wrapper for [crate::wallet::LightWallet::sorted_value_transfers].
    pub async fn sorted_value_transfers(&self, newer_first: bool) -> ValueTransfers {
        self.wallet
            .lock()
            .await
            .sorted_value_transfers(newer_first)
            .await
    }

    /// Wrapper for [crate::wallet::LightWallet::value_transfers].
    pub async fn value_transfers(&self) -> ValueTransfers {
        self.wallet.lock().await.value_transfers().await
    }

    /// Wrapper for [crate::wallet::LightWallet::value_transfers_json_string].
    pub async fn value_transfers_json_string(&self) -> String {
        self.wallet.lock().await.value_transfers_json_string().await
    }

    /// Wrapper for [crate::wallet::LightWallet::transaction_summaries].
    pub async fn transaction_summaries(&self) -> TransactionSummaries {
        self.wallet.lock().await.transaction_summaries().await
    }

    /// Wrapper for [crate::wallet::LightWallet::transaction_summaries_json_string].
    pub async fn transaction_summaries_json_string(&self) -> String {
        self.wallet
            .lock()
            .await
            .transaction_summaries_json_string()
            .await
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_seed_phrase(&self) -> Result<AccountBackupInfo, &str> {
        let wallet = self.wallet.lock().await;
        match wallet.mnemonic() {
            Some(m) => Ok(AccountBackupInfo {
                seed_phrase: m.0.phrase().to_string(),
                birthday: wallet.birthday.into(),
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
        let value_transfers = self.sorted_value_transfers(true).await;
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
    // TODO: revisit
    pub async fn do_wallet_last_scanned_height(&self) -> JsonValue {
        json::JsonValue::from(u32::from(
            self.wallet.lock().await.sync_state.fully_scanned_height(),
        ))
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_server(&self) -> std::sync::RwLockReadGuard<http::Uri> {
        self.config.lightwalletd_uri.read().unwrap()
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_server_uri(&self) -> http::Uri {
        self.config.get_lightwalletd_uri()
    }

    // FIXME: zingo2
    // async fn list_sapling_notes(
    //     &self,
    //     all_notes: bool,
    // ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
    //     let mut unspent_sapling_notes: Vec<JsonValue> = vec![];
    //     let mut pending_spent_sapling_notes: Vec<JsonValue> = vec![];
    //     let mut spent_sapling_notes: Vec<JsonValue> = vec![];
    //     let wallet = self.wallet.lock().await;

    //     // Collect Sapling notes
    //     wallet.transaction_context.transaction_metadata_set.read().await.transaction_records_by_id.iter()
    //         .flat_map( |(transaction_id, transaction_metadata)| {
    //             transaction_metadata.sapling_notes.iter().cloned().filter_map( |note_metadata|
    //                 if !all_notes && note_metadata.spending_tx_status().is_some() {
    //                     None
    //                 } else {
    //                     let address = LightWallet::note_address::<sapling_crypto::note_encryption::SaplingDomain>(&self.config.chain, &note_metadata, &wallet.wallet_capability());
    //                     let spendable = transaction_metadata.status.is_confirmed() && note_metadata.spending_tx_status().is_none();

    //                     let created_block:u32 = transaction_metadata.status.get_height().into();
    //                     // this object should be created by the DomainOuput trait if this doesnt get deprecated
    //                     Some(object!{
    //                         "created_in_block"   => created_block,
    //                         "datetime"           => transaction_metadata.datetime,
    //                         "created_in_txid"    => format!("{}", transaction_id.clone()),
    //                         "value"              => note_metadata.sapling_crypto_note.value().inner(),
    //                         "pending"        => !transaction_metadata.status.is_confirmed(),
    //                         "address"            => address,
    //                         "spendable"          => spendable,
    //                         "spent"    => note_metadata.spending_tx_status().and_then(|(s_txid, status)| {if status.is_confirmed() {Some(format!("{}", s_txid))} else {None}}),
    //                         "pending_spent"    => note_metadata.spending_tx_status().and_then(|(s_txid, status)| {if !status.is_confirmed() {Some(format!("{}", s_txid))} else {None}}),
    //                         "spent_at_height"    => note_metadata.spending_tx_status().map(|(_, status)| u32::from(status.get_height())),
    //                     })
    //                 }
    //             )
    //         })
    //         .for_each( |note| {
    //             self.unspent_pending_spent(note, &mut unspent_sapling_notes, &mut pending_spent_sapling_notes, &mut spent_sapling_notes)
    //         });
    //     (
    //         unspent_sapling_notes,
    //         spent_sapling_notes,
    //         pending_spent_sapling_notes,
    //     )
    // }

    // FIXME: zingo2
    // async fn list_orchard_notes(
    //     &self,
    //     all_notes: bool,
    // ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
    //     let mut unspent_orchard_notes: Vec<JsonValue> = vec![];
    //     let mut pending_spent_orchard_notes: Vec<JsonValue> = vec![];
    //     let mut spent_orchard_notes: Vec<JsonValue> = vec![];
    //     let wallet = self.wallet.lock().await;

    //     wallet.transaction_context.transaction_metadata_set.read().await.transaction_records_by_id.iter()
    //         .flat_map( |(transaction_id, transaction_metadata)| {
    //             transaction_metadata.orchard_notes.iter().cloned().filter_map(|note_metadata|
    //                 if !all_notes && note_metadata.is_spent_confirmed() {
    //                     None
    //                 } else {
    //                     let address = LightWallet::note_address::<OrchardDomain>(&self.config.chain, &note_metadata, &wallet.wallet_capability());
    //                     let spendable = transaction_metadata.status.is_confirmed() && note_metadata.spending_tx_status().is_none();

    //                     let created_block:u32 = transaction_metadata.status.get_height().into();
    //                     Some(object!{
    //                         "created_in_block"   => created_block,
    //                         "datetime"           => transaction_metadata.datetime,
    //                         "created_in_txid"    => format!("{}", transaction_id.clone()),
    //                         "value"              => note_metadata.orchard_crypto_note.value().inner(),
    //                         "pending"        => !transaction_metadata.status.is_confirmed(),
    //                         "address"            => address,
    //                         "spendable"          => spendable,
    //                         "spent"    => note_metadata.spending_tx_status().and_then(|(s_txid, status)| {if status.is_confirmed() {Some(format!("{}", s_txid))} else {None}}),
    //                         "pending_spent"    => note_metadata.spending_tx_status().and_then(|(s_txid, status)| {if !status.is_confirmed() {Some(format!("{}", s_txid))} else {None}}),
    //                         "spent_at_height"    => note_metadata.spending_tx_status().map(|(_, status)| u32::from(status.get_height())),
    //                     })
    //                 }
    //             )
    //         })
    //         .for_each( |note| {
    //             self.unspent_pending_spent(note, &mut unspent_orchard_notes, &mut pending_spent_orchard_notes, &mut spent_orchard_notes)
    //         });
    //     (
    //         unspent_orchard_notes,
    //         spent_orchard_notes,
    //         pending_spent_orchard_notes,
    //     )
    // }

    // FIXME: zingo2
    // async fn list_transparent_outputs(
    //     &self,
    //     all_notes: bool,
    // ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
    //     let mut unspent_transparent_notes: Vec<JsonValue> = vec![];
    //     let mut pending_spent_transparent_note: Vec<JsonValue> = vec![];
    //     let mut spent_transparent_notes: Vec<JsonValue> = vec![];
    //     let wallet = self.wallet.lock().await;

    //     wallet.transaction_context.transaction_metadata_set.read().await.transaction_records_by_id.iter()
    //         .flat_map( |(transaction_id, transaction_record)| {
    //             transaction_record.transparent_outputs.iter().cloned().filter_map(|utxo|
    //                 if !all_notes && utxo.is_spent_confirmed() {
    //                     None
    //                 } else {
    //                     let created_block:u32 = transaction_record.status.get_height().into();
    //                     let recipient = zcash_client_backend::address::Address::decode(&self.config.chain, &utxo.address);
    //                     let taddr = match recipient {
    //                     Some(zcash_client_backend::address::Address::Transparent(taddr)) => taddr,
    //                         _otherwise => panic!("Read invalid taddr from wallet-local Utxo, this should be impossible"),
    //                     };

    //                     let spendable = transaction_record.status.is_confirmed() && utxo.spending_tx_status().is_none();
    //                     Some(object!{
    //                         "created_in_block"   => created_block,
    //                         "datetime"           => transaction_record.datetime,
    //                         "created_in_txid"    => format!("{}", transaction_id.clone()),
    //                         "value"              => utxo.value,
    //                         "scriptkey"          => hex::encode(utxo.script.clone()),
    //                         "address"            => wallet.wallet_capability().get_ua_from_contained_transparent_receiver(&taddr).map(|ua| ua.encode(&self.config.chain)),
    //                         "spendable"          => spendable,
    //                         "spent"    => utxo.spending_tx_status().and_then(|(s_txid, status)| {if status.is_confirmed() {Some(format!("{}", s_txid))} else {None}}),
    //                         "pending_spent"    => utxo.spending_tx_status().and_then(|(s_txid, status)| {if !status.is_confirmed() {Some(format!("{}", s_txid))} else {None}}),
    //                         "spent_at_height"    => utxo.spending_tx_status().map(|(_, status)| u32::from(status.get_height())),
    //                     })
    //                 }
    //             )
    //         })
    //         .for_each( |note| {
    //             self.unspent_pending_spent(note, &mut unspent_transparent_notes, &mut pending_spent_transparent_note, &mut spent_transparent_notes)
    //         });

    //     (
    //         unspent_transparent_notes,
    //         spent_transparent_notes,
    //         pending_spent_transparent_note,
    //     )
    // }

    // FIXME: zingo2
    // /// Get all the outputs packed into an Output vector
    // ///  This method will replace do_list_notes
    // pub async fn list_outputs(&self) -> Vec<crate::wallet::notes::Output> {
    //     self.wallet
    //         .lock()
    //         .await
    //         .transaction_context
    //         .transaction_metadata_set
    //         .read()
    //         .await
    //         .transaction_records_by_id
    //         .0
    //         .values()
    //         .flat_map(Output::get_record_outputs)
    //         .collect()
    // }

    // FIXME: zingo2
    // /// Return a list of notes, if `all_notes` is false, then only return unspent notes
    // ///  * TODO:  This fn does not handle failure it must be promoted to return a Result
    // ///  * TODO:  The Err variant of the result must be a proper type
    // ///  * TODO:  remove all_notes bool
    // ///  * TODO:   This fn must (on success) return an Ok(Vec\<Notes\>) where Notes is a 3 variant enum....
    // ///  * TODO:   type-associated to the variants of the enum must impl From\<Type\> for JsonValue
    // ///  * TODO:  DEPRECATE in favor of list_outputs
    // #[cfg(any(test, feature = "test-elevation"))]
    // pub async fn do_list_notes(&self, all_notes: bool) -> JsonValue {
    //     let (mut unspent_sapling_notes, mut spent_sapling_notes, mut pending_spent_sapling_notes) =
    //         self.list_sapling_notes(all_notes).await;
    //     let (mut unspent_orchard_notes, mut spent_orchard_notes, mut pending_spent_orchard_notes) =
    //         self.list_orchard_notes(all_notes).await;
    //     let (
    //         mut unspent_transparent_notes,
    //         mut spent_transparent_notes,
    //         mut pending_spent_transparent_notes,
    //     ) = self.list_transparent_outputs(all_notes).await;

    //     unspent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
    //     spent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
    //     pending_spent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
    //     unspent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
    //     spent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
    //     pending_spent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
    //     unspent_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());
    //     pending_spent_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());
    //     spent_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());

    //     let mut res = object! {
    //         "unspent_sapling_notes" => unspent_sapling_notes,
    //         "pending_sapling_notes" => pending_spent_sapling_notes,
    //         "unspent_orchard_notes" => unspent_orchard_notes,
    //         "pending_orchard_notes" => pending_spent_orchard_notes,
    //         "utxos"         => unspent_transparent_notes,
    //         "pending_utxos" => pending_spent_transparent_notes,
    //     };

    //     if all_notes {
    //         res["spent_sapling_notes"] = JsonValue::Array(spent_sapling_notes);
    //         res["spent_orchard_notes"] = JsonValue::Array(spent_orchard_notes);
    //         res["spent_utxos"] = JsonValue::Array(spent_transparent_notes);
    //     }

    //     res
    // }

    async fn value_transfer_by_to_address(&self) -> finsight::ValuesSentToAddress {
        let value_transfers = self.wallet.lock().await.sorted_value_transfers(false).await;
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
        finsight::ValuesSentToAddress(amount_by_address)
    }
}
