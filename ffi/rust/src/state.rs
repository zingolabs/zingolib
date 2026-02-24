use std::sync::Arc;

use tokio::{sync::RwLock, task::JoinHandle};
use zingolib::lightclient::LightClient;

use crate::{BalanceSnapshot, WalletBackend, ZingolibBackend};

pub(crate) struct EngineState {
    pub backend: Option<Arc<dyn WalletBackend>>,
    pub syncing: bool,
    pub sync_task: Option<tokio::task::JoinHandle<()>>,
    pub last_balance: Option<BalanceSnapshot>,
}

impl EngineState {
    /// Fresh engine state with no wallet loaded yet.
    ///
    /// Call `init_new/init_from_seed/init_from_ufvk` to install a real backend later.
    pub(crate) fn new() -> Self {
        Self {
            backend: None,
            syncing: false,
            sync_task: None,
            last_balance: None,
        }
    }

    /// Convenience constructor that injects a backend directly.
    pub(crate) fn with_backend(backend: Arc<dyn WalletBackend>) -> Self {
        Self {
            backend: Some(backend),
            syncing: false,
            sync_task: None,
            last_balance: None,
        }
    }

    /// Replace any existing backend.
    pub(crate) fn set_backend(&mut self, backend: Arc<dyn WalletBackend>) {
        self.backend = Some(backend);
        self.last_balance = None;
        self.syncing = false;
        self.sync_task = None;
    }

    /// Clear the backend.
    pub(crate) fn clear_backend(&mut self) {
        self.backend = None;
        self.last_balance = None;
        self.syncing = false;
        self.sync_task = None;
    }
}
