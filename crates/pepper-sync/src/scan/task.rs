use std::{
    borrow::BorrowMut,
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Range,
    time::Duration,
};

use futures::FutureExt;
use tokio::{
    sync::mpsc,
    task::{JoinError, JoinHandle},
};

use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_protocol::ShieldedPool;
use zcash_protocol::consensus::{self, BlockHeight};
use zingolib_netutils::lightwallet_protocol::CompactBlock;
use zip32::AccountId;

use crate::engine::{SyncStateOps, WalletTransactionOps};
use crate::{
    client::{self, FetchRequest},
    config::PerformanceLevel,
    error::{ScanError, ServerError, SyncError},
    keys::transparent::TransparentAddressId,
    sync::{self, ScanPriority, ScanRange},
    utils::block,
    wallet::{
        ScanTarget, WalletBlock,
        traits::{SyncBlocks, SyncNullifiers, SyncTransactions, SyncWallet},
    },
};

use super::{ScanResults, scan};

const MAX_WORKER_POOLSIZE: usize = 2;
const MAX_LOAD_NULLIFIERS: usize = 2usize.pow(14);

use zingolib_netutils::time::{SCANNER_SHUTDOWN_TIMEOUT, STREAM_MSG_TIMEOUT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanTaskRunnerActivity {
    Idle,
    Busy,
    Ended,
}

impl ScanTaskRunnerActivity {
    fn of(arrival_awaited: bool, scan_task_runner_ended: bool) -> Self {
        match (scan_task_runner_ended, arrival_awaited) {
            (true, _) => Self::Ended,
            (false, false) => Self::Idle,
            (false, true) => Self::Busy,
        }
    }
}

type LoadKey = (TaskId, BlockHeight);

fn awaited_after<K: PartialEq>(awaited: Option<K>, arrived: &K) -> Option<K> {
    awaited.filter(|key| key != arrived)
}

struct ScanTaskRunner<T> {
    handle: Option<JoinHandle<T>>,
    awaited: Option<LoadKey>,
    scan_task_sender: Option<mpsc::Sender<ScanTask>>,
}

impl<T> ScanTaskRunner<T> {
    fn new() -> Self {
        Self {
            handle: None,
            awaited: None,
            scan_task_sender: None,
        }
    }

    fn started(&mut self, handle: JoinHandle<T>, scan_task_sender: mpsc::Sender<ScanTask>) {
        self.handle = Some(handle);
        self.scan_task_sender = Some(scan_task_sender);
    }

    fn activity(&self) -> ScanTaskRunnerActivity {
        ScanTaskRunnerActivity::of(
            self.awaited.is_some(),
            self.handle.as_ref().is_none_or(JoinHandle::is_finished),
        )
    }

    fn send(&mut self, scan_task: ScanTask, awaited: LoadKey) {
        self.awaited = Some(awaited);
        self.scan_task_sender
            .as_ref()
            .expect("run stores the sender before the scanner sends a scan task, and only shutdown takes it")
            .try_send(scan_task)
            .expect("the scanner sends a scan task only to an idle scan task runner, which holds the receiver of an empty channel");
    }

    fn arrived(&mut self, arrived: &LoadKey) {
        self.awaited = awaited_after(self.awaited, arrived);
    }
}

pub(crate) enum ScannerState {
    Verification,
    Scan,
    Complete,
}

impl ScannerState {
    pub(crate) fn verified(&mut self) {
        *self = ScannerState::Scan;
    }

    fn completed(&mut self) {
        *self = ScannerState::Complete;
    }

    pub(crate) fn reverify(&mut self) {
        *self = ScannerState::Verification;
    }
}

pub(crate) struct Scanner<P> {
    pub(crate) state: ScannerState,
    loader: Option<Loader<P>>,
    pub(crate) workers: Vec<ScanWorker<P>>,
    unique_id: usize,
    next_task_id: TaskId,
    in_flight_tasks: BTreeMap<TaskId, ScanRange>,
    load_order: LoadOrder,
    scan_results_sender: mpsc::UnboundedSender<(ScanLoad, Result<ScanResults, ScanError>)>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    consensus_parameters: P,
    ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
    transparent_gap_limit: u32,
    pub(crate) transparent_gap_addresses: HashMap<String, TransparentAddressId>,
}

impl<P> Scanner<P>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    pub(crate) fn new(
        consensus_parameters: P,
        scan_results_sender: mpsc::UnboundedSender<(ScanLoad, Result<ScanResults, ScanError>)>,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
        transparent_gap_limit: u32,
    ) -> Self {
        let workers: Vec<ScanWorker<P>> = Vec::with_capacity(MAX_WORKER_POOLSIZE);

        Self {
            state: ScannerState::Verification,
            loader: None,
            workers,
            unique_id: 0,
            next_task_id: TaskId::first(),
            in_flight_tasks: BTreeMap::new(),
            load_order: LoadOrder::default(),
            scan_results_sender,
            fetch_request_sender,
            consensus_parameters,
            ufvks,
            transparent_gap_limit,
            transparent_gap_addresses: HashMap::new(),
        }
    }

    /// Returns the selected range of every task in flight, keyed by task in selection order.
    pub(crate) fn in_flight_tasks(&self) -> &BTreeMap<TaskId, ScanRange> {
        &self.in_flight_tasks
    }

    /// Forgets every task whose selected range no longer overlaps a wallet range held at its in-flight priority,
    /// with any results of its loads held back for ordering.
    pub(crate) fn retire_finished_tasks(&mut self, scan_ranges: &[ScanRange]) {
        self.in_flight_tasks.retain(|_, task| {
            sync::held_at(scan_ranges, task.priority().in_flight())
                .any(|held| held.overlaps(task.block_range()))
        });
        self.load_order.retain(&self.in_flight_tasks);
    }

    /// - Holds the results of a load above an unprocessed lower load of its task back until that load arrives.
    pub(crate) fn admit_scan_results(
        &mut self,
        load: ScanLoad,
        scan_results: Result<ScanResults, ScanError>,
    ) -> Vec<(ScanLoad, Result<ScanResults, ScanError>)> {
        let arrived = (load.task_id, load.scan_range.block_range().start);
        for worker in &mut self.workers {
            worker.results_arrived(&arrived);
        }
        self.load_order.admit(load, scan_results)
    }

    /// Applies the changes to the transparent gap addresses found by a scan.
    ///
    /// The gap addresses found in use are removed and the gap addresses derived to replace them are added.
    pub(crate) fn update_transparent_gap_addresses(
        &mut self,
        new_inuse_addresses: &HashMap<String, TransparentAddressId>,
        new_gap_addresses: HashMap<String, TransparentAddressId>,
    ) {
        self.transparent_gap_addresses
            .retain(|address, _| !new_inuse_addresses.contains_key(address));
        self.transparent_gap_addresses.extend(new_gap_addresses);
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

    pub(crate) async fn shutdown_loader(&mut self) -> Result<(), ServerError> {
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
            self.transparent_gap_limit,
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

    /// Shutdown worker by `worker_id`.
    ///
    /// Panics if worker with given `worker_id` is not found.
    pub(crate) async fn shutdown_worker(&mut self, worker_id: usize) {
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
        nullifier_map_limit_exceeded: bool,
    ) -> Result<(), SyncError<W::Error>>
    where
        W: SyncWallet + SyncBlocks + SyncNullifiers + SyncTransactions,
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
            ScannerState::Complete => {}
        }

        Ok(())
    }

    fn update_workers(&mut self) {
        let scanning_workers = self
            .workers
            .iter()
            .filter(|worker| matches!(worker.activity(), ScanTaskRunnerActivity::Busy))
            .count();
        if !load_admitted(scanning_workers, self.load_order.held()) {
            return;
        }
        let Some(worker) = self
            .workers
            .iter_mut()
            .find(|worker| matches!(worker.activity(), ScanTaskRunnerActivity::Idle))
        else {
            return;
        };
        let loader = self.loader.as_mut().expect(
            "the scanner launches its loader before its first update and takes it only at shutdown",
        );
        if let Some(load) = loader.load.take() {
            worker.add_scan_task(load);
        }
    }

    fn update_loader<W>(
        &mut self,
        wallet: &mut W,
        nullifier_map_limit_exceeded: bool,
    ) -> Result<(), W::Error>
    where
        W: SyncWallet + SyncBlocks + SyncNullifiers + SyncTransactions,
    {
        let loader = self.loader.as_mut().expect(
            "the scanner launches its loader before its first update and takes it only at shutdown",
        );
        match loader.activity() {
            ScanTaskRunnerActivity::Idle => {
                if let Some(scan_task) = sync::state::create_scan_task(
                    &self.consensus_parameters,
                    wallet,
                    nullifier_map_limit_exceeded,
                    self.transparent_gap_addresses.clone(),
                    self.next_task_id,
                )? {
                    self.in_flight_tasks
                        .insert(scan_task.task_id, scan_task.scan_range.clone());
                    self.load_order
                        .track(scan_task.task_id, scan_task.scan_range.block_range().start);
                    self.next_task_id = self.next_task_id.next();
                    loader.add_scan_task(scan_task);
                } else if wallet.get_sync_state()?.scan_complete() {
                    // if sync is complete, all nullifiers will have been re-fetched so this note metadata can be discarded.
                    for transaction in wallet.get_wallet_transactions_mut()?.values_mut() {
                        transaction.set_refetch_nullifier_ranges(&[]);
                    }
                    self.state.completed();
                }
            }
            ScanTaskRunnerActivity::Busy | ScanTaskRunnerActivity::Ended => (),
        }

        Ok(())
    }

    pub(crate) fn is_verified(&self) -> bool {
        !matches!(self.state, ScannerState::Verification)
    }
}

struct Loader<P> {
    runner: ScanTaskRunner<Result<(), ServerError>>,
    load: Option<ScanTask>,
    consensus_parameters: P,
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
            runner: ScanTaskRunner::new(),
            load: None,
            consensus_parameters,
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

                let mut block_stream = match open_block_stream(
                    fetch_request_sender.clone(),
                    scan_task.scan_range.block_range().clone(),
                    fetch_nullifiers_only,
                    scan_task.transparent_scan_floor,
                )
                .await
                {
                    Ok(block_stream) => block_stream,
                    Err(e) => {
                        return Err(fetch_failure(
                            fetch_request_sender.clone(),
                            scan_task.scan_range.block_range(),
                            e,
                        )
                        .await);
                    }
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

                            block_stream = match open_block_stream(
                                fetch_request_sender.clone(),
                                retry_height..scan_task.scan_range.block_range().end,
                                fetch_nullifiers_only,
                                scan_task.transparent_scan_floor,
                            )
                            .await
                            {
                                Ok(block_stream) => block_stream,
                                Err(e) => {
                                    return Err(fetch_failure(
                                        fetch_request_sender.clone(),
                                        scan_task.scan_range.block_range(),
                                        e,
                                    )
                                    .await);
                                }
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
                                Err(e) => {
                                    return Err(fetch_failure(
                                        fetch_request_sender.clone(),
                                        scan_task.scan_range.block_range(),
                                        e.into(),
                                    )
                                    .await);
                                }
                            }
                        }
                        Err(e) => {
                            return Err(fetch_failure(
                                fetch_request_sender.clone(),
                                scan_task.scan_range.block_range(),
                                e.into(),
                            )
                            .await);
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
                                crate::wallet::wallet_block_from_compact_block(
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
                                crate::wallet::wallet_block_from_compact_block(
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
                        && splittable_at(
                            &scan_task.scan_range,
                            block::get_compact_height(&compact_block),
                        )
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
            }

            Ok(())
        });

        self.runner.started(handle, scan_task_sender);
        self.load_receiver = Some(load_receiver);
    }

    fn activity(&self) -> ScanTaskRunnerActivity {
        self.runner.activity()
    }

    fn add_scan_task(&mut self, scan_task: ScanTask) {
        tracing::trace!("Adding scan task to loader:\n{:#?}", &scan_task);
        let awaited_final_load = (scan_task.task_id, scan_task.scan_range.block_range().end);
        self.runner.send(scan_task, awaited_final_load);
    }

    fn update_load_store(&mut self) {
        let load_receiver = self
            .load_receiver
            .as_mut()
            .expect("loader should be running");
        if self.load.is_none()
            && let Ok(load) = load_receiver.try_recv()
        {
            let arrived = (load.task_id, load.scan_range.block_range().end);
            self.runner.arrived(&arrived);
            self.load = Some(load);
        }
    }

    fn check_error(&mut self) -> Result<(), ServerError> {
        if let Some(mut handle) = self.runner.handle.take() {
            if let Some(result) = handle.borrow_mut().now_or_never() {
                result.expect("task panicked")?;
            } else {
                self.runner.handle = Some(handle);
            }
        }

        Ok(())
    }

    /// Shuts down loader by dropping the sender to the loader task and awaiting the handle.
    ///
    /// This should always be called in the context of the scanner as it must be also be taken from the Scanner struct.
    async fn shutdown(&mut self) -> Result<(), ServerError> {
        tracing::debug!("Shutting down loader");
        if let Some(sender) = self.runner.scan_task_sender.take() {
            drop(sender);
        }
        if let Some(receiver) = self.load_receiver.take() {
            drop(receiver);
        }

        let mut handle = self
            .runner
            .handle
            .take()
            .expect("loader should always have a handle to take!");

        match tokio::time::timeout(SCANNER_SHUTDOWN_TIMEOUT, &mut handle).await {
            Ok(res) => res.expect("task panicked")?,
            Err(_) => {
                tracing::warn!("Loader shutdown timed out!");
                handle.abort();
                let _ = handle.await;
            }
        }

        Ok(())
    }
}

/// Returns the error the loader ends with after fetching the scan range of `block_range` (end exclusive) failed with
/// `error`.
///
/// A re-org can lower the server's chain height below a scan range after it was selected, and a server behind the
/// chain tip may hold only part of it. The server then has no block to serve for the top of the scan range. The
/// chain height is fetched to tell this apart from any other failure. If it is below the last block of the scan
/// range, [`ServerError::ChainHeightBelowScanRange`] is returned, which recommends syncing again to verify the
/// wallet against the server's chain. Otherwise `error` is returned.
async fn fetch_failure(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    block_range: &Range<BlockHeight>,
    error: ServerError,
) -> ServerError {
    let scan_range_end = block_range.end - 1;
    match client::get_chain_height(fetch_request_sender).await {
        Ok(chain_height) if chain_height < scan_range_end => {
            ServerError::ChainHeightBelowScanRange {
                chain_height,
                scan_range_end,
            }
        }
        _ => error,
    }
}

/// Returns true when a load budget reached at `block_height` may split the scan task of `scan_range`, which holds above its first block.
fn splittable_at(scan_range: &ScanRange, block_height: BlockHeight) -> bool {
    scan_range.block_range().start != block_height
}

/// Opens a stream of compact blocks for `block_range` (end exclusive), or of nullifiers only if `fetch_nullifiers_only`
/// is true.
///
/// Compact block transparent data is only fetched if `block_range` contains blocks above the `transparent_scan_floor`.
async fn open_block_stream(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    block_range: Range<BlockHeight>,
    fetch_nullifiers_only: bool,
    transparent_scan_floor: BlockHeight,
) -> Result<tonic::Streaming<CompactBlock>, ServerError> {
    if fetch_nullifiers_only {
        client::get_nullifier_range(fetch_request_sender, block_range).await
    } else {
        let include_transparent = includes_blocks_above_floor(&block_range, transparent_scan_floor);
        client::get_compact_block_range(fetch_request_sender, block_range, include_transparent)
            .await
    }
}

/// Returns true if `block_range` (end exclusive) contains blocks above the `transparent_scan_floor`, so their
/// compact block transparent data must be fetched. Transparent data is not scanned at or below the floor, so fetching
/// it for ranges at or below the floor wastes bandwidth.
///
/// Scan ranges do not span the floor: the floor is set to the chain height at the start of the sync session, and newly
/// mined blocks form new scan ranges above it. If a re-org lowers the floor, it is lowered to one below the start of the
/// verification range. If a range did span the floor, transparent data is fetched for the whole range so the transparent
/// data of the blocks above the floor is not missed.
fn includes_blocks_above_floor(
    block_range: &Range<BlockHeight>,
    transparent_scan_floor: BlockHeight,
) -> bool {
    block_range.end > transparent_scan_floor + 1
}

pub(crate) struct ScanWorker<P> {
    id: usize,
    runner: ScanTaskRunner<()>,
    consensus_parameters: P,
    scan_results_sender: mpsc::UnboundedSender<(ScanLoad, Result<ScanResults, ScanError>)>,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
    transparent_gap_limit: u32,
}

impl<P> ScanWorker<P>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    fn new(
        id: usize,
        consensus_parameters: P,
        scan_results_sender: mpsc::UnboundedSender<(ScanLoad, Result<ScanResults, ScanError>)>,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        ufvks: HashMap<AccountId, UnifiedFullViewingKey>,
        transparent_gap_limit: u32,
    ) -> Self {
        Self {
            id,
            runner: ScanTaskRunner::new(),
            consensus_parameters,
            scan_results_sender,
            fetch_request_sender,
            ufvks,
            transparent_gap_limit,
        }
    }

    pub(crate) fn id(&self) -> usize {
        self.id
    }

    /// Runs the worker in a new tokio task.
    ///
    /// Waits for a scan task and then calls [`crate::scan::scan`] on the given range.
    // TODO: max_outputs can be moved to scan worker field
    fn run(&mut self, max_outputs: usize) {
        let (scan_task_sender, mut scan_task_receiver) = mpsc::channel::<ScanTask>(1);

        let scan_results_sender = self.scan_results_sender.clone();
        let fetch_request_sender = self.fetch_request_sender.clone();
        let consensus_parameters = self.consensus_parameters.clone();
        let ufvks = self.ufvks.clone();
        let transparent_gap_limit = self.transparent_gap_limit;

        let handle = tokio::spawn(async move {
            while let Some(scan_task) = scan_task_receiver.recv().await {
                let load = ScanLoad {
                    task_id: scan_task.task_id,
                    scan_range: scan_task.scan_range.clone(),
                    chain_height: scan_task.chain_height,
                    highest_scanned_height: scan_task.highest_scanned_height,
                };
                let scan_results = scan(
                    fetch_request_sender.clone(),
                    &consensus_parameters,
                    &ufvks,
                    scan_task,
                    max_outputs,
                    transparent_gap_limit,
                )
                .await;
                let _ignore_error = scan_results_sender.send((load, scan_results));
            }
        });

        self.runner.started(handle, scan_task_sender);
    }

    fn activity(&self) -> ScanTaskRunnerActivity {
        self.runner.activity()
    }

    fn results_arrived(&mut self, arrived: &LoadKey) {
        self.runner.arrived(arrived);
    }

    fn add_scan_task(&mut self, scan_task: ScanTask) {
        tracing::trace!("Adding scan task to worker {}:\n{:#?}", self.id, &scan_task);
        let awaited_results = (scan_task.task_id, scan_task.scan_range.block_range().start);
        self.runner.send(scan_task, awaited_results);
    }

    /// Shuts down worker by dropping the sender to the worker task and awaiting the handle.
    ///
    /// This should always be called in the context of the scanner as it must be also be removed from the worker pool
    /// (See `Scanner::shutdown_worker`).
    async fn shutdown(&mut self) -> Result<(), JoinError> {
        tracing::debug!("Shutting down worker {}", self.id);
        if let Some(sender) = self.runner.scan_task_sender.take() {
            drop(sender);
        }

        let mut handle = self
            .runner
            .handle
            .take()
            .expect("worker should always have a handle to take!");

        match tokio::time::timeout(SCANNER_SHUTDOWN_TIMEOUT, &mut handle).await {
            Ok(res) => res,
            Err(_) => {
                tracing::warn!("Worker shutdown timed out!");
                handle.abort();
                let _ = handle.await; // ignore join error after abort
                Ok(())
            }
        }
    }
}

/// The identity of one scan task, in selection order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TaskId(u64);

impl TaskId {
    /// Returns the identity of the first task a scanner selects.
    pub(crate) fn first() -> Self {
        TaskId(0)
    }

    /// Returns the identity of the task selected after this one.
    pub(crate) fn next(self) -> Self {
        TaskId(self.0 + 1)
    }
}

/// One load of a scan task, named by its task and holding the load's blocks at the task's selected priority.
#[derive(Debug, Clone)]
pub(crate) struct ScanLoad {
    pub(crate) task_id: TaskId,
    pub(crate) scan_range: ScanRange,
    // the heights the task placed the load's checkpoints from, for the shard tree update to look there
    pub(crate) chain_height: BlockHeight,
    pub(crate) highest_scanned_height: BlockHeight,
}

// a held result occupies the place of the compact blocks its lower load's worker still holds, so the loads in
// memory never exceed the worker pool.
fn load_admitted(scanning_workers: usize, held_results: usize) -> bool {
    scanning_workers + held_results < MAX_WORKER_POOLSIZE
}

// the loads of a task are built in block height order and their results return in completion order. a task's
// results are held back until every lower load of the task has been released, so a note found in a lower load is
// known to the wallet before the spends of any higher load are located.
#[derive(Debug, Default)]
struct LoadOrder {
    // the start of the load each tracked task releases next.
    next_load_start: BTreeMap<TaskId, BlockHeight>,
    // results of loads above their task's next load, keyed by task and load start.
    held: BTreeMap<(TaskId, BlockHeight), (ScanLoad, Result<ScanResults, ScanError>)>,
}

impl LoadOrder {
    /// - Tracks `task_id` from `first_load_start`.
    fn track(&mut self, task_id: TaskId, first_load_start: BlockHeight) {
        self.next_load_start.insert(task_id, first_load_start);
    }

    /// - Holds `scan_results` back when `load` is above its task's next load.
    /// - Otherwise releases it and every held load that then follows it, advancing the task's next load past each.
    ///
    /// The results of an untracked task are released as they are.
    fn admit(
        &mut self,
        load: ScanLoad,
        scan_results: Result<ScanResults, ScanError>,
    ) -> Vec<(ScanLoad, Result<ScanResults, ScanError>)> {
        let task_id = load.task_id;
        if !self.next_load_start.contains_key(&task_id) {
            return vec![(load, scan_results)];
        }
        self.held.insert(
            (task_id, load.scan_range.block_range().start),
            (load, scan_results),
        );

        let mut released = Vec::new();
        while let Some(next_load_start) = self.next_load_start.get_mut(&task_id)
            && let Some((load, scan_results)) = self.held.remove(&(task_id, *next_load_start))
        {
            *next_load_start = load.scan_range.block_range().end;
            released.push((load, scan_results));
        }

        released
    }

    fn held(&self) -> usize {
        self.held.len()
    }

    /// - Drops the tracking and held results of every task absent from `in_flight_tasks`.
    fn retain(&mut self, in_flight_tasks: &BTreeMap<TaskId, ScanRange>) {
        self.next_load_start
            .retain(|task_id, _| in_flight_tasks.contains_key(task_id));
        self.held
            .retain(|(task_id, _), _| in_flight_tasks.contains_key(task_id));
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ScanTask {
    pub(crate) task_id: TaskId,
    pub(crate) compact_blocks: Vec<CompactBlock>,
    pub(crate) scan_range: ScanRange,
    pub(crate) start_seam_block: Option<WalletBlock>,
    pub(crate) end_seam_block: Option<WalletBlock>,
    pub(crate) scan_targets: BTreeSet<ScanTarget>,
    pub(crate) transparent_inuse_addresses: HashMap<String, TransparentAddressId>,
    pub(crate) transparent_gap_addresses: HashMap<String, TransparentAddressId>,
    pub(crate) transparent_scan_floor: BlockHeight,
    // the session's chain height when the task was selected, which the rolling checkpoint window hangs from,
    // and the wallet's highest scanned block then, which places the anchor boundaries of each load with the
    // load's last block. the shard tree update looks where the scan placed the checkpoints.
    pub(crate) chain_height: BlockHeight,
    pub(crate) highest_scanned_height: BlockHeight,
}

impl ScanTask {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        task_id: TaskId,
        scan_range: ScanRange,
        start_seam_block: Option<WalletBlock>,
        end_seam_block: Option<WalletBlock>,
        scan_targets: BTreeSet<ScanTarget>,
        transparent_inuse_addresses: HashMap<String, TransparentAddressId>,
        transparent_gap_addresses: HashMap<String, TransparentAddressId>,
        transparent_scan_floor: BlockHeight,
        chain_height: BlockHeight,
        highest_scanned_height: BlockHeight,
    ) -> Self {
        Self {
            task_id,
            compact_blocks: Vec::new(),
            scan_range,
            start_seam_block,
            end_seam_block,
            scan_targets,
            transparent_inuse_addresses,
            transparent_gap_addresses,
            transparent_scan_floor,
            chain_height,
            highest_scanned_height,
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
        let upper_task_scan_targets =
            lower_task_scan_targets.split_off(&ScanTarget::new(block_height, None));

        let lower_task_last_block = if let Some(block) = lower_compact_blocks.last() {
            Some(
                crate::wallet::wallet_block_from_compact_block(
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
                crate::wallet::wallet_block_from_compact_block(
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
                task_id: self.task_id,
                compact_blocks: lower_compact_blocks,
                scan_range: self
                    .scan_range
                    .truncate_end(block_height)
                    .expect("block height should be within block range"),
                start_seam_block: self.start_seam_block,
                end_seam_block: upper_task_first_block,
                scan_targets: lower_task_scan_targets,
                transparent_inuse_addresses: self.transparent_inuse_addresses.clone(),
                transparent_gap_addresses: self.transparent_gap_addresses.clone(),
                transparent_scan_floor: self.transparent_scan_floor,
                chain_height: self.chain_height,
                highest_scanned_height: self.highest_scanned_height,
            },
            ScanTask {
                task_id: self.task_id,
                compact_blocks: upper_compact_blocks,
                scan_range: self
                    .scan_range
                    .truncate_start(block_height)
                    .expect("block height should be within block range"),
                start_seam_block: lower_task_last_block,
                end_seam_block: self.end_seam_block,
                scan_targets: upper_task_scan_targets,
                transparent_inuse_addresses: self.transparent_inuse_addresses,
                transparent_gap_addresses: self.transparent_gap_addresses,
                transparent_scan_floor: self.transparent_scan_floor,
                chain_height: self.chain_height,
                highest_scanned_height: self.highest_scanned_height,
            },
        ))
    }
}

#[cfg(test)]
mod tests;
