//! These functions can be called by consumer to learn about the LightClient.

use std::collections::HashMap;

use crate::lightclient::LightClient;
use crate::wallet::data::{
    finsight,
    summaries::{SentValueTransfer, TransactionSummaries, ValueTransferKind, ValueTransfers},
};
use crate::wallet::error::SummaryError;

impl LightClient {
    /// Returns server information.
    pub async fn do_info(&self) -> String {
        match crate::grpc_connector::get_info(self.get_server_uri()).await {
            Ok(i) => {
                let o = json::object! {
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
    // TODO: move to wallet
    pub async fn messages_containing(
        &self,
        filter: Option<&str>,
    ) -> Result<ValueTransfers, SummaryError> {
        let mut value_transfers = self.sorted_value_transfers(true).await?;
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

    /// Wrapper for [crate::wallet::LightWallet::sorted_value_transfers].
    pub async fn sorted_value_transfers(
        &self,
        newer_first: bool,
    ) -> Result<ValueTransfers, SummaryError> {
        self.wallet
            .read()
            .await
            .sorted_value_transfers(newer_first)
            .await
    }

    /// Wrapper for [crate::wallet::LightWallet::value_transfers].
    pub async fn value_transfers(&self) -> Result<ValueTransfers, SummaryError> {
        self.wallet.read().await.value_transfers().await
    }

    /// Wrapper for [crate::wallet::LightWallet::transaction_summaries].
    pub async fn transaction_summaries(&self) -> Result<TransactionSummaries, SummaryError> {
        self.wallet.read().await.transaction_summaries().await
    }

    /// TODO: Add Doc Comment Here!
    // TODO: move to wallet
    pub async fn do_total_memobytes_to_address(
        &self,
    ) -> Result<finsight::TotalMemoBytesToAddress, SummaryError> {
        let value_transfers = self.sorted_value_transfers(true).await?;
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
        Ok(finsight::TotalMemoBytesToAddress(memobytes_by_address))
    }

    /// TODO: Add Doc Comment Here!
    // TODO: move to wallet
    pub async fn do_total_spends_to_address(
        &self,
    ) -> Result<finsight::TotalSendsToAddress, SummaryError> {
        let values_sent_to_addresses = self.value_transfer_by_to_address().await?;
        let mut by_address_number_sends = HashMap::new();
        for key in values_sent_to_addresses.0.keys() {
            let number_sends = values_sent_to_addresses.0[key].len() as u64;
            by_address_number_sends.insert(key.clone(), number_sends);
        }

        Ok(finsight::TotalSendsToAddress(by_address_number_sends))
    }

    /// TODO: Add Doc Comment Here!
    // TODO: move to wallet
    pub async fn do_total_value_to_address(
        &self,
    ) -> Result<finsight::TotalValueToAddress, SummaryError> {
        let values_sent_to_addresses = self.value_transfer_by_to_address().await?;
        let mut by_address_total = HashMap::new();
        for key in values_sent_to_addresses.0.keys() {
            let sum = values_sent_to_addresses.0[key].iter().sum();
            by_address_total.insert(key.clone(), sum);
        }

        Ok(finsight::TotalValueToAddress(by_address_total))
    }

    /// Returns URI of the server the lightclient is connected to.
    pub fn get_server_uri(&self) -> http::Uri {
        self.config.get_lightwalletd_uri()
    }

    // TODO: move to wallet
    async fn value_transfer_by_to_address(
        &self,
    ) -> Result<finsight::ValuesSentToAddress, SummaryError> {
        let value_transfers = self
            .wallet
            .read()
            .await
            .sorted_value_transfers(false)
            .await?;
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

        Ok(finsight::ValuesSentToAddress(amount_by_address))
    }
}
