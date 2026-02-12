//! TODO: Add Mod Description Here!

use std::{
    fs::File,
    io::BufReader,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8},
    },
};

use json::JsonValue;
use tokio::{sync::RwLock, task::JoinHandle};

use zcash_client_backend::tor;
use zcash_keys::address::UnifiedAddress;
use zcash_primitives::{consensus::BlockHeight, legacy::TransparentAddress};

use pepper_sync::{
    error::SyncError, keys::transparent::TransparentAddressId, sync::SyncResult, wallet::SyncMode,
};

use crate::{
    config::ZingoConfig,
    data::proposal::ZingoProposal,
    wallet::{
        LightWallet, WalletBase,
        balance::AccountBalance,
        error::{BalanceError, KeyError, SummaryError, WalletError},
        keys::unified::{ReceiverSelection, UnifiedAddressId},
        summary::data::{
            TransactionSummaries, ValueTransfers,
            finsight::{TotalMemoBytesToAddress, TotalSendsToAddress, TotalValueToAddress},
        },
    },
};
use error::LightClientError;

pub mod error;
pub mod propose;
pub mod save;
pub mod send;
pub mod sync;

/// Struct which owns and manages the [`crate::wallet::LightWallet`]. Responsible for network operations such as
/// storing the indexer URI, creating gRPC clients and syncing the wallet to the blockchain.
///
/// `sync_mode` is an atomic representation of [`pepper_sync::wallet::SyncMode`].
pub struct LightClient {
    // TODO: split zingoconfig so data is not duplicated
    pub config: ZingoConfig,
    /// Tor client
    tor_client: Option<tor::Client>,
    /// Wallet data
    pub wallet: Arc<RwLock<LightWallet>>,
    sync_mode: Arc<AtomicU8>,
    sync_handle: Option<JoinHandle<Result<SyncResult, SyncError<WalletError>>>>,
    save_active: Arc<AtomicBool>,
    save_handle: Option<JoinHandle<std::io::Result<()>>>,
    latest_proposal: Option<ZingoProposal>, // TODO: move to wallet
}

impl LightClient {
    /// Creates a `LightClient` with a new wallet from fresh entropy and a birthday of `chain_height`.
    /// Will fail if a wallet file already exists in the given data directory unless `overwrite` is `true`.
    ///
    /// It is worth considering setting `chain_height` to 100 blocks below current height of block chain to protect
    /// from re-orgs.
    #[allow(clippy::result_large_err)]
    pub fn new(
        config: ZingoConfig,
        chain_height: BlockHeight,
        overwrite: bool,
    ) -> Result<Self, LightClientError> {
        Self::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::FreshEntropy {
                    no_of_accounts: config.no_of_accounts,
                },
                chain_height,
                config.wallet_settings.clone(),
            )?,
            config,
            overwrite,
        )
    }

    /// Creates a `LightClient` from a `wallet` and `config`.
    /// Will fail if a wallet file already exists in the given data directory unless `overwrite` is `true`.
    #[allow(clippy::result_large_err)]
    pub fn create_from_wallet(
        wallet: LightWallet,
        config: ZingoConfig,
        overwrite: bool,
    ) -> Result<Self, LightClientError> {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            if !overwrite && config.wallet_path_exists() {
                return Err(LightClientError::FileError(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "Cannot save to given data directory as a wallet file already exists at:\n{}",
                        config.get_wallet_pathbuf().to_string_lossy()
                    ),
                )));
            }
        }
        Ok(LightClient {
            config,
            tor_client: None,
            wallet: Arc::new(RwLock::new(wallet)),
            sync_mode: Arc::new(AtomicU8::new(SyncMode::NotRunning as u8)),
            sync_handle: None,
            save_active: Arc::new(AtomicBool::new(false)),
            save_handle: None,
            latest_proposal: None,
        })
    }

    /// Create a `LightClient` from an existing wallet file.
    #[allow(clippy::result_large_err)]
    pub fn create_from_wallet_path(config: ZingoConfig) -> Result<Self, LightClientError> {
        let wallet_path = if config.wallet_path_exists() {
            config.get_wallet_path()
        } else {
            return Err(LightClientError::FileError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "Cannot read wallet. No file at {}",
                    config.get_wallet_path().display()
                ),
            )));
        };

        let buffer = BufReader::new(File::open(wallet_path)?);

        Self::create_from_wallet(LightWallet::read(buffer, config.chain)?, config, true)
    }

    /// Returns config used to create lightclient.
    pub fn config(&self) -> &ZingoConfig {
        &self.config
    }

    /// Returns tor client.
    pub fn tor_client(&self) -> Option<&tor::Client> {
        self.tor_client.as_ref()
    }

    /// Returns URI of the server the lightclient is connected to.
    pub fn server_uri(&self) -> http::Uri {
        self.config.get_lightwalletd_uri()
    }

    /// Set the server uri.
    pub fn set_server(&self, server: http::Uri) {
        *self.config.lightwalletd_uri.write().unwrap() = server;
    }

    /// Creates a tor client for current price updates.
    ///
    /// If `tor_dir` is `None` it will be set to the wallet's data directory.
    pub async fn create_tor_client(
        &mut self,
        tor_dir: Option<PathBuf>,
    ) -> Result<(), LightClientError> {
        let tor_dir =
            tor_dir.unwrap_or_else(|| self.config.get_zingo_wallet_dir().to_path_buf().join("tor"));
        tokio::fs::create_dir_all(tor_dir.as_path()).await?;
        self.tor_client = Some(tor::Client::create(tor_dir.as_path(), |_| {}).await?);

        Ok(())
    }

    /// Removes the tor client.
    pub async fn remove_tor_client(&mut self) {
        self.tor_client = None;
    }

    /// Returns server information.
    // TODO: return concrete struct with from json impl
    pub async fn do_info(&self) -> String {
        match crate::grpc_connector::get_info(self.server_uri()).await {
            Ok(i) => {
                let o = json::object! {
                    "version" => i.version,
                    "git_commit" => i.git_commit,
                    "server_uri" => self.server_uri().to_string(),
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

    /// Wrapper for [`crate::wallet::LightWallet::generate_unified_address`].
    pub async fn generate_unified_address(
        &mut self,
        receivers: ReceiverSelection,
        account_id: zip32::AccountId,
    ) -> Result<(UnifiedAddressId, UnifiedAddress), KeyError> {
        self.wallet
            .write()
            .await
            .generate_unified_address(receivers, account_id)
    }

    /// Wrapper for [`crate::wallet::LightWallet::generate_transparent_address`].
    pub async fn generate_transparent_address(
        &mut self,
        account_id: zip32::AccountId,
        enforce_no_gap: bool,
    ) -> Result<(TransparentAddressId, TransparentAddress), KeyError> {
        self.wallet
            .write()
            .await
            .generate_transparent_address(account_id, enforce_no_gap)
    }

    /// Wrapper for [`crate::wallet::LightWallet::unified_addresses_json`].
    pub async fn unified_addresses_json(&self) -> JsonValue {
        self.wallet.read().await.unified_addresses_json()
    }

    /// Wrapper for [`crate::wallet::LightWallet::transparent_addresses_json`].
    pub async fn transparent_addresses_json(&self) -> JsonValue {
        self.wallet.read().await.transparent_addresses_json()
    }

    /// Wrapper for [`crate::wallet::LightWallet::account_balance`].
    pub async fn account_balance(
        &self,
        account_id: zip32::AccountId,
    ) -> Result<AccountBalance, BalanceError> {
        self.wallet.read().await.account_balance(account_id)
    }

    /// Wrapper for [`crate::wallet::LightWallet::transaction_summaries`].
    pub async fn transaction_summaries(
        &self,
        reverse_sort: bool,
    ) -> Result<TransactionSummaries, SummaryError> {
        self.wallet
            .read()
            .await
            .transaction_summaries(reverse_sort)
            .await
    }

    /// Wrapper for [`crate::wallet::LightWallet::value_transfers`].
    pub async fn value_transfers(
        &self,
        sort_highest_to_lowest: bool,
    ) -> Result<ValueTransfers, SummaryError> {
        self.wallet
            .read()
            .await
            .value_transfers(sort_highest_to_lowest)
            .await
    }

    /// Wrapper for [`crate::wallet::LightWallet::messages_containing`].
    pub async fn messages_containing(
        &self,
        filter: Option<&str>,
    ) -> Result<ValueTransfers, SummaryError> {
        self.wallet.read().await.messages_containing(filter).await
    }

    /// Wrapper for [`crate::wallet::LightWallet::do_total_memobytes_to_address`].
    pub async fn do_total_memobytes_to_address(
        &self,
    ) -> Result<TotalMemoBytesToAddress, SummaryError> {
        self.wallet
            .read()
            .await
            .do_total_memobytes_to_address()
            .await
    }

    /// Wrapper for [`crate::wallet::LightWallet::do_total_spends_to_address`].
    pub async fn do_total_spends_to_address(&self) -> Result<TotalSendsToAddress, SummaryError> {
        self.wallet.read().await.do_total_spends_to_address().await
    }

    /// Wrapper for [`crate::wallet::LightWallet::do_total_value_to_address`].
    pub async fn do_total_value_to_address(&self) -> Result<TotalValueToAddress, SummaryError> {
        self.wallet.read().await.do_total_value_to_address().await
    }
}

impl std::fmt::Debug for LightClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LightClient")
            .field("config", &self.config)
            .field("sync_mode", &self.sync_mode())
            .field(
                "save_active",
                &self.save_active.load(std::sync::atomic::Ordering::Acquire),
            )
            .field("latest_proposal", &self.latest_proposal)
            .finish()
    }
}
#[cfg(test)]
mod tests {
    use crate::{
        config::{ChainType, ZingoConfig},
        lightclient::error::LightClientError,
        wallet::LightWallet,
    };
    use bip0039::Mnemonic;
    use tempfile::TempDir;
    use zingo_common_components::protocol::activation_heights::for_test;
    use zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED;

    use crate::{lightclient::LightClient, wallet::WalletBase};

    #[tokio::test]
    async fn new_wallet_from_phrase() {
        let temp_dir = TempDir::new().unwrap();
        let config = ZingoConfig::build(ChainType::Regtest(for_test::all_height_one_nus()))
            .set_wallet_dir(temp_dir.path().to_path_buf())
            .create();
        let mut lc = LightClient::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::Mnemonic {
                    mnemonic: Mnemonic::from_phrase(CHIMNEY_BETTER_SEED.to_string()).unwrap(),
                    no_of_accounts: config.no_of_accounts,
                },
                0.into(),
                config.wallet_settings.clone(),
            )
            .unwrap(),
            config.clone(),
            false,
        )
        .unwrap();

        lc.save_task().await;
        lc.wait_for_save().await;

        let lc_file_exists_error = LightClient::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::Mnemonic {
                    mnemonic: Mnemonic::from_phrase(CHIMNEY_BETTER_SEED.to_string()).unwrap(),
                    no_of_accounts: config.no_of_accounts,
                },
                0.into(),
                config.wallet_settings.clone(),
            )
            .unwrap(),
            config,
            false,
        )
        .unwrap_err();

        assert!(matches!(
            lc_file_exists_error,
            LightClientError::FileError(_)
        ));

        // The first transparent address and unified address should be derived
        assert_eq!(
            "tmYd5GP6JxUxTUcz98NLPumEotvaMPaXytz".to_string(),
            lc.transparent_addresses_json().await[0]["encoded_address"]
        );
        assert_eq!(
            "uregtest15en5x5cnsc7ye3wfy0prnh3ut34ns9w40htunlh9htfl6k5p004ja5gprxfz8fygjeax07a8489wzjk8gsx65thcp6d3ku8umgaka6f0"
                .to_string(),
            lc.unified_addresses_json().await[0]["encoded_address"]
        );
    }
}
