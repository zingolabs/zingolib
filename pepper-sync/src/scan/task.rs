use std::{
    borrow::BorrowMut,
    collections::{BTreeSet, HashMap},
    sync::{
        Arc,
        atomic::{self, AtomicBool},
    },
};

use futures::FutureExt;
use tokio::{
    sync::mpsc,
    task::{JoinError, JoinHandle},
};

use zcash_client_backend::proto::compact_formats::CompactBlock;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::{transaction::TxId, zip32::AccountId};
use zcash_protocol::consensus::{self, BlockHeight};

use crate::{
    client::{self, FetchRequest},
    config::PerformanceLevel,
    error::{ScanError, ServerError, SyncError},
    keys::transparent::TransparentAddressId,
    sync::{self, ScanPriority, ScanRange},
    wallet::{
        ScanTarget, WalletBlock,
        traits::{SyncBlocks, SyncNullifiers, SyncWallet},
    },
};

use super::{ScanResults, scan};

const MAX_WORKER_POOLSIZE: usize = 2;
const MAX_BATCH_NULLIFIERS: usize = 2usize.pow(14);

pub(crate) enum ScannerState {
    Verification,
    Scan,
    Shutdown,
}

impl ScannerState {
    fn verified(&mut self) {
        *self = ScannerState::Scan;
    }

    fn shutdown(&mut self) {
        *self = ScannerState::Shutdown;
    }
}

pub(crate) struct Scanner<P> {
    pub(crate) state: ScannerState,
    batcher: Option<Batcher<P>>,
    workers: Vec<ScanWorker<P>>,
    unique_id: usize,
    scan_results_sender: mpsc::UnboundedSender<(ScanRange, Result<ScanResults, ScanError>)>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    consensus_parameters: P,
    ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
}

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

    pub(crate) fn launch(&mut self, performance_level: PerformanceLevel) {
        let max_batch_outputs = match performance_level {
            PerformanceLevel::Low => 2usize.pow(11),
            PerformanceLevel::Medium => 2usize.pow(13),
            PerformanceLevel::High => 2usize.pow(13),
            PerformanceLevel::Maximum => 2usize.pow(15),
        };

        self.spawn_batcher(max_batch_outputs);
        self.spawn_workers(max_batch_outputs);
    }

    pub(crate) fn worker_poolsize(&self) -> usize {
        self.workers.len()
    }

    /// Spawns the batcher.
    ///
    /// When the batcher is running it will wait for a scan task.
    pub(crate) fn spawn_batcher(&mut self, max_batch_outputs: usize) {
        tracing::debug!("Spawning batcher");
        let mut batcher = Batcher::new(
            self.consensus_parameters.clone(),
            self.fetch_request_sender.clone(),
        );
        batcher.run(max_batch_outputs);
        self.batcher = Some(batcher);
    }

    fn check_batcher_error(&mut self) -> Result<(), ServerError> {
        let batcher = self.batcher.take();
        if let Some(mut batcher) = batcher {
            batcher.check_error()?;
            self.batcher = Some(batcher);
        }

        Ok(())
    }

    async fn shutdown_batcher(&mut self) -> Result<(), ServerError> {
        let batcher = self.batcher.take();
        if let Some(mut batcher) = batcher {
            batcher.shutdown().await
        } else {
            Ok(())
        }
    }

    /// Spawns a worker.
    ///
    /// When the worker is running it will wait for a scan task.
    pub(crate) fn spawn_worker(&mut self, max_batch_outputs: usize) {
        tracing::debug!("Spawning worker {}", self.unique_id);
        let mut worker = ScanWorker::new(
            self.unique_id,
            self.consensus_parameters.clone(),
            self.scan_results_sender.clone(),
            self.fetch_request_sender.clone(),
            self.ufvks.clone(),
        );
        worker.run(max_batch_outputs);
        self.workers.push(worker);
        self.unique_id += 1;
    }

    /// Spawns the initial pool of workers.
    ///
    /// Poolsize is set by [`self::MAX_WORKER_POOLSIZE`].
    pub(crate) fn spawn_workers(&mut self, max_batch_outputs: usize) {
        for _ in 0..MAX_WORKER_POOLSIZE {
            self.spawn_worker(max_batch_outputs);
        }
    }

    fn idle_worker(&self) -> Option<&ScanWorker<P>> {
        if let Some(idle_worker) = self.workers.iter().find(|worker| !worker.is_scanning()) {
            Some(idle_worker)
        } else {
            None
        }
    }

    /// Shutdown worker by `worker_id`.
    ///
    /// Panics if worker with given `worker_id` is not found.
    async fn shutdown_worker(&mut self, worker_id: usize) {
        let worker_index = self
            .workers
            .iter()
            .position(|worker| worker.id == worker_id)
            .expect("worker should exist");

        let mut worker = self.workers.swap_remove(worker_index);
        worker.shutdown().await.expect("worker task panicked");
    }

    /// Updates the scanner.
    ///
    /// Creates a new scan task and sends to batcher if it's idle.
    /// The batcher will stream compact blocks into the scan task, splitting the scan task when the maximum number of
    /// outputs is reached. When a scan task is ready it is stored in the batcher ready to be taken by an idle scan
    /// worker for scanning.
    /// When verification is still in progress, only scan tasks with `Verify` scan priority are created.
    /// When all ranges are scanned, the batcher, idle workers and mempool are shutdown.
    pub(crate) async fn update<W>(
        &mut self,
        wallet: &mut W,
        shutdown_mempool: Arc<AtomicBool>,
        performance_level: PerformanceLevel,
    ) -> Result<(), SyncError<W::Error>>
    where
        W: SyncWallet + SyncBlocks + SyncNullifiers,
    {
        self.check_batcher_error()?;

        match self.state {
            ScannerState::Verification => {
                self.batcher
                    .as_mut()
                    .expect("batcher should be running")
                    .update_batch_store();
                self.update_workers();

                let sync_state = wallet.get_sync_state().map_err(SyncError::WalletError)?;
                if !sync_state
                    .scan_ranges()
                    .iter()
                    .any(|scan_range| scan_range.priority() == ScanPriority::Verify)
                {
                    if sync_state
                        .scan_ranges()
                        .iter()
                        .any(|scan_range| scan_range.priority() == ScanPriority::Scanning)
                    {
                        // the last scan ranges with `Verify` priority are currently being scanned.
                        return Ok(());
                    }
                    // verification complete
                    self.state.verified();
                    return Ok(());
                }

                // scan ranges with `Verify` priority
                self.update_batcher(wallet, performance_level)
                    .map_err(SyncError::WalletError)?;
            }
            ScannerState::Scan => {
                self.batcher
                    .as_mut()
                    .expect("batcher should be running")
                    .update_batch_store();
                self.update_workers();
                self.update_batcher(wallet, performance_level)
                    .map_err(SyncError::WalletError)?;
            }
            ScannerState::Shutdown => {
                shutdown_mempool.store(true, atomic::Ordering::Release);
                while let Some(worker) = self.idle_worker() {
                    self.shutdown_worker(worker.id).await;
                }
                self.shutdown_batcher().await?;
            }
        }

        Ok(())
    }

    fn update_workers(&mut self) {
        let batcher = self.batcher.as_ref().expect("batcher should be running");
        if batcher.batch.is_some()
            && let Some(worker) = self.idle_worker()
        {
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

    fn update_batcher<W>(
        &mut self,
        wallet: &mut W,
        performance_level: PerformanceLevel,
    ) -> Result<(), W::Error>
    where
        W: SyncWallet + SyncBlocks + SyncNullifiers,
    {
        let batcher = self.batcher.as_ref().expect("batcher should be running");
        if !batcher.is_batching() {
            if let Some(scan_task) = sync::state::create_scan_task(
                &self.consensus_parameters,
                wallet,
                performance_level,
            )? {
                batcher.add_scan_task(scan_task);
            } else if wallet.get_sync_state()?.scan_complete() {
                self.state.shutdown();
            }
        }

        Ok(())
    }
}

struct Batcher<P> {
    handle: Option<JoinHandle<Result<(), ServerError>>>,
    is_batching: Arc<AtomicBool>,
    batch: Option<ScanTask>,
    consensus_parameters: P,
    scan_task_sender: Option<mpsc::Sender<ScanTask>>,
    batch_receiver: Option<mpsc::Receiver<ScanTask>>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
}

impl<P> Batcher<P>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    fn new(
        consensus_parameters: P,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ) -> Self {
        Self {
            handle: None,
            is_batching: Arc::new(AtomicBool::new(false)),
            batch: None,
            consensus_parameters,
            scan_task_sender: None,
            batch_receiver: None,
            fetch_request_sender,
        }
    }

    /// Runs the batcher in a new tokio task.
    ///
    /// Waits for a scan task and then fetches compact blocks to form fixed output batches. The scan task is split if
    /// needed and the compact blocks are added to each scan task and sent to the scan workers for scanning.
    fn run(&mut self, max_batch_outputs: usize) {
        let (scan_task_sender, mut scan_task_receiver) = mpsc::channel::<ScanTask>(1);
        let (batch_sender, batch_receiver) = mpsc::channel::<ScanTask>(1);

        let is_batching = self.is_batching.clone();
        let fetch_request_sender = self.fetch_request_sender.clone();
        let consensus_parameters = self.consensus_parameters.clone();

        let handle: JoinHandle<Result<(), ServerError>> = tokio::spawn(async move {
            // save seam blocks between scan tasks for linear scanning continuuity checks
            // during non-linear scanning the wallet blocks from the scanned ranges will already be saved in the wallet
            let mut previous_task_first_block: Option<WalletBlock> = None;
            let mut previous_task_last_block: Option<WalletBlock> = None;

            while let Some(mut scan_task) = scan_task_receiver.recv().await {
                let fetch_nullifiers_only =
                    scan_task.scan_range.priority() == ScanPriority::ScannedWithoutMapping;

                let mut retry_height = scan_task.scan_range.block_range().start;
                let mut sapling_output_count = 0;
                let mut orchard_output_count = 0;
                let mut sapling_nullifier_count = 0;
                let mut orchard_nullifier_count = 0;
                let mut first_batch = true;

                let mut block_stream = if fetch_nullifiers_only {
                    client::get_nullifier_range(
                        fetch_request_sender.clone(),
                        scan_task.scan_range.block_range().clone(),
                    )
                    .await?
                } else {
                    client::get_compact_block_range(
                        fetch_request_sender.clone(),
                        scan_task.scan_range.block_range().clone(),
                    )
                    .await?
                };
                while let Some(compact_block) = match block_stream.message().await {
                    Ok(b) => b,
                    Err(e) if e.code() == tonic::Code::DeadlineExceeded => {
                        block_stream = if fetch_nullifiers_only {
                            client::get_nullifier_range(
                                fetch_request_sender.clone(),
                                retry_height..scan_task.scan_range.block_range().end,
                            )
                            .await?
                        } else {
                            client::get_compact_block_range(
                                fetch_request_sender.clone(),
                                retry_height..scan_task.scan_range.block_range().end,
                            )
                            .await?
                        };

                        block_stream.message().await?
                    }
                    Err(e) => {
                        return Err(e.into());
                    }
                } {
                    if fetch_nullifiers_only {
                        sapling_nullifier_count += compact_block
                            .vtx
                            .iter()
                            .fold(0, |acc, transaction| acc + transaction.spends.len());
                        orchard_nullifier_count += compact_block
                            .vtx
                            .iter()
                            .fold(0, |acc, transaction| acc + transaction.actions.len());
                    } else {
                        if let Some(block) = previous_task_last_block.as_ref()
                            && scan_task.start_seam_block.is_none()
                            && scan_task.scan_range.block_range().start == block.block_height() + 1
                        {
                            scan_task.start_seam_block = previous_task_last_block.clone();
                        }
                        if let Some(block) = previous_task_first_block.as_ref()
                            && scan_task.end_seam_block.is_none()
                            && scan_task.scan_range.block_range().end == block.block_height()
                        {
                            scan_task.end_seam_block = previous_task_first_block.clone();
                        }
                        if first_batch {
                            previous_task_first_block = Some(
                                WalletBlock::from_compact_block(
                                    &consensus_parameters,
                                    fetch_request_sender.clone(),
                                    &compact_block,
                                )
                                .await?,
                            );
                            first_batch = false;
                        }
                        if compact_block.height() == scan_task.scan_range.block_range().end - 1 {
                            previous_task_last_block = Some(
                                WalletBlock::from_compact_block(
                                    &consensus_parameters,
                                    fetch_request_sender.clone(),
                                    &compact_block,
                                )
                                .await?,
                            );
                        }

                        sapling_output_count += compact_block
                            .vtx
                            .iter()
                            .fold(0, |acc, transaction| acc + transaction.outputs.len());
                        orchard_output_count += compact_block
                            .vtx
                            .iter()
                            .fold(0, |acc, transaction| acc + transaction.actions.len());
                    }

                    if sapling_output_count + orchard_output_count > max_batch_outputs
                        || sapling_nullifier_count + orchard_nullifier_count > MAX_BATCH_NULLIFIERS
                    {
                        let (full_batch, new_batch) = scan_task
                            .clone()
                            .split(
                                &consensus_parameters,
                                fetch_request_sender.clone(),
                                compact_block.height(),
                            )
                            .await?;

                        let _ignore_error = batch_sender.send(full_batch).await;

                        scan_task = new_batch;
                        sapling_output_count = 0;
                        orchard_output_count = 0;
                        sapling_nullifier_count = 0;
                        orchard_nullifier_count = 0;
                    }

                    retry_height = compact_block.height() + 1;
                    scan_task.compact_blocks.push(compact_block);
                }

                let _ignore_error = batch_sender.send(scan_task).await;

                is_batching.store(false, atomic::Ordering::Release);
            }
            Ok(())
        });

        self.handle = Some(handle);
        self.scan_task_sender = Some(scan_task_sender);
        self.batch_receiver = Some(batch_receiver);
    }

    fn is_batching(&self) -> bool {
        self.is_batching.load(atomic::Ordering::Acquire)
    }

    fn add_scan_task(&self, scan_task: ScanTask) {
        tracing::trace!("Adding scan task to batcher:\n{:#?}", &scan_task);
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

    fn check_error(&mut self) -> Result<(), ServerError> {
        if let Some(mut handle) = self.handle.take() {
            if let Some(result) = handle.borrow_mut().now_or_never() {
                result.expect("task panicked")?;
            } else {
                self.handle = Some(handle);
            }
        }

        Ok(())
    }

    /// Shuts down batcher by dropping the sender to the batcher task and awaiting the handle.
    ///
    /// This should always be called in the context of the scanner as it must be also be taken from the Scanner struct.
    async fn shutdown(&mut self) -> Result<(), ServerError> {
        tracing::debug!("Shutting down batcher");
        if let Some(sender) = self.scan_task_sender.take() {
            drop(sender);
        }
        if let Some(receiver) = self.batch_receiver.take() {
            drop(receiver);
        }
        let handle = self
            .handle
            .take()
            .expect("batcher should always have a handle to take!");

        handle.await.expect("task panicked")
    }
}

pub(crate) struct ScanWorker<P> {
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
    fn run(&mut self, max_batch_outputs: usize) {
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
                    max_batch_outputs,
                )
                .await;

                let _ignore_error = scan_results_sender.send((scan_range, scan_results));

                is_scanning.store(false, atomic::Ordering::Release);
            }
        });

        self.handle = Some(handle);
        self.scan_task_sender = Some(scan_task_sender);
    }

    fn is_scanning(&self) -> bool {
        self.is_scanning.load(atomic::Ordering::Acquire)
    }

    fn add_scan_task(&self, scan_task: ScanTask) {
        tracing::trace!("Adding scan task to worker {}:\n{:#?}", self.id, &scan_task);
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
    pub(crate) scan_targets: BTreeSet<ScanTarget>,
    pub(crate) transparent_addresses: HashMap<String, TransparentAddressId>,
    pub(crate) map_nullifiers: bool,
}

impl ScanTask {
    pub(crate) fn from_parts(
        scan_range: ScanRange,
        start_seam_block: Option<WalletBlock>,
        end_seam_block: Option<WalletBlock>,
        scan_targets: BTreeSet<ScanTarget>,
        transparent_addresses: HashMap<String, TransparentAddressId>,
        map_nullifiers: bool,
    ) -> Self {
        Self {
            compact_blocks: Vec::new(),
            scan_range,
            start_seam_block,
            end_seam_block,
            scan_targets,
            transparent_addresses,
            map_nullifiers,
        }
    }

    /// Splits a scan task into two at `block_height`.
    ///
    /// Panics if `block_height` is not contained in the scan task's block range.
    async fn split(
        self,
        consensus_parameters: &impl consensus::Parameters,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        block_height: BlockHeight,
    ) -> Result<(Self, Self), ServerError> {
        if block_height < self.scan_range.block_range().start
            && block_height > self.scan_range.block_range().end - 1
        {
            panic!("block height should be within scan tasks block range!");
        }

        let mut lower_compact_blocks = self.compact_blocks;
        let upper_compact_blocks = if let Some(index) = lower_compact_blocks
            .iter()
            .position(|block| block.height() == block_height)
        {
            lower_compact_blocks.split_off(index)
        } else {
            Vec::new()
        };

        let mut lower_task_scan_targets = self.scan_targets;
        let upper_task_scan_targets = lower_task_scan_targets.split_off(&ScanTarget {
            block_height,
            txid: TxId::from_bytes([0; 32]),
            narrow_scan_area: false,
        });

        let lower_task_last_block = if let Some(block) = lower_compact_blocks.last() {
            Some(
                WalletBlock::from_compact_block(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    block,
                )
                .await?,
            )
        } else {
            None
        };
        let upper_task_first_block = if let Some(block) = upper_compact_blocks.first() {
            Some(
                WalletBlock::from_compact_block(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    block,
                )
                .await?,
            )
        } else {
            None
        };

        Ok((
            ScanTask {
                compact_blocks: lower_compact_blocks,
                scan_range: self
                    .scan_range
                    .truncate_end(block_height)
                    .expect("block height should be within block range"),
                start_seam_block: self.start_seam_block,
                end_seam_block: upper_task_first_block,
                scan_targets: lower_task_scan_targets,
                transparent_addresses: self.transparent_addresses.clone(),
                map_nullifiers: self.map_nullifiers,
            },
            ScanTask {
                compact_blocks: upper_compact_blocks,
                scan_range: self
                    .scan_range
                    .truncate_start(block_height)
                    .expect("block height should be within block range"),
                start_seam_block: lower_task_last_block,
                end_seam_block: self.end_seam_block,
                scan_targets: upper_task_scan_targets,
                transparent_addresses: self.transparent_addresses,
                map_nullifiers: self.map_nullifiers,
            },
        ))
    }
}
