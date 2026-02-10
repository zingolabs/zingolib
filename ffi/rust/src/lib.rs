use std::{
    panic::{self, AssertUnwindSafe},
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::mpsc;

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum WalletError {
    #[error("Command queue closed")]
    CommandQueueClosed,
    #[error("Listener lock poisoned")]
    ListenerLockPoisoned,
    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Clone, Debug, uniffi::Record)]
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
}

#[derive(uniffi::Object)]
pub struct WalletEngine {
    inner: Arc<EngineInner>,
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

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(async move {
                emit(&inner_for_task, WalletEvent::EngineReady);

                let mut syncing = false;

                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        Command::StartSync => {
                            if syncing {
                                continue;
                            }
                            syncing = true;
                            emit(&inner_for_task, WalletEvent::SyncStarted);

                            for i in 0..=100u32 {
                                tokio::time::sleep(Duration::from_millis(50)).await;

                                emit(
                                    &inner_for_task,
                                    WalletEvent::SyncProgress {
                                        wallet_height: 1000 + i,
                                        network_height: 1100,
                                        percent: i as f32 / 100.0,
                                    },
                                );

                                if !syncing {
                                    break;
                                }
                            }

                            emit(&inner_for_task, WalletEvent::SyncFinished);
                            syncing = false;
                        }
                        Command::PauseSync => {
                            syncing = false;
                            emit(&inner_for_task, WalletEvent::SyncPaused);
                        }
                        Command::Shutdown => break,
                    }
                }
            });
        });

        Ok(Self { inner })
    }

    // Box<dyn WalletListener> for UniFFI
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

    pub fn get_balance_snapshot(&self) -> Result<BalanceSnapshot, WalletError> {
        Ok(BalanceSnapshot {
            confirmed: "0".to_string(),
            total: "0".to_string(),
        })
    }
}

fn emit(inner: &EngineInner, event: WalletEvent) {
    let listener_opt = inner.listener.lock().ok().and_then(|g| g.clone());
    if let Some(listener) = listener_opt {
        // Avoid UnwindSafe errors. don’t let a callback panic crash the engine
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            listener.on_event(event);
        }));
    }
}
