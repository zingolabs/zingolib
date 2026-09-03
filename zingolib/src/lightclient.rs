//! TODO: Add Mod Description Here!

use std::{
    fs::File,
    io::{BufReader, Cursor, Read},
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
#[cfg(feature = "nym")]
mod mixnet;
pub mod offline;
pub mod propose;
pub mod save;
#[cfg(feature = "nym")]
pub mod select;
pub mod send;
pub mod sync;
pub(crate) mod transmit;

pub use save::SaveShutdown;
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

/// A successfully fetched ZEC price with the route it traveled, the
/// source whose answer won the race, and the time the answer took. The
/// route rides the success value so every consumer of
/// [`LightClient::update_current_price`] holds per-fetch evidence of it.
#[cfg(feature = "nym")]
#[derive(Clone, Debug, PartialEq)]
pub struct MixnetPriceFetch {
    /// The current ZEC price in USD.
    pub usd: f32,
    /// The price source whose answer won the race.
    pub source: zingo_price::PriceSource,
    /// Wall-clock time from dispatching the race to the winning answer,
    /// tunnel traversal included on the mixnet route.
    pub round_trip: std::time::Duration,
    /// The route this fetch traveled.
    pub route: PriceFetchRoute,
}

/// The route one price fetch traveled.
#[cfg(feature = "nym")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PriceFetchRoute {
    /// Untunneled HTTP straight to the price sources: the route a
    /// switched-off Mixnet Mode consents to.
    Clearnet,
    /// The mixnet tunnel, reached through its local SOCKS5 endpoint.
    Mixnet {
        /// The local SOCKS5 endpoint the fetch traveled through.
        via_socks5: String,
    },
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
    /// This session's per-indexer attempt history, held in memory for the
    /// life of the process and never written beside the wallet.
    indexer_history: indexer_history::IndexerHistoryHandle,
    /// The interval between transmit retries and queued-verdict probes, held
    /// here so a test can exhaust a probe budget without waiting it out.
    transmit_retry_interval: std::time::Duration,
    /// The mixnet transport slot (ADR 0011, amendment 2026-07-28): the
    /// explicit state Mixnet Mode is read from — unattached, switched off,
    /// or an attached transport. Explicit rather than `Option` so a
    /// deliberate disable stays distinguishable from a transport's absence.
    #[cfg(feature = "nym")]
    mixnet_slot: std::sync::Arc<std::sync::Mutex<crate::mixnet::MixnetSlot>>,
    /// The expiry watchdog driving a new ProofAcquisition the moment the
    /// Standing Client's proof stops being epoch-fresh.
    #[cfg(feature = "nym")]
    standing_watchdog: Option<tokio::task::JoinHandle<()>>,
    /// The rotation watchdog handing the session to a fresh client on the
    /// randomised cadence, where the platform affords one (ADR 0048).
    #[cfg(feature = "nym")]
    rotation_watchdog: Option<tokio::task::JoinHandle<()>>,
    /// The session's exit authority: Reservations, the NodeHealthIndex, and
    /// the acquirer Proven Clients are born from.
    #[cfg(feature = "nym")]
    destination_pools: std::sync::Arc<crate::destination::pool::Pools>,
    /// The session-level Mixnet Mode status channel (ADR 0024, decision 2):
    /// the one shared watch every subscriber reads. Transport transitions
    /// publish from the supervisor's tasks, slot transitions from the
    /// `&mut` methods here; the channel outlives any individual transport,
    /// so an enable after a disable publishes into the same channel a
    /// subscriber already holds.
    #[cfg(feature = "nym")]
    mixnet_status: crate::mixnet::StatusPublisher,
    /// The detached health sweep of the most recent Server-Selection Sweep,
    /// held so revoking consent aborts its traffic with everything else.
    #[cfg(feature = "nym")]
    health_sweep: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The in-flight failover birth, held so a vacate aborts it and a
    /// consent revoked mid-birth is never raced by an untracked task.
    #[cfg(feature = "nym")]
    proof_acquisition: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[cfg(feature = "nym")]
impl Drop for LightClient {
    fn drop(&mut self) {
        // A dropped session must strand no networking task: each held
        // handle is aborted, without awaiting, so the drop stays sync.
        for watchdog in [&mut self.standing_watchdog, &mut self.rotation_watchdog] {
            if let Some(watchdog) = watchdog.take() {
                watchdog.abort();
            }
        }
        for slot in [&self.health_sweep, &self.proof_acquisition] {
            if let Some(task) = slot.lock().expect("a task slot is never poisoned").take() {
                task.abort();
            }
        }
    }
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
            indexer_history: indexer_history::IndexerHistoryHandle::default(),
            transmit_retry_interval: zingo_netutils::time::TRANSMIT_RETRY_INTERVAL,
            #[cfg(feature = "nym")]
            mixnet_slot: std::sync::Arc::new(std::sync::Mutex::new(
                crate::mixnet::MixnetSlot::Unattached,
            )),
            #[cfg(feature = "nym")]
            standing_watchdog: None,
            #[cfg(feature = "nym")]
            rotation_watchdog: None,
            #[cfg(feature = "nym")]
            destination_pools: crate::destination::pool::Pools::new(),
            #[cfg(feature = "nym")]
            mixnet_status: crate::mixnet::status_publisher(),
            #[cfg(feature = "nym")]
            health_sweep: std::sync::Mutex::new(None),
            #[cfg(feature = "nym")]
            proof_acquisition: std::sync::Mutex::new(None),
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
            transmit_retry_interval: zingo_netutils::time::TRANSMIT_RETRY_INTERVAL,
            #[cfg(feature = "nym")]
            mixnet_slot: std::sync::Arc::new(std::sync::Mutex::new(
                crate::mixnet::MixnetSlot::Unattached,
            )),
            #[cfg(feature = "nym")]
            standing_watchdog: None,
            #[cfg(feature = "nym")]
            rotation_watchdog: None,
            #[cfg(feature = "nym")]
            destination_pools: crate::destination::pool::Pools::new(),
            #[cfg(feature = "nym")]
            mixnet_status: crate::mixnet::status_publisher(),
            #[cfg(feature = "nym")]
            health_sweep: std::sync::Mutex::new(None),
            #[cfg(feature = "nym")]
            proof_acquisition: std::sync::Mutex::new(None),
        }
    }

    /// Creates a [`LightClient`] from wallet bytes already in memory, without reading a file.
    ///
    /// Intended for mobile platforms (iOS/Android) where the native layer (Swift/Kotlin) owns
    /// all file I/O: the native side reads the wallet file and passes the raw bytes across the
    /// FFI boundary; Rust deserializes from memory via [`std::io::Cursor`]. This avoids the
    /// staging-to-`temp_dir` workaround that consumers had to use to satisfy the
    /// [`WalletConfig::Read`] variant on platforms where the application sandbox cannot write
    /// to the OS temp directory (notably Android, where `std::env::temp_dir()` resolves to
    /// `/tmp` outside the app UID's reach).
    ///
    /// This is a thin wrapper over [`LightClient::from_reader`] with a [`std::io::Cursor`]. See
    /// that method for how `config` is used.
    #[allow(clippy::result_large_err)]
    pub async fn from_bytes(
        bytes: Vec<u8>,
        config: ClientConfig,
    ) -> Result<Self, LightClientError> {
        Self::from_reader(Cursor::new(bytes), config).await
    }

    /// Creates a [`LightClient`] from a wallet read from `reader`, without a wallet file on disk.
    ///
    /// The `reader` is wrapped in a [`BufReader`] internally, so callers may pass an unbuffered
    /// source such as a [`File`]. The read happens synchronously inside this `async fn`, so a
    /// reader backed by blocking I/O blocks the executor thread for the duration of the read.
    ///
    /// The `config` is still required for the indexer URI and chain type. `wallet_dir` /
    /// `wallet_name` within the config are retained on the resulting [`LightClient`] for any
    /// subsequent save operations, but no file is read here.
    #[allow(clippy::result_large_err)]
    pub async fn from_reader<R: Read>(
        reader: R,
        config: ClientConfig,
    ) -> Result<Self, LightClientError> {
        // For https URIs GrpcIndexer::new pre-builds a TLS endpoint, which requires a rustls CryptoProvider.
        zingo_netutils::ensure_default_crypto_provider();

        let wallet = LightWallet::read(BufReader::new(reader), config.chain_type())
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
            indexer_history: indexer_history::IndexerHistoryHandle::default(),
            transmit_retry_interval: zingo_netutils::time::TRANSMIT_RETRY_INTERVAL,
            #[cfg(feature = "nym")]
            mixnet_slot: std::sync::Arc::new(std::sync::Mutex::new(
                crate::mixnet::MixnetSlot::Unattached,
            )),
            #[cfg(feature = "nym")]
            standing_watchdog: None,
            #[cfg(feature = "nym")]
            rotation_watchdog: None,
            #[cfg(feature = "nym")]
            destination_pools: crate::destination::pool::Pools::new(),
            #[cfg(feature = "nym")]
            mixnet_status: crate::mixnet::status_publisher(),
            #[cfg(feature = "nym")]
            health_sweep: std::sync::Mutex::new(None),
            #[cfg(feature = "nym")]
            proof_acquisition: std::sync::Mutex::new(None),
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

    /// A cloneable handle to this session's per-indexer attempt history,
    /// which [`indexer_history::IndexerHistoryHandle::load`] reads for
    /// display or scoring.
    /// Sets the interval this client waits between transmit retries and queued-verdict probes.
    #[cfg(feature = "testutils")]
    pub fn set_transmit_retry_interval(&mut self, interval: std::time::Duration) {
        self.transmit_retry_interval = interval;
    }

    pub fn indexer_history_handle(&self) -> indexer_history::IndexerHistoryHandle {
        self.indexer_history.clone()
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
            self.abort_health_sweep().await;
            self.vacate_mixnet_slot().await;
            self.publish_mixnet_slot_state();
        }
        self.indexer = None;
        self.migration_transmission_uri = None;
    }

    /// Aborts the detached health sweep, if one is still surveying, so a
    /// revoked consent stops every emission the session started.
    #[cfg(feature = "nym")]
    pub(crate) async fn abort_health_sweep(&self) {
        let held = self
            .health_sweep
            .lock()
            .expect("the health-sweep slot is never poisoned")
            .take();
        if let Some(sweep) = held {
            sweep.abort();
            // Await the cancellation, so the caller's offline promise holds
            // the moment this returns; dropping the sweep's transport kills
            // its child and recycles its lease.
            let _cancelled = sweep.await;
        }
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
    /// [`Indicator::SwitchedOff`](crate::mixnet::Indicator) — the same act
    /// the CLI's `network off` performs — so scenario sends transmit over
    /// clearnet instead of refusing `MixnetNotReady`. Without the `nym`
    /// feature the wallet has no mixnet surface and this is a no-op.
    ///
    /// Deliberately unconditional, because a caller keying the consent on
    /// its own feature set desyncs from zingolib's and compiles the consent
    /// out exactly when the refusal is compiled in.
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
    use std::io::{Cursor, Read};

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

    /// A reader that yields at most one byte per `read` call.
    struct OneByteReader(Cursor<Vec<u8>>);

    impl Read for OneByteReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let len = buf.len().min(1);
            self.0.read(&mut buf[..len])
        }
    }

    /// Feeds one byte per call to `from_reader` and checks the wallet matches `from_bytes`.
    #[tokio::test]
    async fn from_reader_with_one_byte_reads_matches_from_bytes() {
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

        let source = LightClient::new(config, false).await.unwrap();
        let bytes = source
            .wallet()
            .write()
            .await
            .save()
            .expect("save returned an error")
            .expect("nothing to save");

        let read_config = || {
            ClientConfig::builder()
                .set_chain_type(ChainType::Regtest(ActivationHeights::default()))
                .set_wallet_dir(temp_dir.path().to_path_buf())
                .set_wallet_config(WalletConfig::Read)
                .build()
                .unwrap()
        };
        let from_bytes = LightClient::from_bytes(bytes.clone(), read_config())
            .await
            .unwrap();
        let from_reader =
            LightClient::from_reader(OneByteReader(Cursor::new(bytes)), read_config())
                .await
                .unwrap();

        assert_eq!(
            from_reader.mnemonic_phrase().as_deref(),
            Some(CHIMNEY_BETTER_SEED)
        );
        assert_eq!(from_bytes.mnemonic_phrase(), from_reader.mnemonic_phrase());
        assert_eq!(from_bytes.birthday(), from_reader.birthday());
        assert_eq!(
            from_bytes.transparent_addresses_json().await,
            from_reader.transparent_addresses_json().await,
        );
        assert_eq!(
            from_bytes.unified_addresses_json().await,
            from_reader.unified_addresses_json().await,
        );
        let chain_type = from_bytes.chain_type();
        let mut bytes_serialized = Vec::new();
        from_bytes
            .wallet()
            .write()
            .await
            .write(&mut bytes_serialized, &chain_type)
            .unwrap();
        let mut reader_serialized = Vec::new();
        from_reader
            .wallet()
            .write()
            .await
            .write(&mut reader_serialized, &chain_type)
            .unwrap();
        assert_eq!(bytes_serialized, reader_serialized);
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
}
