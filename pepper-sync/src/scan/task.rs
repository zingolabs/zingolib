use std::{
    borrow::BorrowMut,
    collections::{BTreeSet, HashMap},
    sync::{
        Arc,
        atomic::{self, AtomicBool},
    },
    time::Duration,
};

use futures::FutureExt;
use tokio::{
    sync::mpsc,
    task::{JoinError, JoinHandle},
};

use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::transaction::TxId;
use zcash_protocol::ShieldedPool;
use zcash_protocol::consensus::{self, BlockHeight};
use zingo_netutils::lightwallet_protocol::CompactBlock;
use zip32::AccountId;

use crate::{
    client::{self, FetchRequest},
    config::PerformanceLevel,
    error::{ScanError, ServerError, SyncError},
    keys::transparent::TransparentAddressId,
    sync::{self, ScanPriority, ScanRange},
    utils::block,
    wallet::{
        ScanTarget, WalletBlock,
        traits::{SyncBlocks, SyncNullifiers, SyncWallet},
    },
};

use super::{ScanResults, scan};

const MAX_WORKER_POOLSIZE: usize = 2;
const MAX_LOAD_NULLIFIERS: usize = 2usize.pow(14);

use zingo_netutils::time::{SCANNER_SHUTDOWN_TIMEOUT, STREAM_MSG_TIMEOUT};

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
    loader: Option<Loader<P>>,
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
            loader: None,
            workers,
            unique_id: 0,
            scan_results_sender,
            fetch_request_sender,
            consensus_parameters,
            ufvks,
        }
    }

    pub(crate) fn launch(&mut self, performance_level: PerformanceLevel) {
        let max_outputs = match performance_level {
            PerformanceLevel::Low => 2usize.pow(11),
            PerformanceLevel::Medium => 2usize.pow(13),
            PerformanceLevel::High => 2usize.pow(13),
            PerformanceLevel::Maximum => 2usize.pow(15),
        };

        self.spawn_loader(max_outputs);
        self.spawn_workers(max_outputs);
    }

    pub(crate) fn worker_poolsize(&self) -> usize {
        self.workers.len()
    }

    /// Spawns the loader.
    ///
    /// When the loader is running it will wait for a scan task.
    pub(crate) fn spawn_loader(&mut self, max_load_outputs: usize) {
        tracing::debug!("Spawning loader");
        let mut loader = Loader::new(
            self.consensus_parameters.clone(),
            self.fetch_request_sender.clone(),
        );
        loader.run(max_load_outputs);
        self.loader = Some(loader);
    }

    fn check_loader_error(&mut self) -> Result<(), ServerError> {
        let loader = self.loader.take();
        if let Some(mut loader) = loader {
            loader.check_error()?;
            self.loader = Some(loader);
        }

        Ok(())
    }

    async fn shutdown_loader(&mut self) -> Result<(), ServerError> {
        let loader = self.loader.take();
        if let Some(mut loader) = loader {
            loader.shutdown().await
        } else {
            Ok(())
        }
    }

    /// Spawns a worker.
    ///
    /// When the worker is running it will wait for a scan task.
    pub(crate) fn spawn_worker(&mut self, max_outputs: usize) {
        tracing::debug!("Spawning worker {}", self.unique_id);
        let mut worker = ScanWorker::new(
            self.unique_id,
            self.consensus_parameters.clone(),
            self.scan_results_sender.clone(),
            self.fetch_request_sender.clone(),
            self.ufvks.clone(),
        );
        worker.run(max_outputs);
        self.workers.push(worker);
        self.unique_id += 1;
    }

    /// Spawns the initial pool of workers.
    ///
    /// Poolsize is set by [`self::MAX_WORKER_POOLSIZE`].
    pub(crate) fn spawn_workers(&mut self, max_outputs: usize) {
        for _ in 0..MAX_WORKER_POOLSIZE {
            self.spawn_worker(max_outputs);
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
    /// Creates a new scan task and sends to loader if it's idle.
    /// The loader will stream compact blocks into the scan task, splitting the scan task when the maximum number of
    /// outputs is reached. When a scan task is ready it is stored in the loader ready to be taken by an idle scan
    /// worker for scanning.
    /// When verification is still in progress, only scan tasks with `Verify` scan priority are created.
    /// When all ranges are scanned, the loader, idle workers and mempool are shutdown.
    pub(crate) async fn update<W>(
        &mut self,
        wallet: &mut W,
        shutdown_mempool: Arc<AtomicBool>,
        nullifier_map_limit_exceeded: bool,
    ) -> Result<(), SyncError<W::Error>>
    where
        W: SyncWallet + SyncBlocks + SyncNullifiers,
    {
        self.check_loader_error()?;

        match self.state {
            ScannerState::Verification => {
                self.loader
                    .as_mut()
                    .expect("loader should be running")
                    .update_load_store();
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
                self.update_loader(wallet, nullifier_map_limit_exceeded)
                    .map_err(SyncError::WalletError)?;
            }
            ScannerState::Scan => {
                self.loader
                    .as_mut()
                    .expect("loader should be running")
                    .update_load_store();
                self.update_workers();
                self.update_loader(wallet, nullifier_map_limit_exceeded)
                    .map_err(SyncError::WalletError)?;
            }
            ScannerState::Shutdown => {
                shutdown_mempool.store(true, atomic::Ordering::Release);
                while let Some(worker) = self.idle_worker() {
                    self.shutdown_worker(worker.id).await;
                }
                self.shutdown_loader().await?;
            }
        }

        Ok(())
    }

    fn update_workers(&mut self) {
        let loader = self.loader.as_ref().expect("loader should be running");
        if loader.load.is_some()
            && let Some(worker) = self.idle_worker()
        {
            let load = loader
                .load
                .clone()
                .expect("load should exist in this closure");
            worker.add_scan_task(load);
            self.loader.as_mut().expect("loader should be running").load = None;
        }
    }

    fn update_loader<W>(
        &mut self,
        wallet: &mut W,
        nullifier_map_limit_exceeded: bool,
    ) -> Result<(), W::Error>
    where
        W: SyncWallet + SyncBlocks + SyncNullifiers,
    {
        let loader = self.loader.as_ref().expect("loader should be running");
        if !loader.is_loading() {
            if let Some(scan_task) = sync::state::create_scan_task(
                &self.consensus_parameters,
                wallet,
                nullifier_map_limit_exceeded,
            )? {
                loader.add_scan_task(scan_task);
            } else if wallet.get_sync_state()?.scan_complete() {
                self.state.shutdown();
            }
        }

        Ok(())
    }
}

struct Loader<P> {
    handle: Option<JoinHandle<Result<(), ServerError>>>,
    is_loading: Arc<AtomicBool>,
    load: Option<ScanTask>,
    consensus_parameters: P,
    scan_task_sender: Option<mpsc::Sender<ScanTask>>,
    load_receiver: Option<mpsc::Receiver<ScanTask>>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
}

impl<P> Loader<P>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    fn new(
        consensus_parameters: P,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ) -> Self {
        Self {
            handle: None,
            is_loading: Arc::new(AtomicBool::new(false)),
            load: None,
            consensus_parameters,
            scan_task_sender: None,
            load_receiver: None,
            fetch_request_sender,
        }
    }

    /// Runs the loader in a new tokio task.
    ///
    /// Waits for a scan task and then fetches compact blocks to form loads within a fixed output budget. The scan
    /// task is split if needed and the compact blocks are added to each scan task and sent to the scan workers for
    /// scanning.
    fn run(&mut self, max_load_outputs: usize) {
        let (scan_task_sender, mut scan_task_receiver) = mpsc::channel::<ScanTask>(1);
        let (load_sender, load_receiver) = mpsc::channel::<ScanTask>(1);

        let is_loading = self.is_loading.clone();
        let fetch_request_sender = self.fetch_request_sender.clone();
        let consensus_parameters = self.consensus_parameters.clone();

        let handle: JoinHandle<Result<(), ServerError>> = tokio::spawn(async move {
            // save seam blocks between scan tasks for linear scanning continuity checks
            // during non-linear scanning the wallet blocks from the scanned ranges will already be saved in the wallet
            let mut previous_task_first_block: Option<WalletBlock> = None;
            let mut previous_task_last_block: Option<WalletBlock> = None;

            while let Some(mut scan_task) = scan_task_receiver.recv().await {
                let fetch_nullifiers_only =
                    scan_task.scan_range.priority() == ScanPriority::ScannedWithoutMapping;

                let mut retry_height = scan_task.scan_range.block_range().start;
                let mut load_sapling_output_count = 0;
                let mut load_orchard_output_count = 0;
                let mut load_ironwood_output_count = 0;
                let mut load_sapling_nullifier_count = 0;
                let mut load_orchard_nullifier_count = 0;
                let mut load_ironwood_nullifier_count = 0;
                let mut current_block_sapling_output_count = 0;
                let mut current_block_orchard_output_count = 0;
                let mut current_block_ironwood_output_count = 0;
                let mut current_block_sapling_nullifier_count = 0;
                let mut current_block_orchard_nullifier_count = 0;
                let mut current_block_ironwood_nullifier_count = 0;
                let mut awaiting_first_block = true;

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

                loop {
                    let msg_res: Result<Option<CompactBlock>, tonic::Status> =
                        match tokio::time::timeout(STREAM_MSG_TIMEOUT, block_stream.message()).await
                        {
                            Ok(res) => res,
                            Err(_) => {
                                Err(tonic::Status::deadline_exceeded("stream message timeout"))
                            }
                        };

                    let maybe_block = match msg_res {
                        Ok(b) => b,
                        Err(e)
                            if e.code() == tonic::Code::DeadlineExceeded
                                || e.message().contains("Unexpected EOF decoding stream.") =>
                        {
                            tokio::time::sleep(Duration::from_secs(3)).await;

                            let retry_range = retry_height..scan_task.scan_range.block_range().end;

                            block_stream = if fetch_nullifiers_only {
                                client::get_nullifier_range(
                                    fetch_request_sender.clone(),
                                    retry_range,
                                )
                                .await?
                            } else {
                                client::get_compact_block_range(
                                    fetch_request_sender.clone(),
                                    retry_range,
                                )
                                .await?
                            };

                            let first_msg_res: Result<Option<CompactBlock>, tonic::Status> =
                                match tokio::time::timeout(
                                    STREAM_MSG_TIMEOUT,
                                    block_stream.message(),
                                )
                                .await
                                {
                                    Ok(res) => res,
                                    Err(_) => Err(tonic::Status::deadline_exceeded(
                                        "stream message timeout after retry",
                                    )),
                                };

                            match first_msg_res {
                                Ok(b) => b,
                                Err(e) => return Err(e.into()),
                            }
                        }
                        Err(e) => {
                            return Err(e.into());
                        }
                    };

                    let Some(compact_block) = maybe_block else {
                        break;
                    };

                    if fetch_nullifiers_only {
                        current_block_sapling_nullifier_count =
                            block::shielded_input_count(&compact_block, ShieldedPool::Sapling)
                                as usize;
                        load_sapling_nullifier_count += current_block_sapling_nullifier_count;
                        current_block_orchard_nullifier_count =
                            block::shielded_input_count(&compact_block, ShieldedPool::Orchard)
                                as usize;
                        load_orchard_nullifier_count += current_block_orchard_nullifier_count;
                        current_block_ironwood_nullifier_count =
                            block::shielded_input_count(&compact_block, ShieldedPool::Ironwood)
                                as usize;
                        load_ironwood_nullifier_count += current_block_ironwood_nullifier_count;
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
                        if awaiting_first_block {
                            previous_task_first_block = Some(
                                WalletBlock::from_compact_block(
                                    &consensus_parameters,
                                    fetch_request_sender.clone(),
                                    &compact_block,
                                )
                                .await?,
                            );
                            awaiting_first_block = false;
                        }
                        if block::get_compact_height(&compact_block)
                            == scan_task.scan_range.block_range().end - 1
                        {
                            previous_task_last_block = Some(
                                WalletBlock::from_compact_block(
                                    &consensus_parameters,
                                    fetch_request_sender.clone(),
                                    &compact_block,
                                )
                                .await?,
                            );
                        }

                        current_block_sapling_output_count =
                            block::shielded_output_count(&compact_block, ShieldedPool::Sapling)
                                as usize;
                        load_sapling_output_count += current_block_sapling_output_count;
                        current_block_orchard_output_count =
                            block::shielded_output_count(&compact_block, ShieldedPool::Orchard)
                                as usize;
                        load_orchard_output_count += current_block_orchard_output_count;
                        current_block_ironwood_output_count =
                            block::shielded_output_count(&compact_block, ShieldedPool::Ironwood)
                                as usize;
                        load_ironwood_output_count += current_block_ironwood_output_count;
                    }

                    if (load_sapling_output_count
                        + load_orchard_output_count
                        + load_ironwood_output_count
                        > max_load_outputs
                        || load_sapling_nullifier_count
                            + load_orchard_nullifier_count
                            + load_ironwood_nullifier_count
                            > MAX_LOAD_NULLIFIERS)
                        && scan_task.scan_range.block_range().start
                            != block::get_compact_height(&compact_block)
                    {
                        let (full_load, new_load) = scan_task
                            .clone()
                            .split(
                                &consensus_parameters,
                                fetch_request_sender.clone(),
                                block::get_compact_height(&compact_block),
                            )
                            .await?;

                        let _ignore_error = load_sender.send(full_load).await;

                        scan_task = new_load;
                        load_sapling_output_count = current_block_sapling_output_count;
                        load_orchard_output_count = current_block_orchard_output_count;
                        load_ironwood_output_count = current_block_ironwood_output_count;
                        load_sapling_nullifier_count = current_block_sapling_nullifier_count;
                        load_orchard_nullifier_count = current_block_orchard_nullifier_count;
                        load_ironwood_nullifier_count = current_block_ironwood_nullifier_count;
                    }

                    retry_height = block::get_compact_height(&compact_block) + 1;
                    scan_task.compact_blocks.push(compact_block);
                }

                let _ignore_error = load_sender.send(scan_task).await;

                is_loading.store(false, atomic::Ordering::Release);
            }

            is_loading.store(false, atomic::Ordering::Release);

            Ok(())
        });

        self.handle = Some(handle);
        self.scan_task_sender = Some(scan_task_sender);
        self.load_receiver = Some(load_receiver);
    }

    fn is_loading(&self) -> bool {
        self.is_loading.load(atomic::Ordering::Acquire)
    }

    fn add_scan_task(&self, scan_task: ScanTask) {
        tracing::trace!("Adding scan task to loader:\n{:#?}", &scan_task);
        self.scan_task_sender
            .clone()
            .expect("loader should be running")
            .try_send(scan_task)
            .expect("loader should never be sent multiple tasks at one time");
        self.is_loading.store(true, atomic::Ordering::Release);
    }

    fn update_load_store(&mut self) {
        let load_receiver = self
            .load_receiver
            .as_mut()
            .expect("loader should be running");
        if self.load.is_none() && !load_receiver.is_empty() {
            self.load = Some(
                load_receiver
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

    /// Shuts down loader by dropping the sender to the loader task and awaiting the handle.
    ///
    /// This should always be called in the context of the scanner as it must be also be taken from the Scanner struct.
    async fn shutdown(&mut self) -> Result<(), ServerError> {
        tracing::debug!("Shutting down loader");
        if let Some(sender) = self.scan_task_sender.take() {
            drop(sender);
        }
        if let Some(receiver) = self.load_receiver.take() {
            drop(receiver);
        }

        let mut handle = self
            .handle
            .take()
            .expect("loader should always have a handle to take!");

        match tokio::time::timeout(SCANNER_SHUTDOWN_TIMEOUT, &mut handle).await {
            Ok(join_res) => join_res.expect("task panicked")?,
            Err(_) => {
                handle.abort();
                let _ = handle.await;
                return Err(tonic::Status::deadline_exceeded("loader shutdown timeout").into());
            }
        }

        Ok(())
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
    fn run(&mut self, max_outputs: usize) {
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
                    max_outputs,
                )
                .await;
                let _ignore_error = scan_results_sender.send((scan_range, scan_results));

                is_scanning.store(false, atomic::Ordering::Release);
            }

            is_scanning.store(false, atomic::Ordering::Release);
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

        let mut handle = self
            .handle
            .take()
            .expect("worker should always have a handle to take!");

        match tokio::time::timeout(SCANNER_SHUTDOWN_TIMEOUT, &mut handle).await {
            Ok(res) => res,
            Err(_) => {
                handle.abort();
                let _ = handle.await; // ignore join error after abort
                Ok(())
            }
        }
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
}

impl ScanTask {
    pub(crate) fn from_parts(
        scan_range: ScanRange,
        start_seam_block: Option<WalletBlock>,
        end_seam_block: Option<WalletBlock>,
        scan_targets: BTreeSet<ScanTarget>,
        transparent_addresses: HashMap<String, TransparentAddressId>,
    ) -> Self {
        Self {
            compact_blocks: Vec::new(),
            scan_range,
            start_seam_block,
            end_seam_block,
            scan_targets,
            transparent_addresses,
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
            || block_height > self.scan_range.block_range().end - 1
        {
            panic!("block height should be within scan tasks block range!");
        }

        let mut lower_compact_blocks = self.compact_blocks;
        let upper_compact_blocks = if let Some(index) = lower_compact_blocks
            .iter()
            .position(|block| block::get_compact_height(block) == block_height)
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
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // REPRO: `ScanWorker::run` never observes the `JoinHandle` of its spawned task and only clears
    // `is_scanning` at the end of the loop body. If `scan` panics (reachable from server data via the
    // `panic!("compact blocks do not match scan range!")` check in `scan.rs`), the worker stays
    // "scanning" forever, no result or error is ever reported, and `Scanner::idle_worker` never returns
    // it. Intended invariant: a panicked worker must be reported (error on the result channel or
    // through the handle) or must go idle.
    #[tokio::test]
    async fn panicked_scan_worker_is_reported_or_idle() {
        let (scan_results_sender, mut scan_results_receiver) = mpsc::unbounded_channel();
        let (fetch_request_sender, _fetch_request_receiver) =
            mpsc::unbounded_channel::<FetchRequest>();

        let mut worker = ScanWorker::new(
            0,
            consensus::MainNetwork,
            scan_results_sender,
            fetch_request_sender,
            HashMap::new(),
        );
        worker.run(1024);

        // Scan range 10..20 with no compact blocks: `scan` panics before producing any result.
        let scan_task = ScanTask::from_parts(
            ScanRange::from_parts(
                BlockHeight::from_u32(10)..BlockHeight::from_u32(20),
                ScanPriority::Historic,
            ),
            None,
            None,
            BTreeSet::new(),
            HashMap::new(),
        );
        worker.add_scan_task(scan_task);

        tokio::time::sleep(Duration::from_millis(500)).await;

        let handle = worker.handle.as_ref().expect("worker should be running");
        assert!(
            handle.is_finished(),
            "worker task should have panicked and finished"
        );

        let reported = matches!(scan_results_receiver.try_recv(), Ok((_, Err(_))));

        assert!(
            reported || !worker.is_scanning(),
            "a panicked scan worker must be reported or marked idle, but is_scanning={} and no result was sent",
            worker.is_scanning()
        );
    }
}
