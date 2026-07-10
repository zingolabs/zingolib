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
    utils::now,
    wallet::{
        LightWallet,
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
pub mod propose;
pub mod save;
pub mod send;
pub mod sync;

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
pub struct LightClient {
    indexer: zingo_netutils::GrpcIndexer,
    migration_broadcast_uri: Option<http::Uri>,
    wallet: WalletMeta,
    sync_mode: Arc<AtomicU8>,
    sync_handle: Option<JoinHandle<Result<SyncResult, SyncError<WalletError>>>>,
    save_active: Arc<AtomicBool>,
    save_handle: Option<JoinHandle<std::io::Result<()>>>,
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

        let indexer = zingo_netutils::GrpcIndexer::new(config.indexer_uri()).await?;

        Ok(LightClient {
            indexer,
            migration_broadcast_uri: config.migration_broadcast_uri(),
            wallet: WalletMeta::new(config.get_wallet_path().to_path_buf(), wallet),
            sync_mode: Arc::new(AtomicU8::new(SyncMode::NotRunning as u8)),
            sync_handle: None,
            save_active: Arc::new(AtomicBool::new(false)),
            save_handle: None,
        })
    }

    /// Wraps an already-constructed wallet — typically from
    /// [`crate::testutils::synthetic_wallet::SyntheticWalletBuilder`] — so
    /// client-level APIs that only read wallet state (proposing, balances,
    /// summaries) can be exercised offline. The indexer URI points at
    /// localhost and is never contacted; the wallet path lives under the OS
    /// temp directory and is never written unless a test saves explicitly.
    #[cfg(any(test, feature = "testutils"))]
    pub async fn new_for_test(wallet: crate::wallet::LightWallet) -> Self {
        zingo_netutils::ensure_default_crypto_provider();
        let indexer =
            zingo_netutils::GrpcIndexer::new_lazy(crate::testutils::port_to_localhost_uri(1))
                .expect("lazy endpoint construction succeeds without connecting");
        LightClient {
            indexer,
            migration_broadcast_uri: None,
            wallet: WalletMeta::new(
                std::env::temp_dir().join("zingolib-synthetic-wallet"),
                wallet,
            ),
            sync_mode: Arc::new(AtomicU8::new(SyncMode::NotRunning as u8)),
            sync_handle: None,
            save_active: Arc::new(AtomicBool::new(false)),
            save_handle: None,
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

        let indexer = zingo_netutils::GrpcIndexer::new(config.indexer_uri()).await?;

        Ok(LightClient {
            indexer,
            migration_broadcast_uri: config.migration_broadcast_uri(),
            wallet: WalletMeta::new(config.get_wallet_path().to_path_buf(), wallet),
            sync_mode: Arc::new(AtomicU8::new(SyncMode::NotRunning as u8)),
            sync_handle: None,
            save_active: Arc::new(AtomicBool::new(false)),
            save_handle: None,
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

    /// Returns URI of the indexer the lightclient is connected to.
    pub fn indexer_uri(&self) -> &http::Uri {
        self.indexer.uri()
    }

    /// Set indexer URI.
    ///
    /// Replaces the current gRPC client(s) with new ones that point at the provided URI.
    pub async fn set_indexer_uri(
        &mut self,
        server: http::Uri,
    ) -> Result<(), zingo_netutils::GetClientError> {
        self.indexer = zingo_netutils::GrpcIndexer::new(server).await?;
        Ok(())
    }

    /// Returns server information as a JSON string.
    ///
    /// The data channel carries only JSON; failure travels on the error
    /// channel. Callers never inspect the returned value's content to
    /// learn whether the call succeeded (zingolabs/zingolib#2446).
    // TODO: return concrete struct with from json impl
    pub async fn do_info(&mut self) -> Result<String, LightClientError> {
        let i = self
            .indexer
            .get_lightd_info(DEFAULT_REQUEST_TIMEOUT)
            .await?;
        let o = json::object! {
            "version" => i.version,
            "git_commit" => i.git_commit,
            "server_uri" => self.indexer.uri().to_string(),
            "vendor" => i.vendor,
            "taddr_support" => i.taddr_support,
            "chain_name" => i.chain_name,
            "sapling_activation_height" => i.sapling_activation_height,
            "consensus_branch_id" => i.consensus_branch_id,
            "latest_block_height" => i.block_height
        };
        Ok(o.pretty(2))
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
            .build();

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
            .build();

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
            .build();
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

    /// The `do_info` data/error channel contract (zingolabs/zingolib#2446).
    ///
    /// Failure must travel on the error channel; the data channel carries
    /// only JSON. Downstream FFIs must never have to inspect a returned
    /// value's content to learn whether the call succeeded.
    mod info_contract {
        use crate::lightclient::LightClient;
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

        /// The test client's lazy endpoint (localhost:1) is never
        /// listening, so the info request must fail — and that failure
        /// must not surface as prose in the data channel.
        ///
        /// This began as the red TDD test for the migration: `do_info`
        /// returned `String`, and the connection failure arrived as a
        /// `Status {..}` Debug string indistinguishable by type from
        /// data. It now pins the migrated contract: the failure is a
        /// typed `IndexerError` on the error channel, and the only
        /// remaining `Ok` construction site builds JSON.
        #[tokio::test]
        async fn do_info_failure_stays_out_of_the_data_channel() {
            let wallet =
                SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                    .build();
            let mut client = LightClient::new_for_test(wallet).await;

            let error = client
                .do_info()
                .await
                .expect_err("nothing listens on the test endpoint");
            assert!(
                matches!(
                    error,
                    crate::lightclient::error::LightClientError::IndexerError(_)
                ),
                "the failure must be typed, not prose: {error}"
            );
        }
    }
}
