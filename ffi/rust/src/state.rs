use std::sync::Arc;

use tokio::{sync::RwLock, task::JoinHandle};
use zingolib::lightclient::LightClient;

use crate::BalanceSnapshot;

pub(crate) struct EngineState {
    // TODO: add more documentation
    /// `LightClient` lives behind an async RW lock so that sync only takes the lock when starting
    /// pausing or stopping, and so that read-only calls (like getting the wallet's balance and
    /// fetching the chain height) can use a read lock.
    pub(crate) lightclient: Option<Arc<RwLock<LightClient>>>,
    pub(crate) indexer_uri: Option<http::Uri>,

    pub(crate) syncing: bool,
    pub(crate) sync_task: Option<JoinHandle<()>>,

    pub(crate) last_balance: Option<BalanceSnapshot>,
}

impl EngineState {
    pub(crate) fn new() -> Self {
        Self {
            lightclient: None,
            indexer_uri: None,

            syncing: false,
            sync_task: None,

            last_balance: None,
        }
    }
}
