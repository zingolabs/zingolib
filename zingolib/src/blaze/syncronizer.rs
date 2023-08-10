use crate::blaze::syncdata::BlazeSyncData;
use crate::ZingoConfig;
use std::sync::Arc;
use tokio::sync::{mpsc::unbounded_channel, oneshot, Mutex, RwLock};

pub struct BlazeSyncronizer {
    pub lock: Mutex<()>,

    pub mempool_monitor: std::sync::RwLock<Option<std::thread::JoinHandle<()>>>,

    pub data: Arc<RwLock<BlazeSyncData>>,
    pub interrupt: Arc<RwLock<bool>>,
}
impl BlazeSyncronizer {
    pub fn new(config: &ZingoConfig) -> Self {
        BlazeSyncronizer {
            lock: Mutex::new(()),
            mempool_monitor: std::sync::RwLock::new(None),
            data: Arc::new(RwLock::new(BlazeSyncData::new(config))),
            interrupt: Arc::new(RwLock::new(false)),
        }
    }
}
