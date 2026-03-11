pub mod config;
pub mod error;
pub mod state;

use std::{
    panic::{self, AssertUnwindSafe},
    sync::Arc,
    thread,
    time::Duration,
};

use async_trait::async_trait;

use bip0039::Mnemonic;
use pepper_sync::{error::SyncError, sync::SyncResult, wallet::SyncMode};
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::zip32::AccountId;
use zingolib::{
    data::PollReport,
    wallet::{LightWallet, WalletBase, balance::AccountBalance},
};

use crate::{config::construct_config, error::WalletError, state::EngineState};

uniffi::setup_scaffolding!();

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum Chain {
    Mainnet,
    Testnet,
    Regtest,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum Performance {
    Maximum,
    High,
    Medium,
    Low,
}

#[derive(Clone, Debug, uniffi::Record, PartialEq, Eq)]
pub struct BalanceSnapshot {
    pub confirmed: String,
    pub total: String,
}

#[derive(Clone, Debug, uniffi::Record, PartialEq, Eq)]
pub struct SeedPhrase {
    pub words: String,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RestoreParams {
    pub seed_phrase: SeedPhrase,
    pub birthday: u32,
    pub indexer_uri: String,
    pub chain: Chain,
    pub perf: Performance,
    pub minconf: u32,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct UFVKImportParams {
    pub wallet_dir: String,
    pub ufvk: String,
    pub birthday: u32,
    pub indexer_uri: String,
    pub chain: Chain,
    pub perf: Performance,
    pub minconf: u32,
}

#[derive(Clone, Debug, uniffi::Enum)]
pub enum WalletEvent {
    EngineReady,
    SyncStarted,
    SyncProgress {
        wallet_height: u32,
        network_height: u32,
        percent: f32,
    },
    SyncPaused,
    SyncFinished,
    BalanceChanged(BalanceSnapshot),
    Error {
        code: String,
        message: String,
    },
    Unloaded,
}

#[uniffi::export(callback_interface)]
pub trait WalletListener: Send + Sync {
    fn on_event(&self, event: WalletEvent);
}

#[async_trait]
pub trait WalletBackend: Send + Sync {
    /// Starts a sync run.
    ///
    /// This is expected to *kick off* syncing and return reasonably quickly.
    /// It should **not** block until the sync is fully complete.
    ///
    /// Typical behavior:
    /// - If the backend is currently paused, this should resume.
    /// - If no sync is running, this should start a new one.
    /// - If a sync is already running, this may return `Ok(())` (idempotent) or a
    ///   descriptive error string, depending on your policy.
    ///
    /// Errors:
    /// - Returns `Err(String)` for backend-specific failures (e.g. cannot start sync,
    ///   bad internal state).
    async fn start_sync(&self) -> Result<(), String>;

    /// Polls the status of the currently running sync task.
    ///
    /// This should be a *non-blocking* operation that reports progress/completion
    /// for the sync started via [`WalletBackend::start_sync`].
    ///
    /// The engine will typically call this on a timer (e.g. every 250ms) to drive
    /// event emission:
    /// - `PollReport::NotReady` → still syncing
    /// - `PollReport::Ready(Ok(_))` → finished successfully
    /// - `PollReport::Ready(Err(_))` → finished with error
    /// - `PollReport::NoHandle` → no sync is currently running / nothing to poll
    ///
    /// Note:
    /// - Even though this is `async`, implementations should keep it fast. Use a
    ///   short lock section if you must acquire interior mutability.
    async fn poll_sync(&self) -> PollReport<SyncResult, SyncError<WalletError>>;

    /// Requests that an in-progress sync pause.
    ///
    /// This is best-effort: depending on the backend, the sync may pause at the
    /// next safe point, or it may complete before pausing.
    ///
    /// Typical behavior:
    /// - If syncing is active, request a pause and return `Ok(())` if the request
    ///   was accepted.
    /// - If no sync is running, this may return `Ok(())` or an error, depending on policy.
    ///
    /// Errors:
    /// - Returns `Err(String)` for backend-specific failures (e.g. cannot pause).
    async fn pause_sync(&self) -> Result<(), String>;

    /// Returns the current sync mode/state.
    ///
    /// This is used by the engine to determine whether a sync is paused vs running,
    /// and to gate transitions (e.g. “start sync” may mean “resume”).
    async fn sync_mode(&self) -> SyncMode;

    // data
    async fn wallet_height(&self) -> u32;
    async fn balance_snapshot(&self) -> Option<BalanceSnapshot>;
    async fn network_height(&self) -> u32;
}

/// Zingolib-backed implementation.
/// Keeps all zingolib state behind async locks so engine can remain responsive.
pub struct ZingolibBackend {
    lc: Arc<RwLock<zingolib::lightclient::LightClient>>,
    indexer_uri: http::Uri,
}

impl ZingolibBackend {
    pub fn new(lc: zingolib::lightclient::LightClient, indexer_uri: http::Uri) -> Self {
        Self {
            lc: Arc::new(RwLock::new(lc)),
            indexer_uri,
        }
    }
}

#[async_trait]
impl WalletBackend for ZingolibBackend {
    async fn start_sync(&self) -> Result<(), String> {
        let mut guard = self.lc.write().await;

        if guard.sync_mode() == SyncMode::Paused {
            // TODO: replace with proper resume when available
            guard.resume_sync().map_err(|e| e.to_string())
        } else {
            guard.sync().await.map_err(|e| e.to_string())
        }
    }

    async fn poll_sync(&self) -> PollReport<SyncResult, SyncError<crate::error::WalletError>> {
        let mut guard = self.lc.write().await;

        match guard.poll_sync() {
            PollReport::NotReady => PollReport::NotReady,
            PollReport::NoHandle => PollReport::NoHandle,
            PollReport::Ready(Ok(r)) => PollReport::Ready(Ok(r)),
            PollReport::Ready(Err(e)) => PollReport::Ready(Err(map_sync_error_from_zingolib(e))),
        }
    }

    async fn pause_sync(&self) -> Result<(), String> {
        let guard = self.lc.write().await;
        guard.pause_sync().map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn sync_mode(&self) -> SyncMode {
        let guard = self.lc.read().await;
        guard.sync_mode()
    }

    async fn wallet_height(&self) -> u32 {
        let guard = self.lc.read().await;
        let w = guard.wallet.read().await;
        w.sync_state
            .highest_scanned_height()
            .map(u32::from)
            .unwrap_or(0)
    }

    async fn balance_snapshot(&self) -> Option<BalanceSnapshot> {
        let guard = self.lc.read().await;
        guard
            .account_balance(AccountId::ZERO)
            .await
            .ok()
            .map(|b| balance_snapshot_from_balance(&b))
    }

    async fn network_height(&self) -> u32 {
        zingolib::grpc_connector::get_latest_block(self.indexer_uri.clone())
            .await
            .map(|b| b.height as u32)
            .unwrap_or(0)
    }
}

fn balance_snapshot_from_balance(b: &AccountBalance) -> BalanceSnapshot {
    let confirmed = b
        .confirmed_orchard_balance
        .map(|v| v.into_u64())
        .unwrap_or(0)
        + b.confirmed_sapling_balance
            .map(|v| v.into_u64())
            .unwrap_or(0)
        + b.confirmed_transparent_balance
            .map(|v| v.into_u64())
            .unwrap_or(0);

    let total = b.total_orchard_balance.map(|v| v.into_u64()).unwrap_or(0)
        + b.total_sapling_balance.map(|v| v.into_u64()).unwrap_or(0)
        + b.total_transparent_balance
            .map(|v| v.into_u64())
            .unwrap_or(0);

    BalanceSnapshot {
        confirmed: confirmed.to_string(),
        total: total.to_string(),
    }
}

fn map_sync_error_from_zingolib(
    e: SyncError<zingolib::wallet::error::WalletError>,
) -> SyncError<crate::error::WalletError> {
    use pepper_sync::error::SyncError as SE;

    match e {
        SE::MempoolError(x) => SE::MempoolError(x),
        SE::ScanError(x) => SE::ScanError(x),
        SE::ServerError(x) => SE::ServerError(x),
        SE::SyncModeError(x) => SE::SyncModeError(x),
        SE::ChainError(a, b, c) => SE::ChainError(a, b, c),
        SE::ShardTreeError(x) => SE::ShardTreeError(x),
        SE::TruncationError(a, b) => SE::TruncationError(a, b),
        SE::TransparentAddressDerivationError(x) => SE::TransparentAddressDerivationError(x),
        SE::BirthdayBelowSapling(a, b) => SE::BirthdayBelowSapling(a, b),

        SE::WalletError(w) => SE::WalletError(crate::error::WalletError::Internal(w.to_string())),
    }
}

struct EngineInner {
    cmd_tx: mpsc::Sender<Command>,
    listener: std::sync::Mutex<Option<Arc<dyn WalletListener>>>,
}

impl EngineInner {
    pub(crate) async fn handle_unload_wallet(
        self: Arc<Self>,
        engine_state: Arc<tokio::sync::Mutex<EngineState>>,
        reply: oneshot::Sender<Result<(), WalletError>>,
    ) {
        let (backend, was_syncing, sync_task) = {
            let mut state = engine_state.lock().await;

            let backend = state.backend.clone();
            let was_syncing = state.syncing;

            let sync_task = state.sync_task.take();

            state.syncing = false;

            (backend, was_syncing, sync_task)
        };

        if was_syncing {
            if let Some(task) = sync_task {
                task.abort();
            }

            // TODO: maybe remove, or put first
            if let Some(b) = backend.as_ref() {
                let _ = b.pause_sync().await; // TODO: this should be replaced with the future "stop" command
            }
        }

        {
            let mut state = engine_state.lock().await;
            state.backend = None;
            state.last_balance = None;
        }

        emit(&self, WalletEvent::Unloaded);

        let _ = reply.send(Ok(()));
    }

    pub(crate) async fn handle_start_sync_spawn(
        self: Arc<Self>,
        engine_state: Arc<Mutex<EngineState>>,
    ) {
        let backend = {
            let mut state = engine_state.lock().await;

            if state.syncing {
                return;
            }

            let Some(backend) = state.backend.as_ref().cloned() else {
                emit(
                    &self,
                    WalletEvent::Error {
                        code: "start_sync_failed".into(),
                        message: WalletError::NotInitialized.to_string(),
                    },
                );
                return;
            };

            state.syncing = true;
            backend
        };

        emit(&self, WalletEvent::SyncStarted);

        let inner = self.clone();
        let st_for_task = engine_state.clone();

        let task = tokio::spawn(async move {
            if let Err(e) = backend.start_sync().await {
                emit(
                    &inner,
                    WalletEvent::Error {
                        code: "sync_failed".into(),
                        message: e,
                    },
                );
                let mut s = st_for_task.lock().await;
                s.syncing = false;
                return;
            }

            let mut last_balance_emitted: Option<BalanceSnapshot> = None;

            loop {
                if backend.sync_mode().await == SyncMode::Paused {
                    emit(&inner, WalletEvent::SyncPaused);
                    let mut s = st_for_task.lock().await;
                    s.syncing = false;
                    break;
                }

                let wallet_height = backend.wallet_height().await;
                let network_height = backend.network_height().await;

                let percent = if network_height > 0 {
                    (wallet_height as f32 / network_height as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };

                emit(
                    &inner,
                    WalletEvent::SyncProgress {
                        wallet_height,
                        network_height,
                        percent,
                    },
                );

                if let Some(snap) = backend.balance_snapshot().await {
                    if last_balance_emitted.as_ref() != Some(&snap) {
                        last_balance_emitted = Some(snap.clone());
                        emit(&inner, WalletEvent::BalanceChanged(snap));
                    }
                }

                match backend.poll_sync().await {
                    PollReport::Ready(Ok(_)) => {
                        emit(&inner, WalletEvent::SyncFinished);
                        let mut s = st_for_task.lock().await;
                        s.syncing = false;
                        break;
                    }
                    PollReport::Ready(Err(e)) => {
                        emit(
                            &inner,
                            WalletEvent::Error {
                                code: "sync_failed".into(),
                                message: e.to_string(),
                            },
                        );
                        let mut s = st_for_task.lock().await;
                        s.syncing = false;
                        break;
                    }
                    PollReport::NotReady | PollReport::NoHandle => {}
                }

                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        });

        let mut s = engine_state.lock().await;
        s.sync_task = Some(task);
    }

    pub(crate) async fn handle_init_new(
        &self,
        st: &mut EngineState,
        indexer_uri: String,
        chain: Chain,
        perf: Performance,
        minconf: u32,
        reply: oneshot::Sender<Result<(), WalletError>>,
    ) {
        let res: Result<(), WalletError> = (async {
            let (config, lw_uri) = construct_config(indexer_uri, chain, perf, minconf, None)?;

            let chain_height = zingolib::grpc_connector::get_latest_block(lw_uri.clone())
                .await
                .map(|b| BlockHeight::from_u32(b.height as u32))
                .map_err(|e| WalletError::Internal(format!("get_latest_block: {e}")))?;

            let birthday = chain_height.saturating_sub(100);

            let lc = zingolib::lightclient::LightClient::new(config, birthday, false)
                .map_err(|e| WalletError::Internal(format!("LightClient::new: {e}")))?;

            st.set_backend(Arc::new(ZingolibBackend::new(lc, lw_uri)));
            Ok(())
        })
        .await;

        let _ = reply.send(res);
    }

    pub(crate) async fn handle_init_from_seed(
        &self,
        st: &mut EngineState,
        seed_phrase: String,
        birthday: u32,
        indexer_uri: String,
        chain: Chain,
        perf: Performance,
        minconf: u32,
        reply: oneshot::Sender<Result<(), WalletError>>,
    ) {
        let res: Result<(), WalletError> = (async {
            let (config, lw_uri) = construct_config(indexer_uri, chain, perf, minconf, None)?;

            let mnemonic = Mnemonic::from_phrase(seed_phrase)
                .map_err(|e| WalletError::Internal(format!("Mnemonic: {e}")))?;

            let wallet = LightWallet::new(
                config.chain,
                WalletBase::Mnemonic {
                    mnemonic,
                    no_of_accounts: config.no_of_accounts,
                },
                BlockHeight::from_u32(birthday),
                config.wallet_settings.clone(),
            )
            .map_err(|e| WalletError::Internal(format!("LightWallet::new: {e}")))?;

            let lc = zingolib::lightclient::LightClient::create_from_wallet(wallet, config, false)
                .map_err(|e| WalletError::Internal(format!("create_from_wallet: {e}")))?;

            st.set_backend(Arc::new(ZingolibBackend::new(lc, lw_uri)));
            Ok(())
        })
        .await;

        let _ = reply.send(res);
    }

    pub(crate) async fn handle_init_view_only(
        &self,
        st: &mut EngineState,
        viewing_key: String,
        birthday: u32,
        indexer_uri: String,
        chain: Chain,
        perf: Performance,
        minconf: u32,
        wallet_dir: String,
        reply: oneshot::Sender<Result<(), WalletError>>,
    ) {
        let res: Result<(), WalletError> = (async {
            let (config, lw_uri) = construct_config(
                indexer_uri,
                chain,
                perf,
                minconf,
                Some(wallet_dir.parse().unwrap()),
            )?;

            let wallet = LightWallet::new(
                config.chain,
                WalletBase::Ufvk(viewing_key),
                BlockHeight::from_u32(birthday),
                config.wallet_settings.clone(),
            )
            .map_err(|e| WalletError::Internal(format!("LightWallet::new: {e}")))?;

            let lc = zingolib::lightclient::LightClient::create_from_wallet(wallet, config, false)
                .map_err(|e| WalletError::Internal(format!("create_from_wallet: {e}")))?;

            st.set_backend(Arc::new(ZingolibBackend::new(lc, lw_uri)));
            Ok(())
        })
        .await;

        let _ = reply.send(res);
    }

    pub(crate) async fn handle_get_balance(
        &self,
        backend: Option<Arc<dyn WalletBackend>>,
        reply: oneshot::Sender<Result<BalanceSnapshot, WalletError>>,
    ) {
        let res: Result<BalanceSnapshot, WalletError> = async {
            let backend = backend.ok_or(WalletError::NotInitialized)?;

            backend
                .balance_snapshot()
                .await
                .ok_or_else(|| WalletError::Internal("balance unavailable".into()))
        }
        .await;

        let _ = reply.send(res);
    }

    pub(crate) async fn handle_get_network_height(
        &self,
        backend: Option<Arc<dyn WalletBackend>>,
        reply: oneshot::Sender<Result<u32, WalletError>>,
    ) {
        let res: Result<u32, WalletError> = async {
            let backend = backend.ok_or(WalletError::NotInitialized)?;
            Ok(backend.network_height().await)
        }
        .await;

        let _ = reply.send(res);
    }

    pub(crate) async fn handle_pause_sync(&self, backend: Option<Arc<dyn WalletBackend>>) {
        let Some(backend) = backend else {
            emit(
                self,
                WalletEvent::Error {
                    code: "pause_sync_failed".into(),
                    message: WalletError::NotInitialized.to_string(),
                },
            );
            return;
        };

        match backend.pause_sync().await {
            Ok(_) => emit(self, WalletEvent::SyncPaused),
            Err(e) => emit(
                self,
                WalletEvent::Error {
                    code: "pause_sync_failed".into(),
                    message: e,
                },
            ),
        }
    }
}

fn emit(inner: &EngineInner, event: WalletEvent) {
    let listener_opt = inner.listener.lock().ok().and_then(|g| g.clone());
    if let Some(listener) = listener_opt {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            listener.on_event(event);
        }));
    }
}

// TODO; Remove repetition!!
enum Command {
    InitNew {
        indexer_uri: String,
        chain: Chain,
        perf: Performance,
        minconf: u32,
        reply: oneshot::Sender<Result<(), WalletError>>,
    },
    InitFromSeed {
        seed_phrase: String,
        birthday: u32,
        indexer_uri: String,
        chain: Chain,
        perf: Performance,
        minconf: u32,
        reply: oneshot::Sender<Result<(), WalletError>>,
    },
    InitViewOnly {
        wallet_dir: String,
        viewing_key: String,
        birthday: u32,
        indexer_uri: String,
        chain: Chain,
        perf: Performance,
        minconf: u32,

        reply: oneshot::Sender<Result<(), WalletError>>,
    },
    GetBalance {
        reply: oneshot::Sender<Result<BalanceSnapshot, WalletError>>,
    },
    GetNetworkHeight {
        reply: oneshot::Sender<Result<u32, WalletError>>,
    },
    StartSync,
    PauseSync,
    ShutdownSync,
    Unload {
        reply: oneshot::Sender<Result<(), WalletError>>,
    },
}

#[derive(uniffi::Object, Clone)]
pub struct WalletEngine {
    inner: Arc<EngineInner>,
}

/// Engine thread runtime only.
fn create_engine_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("tokio runtime")
}

#[uniffi::export]
impl WalletEngine {
    /// Creates a new [`WalletEngine`] and starts the internal engine thread.
    ///
    /// This constructor:
    /// - Allocates a command queue used to communicate with the engine thread.
    /// - Spawns a dedicated OS thread that owns a Tokio runtime and all async wallet state, through the [`LightClient`].
    /// - Emits [`WalletEvent::EngineReady`] once the engine thread is running.
    ///
    /// ## Threading / FFI design
    /// All UniFFI-exposed methods on [`WalletEngine`] are *synchronous* and safe to call from
    /// Swift/Kotlin. Any async work is executed on the engine thread.
    ///
    /// ## Errors
    /// Returns [`WalletError`] if the engine cannot be created.
    #[uniffi::constructor]
    pub fn new() -> Result<Self, WalletError> {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(64);

        let inner = Arc::new(EngineInner {
            cmd_tx,
            listener: std::sync::Mutex::new(None),
        });

        let inner_for_task = inner.clone();

        thread::spawn(move || {
            let rt = create_engine_runtime();
            rt.block_on(async move {
                emit(&inner_for_task, WalletEvent::EngineReady);

                let st = Arc::new(Mutex::new(EngineState::new()));

                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        Command::InitNew {
                            indexer_uri,
                            chain,
                            perf,
                            minconf,
                            reply,
                        } => {
                            let mut guard = st.lock().await;
                            inner_for_task
                                .handle_init_new(
                                    &mut guard,
                                    indexer_uri,
                                    chain,
                                    perf,
                                    minconf,
                                    reply,
                                )
                                .await;
                        }

                        Command::InitFromSeed {
                            seed_phrase,
                            birthday,
                            indexer_uri,
                            chain,
                            perf,
                            minconf,
                            reply,
                        } => {
                            let mut guard = st.lock().await;
                            inner_for_task
                                .handle_init_from_seed(
                                    &mut guard,
                                    seed_phrase,
                                    birthday,
                                    indexer_uri,
                                    chain,
                                    perf,
                                    minconf,
                                    reply,
                                )
                                .await;
                        }

                        Command::InitViewOnly {
                            wallet_dir,
                            viewing_key,
                            birthday,
                            indexer_uri,
                            chain,
                            perf,
                            minconf,
                            reply,
                        } => {
                            let mut guard = st.lock().await;
                            inner_for_task
                                .handle_init_view_only(
                                    &mut guard,
                                    viewing_key,
                                    birthday,
                                    indexer_uri,
                                    chain,
                                    perf,
                                    minconf,
                                    wallet_dir,
                                    reply,
                                )
                                .await;
                        }

                        Command::Unload { reply } => {
                            inner_for_task
                                .clone()
                                .handle_unload_wallet(st.clone(), reply)
                                .await;
                        }

                        Command::GetBalance { reply } => {
                            // TODO: Make this all less repetitive/convoluted
                            let backend = {
                                let guard = st.lock().await;
                                guard.backend.clone()
                            };
                            inner_for_task.handle_get_balance(backend, reply).await;
                        }

                        Command::GetNetworkHeight { reply } => {
                            let backend = {
                                let guard = st.lock().await;
                                guard.backend.clone()
                            };
                            inner_for_task
                                .handle_get_network_height(backend, reply)
                                .await;
                        }

                        Command::StartSync => {
                            inner_for_task
                                .clone()
                                .handle_start_sync_spawn(st.clone())
                                .await;
                        }

                        Command::PauseSync => {
                            let backend = {
                                let guard = st.lock().await;
                                guard.backend.clone()
                            };
                            inner_for_task.handle_pause_sync(backend).await;
                        }

                        Command::ShutdownSync => break,
                    }
                }
            });
        });

        Ok(Self { inner })
    }

    /// Installs a listener that receives asynchronous [`WalletEvent`] callbacks.
    ///
    /// The listener is invoked from the engine thread. Implementations must be:
    /// - thread-safe (`Send + Sync`)
    /// - fast / non-blocking (heavy work should be offloaded by the caller)
    ///
    /// If the listener panics, the engine catches the panic to avoid crashing the engine thread.
    ///
    /// Replaces any previously installed listener.
    ///
    /// ## Errors
    /// Returns [`WalletError::ListenerLockPoisoned`] if the listener mutex is poisoned.
    pub fn set_listener(&self, listener: Box<dyn WalletListener>) -> Result<(), WalletError> {
        let mut guard = self
            .inner
            .listener
            .lock()
            .map_err(|_| WalletError::ListenerLockPoisoned)?;
        *guard = Some(Arc::from(listener));
        Ok(())
    }

    /// Clears the currently installed listener, if any.
    ///
    /// After calling this, no further [`WalletEvent`] callbacks will be delivered until a new
    /// listener is set via [`WalletEngine::set_listener`].
    ///
    /// ## Errors
    /// Returns [`WalletError::ListenerLockPoisoned`] if the listener mutex is poisoned.
    pub fn clear_listener(&self) -> Result<(), WalletError> {
        let mut guard = self
            .inner
            .listener
            .lock()
            .map_err(|_| WalletError::ListenerLockPoisoned)?;
        *guard = None;
        Ok(())
    }

    /// Initializes a brand-new wallet on the engine thread.
    ///
    /// This is the entrypoint for new wallets. It:
    /// - Builds a [`ZingoConfig`] from the provided parameters.
    /// - Queries the indexer for the latest block height to derive a conservative birthday.
    /// - Constructs a new [`LightClient`], replacing any previously loaded wallet.
    ///
    /// This method is **blocking** by design. The async work is performed on the
    /// engine thread and the result is returned via a oneshot reply channel.
    ///
    /// ## Parameters
    /// - `indexer_uri`: zainod/lightwalletd URI, e.g. `http://localhost:9067`
    /// - `chain`: chain selection (mainnet/testnet/regtest)
    /// - `perf`: sync performance preset
    /// - `minconf`: minimum confirmations for spendable funds. Must be >= 1.
    ///
    /// ## Events
    /// Does not automatically start syncing. Call [`WalletEngine::start_sync`] to begin a sync round.
    ///
    /// ## Errors
    /// - [`WalletError::CommandQueueClosed`] if the engine thread has exited.
    /// - [`WalletError::Internal`] on config/build errors or indexer gRPC failures.
    pub fn init_new(
        &self,
        indexer_uri: String,
        chain: Chain,
        perf: Performance,
        minconf: u32,
    ) -> Result<(), WalletError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .blocking_send(Command::InitNew {
                indexer_uri,
                chain,
                perf,
                minconf,
                reply: reply_tx,
            })
            .map_err(|_| WalletError::CommandQueueClosed)?;

        reply_rx
            .blocking_recv()
            .map_err(|_| WalletError::CommandQueueClosed)?
    }

    /// Initializes a wallet from a seed phrase and explicit birthday height.
    ///
    /// This is the entrypoint for restoring from seed. It:
    /// - Builds a [`ZingoConfig`] from the provided parameters.
    /// - Parses the BIP39 mnemonic from `seed_phrase`.
    /// - Constructs a [`LightWallet`] using the provided `birthday`.
    /// - Creates a [`LightClient`] from that wallet, replacing any previously loaded wallet.
    ///
    /// This method is **blocking** by design (FFI-friendly). The async work is performed on the
    /// engine thread and the result is returned via a oneshot reply channel.
    ///
    /// ## Parameters
    /// - `seed_phrase`: BIP39 mnemonic words separated by spaces
    /// - `birthday`: wallet birthday (starting scan height)
    /// - `indexer_uri`: lightwalletd URI
    /// - `chain`: chain selection (mainnet/testnet/regtest)
    /// - `perf`: sync performance preset
    /// - `minconf`: minimum confirmations for spendable funds. Must be >= 1.
    ///
    /// ## Events
    /// Does not automatically start syncing. Call [`WalletEngine::start_sync`] to begin a sync round.
    ///
    /// ## Errors
    /// - [`WalletError::CommandQueueClosed`] if the engine thread has exited.
    /// - [`WalletError::Internal`] on config/mnemonic/wallet construction errors.
    pub fn init_from_seed(&self, params: RestoreParams) -> Result<(), WalletError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .blocking_send(Command::InitFromSeed {
                seed_phrase: params.seed_phrase.words,
                birthday: params.birthday,
                indexer_uri: params.indexer_uri,
                chain: params.chain,
                perf: params.perf,
                minconf: params.minconf,
                reply: reply_tx,
            })
            .map_err(|_| WalletError::CommandQueueClosed)?;

        reply_rx
            .blocking_recv()
            .map_err(|_| WalletError::CommandQueueClosed)?
    }

    pub fn init_from_ufvk(&self, params: UFVKImportParams) -> Result<(), WalletError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .blocking_send(Command::InitViewOnly {
                viewing_key: params.ufvk,
                birthday: params.birthday,
                indexer_uri: params.indexer_uri,
                chain: params.chain,
                perf: params.perf,
                minconf: params.minconf,
                wallet_dir: params.wallet_dir,
                reply: reply_tx,
            })
            .map_err(|_| WalletError::CommandQueueClosed)?;

        reply_rx
            .blocking_recv()
            .map_err(|_| WalletError::CommandQueueClosed)?
    }

    /// Returns a snapshot of the wallet balance for Account 0.
    ///
    /// The returned [`BalanceSnapshot`] is a simplified, FFI-stable view derived from the
    /// underlying zingolib [`AccountBalance`] type.
    ///
    /// This method is **blocking**. The balance query runs on the engine thread and the result
    /// is returned via a oneshot reply channel.
    ///
    /// ## Errors
    /// - [`WalletError::NotInitialized`] if no wallet has been initialized.
    /// - [`WalletError::CommandQueueClosed`] if the engine thread has exited.
    /// - [`WalletError::Internal`] if the underlying balance query fails.
    pub fn get_balance_snapshot(&self) -> Result<BalanceSnapshot, WalletError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .blocking_send(Command::GetBalance { reply: reply_tx })
            .map_err(|_| WalletError::CommandQueueClosed)?;

        reply_rx
            .blocking_recv()
            .map_err(|_| WalletError::CommandQueueClosed)?
    }

    /// Returns the latest known network height from the configured indexer.
    ///
    /// This is a gRPC call to the indexer (`get_latest_block`) and is useful for:
    /// - UI display (“current tip”)
    /// - tests that need to observe tip movement independently of sync progress
    ///
    /// This method is **blocking**. The gRPC call runs on the engine thread and the result is returned
    /// via a oneshot reply channel.
    ///
    /// ## Errors
    /// - [`WalletError::NotInitialized`] if no indexer has been configured yet.
    /// - [`WalletError::CommandQueueClosed`] if the engine thread has exited.
    /// - [`WalletError::Internal`] if the indexer gRPC fails.
    pub fn get_network_height(&self) -> Result<u32, WalletError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .blocking_send(Command::GetNetworkHeight { reply: reply_tx })
            .map_err(|_| WalletError::CommandQueueClosed)?;

        reply_rx
            .blocking_recv()
            .map_err(|_| WalletError::CommandQueueClosed)?
    }

    /// Starts a **single manual sync round**.
    ///
    /// The sync round runs on the engine thread and emits events:
    /// - [`WalletEvent::SyncStarted`] immediately when accepted
    /// - repeated [`WalletEvent::SyncProgress`] updates while syncing
    /// - optional [`WalletEvent::BalanceChanged`] updates when balance changes
    /// - [`WalletEvent::SyncFinished`] once the sync completes successfully
    /// - [`WalletEvent::Error`] if the sync fails
    ///
    /// ## Manual model
    /// This method performs **one** round per invocation. It does not “follow” the chain forever.
    /// If the network tip advances later and you want to catch up again, call `start_sync()` again.
    ///
    /// ## Reentrancy
    /// If a sync is already running, additional `start_sync()` calls are ignored.
    ///
    /// ## Errors
    /// Returns [`WalletError::CommandQueueClosed`] if the engine thread has exited.
    /// If the wallet is not initialized, an async [`WalletEvent::Error`] is emitted with
    /// `code="start_sync_failed"`.
    pub fn start_sync(&self) -> Result<(), WalletError> {
        self.inner
            .cmd_tx
            .try_send(Command::StartSync)
            .map_err(|_| WalletError::CommandQueueClosed)
    }

    /// Requests that an in-progress sync pause.
    ///
    /// This calls into zingolib's pause mechanism. If successful, the engine emits
    /// [`WalletEvent::SyncPaused`].
    ///
    /// Note: pausing is best-effort. If no wallet exists, an async [`WalletEvent::Error`] is emitted
    /// with `code="pause_sync_failed"`.
    ///
    /// ## Errors
    /// Returns [`WalletError::CommandQueueClosed`] if the engine thread has exited.
    pub fn pause_sync(&self) -> Result<(), WalletError> {
        self.inner
            .cmd_tx
            .try_send(Command::PauseSync)
            .map_err(|_| WalletError::CommandQueueClosed)
    }

    /// Shuts down the engine thread.
    ///
    /// This sends a shutdown command to the engine loop. After shutdown:
    /// - all subsequent method calls that require the engine thread will typically fail with
    ///   [`WalletError::CommandQueueClosed`]
    /// - no further [`WalletEvent`] callbacks will be delivered
    ///
    /// Shutdown is best-effort; the command is queued if possible.
    ///
    /// ## Errors
    /// Returns [`WalletError::CommandQueueClosed`] if the command queue is already closed.
    pub fn shutdown(&self) -> Result<(), WalletError> {
        self.inner
            .cmd_tx
            .try_send(Command::ShutdownSync)
            .map_err(|_| WalletError::CommandQueueClosed)
    }

    /// Shuts down the engine thread and unloads the wallet from memory.
    pub fn unload_wallet(&self) -> Result<(), WalletError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .blocking_send(Command::Unload { reply: reply_tx })
            .map_err(|_| WalletError::CommandQueueClosed)?;

        reply_rx
            .blocking_recv()
            .map_err(|_| WalletError::CommandQueueClosed)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as std_mpsc;

    /// Test-only listener that forwards every [`WalletEvent`] it receives into a
    /// standard-library `mpsc` channel.
    ///
    /// This is used in unit tests to:
    /// - observe asynchronous events emitted by the engine thread
    /// - make assertions about ordering (e.g. `EngineReady` then `SyncStarted`)
    /// - avoid blocking the engine thread (sending into `std::sync::mpsc::Sender` is fast)
    #[derive(Clone)]
    struct CapturingListener {
        tx: std_mpsc::Sender<WalletEvent>,
    }

    impl WalletListener for CapturingListener {
        fn on_event(&self, event: WalletEvent) {
            let _ = self.tx.send(event);
        }
    }

    /// A listener that panics on every callback, to verify panic containment.
    struct PanickingListener;

    impl WalletListener for PanickingListener {
        fn on_event(&self, _event: WalletEvent) {
            panic!("listener panicked");
        }
    }

    mod fake_backend_tests {
        use std::{
            sync::{
                Arc,
                atomic::{AtomicUsize, Ordering},
            },
            thread,
            time::{Duration, Instant},
        };

        use async_trait::async_trait;
        use pepper_sync::{error::SyncError, sync::SyncResult, wallet::SyncMode};
        use tokio::sync::{Mutex, mpsc};
        use zingolib::data::PollReport;

        use crate::{
            BalanceSnapshot, Command, EngineInner, WalletBackend, WalletEngine, WalletEvent,
            create_engine_runtime, emit,
            error::WalletError,
            state::EngineState,
            tests::{CapturingListener, recv_timeout},
        };

        struct FakeBackend {
            start_sync_calls: AtomicUsize,
            poll_calls: AtomicUsize,
            balance_calls: AtomicUsize,
            wallet_height: AtomicUsize,
            network_height: AtomicUsize,
        }

        impl FakeBackend {
            fn new() -> Self {
                Self {
                    start_sync_calls: AtomicUsize::new(0),
                    poll_calls: AtomicUsize::new(0),
                    balance_calls: AtomicUsize::new(0),
                    wallet_height: AtomicUsize::new(100),
                    network_height: AtomicUsize::new(200),
                }
            }

            fn start_sync_call_count(&self) -> usize {
                self.start_sync_calls.load(Ordering::SeqCst)
            }
        }

        #[async_trait]
        impl WalletBackend for FakeBackend {
            async fn start_sync(&self) -> Result<(), String> {
                self.start_sync_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }

            async fn poll_sync(&self) -> PollReport<SyncResult, SyncError<WalletError>> {
                self.poll_calls.fetch_add(1, Ordering::SeqCst);

                // Keep the sync task "alive" but always yielding.
                tokio::time::sleep(Duration::from_millis(25)).await;

                // Never finish; this is enough to prove responsiveness while "syncing".
                PollReport::NotReady
            }

            async fn pause_sync(&self) -> Result<(), String> {
                Ok(())
            }

            async fn sync_mode(&self) -> SyncMode {
                // We only need "not paused" for the engine loop to keep going.
                // In production backends, this is a real mode.
                //
                // For tests, we avoid relying on non-Paused enum variant names (which may change)
                // by using an unsafe transmute to "some other variant".
                //
                // SAFETY: this is test-only code; any mismatch just fails the test build.
                #[allow(unsafe_code)]
                unsafe {
                    // Choose a discriminant different than the one for `Paused`.
                    // This assumes `Paused` is not discriminant 0; if it is, swap 0/1.
                    // If this ever breaks, just update the number to match pepper_sync.
                    std::mem::transmute::<u8, SyncMode>(0)
                }
            }

            async fn wallet_height(&self) -> u32 {
                self.wallet_height.load(Ordering::SeqCst) as u32
            }

            async fn balance_snapshot(&self) -> Option<BalanceSnapshot> {
                self.balance_calls.fetch_add(1, Ordering::SeqCst);

                // Fast path: returns immediately (this is what we want to verify is reachable
                // while sync is running).
                Some(BalanceSnapshot {
                    confirmed: "1".to_string(),
                    total: "2".to_string(),
                })
            }

            async fn network_height(&self) -> u32 {
                self.network_height.load(Ordering::SeqCst) as u32
            }
        }

        fn spawn_test_engine_with_backend(
            backend: Arc<dyn WalletBackend>,
        ) -> (WalletEngine, std::sync::mpsc::Receiver<WalletEvent>) {
            let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(64);

            let inner = Arc::new(EngineInner {
                cmd_tx,
                listener: std::sync::Mutex::new(None),
            });

            let engine = WalletEngine {
                inner: inner.clone(),
            };

            let (ev_tx, ev_rx) = std::sync::mpsc::channel();
            engine
                .set_listener(Box::new(CapturingListener { tx: ev_tx }))
                .expect("set_listener");

            thread::spawn(move || {
                let rt = create_engine_runtime();
                rt.block_on(async move {
                    emit(&inner, WalletEvent::EngineReady);

                    let st = Arc::new(Mutex::new(EngineState::new()));

                    // IMPORTANT: do NOT use blocking_lock() inside runtime
                    {
                        let mut guard = st.lock().await;
                        guard.backend = Some(backend);
                    }

                    while let Some(cmd) = cmd_rx.recv().await {
                        let backend = {
                            let guard = st.lock().await;
                            guard.backend.clone()
                        };
                        match cmd {
                            Command::GetBalance { reply } => {
                                inner.handle_get_balance(backend, reply).await;
                            }
                            Command::GetNetworkHeight { reply } => {
                                inner.handle_get_network_height(backend, reply).await;
                            }
                            Command::StartSync => {
                                inner.clone().handle_start_sync_spawn(st.clone()).await;
                            }
                            Command::PauseSync => {
                                inner.handle_pause_sync(backend).await;
                            }
                            Command::ShutdownSync => break,
                            Command::InitNew { reply, .. }
                            | Command::InitFromSeed { reply, .. }
                            | Command::InitViewOnly { reply, .. } => {
                                let _ = reply.send(Err(WalletError::Internal(
                                    "init disabled in fake backend tests".into(),
                                )));
                            }
                            Command::Unload { reply } => todo!(),
                        }
                    }
                });
            });

            (engine, ev_rx)
        }

        /// Proves: starting sync does NOT make the engine loop unusable.
        ///
        /// Specifically: while the spawned sync task is running (and continuously polling),
        /// we can still call get_balance_snapshot() and get an answer quickly.
        #[test]
        fn sync_does_not_block_get_balance_snapshot() {
            let fake = Arc::new(FakeBackend::new());
            let (engine, rx) = spawn_test_engine_with_backend(fake);

            // EngineReady
            let ev = recv_timeout(&rx, Duration::from_secs(2));
            assert!(matches!(ev, WalletEvent::EngineReady), "got: {ev:?}");

            engine.start_sync().expect("start_sync");

            // SyncStarted
            let ev = recv_timeout(&rx, Duration::from_secs(2));
            assert!(matches!(ev, WalletEvent::SyncStarted), "got: {ev:?}");

            // Hammer balance while sync loop is active.
            for i in 0..30 {
                let t0 = Instant::now();
                let bal = engine.get_balance_snapshot().expect("balance snapshot");
                let dt = t0.elapsed();

                assert_eq!(bal.confirmed, "1");
                assert_eq!(bal.total, "2");

                // Thread hop and oneshot should stay well under this.
                assert!(
                    dt < Duration::from_millis(150),
                    "get_balance_snapshot call {i} too slow: {dt:?} (sync may be blocking)"
                );

                // small sleep so we interleave with poll ticks (is this truly necessary though?)
                std::thread::sleep(Duration::from_millis(10));
            }

            engine.shutdown().ok();
        }

        /// Proves: StartSync is re-entrancy protected (second call is ignored while syncing=true).
        ///
        /// This also indirectly proves the command loop remains responsive enough to *process*
        /// multiple StartSync commands while a sync task is running.
        #[test]
        fn start_sync_is_idempotent_while_running() {
            let fake = Arc::new(FakeBackend::new());
            let (engine, rx) = spawn_test_engine_with_backend(fake.clone());

            let _ = recv_timeout(&rx, Duration::from_secs(2)); // EngineReady

            engine.start_sync().expect("start_sync #1");
            let ev = recv_timeout(&rx, Duration::from_secs(2));
            assert!(matches!(ev, WalletEvent::SyncStarted), "got: {ev:?}");

            // Should be ignored. Should not emit SyncStarted. TODO: how can we assert that?
            engine.start_sync().expect("start_sync #2");

            // Give a little time for command loop + potential bogus second start
            std::thread::sleep(Duration::from_millis(200));

            assert_eq!(
                fake.start_sync_call_count(),
                1,
                "backend.start_sync() was called more than once; StartSync was not guarded by syncing flag"
            );

            engine.shutdown().ok();
        }

        /// Proves that while sync task is running we still see progress events,
        /// indicating the runtime is scheduling both the sync task and the command loop.
        #[test]
        fn sync_task_runs_concurrently_with_command_loop() {
            let fake_backend = Arc::new(FakeBackend::new());
            let (engine, rx) = spawn_test_engine_with_backend(fake_backend);

            let _ = recv_timeout(&rx, Duration::from_secs(2)); // EngineReady

            engine.start_sync().expect("start_sync");
            let event = recv_timeout(&rx, Duration::from_secs(2));
            assert!(matches!(event, WalletEvent::SyncStarted), "got: {event:?}");

            // We should see at least one SyncProgress fairly soon.
            // (FakeBackend.poll_sync sleeps 25ms and returns NotReady, so engine loop should emit progress regularly.)
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut saw_progress = false;

            while Instant::now() < deadline {
                let ev = recv_timeout(&rx, Duration::from_millis(250));
                if matches!(ev, WalletEvent::SyncProgress { .. }) {
                    saw_progress = true;
                    break;
                }
            }

            assert!(
                saw_progress,
                "never saw SyncProgress while sync task running"
            );

            // While progress events are flowing, also do a balance call to ensure the engine loop services commands.
            let bal = engine.get_balance_snapshot().expect("balance snapshot");
            assert_eq!(bal.total, "2");

            engine.shutdown().ok();
        }
    }

    fn recv_timeout(rx: &std_mpsc::Receiver<WalletEvent>, dur: Duration) -> WalletEvent {
        rx.recv_timeout(dur).expect("timeout waiting for event")
    }

    #[test]
    fn emits_engine_ready() {
        let engine = WalletEngine::new().expect("engine new");

        let (tx, rx) = std_mpsc::channel();
        engine
            .set_listener(Box::new(CapturingListener { tx }))
            .expect("set_listener");

        let ev = recv_timeout(&rx, Duration::from_secs(2));
        assert!(matches!(ev, WalletEvent::EngineReady), "got: {ev:?}");
    }

    #[test]
    fn get_balance_snapshot_errors_when_not_initialized() {
        let engine = WalletEngine::new().expect("engine new");
        let res = engine.get_balance_snapshot();
        assert!(
            matches!(res, Err(WalletError::NotInitialized)),
            "got: {res:?}"
        );
    }

    #[test]
    fn start_sync_emits_error_when_not_initialized() {
        let engine = WalletEngine::new().expect("engine new");

        let (tx, rx) = std_mpsc::channel();
        engine
            .set_listener(Box::new(CapturingListener { tx }))
            .expect("set_listener");

        let _ = recv_timeout(&rx, Duration::from_secs(2));

        engine.start_sync().expect("start_sync send command");

        loop {
            let ev = recv_timeout(&rx, Duration::from_secs(2));
            match ev {
                WalletEvent::Error { code, message } => {
                    assert_eq!(code, "start_sync_failed");
                    assert!(!message.is_empty());
                    break;
                }
                _ => {}
            }
        }
    }

    #[test]
    fn listener_panics_do_not_crash_engine_thread() {
        let engine = WalletEngine::new().expect("engine new");

        engine
            .set_listener(Box::new(PanickingListener))
            .expect("set_listener panicking");

        // Trigger something that will cause a callback (and panic).
        engine.start_sync().expect("start_sync send command");

        // Give engine time to process callback panic.
        std::thread::sleep(Duration::from_millis(200));

        // Swap in capturing listener; if engine thread died, no more events ever.
        let (tx, rx) = std_mpsc::channel();
        engine
            .set_listener(Box::new(CapturingListener { tx }))
            .expect("set_listener capturing");

        // We should get EngineReady? Not necessarily (already emitted).
        // Trigger pause (will error because not initialized), and we should receive it.
        engine.pause_sync().expect("pause_sync send command");

        loop {
            let ev = recv_timeout(&rx, Duration::from_secs(2));
            if let WalletEvent::Error { code, .. } = ev {
                assert_eq!(code, "pause_sync_failed");
                break;
            }
        }
    }

    /// Requires network up
    #[test]
    #[ignore = "requires non-existing running regtest networkd"]
    fn real_sync_smoke() {
        let indexer_uri = "http://localhost:20956".to_string();
        let chain = Chain::Regtest;
        let perf = Performance::High;
        let minconf: u32 = 1;

        let engine = WalletEngine::new().expect("engine new");

        let (tx, rx) = std_mpsc::channel();
        engine
            .set_listener(Box::new(CapturingListener { tx }))
            .expect("set_listener");

        // Expect EngineReady
        let _ = recv_timeout(&rx, Duration::from_secs(2));

        engine
            .init_new(indexer_uri, chain, perf, minconf)
            .expect("init_new");

        engine.start_sync().expect("start_sync");

        let deadline = std::time::Instant::now() + Duration::from_secs(90);
        loop {
            if std::time::Instant::now() > deadline {
                panic!("timeout waiting for SyncFinished");
            }

            let ev = recv_timeout(&rx, Duration::from_secs(5));
            match ev {
                WalletEvent::SyncFinished => break,
                WalletEvent::Error { code, message } => {
                    panic!("sync error: {code} {message}");
                }
                _ => {}
            }
        }

        let bal = engine.get_balance_snapshot().expect("balance snapshot");
        eprintln!("balance after sync: {bal:?}");
    }

    /// Real sync smoke test (requires a running regtest lightwalletd at the URI).
    /// Run manually:
    ///   cargo test -p ffi real_sync_progress_smoke -- --ignored --nocapture
    #[test]
    #[ignore = "requires non-existing running regtest networkd"]
    fn real_sync_progress_smoke() {
        let indexer_uri = "http://localhost:20956".to_string();
        let chain = Chain::Regtest;
        let perf = Performance::High;
        let minconf: u32 = 1;

        let engine = WalletEngine::new().expect("engine new");

        let (tx, rx) = std_mpsc::channel();
        engine
            .set_listener(Box::new(CapturingListener { tx }))
            .expect("set_listener");

        // Expect EngineReady
        let _ = recv_timeout(&rx, Duration::from_secs(2));

        engine
            .init_new(indexer_uri, chain, perf, minconf)
            .expect("init_new");

        engine.start_sync().expect("start_sync");

        let deadline = std::time::Instant::now() + Duration::from_secs(120);

        let mut saw_started = false;
        let mut saw_progress = false;
        let mut last_percent: f32 = 0.0;

        loop {
            if std::time::Instant::now() > deadline {
                panic!(
                    "timeout waiting for SyncFinished (started={saw_started}, progress={saw_progress})"
                );
            }

            let ev = recv_timeout(&rx, Duration::from_secs(5));
            match ev {
                WalletEvent::SyncStarted => {
                    saw_started = true;
                    eprintln!("[sync] started");
                }

                WalletEvent::SyncProgress {
                    wallet_height,
                    network_height,
                    percent,
                } => {
                    // Require at least one progress tick.
                    saw_progress = true;

                    if percent + 0.05 < last_percent {
                        eprintln!(
                            "[sync] WARNING: percent regressed: {last_percent:.3} -> {percent:.3}"
                        );
                    }
                    last_percent = percent;

                    eprintln!(
                        "[sync] progress: wallet_height={wallet_height} network_height={network_height} percent={percent:.3}"
                    );
                }

                WalletEvent::BalanceChanged(bal) => {
                    eprintln!("[sync] balance changed: {bal:?}");
                }

                WalletEvent::SyncPaused => {
                    panic!("sync paused unexpectedly");
                }

                WalletEvent::SyncFinished => {
                    eprintln!("[sync] finished");
                    break;
                }

                WalletEvent::Error { code, message } => {
                    panic!("sync error: {code} {message}");
                }

                other => {
                    eprintln!("[sync] other event: {other:?}");
                }
            }
        }

        assert!(saw_started, "never saw SyncStarted");
        assert!(saw_progress, "never saw SyncProgress");

        let bal = engine.get_balance_snapshot().expect("balance snapshot");
        eprintln!("balance after sync: {bal:?}");
    }

    /// Smoke test: sync to tip, then restart sync every 10 seconds until we
    /// observe >= 5 distinct *new* network heights (via SyncProgress) beyond
    /// the initial tip.
    ///
    /// This does NOT query latest block height externally.
    /// It relies purely on SyncProgress events emitted during each sync run.
    #[test]
    #[ignore = "requires non-existing running regtest networkd"]
    fn real_sync_observe_5_new_block_heights_smoke() {
        use std::collections::BTreeSet;
        use std::sync::mpsc as std_mpsc;
        use std::time::{Duration, Instant};

        let indexer_uri = "http://localhost:18892".to_string();
        let chain = Chain::Regtest;
        let perf = Performance::High;
        let minconf: u32 = 1;

        let engine = WalletEngine::new().expect("engine new");

        let (tx, rx) = std_mpsc::channel();
        engine
            .set_listener(Box::new(CapturingListener { tx }))
            .expect("set_listener");

        // Best-effort wait for EngineReady.
        let _ = recv_timeout(&rx, Duration::from_secs(2));

        engine
            .init_new(indexer_uri, chain, perf, minconf)
            .expect("init_new");

        // TODO: refactor. run a sync to SyncFinished and return the last seen (wh, nh) from progress.
        fn sync_to_finished_and_get_last_progress(
            engine: &WalletEngine,
            rx: &std_mpsc::Receiver<WalletEvent>,
            timeout: Duration,
            label: &str,
        ) -> (u32, u32) {
            engine.start_sync().expect("start_sync");
            eprintln!("[{label}] started sync");

            let deadline = Instant::now() + timeout;
            let mut last_progress: Option<(u32, u32)> = None;

            loop {
                if Instant::now() > deadline {
                    panic!("[{label}] timeout waiting for SyncFinished");
                }

                let ev = recv_timeout(rx, Duration::from_secs(5));
                match ev {
                    WalletEvent::SyncProgress {
                        wallet_height,
                        network_height,
                        percent,
                    } => {
                        last_progress = Some((wallet_height, network_height));
                        eprintln!(
                            "[{label}] progress: wh={wallet_height} nh={network_height} pct={percent:.3}"
                        );
                    }
                    WalletEvent::SyncFinished => {
                        let (wh, nh) = last_progress.unwrap_or((0, 0));
                        eprintln!("[{label}] SyncFinished (last wh={wh} nh={nh})");
                        return (wh, nh);
                    }
                    WalletEvent::Error { code, message } => {
                        panic!("[{label}] sync error: {code} {message}");
                    }
                    _ => {}
                }
            }
        }

        let per_sync_timeout = Duration::from_secs(90);
        let overall_deadline = Instant::now() + Duration::from_secs(10 * 60); // 10 minutes

        let (_wh0, nh0) =
            sync_to_finished_and_get_last_progress(&engine, &rx, per_sync_timeout, "initial");
        if nh0 == 0 {
            eprintln!("[follow-test] warning: initial nh0=0 (did you connect to lightwalletd?)");
        }
        eprintln!("[follow-test] baseline nh0={nh0}");

        let mut observed_new_heights: BTreeSet<u32> = BTreeSet::new();

        while observed_new_heights.len() < 5 {
            if Instant::now() > overall_deadline {
                panic!(
                    "timeout waiting for >= 5 distinct new network heights > nh0={nh0}. saw {}: {:?}",
                    observed_new_heights.len(),
                    observed_new_heights
                );
            }

            // Restart sync every 10 seconds (mining frequency unknown). TODO: make this better somehow
            std::thread::sleep(Duration::from_secs(10));
            engine.start_sync().expect("start_sync (restart)");
            eprintln!(
                "[follow-test] restart sync attempt; currently have {}/5 heights",
                observed_new_heights.len()
            );

            // For this run, we listen for progress (and/or finish). Any progress nh > nh0 counts.
            let run_deadline = Instant::now() + per_sync_timeout;
            loop {
                if Instant::now() > run_deadline {
                    eprintln!("[follow-test] restart run timed out; will try again");
                    break;
                }

                let ev = recv_timeout(&rx, Duration::from_secs(5));
                match ev {
                    WalletEvent::SyncProgress {
                        wallet_height,
                        network_height,
                        percent,
                    } => {
                        if network_height > nh0 {
                            let inserted = observed_new_heights.insert(network_height);
                            if inserted {
                                eprintln!(
                                    "[follow-test] NEW network_height observed: nh={network_height} (wh={wallet_height} pct={percent:.3}) distinct={}/5",
                                    observed_new_heights.len()
                                );
                            } else {
                                eprintln!(
                                    "[follow-test] progress: wh={wallet_height} nh={network_height} pct={percent:.3} distinct={}/5",
                                    observed_new_heights.len()
                                );
                            }
                        } else {
                            eprintln!(
                                "[follow-test] progress (no new blocks): wh={wallet_height} nh={network_height} pct={percent:.3}"
                            );
                        }

                        if observed_new_heights.len() >= 5 {
                            break;
                        }
                    }
                    WalletEvent::SyncFinished => {
                        eprintln!("[follow-test] SyncFinished (restart run)");
                        break;
                    }
                    WalletEvent::Error { code, message } => {
                        panic!("[follow-test] sync error while observing: {code} {message}");
                    }
                    _ => {}
                }
            }
        }

        eprintln!(
            "[follow-test] PASS: observed >=5 distinct new network heights beyond nh0={nh0}: {:?}",
            observed_new_heights
        );

        let bal = engine.get_balance_snapshot().expect("balance snapshot");
        eprintln!("[follow-test] balance after follow: {bal:?}");
    }
}
