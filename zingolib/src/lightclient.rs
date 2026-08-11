//! TODO: Add Mod Description Here!

use std::{
    fs::File,
    io::{BufReader, Cursor},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8},
    },
};

use json::JsonValue;
use tokio::{sync::RwLock, task::JoinHandle};

use bip0039::Mnemonic;
use zcash_keys::address::UnifiedAddress;
use zcash_protocol::consensus::BlockHeight;
use zcash_transparent::address::TransparentAddress;

use pepper_sync::{
    error::SyncError,
    keys::transparent::TransparentAddressId,
    sync::{SyncResult, SyncStatus},
    wallet::SyncMode,
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
        summary::data::TransactionSummaries,
    },
};
use error::LightClientError;

pub mod error;
pub mod indexer_history;
pub mod migrate;
pub mod offline;
pub mod propose;
pub mod save;
#[cfg(feature = "nym")]
pub mod select;
pub mod send;
pub mod sync;
pub(crate) mod transmit;

pub use transmit::TransmitProgressHandle;

#[cfg(test)]
mod darkside;
#[cfg(test)]
mod mock_chain_tests;

pub use zingo_netutils::time::DEFAULT_REQUEST_TIMEOUT;

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

/// A successfully fetched ZEC price, attested with the route it traveled,
/// the source that answered, and the time the answer took.
///
/// The route attestation is the mixnet tunnel's local SOCKS5 endpoint the
/// fetch went through. It rides the success value — not a log — so every
/// consumer of [`LightClient::update_current_price`] holds per-fetch
/// evidence that this fetch ran over the mixnet (ADR 0011). The source and
/// round trip ride beside it: the fetch races all three price sources and
/// reports the one whose answer arrived first.
#[cfg(feature = "nym")]
#[derive(Clone, Debug, PartialEq)]
pub struct MixnetPriceFetch {
    /// The current ZEC price in USD.
    pub usd: f32,
    /// The price source whose answer won the three-source race.
    pub source: zingo_price::PriceSource,
    /// Wall-clock time from dispatching the race to the winning answer,
    /// tunnel traversal included.
    pub round_trip: std::time::Duration,
    /// The local SOCKS5 endpoint of the mixnet tunnel this fetch traveled
    /// through.
    pub via_socks5: String,
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
    migration_transmission_uri: Option<http::Uri>,
    wallet: WalletMeta,
    sync_mode: Arc<AtomicU8>,
    sync_handle: Option<JoinHandle<Result<SyncResult, SyncError<WalletError>>>>,
    /// The receiving end of the sync engine's progress channel, answering status queries without the wallet lock.
    sync_progress: tokio::sync::watch::Receiver<Option<SyncStatus>>,
    save_active: Arc<AtomicBool>,
    save_handle: Option<JoinHandle<std::io::Result<()>>>,
    /// Held while a stored proposal is pending, so the wallet state the
    /// proposal selected against cannot shift before the send builds it.
    /// Process-lifetime state beside the stored proposal itself (ADR 0006):
    /// never serialized, minted by the proposing calls, released when the
    /// proposal is consumed, cleared, or fails to come into existence.
    proposal_pause_guard: Option<sync::SyncPauseGuard>,
    /// Live progress of an in-progress immediate migration, or `None` when idle.
    /// A side channel off the wallet lock, so it stays pollable while build and
    /// transmit hold the wallet write lock across their loops.
    immediate_migration_progress: migrate::ImmediateMigrationProgressHandle,
    /// Live progress of the note-splitting round a [`Self::quick_split`] call
    /// is building/transmitting, or `None` when idle. The Phase 1 counterpart
    /// to `immediate_migration_progress`, the same off-the-wallet-lock side channel.
    split_progress: migrate::SplitProgressHandle,
    /// Live progress of a running migration execute batch
    /// ([`Self::execute_due_parts`]), the same side-channel pattern.
    batch_progress: migrate::BatchProgressHandle,
    /// The latest progress line of an in-flight Transmission, or `None` when
    /// idle. A side channel like `immediate_migration_progress`, updated by
    /// `transmit_transactions` (submissions, retries, probes, escalation rounds)
    /// and cleared when the transmission ends.
    transmit_progress: transmit::TransmitProgressHandle,
    /// The cross-session per-indexer attempt history (the indexer diary).
    /// Disk-backed only under the `nym-diary` feature, and recording only
    /// after the session opts in via `set_indexer_diary`. Otherwise the
    /// handle is inert.
    indexer_history: indexer_history::IndexerHistoryHandle,
    /// The mixnet transport slot (ADR 0011, amendment 2026-07-28): the
    /// explicit state Mixnet Mode is read from — unattached, switched off,
    /// or an attached transport. Explicit rather than `Option` so a
    /// deliberate disable stays distinguishable from a transport's absence.
    #[cfg(feature = "nym")]
    mixnet_slot: crate::nym::MixnetSlot,
    /// The Correspondent Pools: ready transports Exit Rotation consumes per
    /// run, refilled in the background under PrioritisePrivacy.
    #[cfg(feature = "nym")]
    correspondent_pools: std::sync::Arc<crate::correspondent::pool::Pools>,
    /// The session tunnel's Clutch, held for the spawned slot proxy's life
    /// and recycled by drop on vacate.
    #[cfg(feature = "nym")]
    slot_clutch: Vec<crate::correspondent::pool::exit_pool::Reservation>,
    /// The session-level Mixnet Mode status channel (ADR 0024, decision 2):
    /// the one shared watch every subscriber reads. Transport transitions
    /// publish from the supervisor's tasks, slot transitions from the
    /// `&mut` methods here; the channel outlives any individual transport,
    /// so an enable after a disable publishes into the same channel a
    /// subscriber already holds.
    #[cfg(feature = "nym")]
    mixnet_status: crate::nym::StatusPublisher,
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
            migration_transmission_uri: config.migration_transmission_uri(),
            wallet: WalletMeta::new(config.get_wallet_path().to_path_buf(), wallet),
            sync_mode: Arc::new(AtomicU8::new(SyncMode::NotRunning as u8)),
            sync_handle: None,
            sync_progress: tokio::sync::watch::channel(None).1,
            save_active: Arc::new(AtomicBool::new(false)),
            save_handle: None,
            proposal_pause_guard: None,
            immediate_migration_progress: migrate::ImmediateMigrationProgressHandle::default(),
            split_progress: migrate::SplitProgressHandle::default(),
            batch_progress: migrate::BatchProgressHandle::default(),
            transmit_progress: transmit::TransmitProgressHandle::default(),
            #[cfg(feature = "nym-diary")]
            indexer_history: indexer_history::IndexerHistoryHandle::beside_wallet(
                &config.get_wallet_path(),
            ),
            #[cfg(not(feature = "nym-diary"))]
            indexer_history: indexer_history::IndexerHistoryHandle::default(),
            #[cfg(feature = "nym")]
            mixnet_slot: crate::nym::MixnetSlot::Unattached,
            #[cfg(feature = "nym")]
            slot_clutch: Vec::new(),
            #[cfg(feature = "nym")]
            correspondent_pools: crate::correspondent::pool::Pools::new(),
            #[cfg(feature = "nym")]
            mixnet_status: crate::nym::status_publisher(),
        })
    }

    /// Wraps an already-constructed wallet, typically from
    /// [`crate::testutils::synthetic_wallet::SyntheticWalletBuilder`], so
    /// client-level APIs that only read wallet state (proposing, balances,
    /// summaries) can be exercised offline. No indexer is configured, so the
    /// client is genuinely offline rather than pointed at a never-contacted
    /// placeholder. The wallet path lives under the OS temp directory and is
    /// never written unless a test saves explicitly.
    #[cfg(any(test, feature = "testutils"))]
    pub async fn new_for_test(wallet: crate::wallet::LightWallet) -> Self {
        zingo_netutils::ensure_default_crypto_provider();
        LightClient {
            indexer: None,
            migration_transmission_uri: None,
            wallet: WalletMeta::new(
                std::env::temp_dir().join("zingolib-synthetic-wallet"),
                wallet,
            ),
            sync_mode: Arc::new(AtomicU8::new(SyncMode::NotRunning as u8)),
            sync_handle: None,
            sync_progress: tokio::sync::watch::channel(None).1,
            save_active: Arc::new(AtomicBool::new(false)),
            save_handle: None,
            proposal_pause_guard: None,
            immediate_migration_progress: migrate::ImmediateMigrationProgressHandle::default(),
            split_progress: migrate::SplitProgressHandle::default(),
            batch_progress: migrate::BatchProgressHandle::default(),
            transmit_progress: transmit::TransmitProgressHandle::default(),
            // Synthetic test wallets have no durable directory; the default
            // handle records nowhere and loads empty.
            indexer_history: indexer_history::IndexerHistoryHandle::default(),
            #[cfg(feature = "nym")]
            mixnet_slot: crate::nym::MixnetSlot::Unattached,
            #[cfg(feature = "nym")]
            slot_clutch: Vec::new(),
            #[cfg(feature = "nym")]
            correspondent_pools: crate::correspondent::pool::Pools::new(),
            #[cfg(feature = "nym")]
            mixnet_status: crate::nym::status_publisher(),
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
            migration_transmission_uri: config.migration_transmission_uri(),
            wallet: WalletMeta::new(config.get_wallet_path().to_path_buf(), wallet),
            sync_mode: Arc::new(AtomicU8::new(SyncMode::NotRunning as u8)),
            sync_handle: None,
            sync_progress: tokio::sync::watch::channel(None).1,
            save_active: Arc::new(AtomicBool::new(false)),
            save_handle: None,
            proposal_pause_guard: None,
            immediate_migration_progress: migrate::ImmediateMigrationProgressHandle::default(),
            split_progress: migrate::SplitProgressHandle::default(),
            batch_progress: migrate::BatchProgressHandle::default(),
            transmit_progress: transmit::TransmitProgressHandle::default(),
            #[cfg(feature = "nym-diary")]
            indexer_history: indexer_history::IndexerHistoryHandle::beside_wallet(
                &config.get_wallet_path(),
            ),
            #[cfg(not(feature = "nym-diary"))]
            indexer_history: indexer_history::IndexerHistoryHandle::default(),
            #[cfg(feature = "nym")]
            mixnet_slot: crate::nym::MixnetSlot::Unattached,
            #[cfg(feature = "nym")]
            slot_clutch: Vec::new(),
            #[cfg(feature = "nym")]
            correspondent_pools: crate::correspondent::pool::Pools::new(),
            #[cfg(feature = "nym")]
            mixnet_status: crate::nym::status_publisher(),
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

    /// A cloneable handle to the in-flight Transmission's latest progress
    /// line, or `None` while no transmission runs. Grab it *before* invoking a
    /// transmitting call (send, shield, transmit, migrate), which borrow
    /// `&mut self`, then poll [`transmit::TransmitProgressHandle::latest`]
    /// concurrently, the same side-channel pattern as
    /// [`Self::immediate_migration_progress_handle`]. The line narrates submissions,
    /// retries, queued probes, and mixnet escalation rounds.
    pub fn transmit_progress_handle(&self) -> transmit::TransmitProgressHandle {
        self.transmit_progress.clone()
    }

    /// A cloneable handle to the cross-session per-indexer attempt history
    /// (the indexer diary).
    /// [`indexer_history::IndexerHistoryHandle::load`] reads the accumulated
    /// record for display or scoring. Transmission arms and diagnostic probes
    /// append to it only in a `nym-diary` build whose session has opted in
    /// via `set_indexer_diary`. In every other configuration the handle is
    /// inert and loads empty.
    pub fn indexer_history_handle(&self) -> indexer_history::IndexerHistoryHandle {
        self.indexer_history.clone()
    }

    /// Opt this session in to (or back out of) recording the indexer diary:
    /// one sanitized line per transmission arm or probe leg, appended to
    /// `indexer-history.tsv` beside the wallet. The choice is never
    /// persisted, and every session starts with recording off.
    #[cfg(feature = "nym-diary")]
    pub fn set_indexer_diary(&self, record: bool) {
        self.indexer_history.set_recording(record);
    }

    /// A cloneable handle to the migration's live progress. Grab it *before*
    /// starting an immediate migration, then poll [`migrate::ImmediateMigrationProgressHandle::status`]
    /// concurrently while the immediate migration holds `&mut self`.
    ///
    /// The handle reads a side channel, not the wallet, so it never blocks on
    /// the wallet write lock the immediate migration holds. It is how a
    /// concurrent poller (a spawned task, or the consumer's existing
    /// sync-status loop) observes progress.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run(
    /// #     mut client: zingolib::lightclient::LightClient,
    /// #     account: zip32::AccountId,
    /// # ) -> Result<(), zingolib::lightclient::error::LightClientError> {
    /// // Grab the handle up front. The immediate migration will borrow `client` exclusively.
    /// let progress = client.immediate_migration_progress_handle();
    ///
    /// // Report from a second task. `status()` reads a side channel, so it
    /// // never blocks on the wallet lock the immediate migration holds across its loops. It
    /// // reads `None` before the immediate migration arms it and once the immediate migration finishes, so
    /// // the `if let` simply skips those ticks.
    /// let reporter = tokio::spawn(async move {
    ///     loop {
    ///         if let Some(p) = progress.status() {
    ///             println!("built {}/{}  sent {}/{}", p.built, p.total, p.sent, p.total);
    ///         }
    ///         tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    ///     }
    /// });
    ///
    /// // The one-call entry pauses any running sync itself, migrates against
    /// // that stable state, and resumes sync afterwards (the `true`).
    /// // Completion is the returned summary (not a progress value), after
    /// // which the handle reads `None` again.
    /// let summary = client.quick_immediate_migration(account, true).await?;
    /// reporter.abort();
    ///
    /// println!(
    ///     "migrated {} zat across {} transactions",
    ///     summary.migrated,
    ///     summary.txids.len(),
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub fn immediate_migration_progress_handle(&self) -> migrate::ImmediateMigrationProgressHandle {
        self.immediate_migration_progress.clone()
    }

    /// A cloneable handle to the note-splitting round's live progress. Grab it
    /// *before* calling [`Self::quick_split`], then poll
    /// [`migrate::SplitProgressHandle::status`] concurrently while the round
    /// holds `&mut self`, the Phase 1 counterpart to
    /// [`Self::immediate_migration_progress_handle`].
    pub fn split_progress_handle(&self) -> migrate::SplitProgressHandle {
        self.split_progress.clone()
    }

    /// A cloneable handle to the execute batch's live progress. Grab it
    /// *before* starting the batch, then poll
    /// [`migrate::BatchProgressHandle::status`] concurrently while the
    /// batch holds `&mut self`, the same pattern as
    /// [`Self::immediate_migration_progress_handle`].
    pub fn batch_progress_handle(&self) -> migrate::BatchProgressHandle {
        self.batch_progress.clone()
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

    /// Disconnects every network capability of the client, returning only
    /// when teardown is complete.
    pub async fn go_offline(&mut self) {
        self.abort_sync().await;
        #[cfg(feature = "nym")]
        {
            self.vacate_mixnet_slot().await;
            self.publish_mixnet_slot_state();
        }
        self.indexer = None;
        self.migration_transmission_uri = None;
    }

    /// Returns a reference to the indexer, or `LightClientError::Offline` if none is configured.
    fn require_indexer(&self) -> Result<&zingo_netutils::GrpcIndexer, LightClientError> {
        self.indexer.as_ref().ok_or(LightClientError::Offline)
    }

    /// Returns the connected server's diagnostics as typed data.
    ///
    /// Failure travels on the error channel. The data channel carries a
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

    /// Update and return the current ZEC price in USD over the Nym mixnet,
    /// hiding the client IP from the price source. The price fetch travels
    /// ONLY over the mixnet (ADR 0011, amendment 2026-07-28, reinstating the
    /// 2026-07-23 rule): the fetch routes through the mixnet when Mixnet Mode
    /// is ready and fails closed in every other state — a typed
    /// [`MixnetNotReady`](crate::nym::MixnetNotReady) refusal while
    /// unattached, bootstrapping, or died, and
    /// [`LightClientError::PriceFetchRequiresMixnet`] while switched off,
    /// because the switched-off consent covers Transmission and never price:
    /// the price API is a third party outside the Zcash ecosystem, and no
    /// availability argument earns it a clearnet tier. A build without the
    /// `nym` feature has no price fetch at all.
    ///
    /// The fetch races all three price sources (Gemini, Kraken, CoinGecko)
    /// through the tunnel and takes the first answer; only when every
    /// source fails does it report an error, naming each source's typed
    /// failure. The returned fetch carries the tunnel endpoint it traveled
    /// through, the winning source, and the race's round-trip time, so
    /// every consumer holds per-fetch evidence of the route rather than
    /// trusting the method name alone.
    #[cfg(feature = "nym")]
    pub async fn update_current_price(&self) -> Result<MixnetPriceFetch, LightClientError> {
        let socks5_addr = match self.mixnet_route()? {
            crate::nym::MixnetRoute::Mixnet(tunnel) => tunnel.into_addr(),
            crate::nym::MixnetRoute::Clearnet => {
                return Err(LightClientError::PriceFetchRequiresMixnet);
            }
        };

        // A spawned session runs the race over its own Price Source Pool
        // member — one fresh Shared exit per run, never the slot's shared
        // tunnel — while an attached session's single platform endpoint
        // carries it as before. The consumed member is stopped whatever
        // the outcome, and the refill draw excludes its exit.
        let pooled = if self
            .mixnet_slot
            .proxy()
            .is_some_and(crate::nym::MixnetProxy::is_spawned)
            && self.correspondent_pools.acquirer().is_some()
        {
            Some(
                self.correspondent_pools
                    .take_or_acquire(|pools| &pools.price)
                    .await
                    .map_err(crate::wallet::error::PriceError::TransportAcquisition)?,
            )
        } else {
            None
        };
        // A pooled member carries its own fresh Shared exit; only an attached
        // session rides the slot's shared tunnel. A member whose transport
        // reports no address died between take and use, so the run refuses
        // rather than silently degrading to the slot tunnel.
        let via_socks5 = match pooled.as_ref() {
            Some(member) => match member.addr() {
                Some(addr) => addr,
                None => {
                    if let Some(member) = pooled {
                        member.retire().await;
                        self.correspondent_pools.ensure_filled();
                    }
                    return Err(LightClientError::from(
                        crate::wallet::error::PriceError::TransportAcquisition(
                            crate::nym::acquire::TransportError::DiedBeforeUse,
                        ),
                    ));
                }
            },
            None => socks5_addr,
        };

        // The fetch runs outside the wallet lock (the net-diag
        // polling-blackout remedy), so a hung tunnel can no longer freeze
        // every wallet-state observer. All sources race through the one
        // tunnel at full width; the first answer wins and the losing legs
        // are cancelled.
        let dispatched = std::time::Instant::now();
        let raced = zingo_price::race_current_price(Some(&via_socks5)).await;
        if let Some(member) = pooled {
            member.retire().await;
            self.correspondent_pools.ensure_filled();
        }
        let raced = raced.map_err(crate::wallet::error::PriceError::from)?;
        let round_trip = dispatched.elapsed();
        self.wallet().write().await.record_price_update(raced.price);
        Ok(MixnetPriceFetch {
            usd: raced.price.price_usd,
            source: raced.source,
            round_trip,
            via_socks5,
        })
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
    /// mode it held before the proposal was created, the decline path of
    /// the two-phase send. Previously the pause a proposal took outlived a
    /// declined proposal until some later send opted into resuming.
    pub async fn clear_proposal(&mut self) {
        self.wallet().write().await.clear_proposal();
        self.release_proposal_pause(true);
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

    /// Record the deliberate clearnet consent for a test client: with the
    /// mixnet compiled in, the slot moves to
    /// [`MixnetMode::SwitchedOff`](crate::nym::MixnetMode) — the same act
    /// the CLI's `network off` performs — so scenario sends transmit over
    /// clearnet instead of refusing `MixnetNotReady`. Without the `nym`
    /// feature the wallet has no mixnet surface and this is a no-op.
    ///
    /// Deliberately unconditional (not `nym`-gated): feature unification
    /// can enable `zingolib/nym` from any workspace member — zingo-cli
    /// carries it as a default feature (ADR 0026) — so a caller keying the
    /// consent on its *own* feature set desyncs from zingolib's and
    /// compiles the consent out exactly when the refusal is compiled in.
    #[cfg(any(test, feature = "testutils"))]
    pub async fn consent_to_clearnet_for_tests(&mut self) {
        #[cfg(feature = "nym")]
        self.disable_mixnet().await;
    }

    #[cfg(any(test, feature = "testutils"))]
    pub async fn new_clearnet_consented(
        config: ClientConfig,
        overwrite: bool,
    ) -> Result<Self, LightClientError> {
        let mut client = Self::new(config, overwrite).await?;
        client.consent_to_clearnet_for_tests().await;
        Ok(client)
    }
}

/// Mixnet Mode toggle (ADR 0011, consumption model A). Enabling spawns the
/// bundled `nym-proxy` child process. Disabling shuts it down. The mode
/// reflects the transport slot's state, and clearnet is reachable only by a
/// deliberate disable, never as a silent fallback: a session that never
/// enabled the mixnet, or whose enable failed, is `Unattached` and refuses.
#[cfg(feature = "nym")]
impl LightClient {
    /// Take whatever transport the slot holds, leaving it `Unattached`, and
    /// shut a held transport down. `Unattached` is deliberately the state a
    /// failed enable leaves behind: by enabling, the user revoked any
    /// standing clearnet consent, and a failure must not silently reinstate
    /// a prior `SwitchedOff`.
    async fn vacate_mixnet_slot(&mut self) {
        self.correspondent_pools.drain_all().await;
        if let crate::nym::MixnetSlot::Attached(running) =
            std::mem::replace(&mut self.mixnet_slot, crate::nym::MixnetSlot::Unattached)
        {
            running.stop().await;
        }
        // Dropping the slot's Clutch recycles the session tunnel's
        // reservations after the transport is gone.
        self.slot_clutch.clear();
    }

    /// Enable Mixnet Mode by spawning the bundled `nym-proxy` binary at
    /// `binary_path`. Returns immediately. [`Self::mixnet_mode`] reports
    /// `Bootstrapping` until the proxy announces its SOCKS5 address and becomes
    /// `Ready`. Enabling while already enabled replaces the running proxy. A
    /// spawn failure leaves the mode `Unattached`, which refuses the mixnet
    /// surfaces — never a fallback to clearnet.
    pub async fn enable_mixnet<R: zingo_netutils::responsiveness::Responsiveness>(
        &mut self,
        binary_path: &std::path::Path,
    ) -> Result<(), crate::nym::acquire::TransportError> {
        self.enable_mixnet_from(
            std::sync::Arc::new(crate::nym::acquire::SpawnedBinary::at(
                binary_path.to_path_buf(),
            )),
            R::CLASS,
        )
        .await
    }

    /// Enables Mixnet Mode on a platform that forbids subprocesses, taking
    /// every transport from `host` instead of spawning one.
    pub async fn enable_mixnet_via_host<R: zingo_netutils::responsiveness::Responsiveness>(
        &mut self,
        host: std::sync::Arc<dyn crate::nym::acquire::ProxyHost>,
    ) -> Result<(), crate::nym::acquire::TransportError> {
        self.enable_mixnet_from(
            std::sync::Arc::new(crate::nym::acquire::HostedProxy::owned_by(host)),
            R::CLASS,
        )
        .await
    }

    /// Enables Mixnet Mode over `acquirer`, the one seam both platforms fill.
    async fn enable_mixnet_from(
        &mut self,
        acquirer: std::sync::Arc<dyn crate::nym::acquire::TransportAcquirable>,
        class: zingo_netutils::responsiveness::ResponsivenessClass,
    ) -> Result<(), crate::nym::acquire::TransportError> {
        self.vacate_mixnet_slot().await;
        let clutch = match self
            .correspondent_pools
            .draw_clutch(acquirer.as_ref())
            .await
        {
            Ok(clutch) => clutch,
            Err(refusal) => {
                // A session that cannot draw a ledgered Clutch refuses;
                // the spawned binary must never self-draw outside the
                // reservation ledger.
                self.publish_mixnet_slot_state();
                return Err(refusal);
            }
        };
        let nodes = crate::correspondent::pool::exit_pool::clutch_nodes(&clutch);
        match crate::nym::acquire::TransportAcquirable::acquire(
            acquirer.as_ref(),
            class,
            &nodes,
            std::sync::Arc::clone(&self.mixnet_status),
        )
        .await
        {
            Ok(proxy) => {
                // The spawn already published Bootstrapping into the session
                // channel; nothing further to announce here. The Clutch is
                // held for the tunnel's life and recycled on vacate.
                self.mixnet_slot = crate::nym::MixnetSlot::Attached(proxy);
                self.slot_clutch = clutch;
                self.correspondent_pools.set_acquirer(acquirer);
                self.correspondent_pools.ensure_filled();
                Ok(())
            }
            Err(error) => {
                // A failed enable leaves Unattached (the user's enable revoked
                // any standing clearnet consent); subscribers must see it.
                // The drawn Clutch drops here, recycling its reservations.
                self.publish_mixnet_slot_state();
                Err(error)
            }
        }
    }

    /// Attaches Mixnet Mode to an already-running, platform-hosted SOCKS5
    /// endpoint that bound `exits`, replacing any running transport.
    pub async fn attach_mixnet(
        &mut self,
        socks5_addr: &str,
        exits: &[String],
    ) -> Result<(), crate::nym::MixnetProxyError> {
        self.vacate_mixnet_slot().await;
        match crate::nym::MixnetProxy::attach(
            socks5_addr,
            exits,
            std::sync::Arc::clone(&self.mixnet_status),
        ) {
            Ok(proxy) => {
                self.mixnet_slot = crate::nym::MixnetSlot::Attached(proxy);
                Ok(())
            }
            Err(error) => {
                self.publish_mixnet_slot_state();
                Err(error)
            }
        }
    }

    /// Disable Mixnet Mode. This is a deliberate, per-session choice — the
    /// only act that reaches [`MixnetMode::SwitchedOff`](crate::nym::MixnetMode):
    /// the mixnet-only surfaces then route over clearnet as informed consent,
    /// and any running transport is shut down.
    pub async fn disable_mixnet(&mut self) {
        self.vacate_mixnet_slot().await;
        self.mixnet_slot = crate::nym::MixnetSlot::SwitchedOff;
        self.publish_mixnet_slot_state();
    }

    /// Publish the slot's current state into the session status channel.
    /// Called after a slot transition settles — never mid-replacement, so
    /// subscribers see deliberate states only, not the transient unattached
    /// between a vacate and its successor.
    fn publish_mixnet_slot_state(&self) {
        self.mixnet_status.send_replace(crate::nym::MixnetStatus {
            mode: self.mixnet_slot.mode(),
            // None for the true slot states; the pinned address of a test
            // stand-in, whose Ready must not publish addressless.
            socks5_addr: self.mixnet_slot.socks5_addr(),
            exits: self.mixnet_slot.exits(),
            bootstrap_detail: None,
            death: None,
        });
    }

    /// The driver entry of the Mixnet Mode session policy (ADR 0024,
    /// decision 2): the one call a session makes at its go-online moment.
    /// Under [`MixnetStartPolicy::ForcedOn`](crate::nym::MixnetStartPolicy)
    /// the transport is provisioned by `strategy` — the bundled binary
    /// spawned from the consumer's platform hints, or an attach to a
    /// platform-hosted endpoint — forcing the mode on so the bootstrap
    /// overlaps sync. Under
    /// [`MixnetStartPolicy::OptedOutThisSession`](crate::nym::MixnetStartPolicy)
    /// the startup opt-out is recorded as the explicit act that reaches
    /// switched off, and nothing is provisioned. A provisioning failure is
    /// returned typed to the caller that expressed the go-online intent and
    /// leaves the mode unattached: refusal, never a silent clearnet.
    ///
    /// Recovery stays explicit (the recovery predicate is
    /// [`MixnetMode::needs_recovery`](crate::nym::MixnetMode::needs_recovery)):
    /// this driver never respawns on its own, and the only wallet-side
    /// clocks remain the supervisor's.
    pub async fn start_mixnet_session(
        &mut self,
        strategy: crate::nym::ProvisionStrategy<'_>,
        policy: crate::nym::MixnetStartPolicy,
    ) -> Result<(), crate::nym::acquire::TransportError> {
        match policy {
            crate::nym::MixnetStartPolicy::OptedOutThisSession => {
                self.disable_mixnet().await;
                Ok(())
            }
            crate::nym::MixnetStartPolicy::ForcedOn => match strategy {
                crate::nym::ProvisionStrategy::Spawn(hints) => {
                    let path = crate::nym::provision::resolve_proxy_path(&hints);
                    log::info!("mixnet session start: spawning nym-proxy at {path}");
                    // The go-online moment is a user act: someone is waiting.
                    self.enable_mixnet::<zingo_netutils::responsiveness::PrioritiseSpeed>(
                        std::path::Path::new(&path),
                    )
                    .await
                }
                crate::nym::ProvisionStrategy::Attach { socks5_addr, exits } => self
                    .attach_mixnet(socks5_addr, exits)
                    .await
                    .map_err(crate::nym::acquire::TransportError::from),
            },
        }
    }

    /// Subscribe to Mixnet Mode: the receiving half of the session's one
    /// status channel, delivering a typed
    /// [`MixnetStatus`](crate::nym::MixnetStatus) snapshot on every
    /// transition. Push replaces poll (ADR 0024, decision 2): no consumer
    /// cadence exists, and the channel's keep-only-latest semantics are the
    /// publication-sequencing guard. The receiver is independent of this
    /// client borrow and survives enable/disable cycles.
    pub fn subscribe_mixnet_status(
        &self,
    ) -> tokio::sync::watch::Receiver<crate::nym::MixnetStatus> {
        self.mixnet_status.subscribe()
    }

    /// The current Mixnet Mode, read from the transport slot:
    /// [`MixnetMode::Unattached`](crate::nym::MixnetMode) before any enable
    /// (and after a failed one),
    /// [`MixnetMode::SwitchedOff`](crate::nym::MixnetMode) after the
    /// deliberate disable, otherwise the transport's lifecycle state
    /// (bootstrapping, ready, or died).
    pub fn mixnet_mode(&self) -> crate::nym::MixnetMode {
        self.mixnet_slot.mode()
    }

    /// The local SOCKS5 address while Mixnet Mode is ready.
    pub fn mixnet_socks5_addr(&self) -> Option<String> {
        self.mixnet_slot.socks5_addr()
    }

    /// Switch Mixnet Mode on for a chain-mock test: the slot reports
    /// [`MixnetMode::Ready`](crate::nym::MixnetMode) at `socks5_addr` with no
    /// child, watcher, or probe behind it, so the test walks the same
    /// fail-closed route resolver and escalation orchestration a live Ready
    /// session does. The transmit path pairs this slot state with arms that
    /// submit over the mock indexer's channel; the address is never dialed.
    #[cfg(any(test, feature = "testutils"))]
    pub async fn switch_on_mixnet_for_tests(&mut self, socks5_addr: &str) {
        self.vacate_mixnet_slot().await;
        self.mixnet_slot = crate::nym::MixnetSlot::AttachedForTests {
            socks5_addr: socks5_addr.to_string(),
        };
        // Every slot transition publishes (the one-shared-watch invariant),
        // the stand-in included.
        self.publish_mixnet_slot_state();
    }

    /// The proxy's latest bootstrap progress line while Mixnet Mode is
    /// bootstrapping, so a user interface can narrate the connect race.
    pub fn mixnet_bootstrap_detail(&self) -> Option<String> {
        self.mixnet_slot
            .proxy()
            .and_then(|proxy| proxy.bootstrap_detail())
    }

    /// Why the transport died, while Mixnet Mode is
    /// [`MixnetMode::Died`](crate::nym::MixnetMode) and the watcher held a
    /// typed cause: the [`zingo_net_diag::NetOpFailure`] record naming the
    /// stage, the target, and the cause chain as a vector, so a `died`
    /// verdict carries *why* without anyone parsing prose.
    pub fn mixnet_death_detail(&self) -> Option<zingo_net_diag::NetOpFailure> {
        self.mixnet_slot
            .proxy()
            .and_then(|proxy| proxy.death_detail())
    }

    /// The latched death read whole — its moment and, when the watcher held
    /// one, its typed cause — while Mixnet Mode is
    /// [`MixnetMode::Died`](crate::nym::MixnetMode); `None` in every other
    /// mode. The moment is what distinguishes a stale latch from a fresh
    /// one; staleness math goes through [`crate::nym::DeathReport::age`].
    pub fn mixnet_death_report(&self) -> Option<crate::nym::DeathReport> {
        self.mixnet_slot
            .proxy()
            .and_then(|proxy| proxy.death_report())
    }

    /// Resolve the fail-closed route every mixnet-only surface must obey: the
    /// mixnet proxy when [`MixnetMode::Ready`](crate::nym::MixnetMode::Ready),
    /// clearnet only when switched off (the deliberate toggle-off), and a
    /// refusal while unattached, bootstrapping, or died. Send, price-fetch,
    /// and the liveness probe share this single resolver.
    pub fn mixnet_route(&self) -> Result<crate::nym::MixnetRoute, crate::nym::MixnetNotReady> {
        crate::nym::resolve_route(self.mixnet_mode(), self.mixnet_socks5_addr())
    }

    /// Runs the mixnet liveness probe against `target`, or against every
    /// Correspondent when `target` is `None`. Indexers are probed
    /// concurrently: each probe runs `GetLightdInfo` through the session's
    /// SOCKS5 proxy and appends its outcome to the cross-session indexer
    /// history. The probe has no clearnet leg and refuses while the mixnet
    /// transport is not ready.
    pub async fn probe_correspondents(
        &self,
        target: Option<http::Uri>,
        timeout: std::time::Duration,
    ) -> Result<Vec<crate::nym::probe::MixnetProbe>, crate::lightclient::error::LightClientError>
    {
        let socks5_addr = match self.mixnet_route()? {
            crate::nym::MixnetRoute::Mixnet(tunnel) => tunnel.into_addr(),
            crate::nym::MixnetRoute::Clearnet => {
                return Err(crate::lightclient::error::LightClientError::ProbeRequiresMixnet);
            }
        };
        if let Some(uri) = &target
            && !crate::nym::probe::probe_eligible(uri)
        {
            return Err(
                crate::lightclient::error::LightClientError::IneligibleProbeTarget(uri.clone()),
            );
        }
        let targets: Vec<http::Uri> = target
            .map_or_else(crate::correspondent::correspondent_indexers, |uri| {
                vec![uri]
            })
            .into_iter()
            .filter(crate::nym::probe::probe_eligible)
            .collect();
        let history = self.indexer_history.clone();
        Ok(futures::future::join_all(targets.iter().map(|indexer| {
            crate::nym::probe::probe_indexer(indexer, &socks5_addr, timeout, &history)
        }))
        .await)
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
    /// `from_bytes` config uses `WalletConfig::Read` with an empty wallet_dir, so no
    /// file is written or read; if `from_bytes` ever regresses into touching disk this
    /// assertion would still pass but the call would fail to construct.
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

        // Reconstruct purely from bytes. Note the config carries no source file path,
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
    /// Failure must travel on the error channel. The data channel carries
    /// only typed data. Downstream FFIs must never have to inspect a
    /// returned value's content to learn whether the call succeeded.
    mod info_contract {
        use crate::lightclient::LightClient;
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

        /// The test client is Indexerless, so the info request must fail,
        /// and that failure must not surface as prose in the data channel.
        ///
        /// This began as the red TDD test for the migration: `do_info`
        /// returned `String`, and the connection failure arrived as a
        /// `Status {..}` Debug string indistinguishable by type from
        /// data. It originally pinned a typed `IndexerError` from a
        /// never-listening lazy endpoint. The offline-mode work replaced
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

    /// The price-fetch error contract for the mixnet route (ADR 0011,
    /// amendments 2026-07-23 and 2026-07-27).
    ///
    /// Every way the opt-in mixnet price fetch can fail must arrive at the
    /// API surface as a typed [`LightClientError`] variant with its source
    /// chain intact: never prose in the data channel, never a silent
    /// clearnet fallback. The route pre-flight variants
    /// (`PriceFetchRequiresMixnet`, `MixnetNotReady::{Unattached, Bootstrapping,
    /// Died}`) pair with `nym::route`'s own `resolve_route` tests; the
    /// tests here pin the surface wiring. The transport-leg contract
    /// (typed connect and timeout failures with their cause chains) is
    /// pinned in `zingo-price`'s own tests, beside the mechanism.
    #[cfg(feature = "nym")]
    mod price_fetch_contract {
        use crate::lightclient::LightClient;
        use crate::lightclient::error::LightClientError;
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

        fn wallet() -> crate::wallet::LightWallet {
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build()
        }

        /// A never-enabled mixnet is a typed refusal, never a clearnet
        /// fallback: the fresh client is `Unattached` (absence is not
        /// consent, ADR 0011 amendment 2026-07-28), and the route pre-flight
        /// runs before any network object is built, so no packet leaves the
        /// process.
        #[tokio::test]
        async fn an_unattached_mixnet_is_a_typed_refusal() {
            let client = LightClient::new_for_test(wallet()).await;

            let error = client
                .update_current_price()
                .await
                .expect_err("no transport was ever enabled, so the mixnet fetch must refuse");
            assert!(
                matches!(
                    error,
                    LightClientError::MixnetNotReady(crate::nym::MixnetNotReady::Unattached)
                ),
                "the refusal must be typed, not prose: {error}"
            );
        }

        /// Mixnet Mode switched off is equally a typed refusal for the
        /// opt-in mixnet fetch: the caller demanded the private route, so a
        /// consented-clearnet mode answers `PriceFetchRequiresMixnet` rather
        /// than quietly fetching over clearnet.
        #[tokio::test]
        async fn switched_off_mode_is_a_typed_refusal() {
            let mut client = LightClient::new_for_test(wallet()).await;
            client.disable_mixnet().await;

            let error = client
                .update_current_price()
                .await
                .expect_err("switched off consents to clearnet, not to a mixnet fetch");
            assert!(
                matches!(error, LightClientError::PriceFetchRequiresMixnet),
                "the refusal must be typed, not prose: {error}"
            );
        }

        /// The startup opt-out is the explicit act (ADR 0024, consent at
        /// start): a deliberate disable on a fresh, never-enabled client
        /// lands SwitchedOff — not Unattached — and the route resolver
        /// consents to clearnet. This is the transition zingo-cli's
        /// --no-mixnet flag records at session start.
        #[tokio::test]
        async fn disable_before_any_enable_records_clearnet_consent() {
            let mut client = LightClient::new_for_test(wallet()).await;
            assert_eq!(client.mixnet_mode(), crate::nym::MixnetMode::Unattached);

            client.disable_mixnet().await;

            assert_eq!(client.mixnet_mode(), crate::nym::MixnetMode::SwitchedOff);
            assert!(matches!(
                client.mixnet_route(),
                Ok(crate::nym::MixnetRoute::Clearnet)
            ));
        }
    }

    /// The session driver's contract (ADR 0024, decision 2).
    ///
    /// The driver entry is the one call a session makes at its go-online
    /// moment; these tests pin its consent-at-start semantics, its typed
    /// refusal on a failed provisioning, the push delivery of every
    /// transition through the session's one status channel, and the
    /// Died-only recovery predicate.
    #[cfg(feature = "nym")]
    mod session_driver_contract {
        use crate::lightclient::LightClient;
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

        fn wallet() -> crate::wallet::LightWallet {
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build()
        }

        /// The driver entry honors the startup opt-out (ADR 0024, consent
        /// at start): OptedOutThisSession lands SwitchedOff without
        /// provisioning anything — the strategy is never exercised — and
        /// the transition reaches subscribers through the session channel.
        #[tokio::test]
        async fn the_driver_records_the_startup_opt_out_and_publishes_it() {
            let mut client = LightClient::new_for_test(wallet()).await;
            let subscriber = client.subscribe_mixnet_status();
            assert_eq!(
                subscriber.borrow().mode,
                crate::nym::MixnetMode::Unattached,
                "the channel opens in the ground state"
            );

            client
                .start_mixnet_session(
                    // A hint set that resolves to no real binary: the
                    // opt-out branch must never try to spawn it.
                    crate::nym::ProvisionStrategy::Spawn(
                        crate::nym::provision::SpawnHints::default(),
                    ),
                    crate::nym::MixnetStartPolicy::OptedOutThisSession,
                )
                .await
                .expect("the opt-out provisions nothing and cannot fail");

            assert_eq!(client.mixnet_mode(), crate::nym::MixnetMode::SwitchedOff);
            assert_eq!(
                subscriber.borrow().mode,
                crate::nym::MixnetMode::SwitchedOff,
                "the slot transition must reach subscribers"
            );
        }

        /// A forced-on attach to a malformed address fails typed and leaves
        /// Unattached — refusal, never clearnet — and publishes the settled
        /// state so a subscriber cannot be left staring at a stale mode.
        #[tokio::test]
        async fn a_failed_forced_on_start_publishes_unattached() {
            let mut client = LightClient::new_for_test(wallet()).await;
            let subscriber = client.subscribe_mixnet_status();

            let error = client
                .start_mixnet_session(
                    crate::nym::ProvisionStrategy::Attach {
                        socks5_addr: "not-a-socket-address",
                        exits: &[],
                    },
                    crate::nym::MixnetStartPolicy::ForcedOn,
                )
                .await
                .expect_err("a malformed attach address must refuse");
            assert!(matches!(
                error,
                crate::nym::acquire::TransportError::Proxy(
                    crate::nym::MixnetProxyError::InvalidAddress { .. }
                )
            ));

            assert_eq!(client.mixnet_mode(), crate::nym::MixnetMode::Unattached);
            assert_eq!(
                subscriber.borrow().mode,
                crate::nym::MixnetMode::Unattached,
                "the failed start's settled state must reach subscribers"
            );
        }

        /// The attached transport's lifecycle reaches subscribers end to
        /// end: a forced-on attach to a refusing localhost port publishes
        /// bootstrapping, then died with the typed readiness failure — all
        /// pushed, never polled. A deliberate disable afterwards publishes
        /// SwitchedOff and, because stop() awaits the aborted watcher, no
        /// stale death can be published over it.
        #[tokio::test]
        async fn attach_lifecycle_and_disable_reach_subscribers_in_order() {
            let mut client = LightClient::new_for_test(wallet()).await;
            let mut subscriber = client.subscribe_mixnet_status();

            client
                .start_mixnet_session(
                    // Port 9 (discard) refuses: readiness fails fast and
                    // the driver lands Died.
                    crate::nym::ProvisionStrategy::Attach {
                        socks5_addr: "127.0.0.1:9",
                        exits: &[],
                    },
                    crate::nym::MixnetStartPolicy::ForcedOn,
                )
                .await
                .expect("a well-formed address attaches");

            let died = subscriber
                .wait_for(|status| status.mode == crate::nym::MixnetMode::Died)
                .await
                .expect("the publisher outlives the wait")
                .clone();
            assert!(
                died.death.and_then(|report| report.detail).is_some(),
                "an attach readiness failure must publish its typed cause"
            );

            client.disable_mixnet().await;
            assert_eq!(
                subscriber.borrow_and_update().mode,
                crate::nym::MixnetMode::SwitchedOff
            );
            tokio::task::yield_now().await;
            assert!(
                !subscriber.has_changed().expect("the publisher is alive"),
                "no stale transport publication may follow the deliberate disable"
            );
        }

        /// The recovery predicate is Died only: the ground state carries no
        /// online intent (a wallet may never have consented to
        /// connectivity), switched off is consent revocation's territory,
        /// and the live states need no repair. Exhaustive over ALL so a new
        /// state must take a position.
        #[test]
        fn the_recovery_predicate_is_died_only() {
            for mode in crate::nym::MixnetMode::ALL {
                assert_eq!(
                    mode.needs_recovery(),
                    matches!(mode, crate::nym::MixnetMode::Died),
                    "{mode} must {}need recovery",
                    if matches!(mode, crate::nym::MixnetMode::Died) {
                        ""
                    } else {
                        "not "
                    }
                );
            }
        }
    }
}
