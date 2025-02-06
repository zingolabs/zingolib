use std::{
    collections::HashMap,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
};

use tokio::{
    sync::mpsc,
    task::{JoinError, JoinHandle},
};

use zcash_client_backend::data_api::scanning::{ScanPriority, ScanRange};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::{
    consensus::{self},
    zip32::AccountId,
};

use crate::{
    client::FetchRequest,
    keys::transparent::TransparentAddressId,
    primitives::{Locator, WalletBlock},
    sync,
    traits::{SyncBlocks, SyncWallet},
};

use super::{error::ScanError, scan, ScanResults};

const MAX_WORKER_POOLSIZE: usize = 2;

pub(crate) enum ScannerState {
    Verification,
    Scan,
    Shutdown,
}

impl ScannerState {
    fn verified(&mut self) {
        *self = ScannerState::Scan
    }

    fn scan_completed(&mut self) {
        *self = ScannerState::Shutdown
    }
}

pub(crate) struct Scanner<P> {
    state: ScannerState,
    workers: Vec<ScanWorker<P>>,
    unique_id: usize,
    scan_results_sender: mpsc::UnboundedSender<(ScanRange, Result<ScanResults, ScanError>)>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    consensus_parameters: P,
    ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
}

// TODO: add fn for checking and handling worker errors
impl<P> Scanner<P>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    pub(crate) fn new(
        consensus_parameters: P,
        scan_results_sender: mpsc::UnboundedSender<(ScanRange, Result<ScanResults, ScanError>)>,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
    ) -> Self {
        let workers: Vec<ScanWorker<P>> = Vec::with_capacity(MAX_WORKER_POOLSIZE);

        Self {
            state: ScannerState::Verification,
            workers,
            unique_id: 0,
            consensus_parameters,
            scan_results_sender,
            fetch_request_sender,
            ufvks,
        }
    }

    pub(crate) fn worker_poolsize(&self) -> usize {
        self.workers.len()
    }

    /// Spawns a worker.
    ///
    /// When the worker is running it will wait for a scan task.
    pub(crate) fn spawn_worker(&mut self) {
        tracing::info!("Spawning worker {}", self.unique_id);
        let mut worker = ScanWorker::new(
            self.unique_id,
            self.consensus_parameters.clone(),
            None,
            self.scan_results_sender.clone(),
            self.fetch_request_sender.clone(),
            self.ufvks.clone(),
        );
        worker.run().unwrap();
        self.workers.push(worker);
        self.unique_id += 1;
    }

    /// Spawns the initial pool of workers.
    ///
    /// Poolsize is set by [`self::MAX_WORKER_POOLSIZE`].
    pub(crate) fn spawn_workers(&mut self) {
        for _ in 0..MAX_WORKER_POOLSIZE {
            self.spawn_worker();
        }
    }

    fn idle_worker(&self) -> Option<&ScanWorker<P>> {
        if let Some(idle_worker) = self.workers.iter().find(|worker| !worker.is_scanning()) {
            Some(idle_worker)
        } else {
            None
        }
    }

    async fn shutdown_worker(&mut self, worker_id: usize) -> Result<(), ()> {
        if let Some(worker_index) = self
            .workers
            .iter()
            .position(|worker| worker.id == worker_id)
        {
            let mut worker = self.workers.swap_remove(worker_index);
            worker
                .shutdown()
                .await
                .expect("worker should not be able to panic");
        } else {
            panic!("id not found in worker pool");
        }

        Ok(())
    }

    /// Updates the scanner.
    ///
    /// If verification is still in progress, do not create scan tasks.
    /// If there is an idle worker, create a new scan task and add to worker.
    /// If there are no more range available to scan, shutdown the idle workers.
    pub(crate) async fn update<W>(&mut self, wallet: &mut W, shutdown_mempool: Arc<AtomicBool>)
    where
        W: SyncWallet + SyncBlocks,
    {
        match self.state {
            ScannerState::Verification => {
                let sync_state = wallet.get_sync_state().unwrap();
                if !sync_state
                    .scan_ranges()
                    .iter()
                    .any(|scan_range| scan_range.priority() == ScanPriority::Verify)
                {
                    if sync_state
                        .scan_ranges()
                        .iter()
                        .any(|scan_range| scan_range.priority() == ScanPriority::Ignored)
                    {
                        // the last scan ranges with `Verify` priority are currently being scanned.
                        return;
                    } else {
                        // verification complete
                        self.state.verified();
                        return;
                    }
                }

                // scan ranges with `Verify` priority
                if let Some(worker) = self.idle_worker() {
                    let scan_task = sync::state::create_scan_task(wallet)
                        .unwrap()
                        .expect("scan range with `Verify` priority must exist!");

                    assert_eq!(scan_task.scan_range.priority(), ScanPriority::Verify);
                    worker.add_scan_task(scan_task).unwrap();
                }
            }
            ScannerState::Scan => {
                // create scan tasks until all ranges are scanned or currently scanning
                if let Some(worker) = self.idle_worker() {
                    if let Some(scan_task) = sync::state::create_scan_task(wallet).unwrap() {
                        worker.add_scan_task(scan_task).unwrap();
                    } else if wallet.get_sync_state().unwrap().scan_complete() {
                        self.state.scan_completed();
                    }
                }
            }
            ScannerState::Shutdown => {
                // shutdown mempool
                shutdown_mempool.store(true, atomic::Ordering::Release);

                // shutdown idle workers
                while let Some(worker) = self.idle_worker() {
                    self.shutdown_worker(worker.id)
                        .await
                        .expect("worker should be in worker pool");
                }
            }
        }

        if !wallet.get_sync_state().unwrap().scan_complete() && self.worker_poolsize() == 0 {
            panic!("worker pool should not be empty with unscanned ranges!")
        }
    }
}

struct ScanWorker<P> {
    id: usize,
    handle: Option<JoinHandle<()>>,
    is_scanning: Arc<AtomicBool>,
    consensus_parameters: P,
    scan_task_sender: Option<mpsc::Sender<ScanTask>>,
    scan_results_sender: mpsc::UnboundedSender<(ScanRange, Result<ScanResults, ScanError>)>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
}

impl<P> ScanWorker<P>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    fn new(
        id: usize,
        consensus_parameters: P,
        scan_task_sender: Option<mpsc::Sender<ScanTask>>,
        scan_results_sender: mpsc::UnboundedSender<(ScanRange, Result<ScanResults, ScanError>)>,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
    ) -> Self {
        Self {
            id,
            handle: None,
            is_scanning: Arc::new(AtomicBool::new(false)),
            consensus_parameters,
            scan_task_sender,
            scan_results_sender,
            fetch_request_sender,
            ufvks,
        }
    }

    /// Runs the worker in a new tokio task.
    ///
    /// Waits for a scan task and then calls [`crate::scan::scan`] on the given range.
    fn run(&mut self) -> Result<(), ()> {
        let (scan_task_sender, mut scan_task_receiver) = mpsc::channel::<ScanTask>(1);

        let is_scanning = self.is_scanning.clone();
        let scan_results_sender = self.scan_results_sender.clone();
        let fetch_request_sender = self.fetch_request_sender.clone();
        let consensus_parameters = self.consensus_parameters.clone();
        let ufvks = self.ufvks.clone();

        let handle = tokio::spawn(async move {
            while let Some(scan_task) = scan_task_receiver.recv().await {
                let scan_results = scan(
                    fetch_request_sender.clone(),
                    &consensus_parameters,
                    &ufvks,
                    scan_task.scan_range.clone(),
                    scan_task.previous_wallet_block,
                    scan_task.locators,
                    scan_task.transparent_addresses,
                )
                .await;

                scan_results_sender
                    .send((scan_task.scan_range, scan_results))
                    .expect("receiver should never be dropped before sender!");

                is_scanning.store(false, atomic::Ordering::Release);
            }
        });

        self.handle = Some(handle);
        self.scan_task_sender = Some(scan_task_sender);

        Ok(())
    }

    fn is_scanning(&self) -> bool {
        self.is_scanning.load(atomic::Ordering::Acquire)
    }

    fn add_scan_task(&self, scan_task: ScanTask) -> Result<(), ()> {
        tracing::info!("Adding scan task to worker {}:\n{:#?}", self.id, &scan_task);
        self.scan_task_sender
            .clone()
            .unwrap()
            .try_send(scan_task)
            .expect("worker should never be sent multiple tasks at one time");
        self.is_scanning.store(true, atomic::Ordering::Release);

        Ok(())
    }

    /// Shuts down worker by dropping the sender to the worker task and awaiting the handle.
    ///
    /// This should always be called in the context of the scanner as it must be also be removed from the worker pool.
    async fn shutdown(&mut self) -> Result<(), JoinError> {
        tracing::info!("Shutting down worker {}", self.id);
        if let Some(sender) = self.scan_task_sender.take() {
            drop(sender);
        }
        let handle = self
            .handle
            .take()
            .expect("worker should always have a handle to take!");

        handle.await
    }
}

#[derive(Debug)]
pub(crate) struct ScanTask {
    scan_range: ScanRange,
    previous_wallet_block: Option<WalletBlock>,
    locators: Vec<Locator>,
    transparent_addresses: HashMap<String, TransparentAddressId>,
}

impl ScanTask {
    pub(crate) fn from_parts(
        scan_range: ScanRange,
        previous_wallet_block: Option<WalletBlock>,
        locators: Vec<Locator>,
        transparent_addresses: HashMap<String, TransparentAddressId>,
    ) -> Self {
        Self {
            scan_range,
            previous_wallet_block,
            locators,
            transparent_addresses,
        }
    }
}
