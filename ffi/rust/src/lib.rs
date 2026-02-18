pub mod config;
pub mod error;
pub mod state;

use std::{
    panic::{self, AssertUnwindSafe},
    sync::Arc,
    thread,
    time::Duration,
};

use bip0039::Mnemonic;
use pepper_sync::wallet::SyncMode;
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
}

#[uniffi::export(callback_interface)]
pub trait WalletListener: Send + Sync {
    fn on_event(&self, event: WalletEvent);
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

struct EngineInner {
    cmd_tx: mpsc::Sender<Command>,
    listener: std::sync::Mutex<Option<Arc<dyn WalletListener>>>,
}

impl EngineInner {
    pub(crate) async fn handle_start_sync_spawn(self: Arc<Self>, st: Arc<Mutex<EngineState>>) {
        let (lc, indexer_uri) = {
            let mut s = st.lock().await;

            if s.syncing {
                return;
            }

            let Some(lc) = s.lightclient.as_ref().cloned() else {
                emit(
                    &self,
                    WalletEvent::Error {
                        code: "start_sync_failed".into(),
                        message: WalletError::NotInitialized.to_string(),
                    },
                );
                return;
            };

            s.syncing = true;
            let indexer_uri = s.indexer_uri.clone();

            (lc, indexer_uri)
        };

        emit(&self, WalletEvent::SyncStarted);

        // Spawn the sync loop so the command loop stays responsive.
        let inner = self.clone();
        let st_for_task = st.clone();

        let task = tokio::spawn(async move {
            // Kick off sync/resume with a short write-lock section.
            {
                let mut guard = lc.write().await;

                if guard.sync_mode() == SyncMode::Paused {
                    // TODO: Replace with resume_sync() when available.
                    if let Err(e) = guard.pause_sync() {
                        emit(
                            &inner,
                            WalletEvent::Error {
                                code: "start_sync_failed".into(),
                                message: format!("resume_sync: {e}"),
                            },
                        );
                        let mut s = st_for_task.lock().await;
                        s.syncing = false;
                        return;
                    }
                } else {
                    // TODO: Thisassumes sync() starts the background sync and returns reasonably quickly.
                    if let Err(e) = guard.sync().await {
                        emit(
                            &inner,
                            WalletEvent::Error {
                                code: "sync_failed".into(),
                                message: e.to_string(),
                            },
                        );
                        let mut s = st_for_task.lock().await;
                        s.syncing = false;
                        return;
                    }
                }
            }

            let mut last_balance_emitted: Option<BalanceSnapshot> = None;

            loop {
                // Read-only stuff
                let (wh, poll, bal_opt) = {
                    let mut guard = lc.write().await;

                    let wh = {
                        let w = guard.wallet.read().await;
                        w.sync_state
                            .highest_scanned_height()
                            .map(u32::from)
                            .unwrap_or(0)
                    };

                    let poll = guard.poll_sync();

                    let bal_opt = match guard.account_balance(AccountId::ZERO).await {
                        Ok(bal) => Some(balance_snapshot_from_balance(&bal)),
                        Err(_) => None,
                    };

                    (wh, poll, bal_opt)
                };

                // network height is independent of LightClient lock
                let nh = match indexer_uri.as_ref() {
                    Some(uri) if *uri != http::Uri::default() => {
                        match zingolib::grpc_connector::get_latest_block(uri.clone()).await {
                            Ok(b) => b.height as u32,
                            Err(_) => 0,
                        }
                    }
                    _ => 0,
                };

                let percent = if nh > 0 {
                    (wh as f32 / nh as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };

                emit(
                    &inner,
                    WalletEvent::SyncProgress {
                        wallet_height: wh,
                        network_height: nh,
                        percent,
                    },
                );

                if let Some(snap) = bal_opt {
                    if last_balance_emitted.as_ref() != Some(&snap) {
                        last_balance_emitted = Some(snap.clone());
                        emit(&inner, WalletEvent::BalanceChanged(snap));
                    }
                }

                match poll {
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
                    PollReport::NotReady | PollReport::NoHandle => {
                        // still running
                    }
                }

                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        });

        let mut s = st.lock().await;
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
            let (config, lw_uri) = construct_config(indexer_uri, chain, perf, minconf)?;

            let chain_height = zingolib::grpc_connector::get_latest_block(lw_uri.clone())
                .await
                .map(|b| BlockHeight::from_u32(b.height as u32))
                .map_err(|e| WalletError::Internal(format!("get_latest_block: {e}")))?;

            let birthday = chain_height.saturating_sub(100);

            let lc = zingolib::lightclient::LightClient::new(config, birthday, false)
                .map_err(|e| WalletError::Internal(format!("LightClient::new: {e}")))?;

            st.lightclient = Some(Arc::new(RwLock::new(lc)));
            st.indexer_uri = Some(lw_uri);
            st.last_balance = None;
            st.syncing = false;
            st.sync_task = None;
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
            let (config, lw_uri) = construct_config(indexer_uri, chain, perf, minconf)?;

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

            st.lightclient = Some(Arc::new(RwLock::new(lc)));
            st.indexer_uri = Some(lw_uri);
            st.last_balance = None;
            st.syncing = false;
            st.sync_task = None;
            Ok(())
        })
        .await;

        let _ = reply.send(res);
    }

    pub(crate) async fn handle_get_balance(
        &self,
        st: &mut EngineState,
        reply: oneshot::Sender<Result<BalanceSnapshot, WalletError>>,
    ) {
        let res: Result<BalanceSnapshot, WalletError> = (async {
            let lc = st
                .lightclient
                .as_ref()
                .ok_or(WalletError::NotInitialized)?
                .clone();

            let guard = lc.read().await;
            let bal = guard
                .account_balance(AccountId::ZERO)
                .await
                .map_err(|e| WalletError::Internal(format!("account_balance: {e}")))?;

            Ok(balance_snapshot_from_balance(&bal))
        })
        .await;

        let _ = reply.send(res);
    }

    pub(crate) async fn handle_get_network_height(
        &self,
        st: &mut EngineState,
        reply: oneshot::Sender<Result<u32, WalletError>>,
    ) {
        let res: Result<u32, WalletError> = (async {
            let uri = st.indexer_uri.clone().ok_or(WalletError::NotInitialized)?;
            let b = zingolib::grpc_connector::get_latest_block(uri)
                .await
                .map_err(|e| WalletError::Internal(format!("get_latest_block: {e}")))?;
            Ok(b.height as u32)
        })
        .await;

        let _ = reply.send(res);
    }

    pub(crate) async fn handle_pause_sync(&self, st: &mut EngineState) {
        let Some(lc) = st.lightclient.as_ref().cloned() else {
            emit(
                self,
                WalletEvent::Error {
                    code: "pause_sync_failed".into(),
                    message: WalletError::NotInitialized.to_string(),
                },
            );
            return;
        };

        let guard = lc.write().await;
        match guard.pause_sync() {
            Ok(_) => emit(self, WalletEvent::SyncPaused),
            Err(e) => emit(
                self,
                WalletEvent::Error {
                    code: "pause_sync_failed".into(),
                    message: e.to_string(),
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
    GetBalance {
        reply: oneshot::Sender<Result<BalanceSnapshot, WalletError>>,
    },
    GetNetworkHeight {
        reply: oneshot::Sender<Result<u32, WalletError>>,
    },
    StartSync,
    PauseSync,
    Shutdown,
}

#[derive(uniffi::Object, Clone)]
pub struct WalletEngine {
    inner: Arc<EngineInner>,
}

/// Engine thread runtime only.
fn create_engine_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("tokio runtime")
}

// TODO: THIS NEEDS TO BE BEHIND AN ASYNC LOCK. With the current setup, sync will block the thread.
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

                        Command::GetBalance { reply } => {
                            let mut guard = st.lock().await;
                            inner_for_task.handle_get_balance(&mut guard, reply).await;
                        }

                        Command::GetNetworkHeight { reply } => {
                            let mut guard = st.lock().await;
                            inner_for_task
                                .handle_get_network_height(&mut guard, reply)
                                .await;
                        }

                        Command::StartSync => {
                            inner_for_task
                                .clone()
                                .handle_start_sync_spawn(st.clone())
                                .await;
                        }

                        Command::PauseSync => {
                            let mut guard = st.lock().await;
                            inner_for_task.handle_pause_sync(&mut guard).await;
                        }

                        Command::Shutdown => break,
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
            .try_send(Command::Shutdown)
            .map_err(|_| WalletError::CommandQueueClosed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc as std_mpsc;

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

        // EngineReady first
        let _ = recv_timeout(&rx, Duration::from_secs(2));

        engine.start_sync().expect("start_sync send command");

        // Should emit error from engine thread
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
