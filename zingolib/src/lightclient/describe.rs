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
