use std::{
    panic::{self, AssertUnwindSafe},
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use tokio::sync::mpsc;

use std::num::NonZeroU32;

use bip0039::Mnemonic;
use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
use pepper_sync::wallet::SyncMode;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::zip32::AccountId;

use zingolib::data::PollReport;
use zingolib::lightclient::LightClient;
use zingolib::wallet::{LightWallet, WalletBase, WalletSettings};
use zingolib::{
    config::{ChainType, ZingoConfig, construct_lightwalletd_uri},
    wallet::balance::AccountBalance,
};

use zingo_common_components::protocol::activation_heights::for_test;

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum WalletError {
    #[error("Command queue closed")]
    CommandQueueClosed,
    #[error("Listener lock poisoned")]
    ListenerLockPoisoned,
    #[error("Wallet not initialized")]
    NotInitialized,
    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Clone, Debug, uniffi::Record, PartialEq, Eq)]
pub struct BalanceSnapshot {
    pub confirmed: String,
    pub total: String,
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
    NewTransaction {
        txid: String,
    },
    Error {
        code: String,
        message: String,
    },
}

#[uniffi::export(callback_interface)]
pub trait WalletListener: Send + Sync {
    fn on_event(&self, event: WalletEvent);
}

enum Command {
    StartSync,
    PauseSync,
    Shutdown,
}

struct EngineInner {
    cmd_tx: mpsc::Sender<Command>,
    listener: Mutex<Option<Arc<dyn WalletListener>>>,

    // Real state
    lightclient: RwLock<Option<LightClient>>,
    server_uri: RwLock<Option<http::Uri>>,

    wallet_path: RwLock<Option<PathBuf>>,
}

#[derive(uniffi::Object)]
pub struct WalletEngine {
    inner: Arc<EngineInner>,
}

fn emit(inner: &EngineInner, event: WalletEvent) {
    let listener_opt = inner.listener.lock().ok().and_then(|g| g.clone());
    if let Some(listener) = listener_opt {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            listener.on_event(event);
        }));
    }
}

fn parse_chain(chain_hint: &str) -> Result<ChainType, WalletError> {
    match chain_hint {
        "main" => Ok(ChainType::Mainnet),
        "test" => Ok(ChainType::Testnet),
        "regtest" => Ok(ChainType::Regtest(for_test::all_height_one_nus())),
        other => Err(WalletError::Internal(format!(
            "Invalid chain hint: {other} (expected main|test|regtest)"
        ))),
    }
}

fn parse_performance(performance_level: &str) -> Result<PerformanceLevel, WalletError> {
    match performance_level {
        "Maximum" => Ok(PerformanceLevel::Maximum),
        "High" => Ok(PerformanceLevel::High),
        "Medium" => Ok(PerformanceLevel::Medium),
        "Low" => Ok(PerformanceLevel::Low),
        other => Err(WalletError::Internal(format!(
            "Invalid performance level: {other} (expected Maximum|High|Medium|Low)"
        ))),
    }
}

fn construct_config(
    server_uri: String,
    chain_hint: String,
    performance_level: String,
    min_confirmations: u32,
) -> Result<(ZingoConfig, http::Uri), WalletError> {
    let lightwalletd_uri = construct_lightwalletd_uri(Some(server_uri));
    let chaintype = parse_chain(chain_hint.as_str())?;
    let performancetype = parse_performance(performance_level.as_str())?;

    let min_conf = NonZeroU32::try_from(min_confirmations)
        .map_err(|_| WalletError::Internal("min_confirmations must be >= 1".into()))?;

    let config = zingolib::config::load_clientconfig(
        lightwalletd_uri.clone(),
        None,
        chaintype,
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                performance_level: performancetype,
            },
            min_confirmations: min_conf,
        },
        NonZeroU32::try_from(1).expect("hard-coded integer"),
        "".to_string(),
    )
    .map_err(|e| WalletError::Internal(format!("Config load error: {e}")))?;

    Ok((config, lightwalletd_uri))
}

fn with_lightclient_mut<T>(
    inner: &EngineInner,
    f: impl FnOnce(&mut LightClient) -> Result<T, WalletError>,
) -> Result<T, WalletError> {
    let mut guard = inner
        .lightclient
        .write()
        .map_err(|_| WalletError::Internal("lightclient lock poisoned".into()))?;
    let lc = guard.as_mut().ok_or(WalletError::NotInitialized)?;
    f(lc)
}

/// One runtime for the engine thread.
fn create_engine_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("tokio runtime")
}

#[uniffi::export]
impl WalletEngine {
    #[uniffi::constructor]
    pub fn new() -> Result<Self, WalletError> {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(64);

        let inner = Arc::new(EngineInner {
            cmd_tx,
            listener: Mutex::new(None),
            lightclient: RwLock::new(None),
            server_uri: RwLock::new(None),
            wallet_path: RwLock::new(None),
        });

        let inner_for_task = inner.clone();

        std::thread::spawn(move || {
            let rt = create_engine_runtime();
            let handle = rt.handle().clone();
            rt.block_on(async move {
                emit(&inner_for_task, WalletEvent::EngineReady);

                // Keep a last-seen snapshot to emit BalanceChanged only on change
                let mut last_balance: Option<BalanceSnapshot> = None;

                let mut syncing = false;

                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        Command::StartSync => {
                            if syncing {
                                continue;
                            }
                            // Must be initialized
                            let start_res = with_lightclient_mut(&inner_for_task, |lc| {
                                // This follows your old run_sync() logic:
                                if lc.sync_mode() == SyncMode::Paused {
                                    lc.resume_sync().map_err(|e| {
                                        WalletError::Internal(format!("resume_sync: {e}"))
                                    })?;
                                    Ok(())
                                } else {
                                    // Launch the zingolib sync task
                                    handle.block_on(async {
                                        lc.sync().await.map(|_| ()).map_err(|e| {
                                            WalletError::Internal(format!("sync(): {e}"))
                                        })
                                    })
                                }
                            });

                            if let Err(e) = start_res {
                                emit(
                                    &inner_for_task,
                                    WalletEvent::Error {
                                        code: "start_sync_failed".into(),
                                        message: e.to_string(),
                                    },
                                );
                                continue;
                            }

                            emit(&inner_for_task, WalletEvent::SyncStarted);

                            // Progress loop: real status from pepper_sync + completion via poll_sync()
                            loop {
                                // If paused externally, break after emitting paused
                                let paused = with_lightclient_mut(&inner_for_task, |lc| {
                                    Ok(lc.sync_mode() == SyncMode::Paused)
                                })
                                .unwrap_or(false);

                                if paused {
                                    emit(&inner_for_task, WalletEvent::SyncPaused);
                                    syncing = false;
                                    break;
                                }

                                let progress_res: Result<(u32, u32), WalletError> =
                                    with_lightclient_mut(&inner_for_task, |lc| {
                                        let wh = wallet_height_u32(lc, &handle)?;

                                        let nh = match get_server_uri(&inner_for_task) {
                                            Some(uri) if uri != http::Uri::default() => {
                                                network_height_u32(uri, &handle)?
                                            }
                                            _ => 0, // offline or not set
                                        };

                                        Ok((wh, nh))
                                    });

                                if let Ok((wh, nh)) = progress_res {
                                    let percent = if nh > 0 {
                                        (wh as f32 / nh as f32).clamp(0.0, 1.0)
                                    } else {
                                        0.0
                                    };

                                    emit(
                                        &inner_for_task,
                                        WalletEvent::SyncProgress {
                                            wallet_height: wh,
                                            network_height: nh,
                                            percent,
                                        },
                                    );
                                }

                                // Emit real balance if it changed
                                let bal_res: Result<BalanceSnapshot, WalletError> =
                                    with_lightclient_mut(&inner_for_task, |lc| {
                                        handle.block_on(async {
                                            let bal = lc
                                                .account_balance(AccountId::ZERO)
                                                .await
                                                .map_err(|e| {
                                                    WalletError::Internal(format!(
                                                        "account_balance: {e}"
                                                    ))
                                                })?;

                                            // Change this mapping to match the concrete balance type you have.
                                            Ok(balance_snapshot_from_balance(&bal))
                                        })
                                    });

                                if let Ok(snap) = bal_res {
                                    if last_balance.as_ref() != Some(&snap) {
                                        last_balance = Some(snap.clone());
                                        emit(&inner_for_task, WalletEvent::BalanceChanged(snap));
                                    }
                                }

                                // Completion check: zingolib's real poll_sync()
                                let done = with_lightclient_mut(&inner_for_task, |lc| {
                                    Ok(matches!(lc.poll_sync(), PollReport::Ready(_)))
                                })
                                .unwrap_or(false);

                                if done {
                                    emit(&inner_for_task, WalletEvent::SyncFinished);
                                    syncing = false;
                                    break;
                                }

                                tokio::time::sleep(Duration::from_millis(250)).await;
                            }
                        }

                        Command::PauseSync => {
                            let res = with_lightclient_mut(&inner_for_task, |lc| {
                                lc.pause_sync().map_err(|e| {
                                    WalletError::Internal(format!("pause_sync: {e}"))
                                })?;
                                Ok(())
                            });

                            match res {
                                Ok(_) => {
                                    emit(&inner_for_task, WalletEvent::SyncPaused);
                                }
                                Err(e) => emit(
                                    &inner_for_task,
                                    WalletEvent::Error {
                                        code: "pause_sync_failed".into(),
                                        message: e.to_string(),
                                    },
                                ),
                            }
                        }

                        Command::Shutdown => break,
                    }
                }
            });
        });

        Ok(Self { inner })
    }

    /// UniFFI callback: Box<dyn WalletListener>
    pub fn set_listener(&self, listener: Box<dyn WalletListener>) -> Result<(), WalletError> {
        let mut guard = self
            .inner
            .listener
            .lock()
            .map_err(|_| WalletError::ListenerLockPoisoned)?;
        *guard = Some(Arc::from(listener));
        Ok(())
    }

    pub fn clear_listener(&self) -> Result<(), WalletError> {
        let mut guard = self
            .inner
            .listener
            .lock()
            .map_err(|_| WalletError::ListenerLockPoisoned)?;
        *guard = None;
        Ok(())
    }

    /// Initialize a brand new wallet (like old init_new)
    pub fn init_new(
        &self,
        server_uri: String,
        chain_hint: String,
        performance_level: String,
        min_confirmations: u32,
    ) -> Result<(), WalletError> {
        let (config, lw_uri) =
            construct_config(server_uri, chain_hint, performance_level, min_confirmations)?;

        // Get latest block from server (real network height) then pick birthday
        let rt = create_engine_runtime();
        let lw_uri_for_height = lw_uri.clone();

        let chain_height = rt.block_on(async move {
            zingolib::grpc_connector::get_latest_block(lw_uri_for_height)
                .await
                .map(|block_id| BlockHeight::from_u32(block_id.height as u32))
                .map_err(|e| WalletError::Internal(format!("get_latest_block: {e}")))
        })?;

        let birthday = chain_height.saturating_sub(100);

        let lc = LightClient::new(config, birthday, false)
            .map_err(|e| WalletError::Internal(format!("LightClient::new: {e}")))?;

        {
            let mut g = self
                .inner
                .lightclient
                .write()
                .map_err(|_| WalletError::Internal("lightclient lock poisoned".into()))?;
            *g = Some(lc);
        }

        {
            let mut g = self
                .inner
                .server_uri
                .write()
                .map_err(|_| WalletError::Internal("server_uri lock poisoned".into()))?;
            *g = Some(lw_uri);
        }

        Ok(())
    }

    /// Initialize from seed (like old init_from_seed)
    pub fn init_from_seed(
        &self,
        seed_phrase: String,
        birthday: u32,
        server_uri: String,
        chain_hint: String,
        performance_level: String,
        min_confirmations: u32,
    ) -> Result<(), WalletError> {
        let (config, lw_uri) =
            construct_config(server_uri, chain_hint, performance_level, min_confirmations)?;

        let mnemonic = Mnemonic::from_phrase(seed_phrase)
            .map_err(|e| WalletError::Internal(format!("Mnemonic parse: {e}")))?;

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

        let lc = LightClient::create_from_wallet(wallet, config, false)
            .map_err(|e| WalletError::Internal(format!("create_from_wallet: {e}")))?;

        {
            let mut g = self
                .inner
                .lightclient
                .write()
                .map_err(|_| WalletError::Internal("lightclient lock poisoned".into()))?;
            *g = Some(lc);
        }

        {
            let mut g = self
                .inner
                .server_uri
                .write()
                .map_err(|_| WalletError::Internal("server_uri lock poisoned".into()))?;
            *g = Some(lw_uri);
        }

        Ok(())
    }

    pub fn start_sync(&self) -> Result<(), WalletError> {
        self.inner
            .cmd_tx
            .try_send(Command::StartSync)
            .map_err(|_| WalletError::CommandQueueClosed)
    }

    pub fn pause_sync(&self) -> Result<(), WalletError> {
        self.inner
            .cmd_tx
            .try_send(Command::PauseSync)
            .map_err(|_| WalletError::CommandQueueClosed)
    }

    pub fn shutdown(&self) -> Result<(), WalletError> {
        self.inner
            .cmd_tx
            .try_send(Command::Shutdown)
            .map_err(|_| WalletError::CommandQueueClosed)
    }

    /// Real balance (Account 0). No simulation.
    pub fn get_balance_snapshot(&self) -> Result<BalanceSnapshot, WalletError> {
        with_lightclient_mut(&self.inner, |lc| {
            let rt = create_engine_runtime();
            rt.block_on(async {
                let bal = lc
                    .account_balance(AccountId::ZERO)
                    .await
                    .map_err(|e| WalletError::Internal(format!("account_balance: {e}")))?;

                Ok(balance_snapshot_from_balance(&bal))
            })
        })
    }

    /// Expose poll_sync result if you still want it.
    pub fn poll_sync(&self) -> Result<bool, WalletError> {
        with_lightclient_mut(&self.inner, |lc| {
            Ok(matches!(lc.poll_sync(), PollReport::Ready(_)))
        })
    }

    /// Change server like old change_server
    pub fn change_server(&self, server_uri: String) -> Result<(), WalletError> {
        let uri = if server_uri.is_empty() {
            http::Uri::default()
        } else {
            http::Uri::from_str(&server_uri)
                .map_err(|e| WalletError::Internal(format!("invalid uri: {e}")))?
        };

        with_lightclient_mut(&self.inner, |lc| {
            lc.set_server(uri.clone());
            Ok(())
        })?;

        let mut g = self
            .inner
            .server_uri
            .write()
            .map_err(|_| WalletError::Internal("server_uri lock poisoned".into()))?;
        *g = Some(uri);

        Ok(())
    }
}

fn get_server_uri(inner: &EngineInner) -> Option<http::Uri> {
    inner.server_uri.read().ok().and_then(|g| g.clone())
}

fn wallet_height_u32(
    lc: &LightClient,
    handle: &tokio::runtime::Handle,
) -> Result<u32, WalletError> {
    handle.block_on(async {
        let wallet = lc.wallet.read().await;
        Ok(wallet
            .sync_state
            .wallet_height()
            .map(u32::from)
            .unwrap_or(0))
    })
}

fn network_height_u32(
    server_uri: http::Uri,
    handle: &tokio::runtime::Handle,
) -> Result<u32, WalletError> {
    handle.block_on(async {
        zingolib::grpc_connector::get_latest_block(server_uri)
            .await
            .map(|block_id| block_id.height as u32)
            .map_err(|e| WalletError::Internal(format!("get_latest_block: {e}")))
    })
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

#[cfg(test)]
mod test {
    use std::time::Duration;

    use tokio::sync::mpsc;

    use crate::{WalletEngine, WalletError, WalletEvent, WalletListener};

    #[derive(Clone)]
    struct CapturingListener {
        tx: mpsc::UnboundedSender<WalletEvent>,
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

    async fn recv_with_timeout(
        rx: &mut mpsc::UnboundedReceiver<WalletEvent>,
        dur: Duration,
    ) -> WalletEvent {
        tokio::time::timeout(dur, rx.recv())
            .await
            .expect("timeout waiting for event")
            .expect("channel closed unexpectedly")
    }

    async fn expect_engine_ready(rx: &mut mpsc::UnboundedReceiver<WalletEvent>) {
        // EngineReady is typically the first event after listener is set,
        // but depending on timing it could already have happened.
        // So: wait a bit for *some* event and accept EngineReady if seen.
        let ev = recv_with_timeout(rx, Duration::from_secs(2)).await;
        assert!(
            matches!(ev, WalletEvent::EngineReady),
            "expected EngineReady, got: {ev:?}"
        );
    }

    async fn expect_error_event(
        rx: &mut mpsc::UnboundedReceiver<WalletEvent>,
        code: &str,
    ) -> WalletEvent {
        // Drain until we see the desired Error code, helps tolerate extra events
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                panic!("timeout waiting for Error({code})");
            }
            let remaining = deadline - now;
            let ev = recv_with_timeout(rx, remaining).await;
            if let WalletEvent::Error { code: c, .. } = &ev {
                if c == code {
                    return ev;
                }
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn emits_engine_ready() {
        let engine = WalletEngine::new().expect("engine new");

        let (tx, mut rx) = mpsc::unbounded_channel();
        engine
            .set_listener(Box::new(CapturingListener { tx }))
            .expect("set_listener");

        expect_engine_ready(&mut rx).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn get_balance_snapshot_errors_when_not_initialized() {
        let engine = WalletEngine::new().expect("engine new");

        let res = engine.get_balance_snapshot();
        assert!(
            matches!(res, Err(WalletError::NotInitialized)),
            "expected NotInitialized, got: {res:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_sync_emits_error_when_not_initialized() {
        let engine = WalletEngine::new().expect("engine new");

        let (tx, mut rx) = mpsc::unbounded_channel();
        engine
            .set_listener(Box::new(CapturingListener { tx }))
            .expect("set_listener");

        expect_engine_ready(&mut rx).await;

        engine.start_sync().expect("start_sync send command");

        // The command loop should emit an Error event when it can't start.
        let ev = expect_error_event(&mut rx, "start_sync_failed").await;
        match ev {
            WalletEvent::Error { code, message } => {
                assert_eq!(code, "start_sync_failed");
                assert!(!message.is_empty(), "expected non-empty error message");
            }
            other => panic!("expected Error event, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn listener_panics_do_not_crash_engine_thread() {
        let engine = WalletEngine::new().expect("engine new");

        // Install a panicking listener.
        engine
            .set_listener(Box::new(PanickingListener))
            .expect("set_listener panicking");

        // Trigger an event from the engine thread. This will call listener and panic,
        // but emit() must catch_unwind and keep the engine alive.
        engine.start_sync().expect("start_sync send command");

        // Give the engine a moment to process and hit the callback.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Swap in a capturing listener. If the engine thread died, we won't get events now.
        let (tx, mut rx) = mpsc::unbounded_channel();
        engine
            .set_listener(Box::new(CapturingListener { tx }))
            .expect("set_listener capturing");

        // Now trigger another command that should result in an Error event
        // (since still not initialized). If we receive it, engine thread is alive.
        engine.pause_sync().expect("pause_sync send command");

        let ev = expect_error_event(&mut rx, "pause_sync_failed").await;
        assert!(
            matches!(ev, WalletEvent::Error { .. }),
            "expected Error, got {ev:?}"
        );
    }

    // TODO: Do not use env vars. Use the infra repo somehow.
    //
    //   ZINGO_TEST_SERVER_URI="https://..." \
    //   ZINGO_TEST_CHAIN_HINT="test" \
    //   ZINGO_TEST_PERF="Medium" \
    //   ZINGO_TEST_MINCONF="1" \
    //   cargo test -p ffi real_sync_smoke -- --ignored --nocapture
    //
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn real_sync_smoke() {
        let server_uri = std::env::var("ZINGO_TEST_SERVER_URI").expect("ZINGO_TEST_SERVER_URI");
        let chain_hint = std::env::var("ZINGO_TEST_CHAIN_HINT").unwrap_or_else(|_| "test".into());
        let perf = std::env::var("ZINGO_TEST_PERF").unwrap_or_else(|_| "Medium".into());
        let minconf: u32 = std::env::var("ZINGO_TEST_MINCONF")
            .unwrap_or_else(|_| "1".into())
            .parse()
            .expect("ZINGO_TEST_MINCONF must be u32");

        let engine = WalletEngine::new().expect("engine new");

        let (tx, mut rx) = mpsc::unbounded_channel();
        engine
            .set_listener(Box::new(CapturingListener { tx }))
            .expect("set_listener");

        expect_engine_ready(&mut rx).await;

        engine
            .init_new(server_uri, chain_hint, perf, minconf)
            .expect("init_new");

        engine.start_sync().expect("start_sync");

        // Wait until SyncFinished or timeout.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                panic!("timeout waiting for SyncFinished");
            }
            let ev = recv_with_timeout(&mut rx, deadline - now).await;
            match ev {
                WalletEvent::SyncFinished => break,
                WalletEvent::Error { code, message } => {
                    panic!("sync error: {code} {message}");
                }
                _ => {}
            }
        }

        let bal = engine.get_balance_snapshot().expect("balance snapshot");
        println!("balance after sync: {bal:?}");
    }
}
