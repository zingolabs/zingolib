use std::{
    borrow::BorrowMut,
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Range,
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
        traits::{SyncBlocks, SyncNullifiers, SyncTransactions, SyncWallet},
    },
};

use super::{ScanResults, scan};

const MAX_WORKER_POOLSIZE: usize = 2;
const MAX_LOAD_NULLIFIERS: usize = 2usize.pow(14);

use zingo_netutils::time::{SCANNER_SHUTDOWN_TIMEOUT, STREAM_MSG_TIMEOUT};

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

    /// Forgets every task whose selected range no longer overlaps a wallet range held at its in-flight priority.
    pub(crate) fn retire_finished_tasks(&mut self, scan_ranges: &[ScanRange]) {
        self.in_flight_tasks.retain(|_, task| {
            sync::held_at(scan_ranges, task.priority().in_flight())
                .any(|held| held.overlaps(task.block_range()))
        });
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

    pub(crate) fn idle_worker(&self) -> Option<&ScanWorker<P>> {
        if let Some(idle_worker) = self.workers.iter().find(|worker| !worker.is_scanning()) {
            Some(idle_worker)
        } else {
            None
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
        W: SyncWallet + SyncBlocks + SyncNullifiers + SyncTransactions,
    {
        let loader = self.loader.as_ref().expect("loader should be running");
        if !loader.is_loading() {
            if let Some(scan_task) = sync::state::create_scan_task(
                &self.consensus_parameters,
                wallet,
                nullifier_map_limit_exceeded,
                self.transparent_gap_addresses.clone(),
                self.next_task_id,
            )? {
                self.in_flight_tasks
                    .insert(scan_task.task_id, scan_task.scan_range.clone());
                self.next_task_id = self.next_task_id.next();
                loader.add_scan_task(scan_task);
            } else if wallet.get_sync_state()?.scan_complete() {
                // if sync is complete, all nullifiers will have been re-fetched so this note metadata can be discarded.
                for transaction in wallet.get_wallet_transactions_mut()?.values_mut() {
                    for note in transaction.sapling_notes.as_mut_slice() {
                        note.refetch_nullifier_ranges = Vec::new();
                    }
                    for note in transaction.orchard_notes.as_mut_slice() {
                        note.refetch_nullifier_ranges = Vec::new();
                    }
                    for note in transaction.ironwood_notes.as_mut_slice() {
                        note.refetch_nullifier_ranges = Vec::new();
                    }
                }
                self.state.completed();
            }
        }

        Ok(())
    }

    pub(crate) fn is_verified(&self) -> bool {
        !matches!(self.state, ScannerState::Verification)
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
    handle: Option<JoinHandle<()>>,
    is_scanning: Arc<AtomicBool>,
    consensus_parameters: P,
    scan_task_sender: Option<mpsc::Sender<ScanTask>>,
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
            handle: None,
            is_scanning: Arc::new(AtomicBool::new(false)),
            consensus_parameters,
            scan_task_sender: None,
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

        let is_scanning = self.is_scanning.clone();
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
    /// This should always be called in the context of the scanner as it must be also be removed from the worker pool
    /// (See `Scanner::shutdown_worker`).
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
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use zcash_protocol::consensus::{MAIN_NETWORK, MainNetwork};
    use zcash_transparent::keys::NonHardenedChildIndex;

    use crate::keys::transparent::TransparentScope;

    use super::*;

    const GAP_LIMIT: u32 = 3;

    /// An address map as held by the scanner, for the given external indexes.
    fn external_addresses(
        indexes: impl IntoIterator<Item = u32>,
    ) -> HashMap<String, TransparentAddressId> {
        indexes
            .into_iter()
            .map(|index| {
                (
                    format!("external address {index}"),
                    TransparentAddressId::new(
                        AccountId::ZERO,
                        TransparentScope::External,
                        NonHardenedChildIndex::from_index(index).unwrap(),
                    ),
                )
            })
            .collect()
    }

    fn scanner_with_gap_addresses(
        gap_addresses: HashMap<String, TransparentAddressId>,
    ) -> Scanner<MainNetwork> {
        let (scan_results_sender, _) = mpsc::unbounded_channel();
        let (fetch_request_sender, _) = mpsc::unbounded_channel();
        let mut scanner = Scanner::new(
            MAIN_NETWORK,
            scan_results_sender,
            fetch_request_sender,
            HashMap::new(),
            GAP_LIMIT,
        );
        scanner.transparent_gap_addresses = gap_addresses;
        scanner
    }

    /// Scan results without compact block transparent data, such as re-fetched nullifiers, carry no changes to the
    /// gap addresses. The scanner keeps the gap addresses it holds.
    #[test]
    fn gap_addresses_are_kept_when_scan_results_carry_no_changes() {
        let mut scanner = scanner_with_gap_addresses(external_addresses(1..=3));

        scanner.update_transparent_gap_addresses(&HashMap::new(), HashMap::new());

        assert_eq!(scanner.transparent_gap_addresses, external_addresses(1..=3));
    }

    /// The gap addresses found in use are removed and the gap addresses derived to replace them are added. The gap
    /// addresses above the highest address found in use are kept.
    #[test]
    fn gap_addresses_found_in_use_are_replaced() {
        let mut scanner = scanner_with_gap_addresses(external_addresses(1..=3));

        scanner.update_transparent_gap_addresses(
            &external_addresses(1..=2),
            external_addresses(4..=5),
        );

        assert_eq!(scanner.transparent_gap_addresses, external_addresses(3..=5));
    }

    const SCAN_RANGE_END: u32 = 20;

    fn block_range() -> Range<BlockHeight> {
        BlockHeight::from_u32(10)..BlockHeight::from_u32(SCAN_RANGE_END + 1)
    }

    fn no_block_error() -> ServerError {
        ServerError::RequestFailed(tonic::Status::not_found("no block"))
    }

    /// Answers chain height requests with `chain_height`.
    fn spawn_chain_height_fetcher(chain_height: u32) -> mpsc::UnboundedSender<FetchRequest> {
        let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(fetch_request) = fetch_request_receiver.recv().await {
                match fetch_request {
                    FetchRequest::ChainTip(reply_sender) => {
                        let _ignore_error =
                            reply_sender.send(Ok(zingo_netutils::lightwallet_protocol::BlockId {
                                height: u64::from(chain_height),
                                hash: Vec::new(),
                            }));
                    }
                    _ => panic!("unexpected fetch request"),
                }
            }
        });

        fetch_request_sender
    }

    /// The server's chain height is below the last block of the scan range, so it has no block to serve for the top
    /// of the scan range.
    #[tokio::test]
    async fn fetch_failure_under_a_lowered_chain_height_reports_the_chain_height() {
        let error = fetch_failure(
            spawn_chain_height_fetcher(SCAN_RANGE_END - 1),
            &block_range(),
            no_block_error(),
        )
        .await;

        assert!(matches!(
            error,
            ServerError::ChainHeightBelowScanRange { chain_height, scan_range_end }
                if chain_height == BlockHeight::from_u32(SCAN_RANGE_END - 1)
                    && scan_range_end == BlockHeight::from_u32(SCAN_RANGE_END)
        ));
    }

    /// The server's chain holds the whole scan range, so the fetch failed for another reason.
    #[tokio::test]
    async fn fetch_failure_with_the_scan_range_on_chain_returns_the_fetch_error() {
        for chain_height in [SCAN_RANGE_END, SCAN_RANGE_END + 1] {
            let error = fetch_failure(
                spawn_chain_height_fetcher(chain_height),
                &block_range(),
                no_block_error(),
            )
            .await;

            assert!(
                matches!(error, ServerError::RequestFailed(_)),
                "chain height {chain_height}"
            );
        }
    }

    /// The chain height request fails with the fetcher gone, leaving the fetch error as the only known cause.
    #[tokio::test]
    async fn fetch_failure_with_no_chain_height_returns_the_fetch_error() {
        let (fetch_request_sender, _) = mpsc::unbounded_channel();

        let error = fetch_failure(fetch_request_sender, &block_range(), no_block_error()).await;

        assert!(matches!(error, ServerError::RequestFailed(_)));
    }

    /// A scan task of any priority is split above its first block when a load budget is reached.
    #[test]
    fn scan_tasks_are_split_above_their_first_block() {
        const START: u32 = 10;
        let block_range = BlockHeight::from_u32(START)..BlockHeight::from_u32(START * 2);

        for (priority, block_height, splittable) in [
            (ScanPriority::ChainTip, START, false),
            (ScanPriority::ChainTip, START + 1, true),
            (ScanPriority::ScannedWithoutMapping, START + 1, true),
            (ScanPriority::Verify, START, false),
            (ScanPriority::Verify, START + 1, true),
        ] {
            assert_eq!(
                splittable_at(
                    &ScanRange::from_parts(block_range.clone(), priority),
                    BlockHeight::from_u32(block_height)
                ),
                splittable,
                "{priority:?} at {block_height}"
            );
        }
    }

    #[test]
    fn transparent_data_is_only_requested_for_ranges_above_transparent_scan_floor() {
        let floor = BlockHeight::from_u32(1_000);
        for (block_range, include_transparent) in [
            // below the floor
            (floor - 10..floor - 5, false),
            // ends at the floor
            (floor - 5..floor + 1, false),
            // spans the floor
            (floor - 5..floor + 5, true),
            // starts above the floor
            (floor + 1..floor + 5, true),
        ] {
            assert_eq!(
                includes_blocks_above_floor(&block_range, floor),
                include_transparent,
                "{block_range:?}"
            );
        }
    }

    /// A load budget splits a scan task of any priority, since the re-org handler resets the wallet range that
    /// encloses the failed load instead of the load's exact range.
    #[test]
    fn a_verify_task_splits_at_a_load_budget_like_any_other() {
        let verify = ScanRange::from_parts(
            BlockHeight::from_u32(21)..BlockHeight::from_u32(41),
            ScanPriority::Verify,
        );

        assert!(splittable_at(&verify, BlockHeight::from_u32(31)));
        assert!(!splittable_at(&verify, BlockHeight::from_u32(21)));
    }
}
