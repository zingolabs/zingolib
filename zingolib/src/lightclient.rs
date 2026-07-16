//! TODO: Add Mod Description Here!

use std::{
    fs::File,
    io::{BufReader, Cursor},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8},
    },
    time::Duration,
};

use json::JsonValue;
use tokio::{sync::RwLock, task::JoinHandle};

use bip0039::Mnemonic;
use zcash_keys::address::UnifiedAddress;
use zcash_protocol::consensus::BlockHeight;
use zcash_transparent::address::TransparentAddress;

use pepper_sync::{
    error::SyncError, keys::transparent::TransparentAddressId, sync::SyncResult, wallet::SyncMode,
};
use zingo_netutils::Indexer as _;

use crate::{
    config::{ChainType, ClientConfig, WalletConfig},
    data::ServerInfo,
    utils::now,
    wallet::{
        LightWallet, RecoveryInfo, WalletSettings,
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
pub mod migrate;
pub mod offline;
pub mod propose;
pub mod save;
pub mod send;
pub mod sync;
pub(crate) mod transmit;

#[cfg(test)]
mod darkside;
#[cfg(test)]
mod mock_chain_tests;

pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Wallet struct owned by a [`crate::lightclient::LightClient`], with metadata and immutable wallet data stored outside
/// the read/write lock.
struct WalletMeta {
    /// Full path to wallet file.
    wallet_path: PathBuf,
    /// The chain type, extracted at construction for lock-free access.
    chain_type: ChainType,
    /// The wallet birthday height.
    birthday: BlockHeight,
    /// The mnemonic seed phrase, if this is a spending wallet.
    mnemonic: Option<Mnemonic>,
    /// The locked mutable wallet state.
    wallet_data: Arc<RwLock<LightWallet>>,
}

impl std::fmt::Debug for WalletMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalletMeta")
            .field("wallet_path", &self.wallet_path)
            .field("chain_type", &self.chain_type)
            .field("birthday", &self.birthday)
            .field("mnemonic", &self.mnemonic)
            .finish()
    }
}

impl WalletMeta {
    /// Creates a new `WalletMeta` by wrapping a [`crate::wallet::LightWallet`] in a lock alongside metadata and
    /// immutable wallet data.
    fn new(wallet_path: PathBuf, wallet: LightWallet) -> Self {
        Self {
            wallet_path,
            chain_type: wallet.chain_type(),
            birthday: wallet.birthday(),
            mnemonic: wallet.mnemonic().cloned(),
            wallet_data: Arc::new(RwLock::new(wallet)),
        }
    }
}

/// Struct which owns and manages the [`crate::wallet::LightWallet`]. Responsible for network operations such as
/// storing the indexer URI, creating gRPC clients and syncing the wallet to the blockchain.
///
/// `sync_mode` is an atomic representation of [`pepper_sync::wallet::SyncMode`].
///
/// When `indexer` is `None` the client is in offline mode: balance, address, history and proposal
/// operations work normally, but sync and transmission return [`error::LightClientError::Offline`].
/// Call [`Self::set_indexer_uri`] to connect.
pub struct LightClient {
    indexer: Option<zingo_netutils::GrpcIndexer>,
    migration_broadcast_uri: Option<http::Uri>,
    wallet: WalletMeta,
    sync_mode: Arc<AtomicU8>,
    sync_handle: Option<JoinHandle<Result<SyncResult, SyncError<WalletError>>>>,
    save_active: Arc<AtomicBool>,
    save_handle: Option<JoinHandle<std::io::Result<()>>>,
    /// Held while a stored proposal is pending, so the wallet state the
    /// proposal selected against cannot shift before the send builds it.
    /// Process-lifetime state beside the stored proposal itself (ADR 0006):
    /// never serialized, minted by the proposing calls, released when the
    /// proposal is consumed, cleared, or fails to come into existence.
    proposal_quiescence: Option<sync::SyncQuiescence>,
}

impl LightClient {
    /// Creates a `LightClient` from [`crate::config::ClientConfig`].
    ///
    /// Will fail if a wallet file already exists in the given data directory unless `overwrite` is `true` or the
    /// [`crate::config::WalletConfig`] is of `Read` variant.
    /// `overwrite` has no effect if a wallet is being read from file.
    #[allow(clippy::result_large_err)]
    pub async fn new(config: ClientConfig, overwrite: bool) -> Result<Self, LightClientError> {
        // For https URIs GrpcIndexer::new pre-builds a TLS endpoint, which requires a rustls CryptoProvider.
        zingo_netutils::ensure_default_crypto_provider();

        let wallet = match config.wallet_config() {
            WalletConfig::Read => {
                let buffer = BufReader::new(
                    File::open(config.get_wallet_path()).map_err(LightClientError::FileError)?,
                );

                LightWallet::read(buffer, config.chain_type())
                    .map_err(LightClientError::FileError)?
            }
            _ => {
                #[cfg(not(any(target_os = "ios", target_os = "android")))]
                {
                    if !overwrite && config.get_wallet_path().exists() {
                        return Err(LightClientError::FileError(std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            format!(
                                "Cannot save to given data directory as a wallet file already exists at:\n{}",
                                config.get_wallet_path().display()
                            ),
                        )));
                    }
                }

                LightWallet::new(config.chain_type(), config.wallet_config())?
            }
        };

        // For https URIs GrpcIndexer::new pre-builds a TLS endpoint, which requires a rustls CryptoProvider.
        zingo_netutils::ensure_default_crypto_provider();

        // No configured URI means the client starts offline; set_indexer_uri() connects later.
        let indexer = match config.indexer_uri() {
            Some(uri) => Some(zingo_netutils::GrpcIndexer::new(uri).await?),
            None => None,
        };

        Ok(LightClient {
            indexer,
            migration_broadcast_uri: config.migration_broadcast_uri(),
            wallet: WalletMeta::new(config.get_wallet_path().to_path_buf(), wallet),
            sync_mode: Arc::new(AtomicU8::new(SyncMode::NotRunning as u8)),
            sync_handle: None,
            save_active: Arc::new(AtomicBool::new(false)),
            save_handle: None,
            proposal_quiescence: None,
        })
    }

    /// Wraps an already-constructed wallet — typically from
    /// [`crate::testutils::synthetic_wallet::SyntheticWalletBuilder`] — so
    /// client-level APIs that only read wallet state (proposing, balances,
    /// summaries) can be exercised offline. No indexer is configured — the
    /// client is genuinely offline rather than pointed at a never-contacted
    /// placeholder; the wallet path lives under the OS temp directory and is
    /// never written unless a test saves explicitly.
    #[cfg(any(test, feature = "testutils"))]
    pub async fn new_for_test(wallet: crate::wallet::LightWallet) -> Self {
        zingo_netutils::ensure_default_crypto_provider();
        LightClient {
            indexer: None,
            migration_broadcast_uri: None,
            wallet: WalletMeta::new(
                std::env::temp_dir().join("zingolib-synthetic-wallet"),
                wallet,
            ),
            sync_mode: Arc::new(AtomicU8::new(SyncMode::NotRunning as u8)),
            sync_handle: None,
            save_active: Arc::new(AtomicBool::new(false)),
            save_handle: None,
            proposal_quiescence: None,
        }
    }

    /// Creates a [`LightClient`] by deserializing wallet bytes directly, without reading from
    /// a file.
    ///
    /// Intended for mobile platforms (iOS/Android) where the native layer (Swift/Kotlin) owns
    /// all file I/O: the native side reads the wallet file and passes the raw bytes across the
    /// FFI boundary; Rust deserializes from memory via [`std::io::Cursor`]. This avoids the
    /// staging-to-`temp_dir` workaround that consumers had to use to satisfy the
    /// [`WalletConfig::Read`] variant on platforms where the application sandbox cannot write
    /// to the OS temp directory (notably Android, where `std::env::temp_dir()` resolves to
    /// `/tmp` outside the app UID's reach).
    ///
    /// The `config` is still required for the indexer URI and chain type. `wallet_dir` /
    /// `wallet_name` within the config are retained on the resulting [`LightClient`] for any
    /// subsequent save operations, but no file is read here.
    #[allow(clippy::result_large_err)]
    pub async fn from_bytes(
        bytes: Vec<u8>,
        config: ClientConfig,
    ) -> Result<Self, LightClientError> {
        // For https URIs GrpcIndexer::new pre-builds a TLS endpoint, which requires a rustls CryptoProvider.
        zingo_netutils::ensure_default_crypto_provider();

        let wallet = LightWallet::read(Cursor::new(bytes), config.chain_type())
            .map_err(LightClientError::FileError)?;

        let indexer = if let Some(uri) = config.indexer_uri() {
            Some(zingo_netutils::GrpcIndexer::new(uri).await?)
        } else {
            None
        };

        Ok(LightClient {
            indexer,
            migration_broadcast_uri: config.migration_broadcast_uri(),
            wallet: WalletMeta::new(config.get_wallet_path().to_path_buf(), wallet),
            sync_mode: Arc::new(AtomicU8::new(SyncMode::NotRunning as u8)),
            sync_handle: None,
            save_active: Arc::new(AtomicBool::new(false)),
            save_handle: None,
            proposal_quiescence: None,
        })
    }

    /// Returns the chain type for lock-free access.
    pub fn chain_type(&self) -> ChainType {
        self.wallet.chain_type
    }

    /// Returns the wallet birthday height for lock-free access.
    pub fn birthday(&self) -> u32 {
        u32::from(self.wallet.birthday)
    }

    /// Returns the wallet's mnemonic phrase as a string.
    pub fn mnemonic_phrase(&self) -> Option<String> {
        self.wallet
            .mnemonic
            .as_ref()
            .map(|m| m.phrase().to_string())
    }

    /// Returns full path to wallet file.
    pub fn wallet_path(&self) -> PathBuf {
        self.wallet.wallet_path.clone()
    }

    /// Returns path to the directory which holds the wallet file.
    pub fn wallet_dir(&self) -> Result<PathBuf, LightClientError> {
        self.wallet
            .wallet_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                LightClientError::FileError(std::io::Error::other("wallet directory not found!"))
            })
    }

    /// Returns a reference to the locked mutable wallet state.
    // TODO: remove this from public API and replace with APIs to pass through all wallet methods without the consumer having access to the rwlock
    pub fn wallet(&self) -> &Arc<RwLock<LightWallet>> {
        &self.wallet.wallet_data
    }

    /// Returns URI of the connected indexer, or `None` when in offline mode.
    pub fn indexer_uri(&self) -> Option<http::Uri> {
        self.indexer.as_ref().map(|i| i.uri().clone())
    }

    /// Connect to an indexer (or switch to a different one).
    ///
    /// Creates a new gRPC connection to the given URI. After this call the client is online and
    /// network operations such as [`Self::sync`] become available.
    pub async fn set_indexer_uri(
        &mut self,
        server: http::Uri,
    ) -> Result<(), zingo_netutils::GetClientError> {
        self.indexer = Some(zingo_netutils::GrpcIndexer::new(server).await?);
        Ok(())
    }

    /// Returns a reference to the indexer, or `LightClientError::Offline` if none is configured.
    fn require_indexer(&self) -> Result<&zingo_netutils::GrpcIndexer, LightClientError> {
        self.indexer.as_ref().ok_or(LightClientError::Offline)
    }

    /// Returns the connected server's diagnostics as typed data.
    ///
    /// Failure travels on the error channel; the data channel carries a
    /// [`ServerInfo`]. No caller ever inspects a returned value's content
    /// to learn whether the call succeeded (zingolabs/zingolib#2446).
    pub async fn info(&mut self) -> Result<ServerInfo, LightClientError> {
        let mut indexer = self.require_indexer()?.clone();
        let i = indexer.get_lightd_info(DEFAULT_REQUEST_TIMEOUT).await?;
        Ok(ServerInfo {
            version: i.version,
            git_commit: i.git_commit,
            server_uri: indexer.uri().clone(),
            vendor: i.vendor,
            taddr_support: i.taddr_support,
            chain_name: i.chain_name,
            sapling_activation_height: i.sapling_activation_height,
            consensus_branch_id: i.consensus_branch_id,
            latest_block_height: i.block_height,
        })
    }

    /// Wrapper for [`crate::wallet::LightWallet::generate_unified_address`].
    pub async fn generate_unified_address(
        &mut self,
        receivers: ReceiverSelection,
        account_id: zip32::AccountId,
    ) -> Result<(UnifiedAddressId, UnifiedAddress), KeyError> {
        self.wallet()
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
        self.wallet()
            .write()
            .await
            .generate_transparent_address(account_id, enforce_no_gap)
    }

    /// Wrapper for [`crate::wallet::LightWallet::unified_addresses_json`].
    pub async fn unified_addresses_json(&self) -> JsonValue {
        self.wallet().read().await.unified_addresses_json()
    }

    /// Wrapper for [`crate::wallet::LightWallet::transparent_addresses_json`].
    pub async fn transparent_addresses_json(&self) -> JsonValue {
        self.wallet().read().await.transparent_addresses_json()
    }

    /// Wrapper for [`crate::wallet::LightWallet::account_balance`].
    pub async fn account_balance(
        &self,
        account_id: zip32::AccountId,
    ) -> Result<AccountBalance, BalanceError> {
        self.wallet().read().await.account_balance(account_id)
    }

    /// Wrapper for [`crate::wallet::LightWallet::transaction_summaries`].
    pub async fn transaction_summaries(
        &self,
        reverse_sort: bool,
    ) -> Result<TransactionSummaries, SummaryError> {
        self.wallet()
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
        self.wallet()
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
        self.wallet().read().await.messages_containing(filter).await
    }

    /// Wrapper for [`crate::wallet::LightWallet::do_total_memobytes_to_address`].
    pub async fn do_total_memobytes_to_address(
        &self,
    ) -> Result<TotalMemoBytesToAddress, SummaryError> {
        self.wallet()
            .read()
            .await
            .do_total_memobytes_to_address()
            .await
    }

    /// Wrapper for [`crate::wallet::LightWallet::do_total_spends_to_address`].
    pub async fn do_total_spends_to_address(&self) -> Result<TotalSendsToAddress, SummaryError> {
        self.wallet()
            .read()
            .await
            .do_total_spends_to_address()
            .await
    }

    /// Wrapper for [`crate::wallet::LightWallet::do_total_value_to_address`].
    pub async fn do_total_value_to_address(&self) -> Result<TotalValueToAddress, SummaryError> {
        self.wallet().read().await.do_total_value_to_address().await
    }

    /// Creates an additional ZIP-32 account derived from the wallet seed.
    ///
    /// Returns an error if the wallet has no mnemonic (view-only wallets cannot create accounts
    /// this way) or if the maximum account count is reached.
    pub async fn create_account(&mut self) -> Result<(), WalletError> {
        self.wallet().write().await.create_new_account()
    }

    /// Returns seed phrase, birthday, and account count for wallet backup and recovery.
    ///
    /// Returns `None` for view-only wallets (those created from a UFVK or USK without a mnemonic).
    pub async fn recovery_info(&self) -> Option<RecoveryInfo> {
        self.wallet().read().await.recovery_info()
    }

    /// Clears any stored send proposal and restores the sync engine to the
    /// mode it held before the proposal was created — the decline path of
    /// the two-phase send. Previously the pause a proposal took outlived a
    /// declined proposal until some later send opted into resuming.
    pub async fn clear_proposal(&mut self) {
        self.wallet().write().await.clear_proposal();
        self.release_proposal_quiescence(true);
    }

    /// Returns `true` if the wallet has unsaved changes.
    pub async fn is_save_required(&self) -> bool {
        self.wallet().read().await.save_required
    }

    /// Replaces the wallet's runtime settings and marks the wallet dirty.
    pub async fn update_wallet_settings(&mut self, settings: WalletSettings) {
        let mut wallet = self.wallet().write().await;
        wallet.wallet_settings = settings;
        wallet.mark_dirty();
    }

    /// Creates a backup file of the current wallet file in the wallet directory.
    pub fn backup_wallet_file(&self) -> Result<(), LightClientError> {
        let backup_time = now();
        let backup_wallet_path = self.wallet_path().with_extension(
            self.wallet_path()
                .extension()
                .map(|e| format!("backup.{}.{}", backup_time, e.to_string_lossy()))
                .unwrap_or_else(|| format!("backup.{}.dat", backup_time)),
        );

        std::fs::copy(self.wallet_path(), backup_wallet_path)
            .map_err(LightClientError::FileError)?;

        Ok(())
    }
}

/// Mixnet Mode toggle (ADR 0011, consumption model A). Enabling spawns the
/// bundled `nym-proxy` child process; disabling shuts it down. The tri-state
/// reflects the child's lifecycle, and clearnet is reachable only by a
/// deliberate disable, never as a silent fallback.
#[cfg(feature = "nym")]
impl LightClient {
    /// Enable Mixnet Mode by spawning the bundled `nym-proxy` binary at
    /// `binary_path`. Returns immediately; [`Self::mixnet_mode`] reports
    /// `Bootstrapping` until the proxy announces its SOCKS5 address and becomes
    /// `Ready`. Enabling while already enabled replaces the running proxy.
    pub async fn enable_mixnet(
        &mut self,
        binary_path: &std::path::Path,
    ) -> Result<(), crate::nym::MixnetProxyError> {
        if let Some(running) = self.mixnet_proxy.take() {
            running.stop().await;
        }
        self.mixnet_proxy = Some(crate::nym::MixnetProxy::spawn(binary_path)?);
        Ok(())
    }

    /// Disable Mixnet Mode. This is a deliberate, per-session choice: the
    /// mixnet-only surfaces then route over clearnet as informed consent, and
    /// the proxy child is shut down.
    pub async fn disable_mixnet(&mut self) {
        if let Some(running) = self.mixnet_proxy.take() {
            running.stop().await;
        }
    }

    /// The current Mixnet Mode: [`MixnetMode::Off`](crate::nym::MixnetMode)
    /// when disabled, otherwise the proxy's tri-state (bootstrapping or ready).
    pub fn mixnet_mode(&self) -> crate::nym::MixnetMode {
        self.mixnet_proxy
            .as_ref()
            .map_or(crate::nym::MixnetMode::Off, |proxy| proxy.mode())
    }

    /// The local SOCKS5 address while Mixnet Mode is ready.
    pub fn mixnet_socks5_addr(&self) -> Option<String> {
        self.mixnet_proxy
            .as_ref()
            .and_then(|proxy| proxy.socks5_addr())
    }
}

impl std::fmt::Debug for LightClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LightClient")
            .field("indexer", &self.indexer)
            .field("wallet_meta", &self.wallet)
            .field("sync_mode", &self.sync_mode())
            .field(
                "save_active",
                &self.save_active.load(std::sync::atomic::Ordering::Acquire),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        config::{ChainType, ClientConfig, WalletConfig},
        lightclient::{LightClient, error::LightClientError},
        testutils::default_test_wallet_settings,
    };
    use tempfile::TempDir;
    use zingo_common_components::protocol::ActivationHeights;
    use zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED;

    #[tokio::test]
    async fn new_wallet_from_phrase() {
        let temp_dir = TempDir::new().unwrap();
        let config = ClientConfig::builder()
            .set_chain_type(ChainType::Regtest(ActivationHeights::default()))
            .set_wallet_dir(temp_dir.path().to_path_buf())
            .set_wallet_config(WalletConfig::MnemonicPhrase {
                mnemonic_phrase: CHIMNEY_BETTER_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            })
            .build()
            .unwrap();

        let mut lc = LightClient::new(config.clone(), false).await.unwrap();

        lc.save_task().await;
        lc.wait_for_save().await;

        let lc_file_exists_error = LightClient::new(config, false).await.unwrap_err();

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

    /// Round-trips a wallet through `save()` and `from_bytes`, asserting the deserialized
    /// `LightClient` exposes the same derived addresses as the source. Crucially, the
    /// `from_bytes` config uses `WalletConfig::Read` with an empty wallet_dir — no file is
    /// written or read; if `from_bytes` ever regresses into touching disk this assertion
    /// would still pass but the call would fail to construct.
    #[tokio::test]
    async fn from_bytes_roundtrip() {
        let temp_dir = TempDir::new().unwrap();
        let config = ClientConfig::builder()
            .set_chain_type(ChainType::Regtest(ActivationHeights::default()))
            .set_wallet_dir(temp_dir.path().to_path_buf())
            .set_wallet_config(WalletConfig::MnemonicPhrase {
                mnemonic_phrase: CHIMNEY_BETTER_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            })
            .build()
            .unwrap();

        // Source wallet → serialized bytes via the same in-memory `save()` that mobile
        // consumers use to ship the wallet across the FFI.
        let source = LightClient::new(config.clone(), false).await.unwrap();
        let bytes = source
            .wallet()
            .write()
            .await
            .save()
            .expect("save returned an error")
            .expect("nothing to save");

        // Reconstruct purely from bytes — note the config carries no source file path,
        // confirming the constructor never touches the filesystem to load the wallet.
        let restored_config = ClientConfig::builder()
            .set_chain_type(ChainType::Regtest(ActivationHeights::default()))
            .set_wallet_dir(temp_dir.path().to_path_buf())
            .set_wallet_config(WalletConfig::Read)
            .build()
            .unwrap();
        let restored = LightClient::from_bytes(bytes, restored_config)
            .await
            .unwrap();

        assert_eq!(
            source.transparent_addresses_json().await[0]["encoded_address"],
            restored.transparent_addresses_json().await[0]["encoded_address"],
        );
        assert_eq!(
            source.unified_addresses_json().await[0]["encoded_address"],
            restored.unified_addresses_json().await[0]["encoded_address"],
        );
    }

    /// The `info` data/error channel contract (zingolabs/zingolib#2446).
    ///
    /// Failure must travel on the error channel; the data channel carries
    /// only typed data. Downstream FFIs must never have to inspect a
    /// returned value's content to learn whether the call succeeded.
    mod info_contract {
        use crate::lightclient::LightClient;
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

        /// The test client is Indexerless, so the info request must fail —
        /// and that failure must not surface as prose in the data channel.
        ///
        /// This began as the red TDD test for the migration: `do_info`
        /// returned `String`, and the connection failure arrived as a
        /// `Status {..}` Debug string indistinguishable by type from
        /// data. It originally pinned a typed `IndexerError` from a
        /// never-listening lazy endpoint; the offline-mode work replaced
        /// that placeholder with a genuinely Indexerless client, whose
        /// typed failure is `Offline`. The migration ended with `do_info`
        /// deleted outright: `info()` returns a typed `ServerInfo`, and
        /// rendering happens only at presentation boundaries.
        #[tokio::test]
        async fn info_failure_stays_out_of_the_data_channel() {
            let wallet =
                SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                    .build();
            let mut client = LightClient::new_for_test(wallet).await;

            let error = client
                .info()
                .await
                .expect_err("an Indexerless client cannot serve info");
            assert!(
                matches!(error, crate::lightclient::error::LightClientError::Offline),
                "the failure must be typed, not prose: {error}"
            );
        }
    }
}
