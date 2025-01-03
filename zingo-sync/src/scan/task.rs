use std::{
    collections::{BTreeSet, HashMap},
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
};

use tokio::{
    sync::mpsc::{self, error::TryRecvError},
    task::{JoinError, JoinHandle},
};

use zcash_client_backend::{
    data_api::scanning::{ScanPriority, ScanRange},
    proto::compact_formats::CompactBlock,
};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::{
    consensus::{self},
    zip32::AccountId,
};

use crate::{
    client::{self, FetchRequest},
    keys::transparent::TransparentAddressId,
    primitives::{Locator, WalletBlock},
    sync,
    traits::{SyncBlocks, SyncWallet},
};

use super::{error::ScanError, scan, ScanResults};

const MAX_WORKER_POOLSIZE: usize = 2;
const MAX_BATCH_OUTPUTS: usize = 16384; // 2^14

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
    batcher: Option<Batcher>,
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
            batcher: None,
            workers,
            unique_id: 0,
            scan_results_sender,
            fetch_request_sender,
            consensus_parameters,
            ufvks,
        }
    }

    pub(crate) fn launch(&mut self) {
        self.spawn_batcher();
        self.spawn_workers();
    }

    pub(crate) fn worker_poolsize(&self) -> usize {
        self.workers.len()
    }

    /// Spawns the batcher.
    ///
    /// When the batcher is running it will wait for a scan task.
    pub(crate) fn spawn_batcher(&mut self) {
        tracing::debug!("Spawning batcher");
        let mut batcher = Batcher::new(self.fetch_request_sender.clone());
        batcher.run().unwrap();
        self.batcher = Some(batcher);
    }

    async fn shutdown_batcher(&mut self) -> Result<(), JoinError> {
        let mut batcher = self.batcher.take().expect("batcher should exist!");
        batcher.shutdown().await?;

        Ok(())
    }

    /// Spawns a worker.
    ///
    /// When the worker is running it will wait for a scan task.
    pub(crate) fn spawn_worker(&mut self) {
        tracing::debug!("Spawning worker {}", self.unique_id);
        let mut worker = ScanWorker::new(
            self.unique_id,
            self.consensus_parameters.clone(),
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
    /// If verification is still in progress, only create scan tasks with `Verify` scan priority.
    /// If there are no batches ready and the batcher is idle,
    /// If there are no more range available to scan, shutdown the batcher, idle workers and mempool.
    pub(crate) async fn update<W>(&mut self, wallet: &mut W, shutdown_mempool: Arc<AtomicBool>)
    where
        W: SyncWallet + SyncBlocks,
    {
        match self.state {
            ScannerState::Verification => {
                self.batcher
                    .as_mut()
                    .expect("batcher should be running")
                    .update_batch_store();
                self.update_workers();

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
                self.update_batcher(wallet);
            }
            ScannerState::Scan => {
                // create scan tasks for batching and scanning until all ranges are scanned
                self.batcher
                    .as_mut()
                    .expect("batcher should be running")
                    .update_batch_store();
                self.update_workers();
                self.update_batcher(wallet);
            }
            ScannerState::Shutdown => {
                // shutdown mempool
                shutdown_mempool.store(true, atomic::Ordering::Release);

                // shutdown idle workers
                while let Some(worker) = self.idle_worker() {
                    self.shutdown_worker(worker.id).await.unwrap();
                }

                // shutdown batcher
                self.shutdown_batcher()
                    .await
                    .expect("batcher should not fail!");
            }
        }

        if !wallet.get_sync_state().unwrap().scan_complete() && self.worker_poolsize() == 0 {
            panic!("worker pool should not be empty with unscanned ranges!")
        }
    }

    fn update_workers(&mut self) {
        let batcher = self.batcher.as_ref().expect("batcher should be running");
        if batcher.batch.is_some() {
            if let Some(worker) = self.idle_worker() {
                let batch = batcher
                    .batch
                    .clone()
                    .expect("batch should exist in this closure");
                worker.add_scan_task(batch);
                self.batcher
                    .as_mut()
                    .expect("batcher should be running")
                    .batch = None;
            }
        }
    }

    fn update_batcher<W>(&mut self, wallet: &mut W)
    where
        W: SyncWallet + SyncBlocks,
    {
        let batcher = self.batcher.as_ref().expect("batcher should be running");
        if !batcher.is_batching() {
            if let Some(scan_task) = sync::state::create_scan_task(wallet).unwrap() {
                batcher.add_scan_task(scan_task);
            } else if wallet.get_sync_state().unwrap().scan_complete() {
                self.state.scan_completed();
            }
        }
    }
}

struct Batcher {
    handle: Option<JoinHandle<()>>,
    is_batching: Arc<AtomicBool>,
    batch: Option<ScanTask>,
    scan_task_sender: Option<mpsc::Sender<ScanTask>>,
    batch_receiver: Option<mpsc::Receiver<ScanTask>>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
}

impl Batcher {
    fn new(fetch_request_sender: mpsc::UnboundedSender<FetchRequest>) -> Self {
        Self {
            handle: None,
            is_batching: Arc::new(AtomicBool::new(false)),
            batch: None,
            scan_task_sender: None,
            batch_receiver: None,
            fetch_request_sender,
        }
    }

    /// Runs the batcher in a new tokio task.
    ///
    /// Waits for a scan task and then fetches compact blocks to form fixed output batches. The scan task is split if
    /// needed and the compact blocks are added to each scan task and sent to the scan workers for scanning.
    fn run(&mut self) -> Result<(), ()> {
        let (scan_task_sender, mut scan_task_receiver) = mpsc::channel::<ScanTask>(1);
        let (batch_sender, batch_receiver) = mpsc::channel::<ScanTask>(1);

        let is_batching = self.is_batching.clone();
        let fetch_request_sender = self.fetch_request_sender.clone();

        let handle = tokio::spawn(async move {
            while let Some(mut scan_task) = scan_task_receiver.recv().await {
                let mut block_stream = client::get_compact_block_range(
                    fetch_request_sender.clone(),
                    scan_task.scan_range.block_range().clone(),
                )
                .await
                .unwrap();
                while let Some(compact_block) = block_stream.message().await.unwrap() {
                    scan_task.compact_blocks.push(compact_block);
                }

                batch_sender
                    .send(scan_task)
                    .await
                    .expect("receiver should never be dropped before sender!");

                is_batching.store(false, atomic::Ordering::Release);
            }
        });

        self.handle = Some(handle);
        self.scan_task_sender = Some(scan_task_sender);
        self.batch_receiver = Some(batch_receiver);

        Ok(())
    }

    fn is_batching(&self) -> bool {
        self.is_batching.load(atomic::Ordering::Acquire)
    }

    fn add_scan_task(&self, scan_task: ScanTask) {
        tracing::debug!("Adding scan task to batcher:\n{:#?}", &scan_task);
        self.scan_task_sender
            .clone()
            .expect("batcher should be running")
            .try_send(scan_task)
            .expect("batcher should never be sent multiple tasks at one time");
        self.is_batching.store(true, atomic::Ordering::Release);
    }

    fn update_batch_store(&mut self) {
        let batch_receiver = self
            .batch_receiver
            .as_mut()
            .expect("batcher should be running");
        if self.batch.is_none() && !batch_receiver.is_empty() {
            self.batch = Some(
                batch_receiver
                    .try_recv()
                    .expect("channel should be non-empty!"),
            );
        }
    }

    /// Shuts down batcher by dropping the sender to the batcher task and awaiting the handle.
    ///
    /// This should always be called in the context of the scanner as it must be also be taken from the Scanner struct.
    async fn shutdown(&mut self) -> Result<(), JoinError> {
        tracing::debug!("Shutting down batcher");
        if let Some(sender) = self.scan_task_sender.take() {
            drop(sender);
        }
        let handle = self
            .handle
            .take()
            .expect("batcher should always have a handle to take!");

        handle.await
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
        scan_results_sender: mpsc::UnboundedSender<(ScanRange, Result<ScanResults, ScanError>)>,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
    ) -> Self {
        Self {
            id,
            handle: None,
            is_scanning: Arc::new(AtomicBool::new(false)),
            consensus_parameters,
            scan_task_sender: None,
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
                let scan_range = scan_task.scan_range.clone();
                let scan_results = scan(
                    fetch_request_sender.clone(),
                    &consensus_parameters,
                    &ufvks,
                    scan_task,
                )
                .await;

                scan_results_sender
                    .send((scan_range, scan_results))
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

    fn add_scan_task(&self, scan_task: ScanTask) {
        tracing::debug!("Adding scan task to worker {}:\n{:#?}", self.id, &scan_task);
        self.scan_task_sender
            .clone()
            .expect("worker should be running")
            .try_send(scan_task)
            .expect("worker should never be sent multiple tasks at one time");
        self.is_scanning.store(true, atomic::Ordering::Release);
    }

    /// Shuts down worker by dropping the sender to the worker task and awaiting the handle.
    ///
    /// This should always be called in the context of the scanner as it must be also be removed from the worker pool.
    async fn shutdown(&mut self) -> Result<(), JoinError> {
        tracing::debug!("Shutting down worker {}", self.id);
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

#[derive(Debug, Clone)]
pub(crate) struct ScanTask {
    pub(crate) compact_blocks: Vec<CompactBlock>,
    pub(crate) scan_range: ScanRange,
    pub(crate) start_seam_block: Option<WalletBlock>,
    pub(crate) end_seam_block: Option<WalletBlock>,
    pub(crate) locators: BTreeSet<Locator>,
    pub(crate) transparent_addresses: HashMap<String, TransparentAddressId>,
}

impl ScanTask {
    pub(crate) fn from_parts(
        scan_range: ScanRange,
        start_seam_block: Option<WalletBlock>,
        end_seam_block: Option<WalletBlock>,
        locators: BTreeSet<Locator>,
        transparent_addresses: HashMap<String, TransparentAddressId>,
    ) -> Self {
        Self {
            compact_blocks: Vec::new(),
            scan_range,
            start_seam_block,
            end_seam_block,
            locators,
            transparent_addresses,
        }
    }
}
