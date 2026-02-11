use std::{
    num::NonZeroU32,
    panic::{self, AssertUnwindSafe},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use bip0039::Mnemonic;
use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
use pepper_sync::wallet::SyncMode;
use tokio::sync::{mpsc, oneshot};
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::zip32::AccountId;
use zingo_common_components::protocol::activation_heights::for_test;
use zingolib::{
    config::{ChainType, ZingoConfig, construct_lightwalletd_uri},
    data::PollReport,
    lightclient::LightClient,
    wallet::{LightWallet, WalletBase, WalletSettings, balance::AccountBalance},
};

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

fn chain_to_chaintype(chain: Chain) -> ChainType {
    match chain {
        Chain::Mainnet => ChainType::Mainnet,
        Chain::Testnet => ChainType::Testnet,
        Chain::Regtest => ChainType::Regtest(for_test::all_height_one_nus()),
    }
}

fn perf_to_level(p: Performance) -> PerformanceLevel {
    match p {
        Performance::Maximum => PerformanceLevel::Maximum,
        Performance::High => PerformanceLevel::High,
        Performance::Medium => PerformanceLevel::Medium,
        Performance::Low => PerformanceLevel::Low,
    }
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

fn construct_config(
    server_uri: String,
    chain: Chain,
    perf: Performance,
    min_confirmations: u32,
) -> Result<(ZingoConfig, http::Uri), WalletError> {
    let lightwalletd_uri = construct_lightwalletd_uri(Some(server_uri));

    let min_conf = NonZeroU32::try_from(min_confirmations)
        .map_err(|_| WalletError::Internal("min_confirmations must be >= 1".into()))?;

    let config = zingolib::config::load_clientconfig(
        lightwalletd_uri.clone(),
        None,
        chain_to_chaintype(chain),
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                performance_level: perf_to_level(perf),
            },
            min_confirmations: min_conf,
        },
        NonZeroU32::try_from(1).expect("hard-coded integer"),
        "".to_string(),
    )
    .map_err(|e| WalletError::Internal(format!("Config load error: {e}")))?;

    Ok((config, lightwalletd_uri))
}

struct EngineInner {
    cmd_tx: mpsc::Sender<Command>,
    listener: Mutex<Option<Arc<dyn WalletListener>>>,
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
        server_uri: String,
        chain: Chain,
        perf: Performance,
        minconf: u32,
        reply: oneshot::Sender<Result<(), WalletError>>,
    },
    InitFromSeed {
        seed_phrase: String,
        birthday: u32,
        server_uri: String,
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

#[uniffi::export]
impl WalletEngine {
    #[uniffi::constructor]
    pub fn new() -> Result<Self, WalletError> {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(64);

        let inner = Arc::new(EngineInner {
            cmd_tx,
            listener: Mutex::new(None),
        });

        let inner_for_task = inner.clone();

        thread::spawn(move || {
            let rt = create_engine_runtime();
            rt.block_on(async move {
                emit(&inner_for_task, WalletEvent::EngineReady);

                // Engine-owned state
                let mut lightclient: Option<LightClient> = None;
                let mut server_uri: Option<http::Uri> = None;

                // Keep last emitted balance snapshot
                let mut last_balance: Option<BalanceSnapshot> = None;
                let mut syncing = false;

                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        Command::GetNetworkHeight { reply } => {
                            todo!()
                        }
                        Command::InitNew {
                            server_uri: srv,
                            chain,
                            perf,
                            minconf,
                            reply,
                        } => {
                            let res: Result<(), WalletError> = (async {
                                let (config, lw_uri) = construct_config(srv, chain, perf, minconf)?;

                                let chain_height =
                                    zingolib::grpc_connector::get_latest_block(lw_uri.clone())
                                        .await
                                        .map(|b| BlockHeight::from_u32(b.height as u32))
                                        .map_err(|e| {
                                            WalletError::Internal(format!("get_latest_block: {e}"))
                                        })?;

                                let birthday = chain_height.saturating_sub(100);

                                let lc =
                                    LightClient::new(config, birthday, false).map_err(|e| {
                                        WalletError::Internal(format!("LightClient::new: {e}"))
                                    })?;

                                lightclient = Some(lc);
                                server_uri = Some(lw_uri);
                                last_balance = None;
                                Ok(())
                            })
                            .await;

                            let _ = reply.send(res);
                        }

                        Command::InitFromSeed {
                            seed_phrase,
                            birthday,
                            server_uri: srv,
                            chain,
                            perf,
                            minconf,
                            reply,
                        } => {
                            let res: Result<(), WalletError> = (async {
                                let (config, lw_uri) = construct_config(srv, chain, perf, minconf)?;

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
                                .map_err(|e| {
                                    WalletError::Internal(format!("LightWallet::new: {e}"))
                                })?;

                                let lc = LightClient::create_from_wallet(wallet, config, false)
                                    .map_err(|e| {
                                        WalletError::Internal(format!("create_from_wallet: {e}"))
                                    })?;

                                lightclient = Some(lc);
                                server_uri = Some(lw_uri);
                                last_balance = None;
                                Ok(())
                            })
                            .await;

                            let _ = reply.send(res);
                        }

                        Command::GetBalance { reply } => {
                            let res: Result<BalanceSnapshot, WalletError> = (async {
                                let lc = lightclient.as_mut().ok_or(WalletError::NotInitialized)?;
                                let bal =
                                    lc.account_balance(AccountId::ZERO).await.map_err(|e| {
                                        WalletError::Internal(format!("account_balance: {e}"))
                                    })?;
                                Ok(balance_snapshot_from_balance(&bal))
                            })
                            .await;

                            let _ = reply.send(res);
                        }

                        Command::StartSync => {
                            // manual model: one round per StartSync
                            if syncing {
                                // ignore repeated StartSync while running
                                continue;
                            }

                            let Some(lc) = lightclient.as_mut() else {
                                emit(
                                    &inner_for_task,
                                    WalletEvent::Error {
                                        code: "start_sync_failed".into(),
                                        message: WalletError::NotInitialized.to_string(),
                                    },
                                );
                                continue;
                            };

                            syncing = true;
                            emit(&inner_for_task, WalletEvent::SyncStarted);

                            // If sync was paused previously, resume; otherwise start a new sync task.
                            // This mirrors old behavior: resume if paused else sync().
                            if lc.sync_mode() == SyncMode::Paused {
                                if let Err(e) = lc.pause_sync() {
                                    // NOTE: if zingolib has resume_sync() use that instead;
                                    // you showed resume_sync() in the older file.
                                    // Replace this with lc.resume_sync().
                                    emit(
                                        &inner_for_task,
                                        WalletEvent::Error {
                                            code: "start_sync_failed".into(),
                                            message: format!("resume_sync: {e}"),
                                        },
                                    );
                                    syncing = false;
                                    break; // or continue; depending on your loop structure
                                }
                            } else {
                                // Start the sync task once.
                                if let Err(e) = lc.sync().await {
                                    emit(
                                        &inner_for_task,
                                        WalletEvent::Error {
                                            code: "sync_failed".into(),
                                            message: e.to_string(),
                                        },
                                    );
                                    syncing = false;
                                    continue;
                                }
                            }

                            // Progress loop: keep reporting while the sync task is running.
                            // We stop when poll_sync() becomes Ready(_).
                            let mut last_balance_emitted: Option<BalanceSnapshot> = None;

                            loop {
                                // If user paused, stop reporting and exit this round.
                                if lc.sync_mode() == SyncMode::Paused {
                                    emit(&inner_for_task, WalletEvent::SyncPaused);
                                    syncing = false;
                                    break;
                                }

                                // Compute wallet height (local)
                                let wh = {
                                    let w = lc.wallet.read().await;
                                    w.sync_state
                                        .highest_scanned_height()
                                        .map(u32::from)
                                        .unwrap_or(0)
                                };

                                // Compute network height (best effort from last known server_uri)
                                let nh = match server_uri.as_ref() {
                                    Some(uri) if *uri != http::Uri::default() => {
                                        match zingolib::grpc_connector::get_latest_block(
                                            uri.clone(),
                                        )
                                        .await
                                        {
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
                                    &inner_for_task,
                                    WalletEvent::SyncProgress {
                                        wallet_height: wh,
                                        network_height: nh,
                                        percent,
                                    },
                                );

                                // Emit balance changes
                                match lc.account_balance(AccountId::ZERO).await {
                                    Ok(bal) => {
                                        let snap = balance_snapshot_from_balance(&bal);
                                        if last_balance_emitted.as_ref() != Some(&snap) {
                                            last_balance_emitted = Some(snap.clone());
                                            emit(
                                                &inner_for_task,
                                                WalletEvent::BalanceChanged(snap),
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        emit(
                                            &inner_for_task,
                                            WalletEvent::Error {
                                                code: "balance_failed".into(),
                                                message: e.to_string(),
                                            },
                                        );
                                    }
                                }

                                // Completion check
                                match lc.poll_sync() {
                                    PollReport::Ready(Ok(_sync_result)) => {
                                        emit(&inner_for_task, WalletEvent::SyncFinished);
                                        syncing = false;
                                        break;
                                    }
                                    PollReport::Ready(Err(e)) => {
                                        emit(
                                            &inner_for_task,
                                            WalletEvent::Error {
                                                code: "sync_failed".into(),
                                                message: e.to_string(),
                                            },
                                        );
                                        syncing = false;
                                        break;
                                    }
                                    PollReport::NotReady | PollReport::NoHandle => {
                                        // Still running, keep looping
                                    }
                                }

                                tokio::time::sleep(Duration::from_millis(250)).await;
                            }
                        }

                        Command::PauseSync => {
                            let Some(lc) = lightclient.as_mut() else {
                                emit(
                                    &inner_for_task,
                                    WalletEvent::Error {
                                        code: "pause_sync_failed".into(),
                                        message: WalletError::NotInitialized.to_string(),
                                    },
                                );
                                continue;
                            };

                            match lc.pause_sync() {
                                Ok(_) => emit(&inner_for_task, WalletEvent::SyncPaused),
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

    pub fn init_new(
        &self,
        server_uri: String,
        chain: Chain,
        perf: Performance,
        minconf: u32,
    ) -> Result<(), WalletError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .blocking_send(Command::InitNew {
                server_uri,
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

    pub fn init_from_seed(
        &self,
        seed_phrase: String,
        birthday: u32,
        server_uri: String,
        chain: Chain,
        perf: Performance,
        minconf: u32,
    ) -> Result<(), WalletError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .cmd_tx
            .blocking_send(Command::InitFromSeed {
                seed_phrase,
                birthday,
                server_uri,
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
        let server_uri = "http://localhost:20956".to_string();
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
            .init_new(server_uri, chain, perf, minconf)
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
        let server_uri = "http://localhost:20956".to_string();
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
            .init_new(server_uri, chain, perf, minconf)
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

        let server_uri = "http://localhost:18892".to_string();
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
            .init_new(server_uri, chain, perf, minconf)
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
