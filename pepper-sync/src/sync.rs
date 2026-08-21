//! Entrypoint for sync engine

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicBool, AtomicU8, AtomicU32};
use std::time::{Duration, SystemTime};

use shardtree::ShardTree;
use shardtree::store::memory::MemoryShardStore;
use tokio::sync::{RwLock, mpsc, watch};

use incrementalmerkletree::{Marking, Retention};
use orchard::tree::MerkleHashOrchard;
use shardtree::store::ShardStore;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::transaction::{Transaction, TxId};
use zcash_protocol::consensus::{self, BlockHeight};
use zcash_protocol::{PoolType, ShieldedPool};
use zingo_netutils::lightwallet_protocol::RawTransaction;
use zingo_netutils::{Indexer, TransparentIndexer};
use zip32::AccountId;

use zingo_status::confirmation_status::ConfirmationStatus;

use crate::client::{self, FetchRequest};
use crate::config::{PerformanceLevel, SyncConfig};
use crate::error::{
    ContinuityError, MempoolError, ScanError, ServerError, SyncError, SyncModeError,
    SyncStatusError,
};
use crate::keys::transparent::TransparentAddressId;
use crate::scan::ScanResults;
use crate::scan::task::{Scanner, ScannerState};
use crate::scan::transactions::scan_transaction;
use crate::shardtree_ext::{RollbackOutcome, ShardTreeExt};
use crate::sync::state::truncate_scan_ranges;
use crate::wallet::traits::{
    SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions, SyncWallet,
};
use crate::wallet::{
    KeyIdInterface, NoteInterface, NullifierMap, OutputId, OutputInterface, PoolActivation,
    ScanTarget, SyncMode, SyncState, WalletBlock, WalletTransaction,
};
use crate::witness::LocatedTreeData;

use crate::witness;

pub(crate) mod spend;
pub(crate) mod state;
pub(crate) mod transparent;
pub mod truncate;

/// The deepest chain reorganization the wallet tolerates, and the
/// repository's single source of truth for that depth. It mirrors the
/// validator's finalization boundary, zebra's
/// `zebra_state::MAX_BLOCK_REORG_HEIGHT` (100), below which blocks are
/// final and no deeper reorg can occur. Zebra crates are not
/// dependencies of this workspace, so the value is pinned to its
/// upstream by documentation rather than import. If zebra ever moves
/// its boundary, this constant is the one place that follows it.
pub const MAX_REORG_ALLOWANCE: u32 = 100;

const VERIFY_BLOCK_RANGE_SIZE: u32 = 10;

/// A snapshot of the current state of sync. Useful for displaying the status of sync to a user / consumer.
///
/// `percentage_outputs_scanned` is a much more accurate indicator of sync completion than `percentage_blocks_scanned`.
/// `percentage_total_outputs_scanned` is the percentage of outputs scanned from birthday to chain height.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct SyncStatus {
    pub scan_ranges: Vec<ScanRange>,
    pub sync_start_height: BlockHeight,
    pub session_blocks_scanned: u32,
    pub total_blocks_scanned: u32,
    pub percentage_session_blocks_scanned: f32,
    pub percentage_total_blocks_scanned: f32,
    pub session_sapling_outputs_scanned: u32,
    pub total_sapling_outputs_scanned: u32,
    pub session_orchard_outputs_scanned: u32,
    pub total_orchard_outputs_scanned: u32,
    pub session_ironwood_outputs_scanned: u32,
    pub total_ironwood_outputs_scanned: u32,
    pub percentage_session_outputs_scanned: f32,
    pub percentage_total_outputs_scanned: f32,
    /// Numerator of the exact scan-progress ratio: outputs scanned so far
    /// across both shielded pools. May exceed `total_outputs`, whose tree
    /// bounds are fixed at sync start, when scanning continues past them
    /// into chain growth.
    pub total_outputs_scanned: u64,
    /// Denominator of the exact scan-progress ratio: outputs in the chain
    /// between the wallet birthday and the last known chain height, across
    /// both shielded pools. Zero when sync has never started, and also when
    /// the range from birthday to chain height contains no shielded outputs.
    pub total_outputs: u64,
}

impl SyncStatus {
    /// Whether sync is complete: sync has started and every scan range is
    /// fully processed ([`ScanPriority::Scanned`]), so no range still awaits
    /// scanning, nullifier mapping, or nullifier refetching.
    ///
    /// This is the sync task's own terminal condition, so it holds even when
    /// the birthday-to-chain-height range contains no shielded outputs and
    /// the output ratio is vacuously 0 / 0.
    pub fn is_complete(&self) -> bool {
        self.sync_start_height != 0.into()
            && self
                .scan_ranges
                .iter()
                .all(|scan_range| scan_range.priority() == ScanPriority::Scanned)
    }
}

// TODO: complete display, scan ranges in raw form are too verbose
impl std::fmt::Display for SyncStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "percentage complete: {}",
            self.percentage_total_outputs_scanned
        )
    }
}

impl From<SyncStatus> for json::JsonValue {
    fn from(value: SyncStatus) -> Self {
        let scan_ranges: Vec<json::JsonValue> = value
            .scan_ranges
            .iter()
            .map(|range| {
                json::object! {
                    "priority" => format!("{:?}", range.priority()),
                    "start_block" => range.block_range().start.to_string(),
                    "end_block" => (range.block_range().end - 1).to_string(),
                }
            })
            .collect();

        json::object! {
            "scan_ranges" => scan_ranges,
            "sync_start_height" => u32::from(value.sync_start_height),
            "session_blocks_scanned" => value.session_blocks_scanned,
            "total_blocks_scanned" => value.total_blocks_scanned,
            "percentage_session_blocks_scanned" => value.percentage_session_blocks_scanned,
            "percentage_total_blocks_scanned" => value.percentage_total_blocks_scanned,
            "session_sapling_outputs_scanned" => value.session_sapling_outputs_scanned,
            "total_sapling_outputs_scanned" => value.total_sapling_outputs_scanned,
            "session_orchard_outputs_scanned" => value.session_orchard_outputs_scanned,
            "total_orchard_outputs_scanned" => value.total_orchard_outputs_scanned,
            "session_ironwood_outputs_scanned" => value.session_ironwood_outputs_scanned,
            "total_ironwood_outputs_scanned" => value.total_ironwood_outputs_scanned,
            "percentage_session_outputs_scanned" => value.percentage_session_outputs_scanned,
            "percentage_total_outputs_scanned" => value.percentage_total_outputs_scanned,
            "total_outputs_scanned" => value.total_outputs_scanned,
            "total_outputs" => value.total_outputs,
        }
    }
}

/// Returned when [`crate::sync::sync`] successfully completes.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct SyncResult {
    pub sync_start_height: BlockHeight,
    pub sync_end_height: BlockHeight,
    pub blocks_scanned: u32,
    pub sapling_outputs_scanned: u32,
    pub orchard_outputs_scanned: u32,
    pub ironwood_outputs_scanned: u32,
    pub percentage_total_outputs_scanned: f32,
}

impl std::fmt::Display for SyncResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Sync completed succesfully:
{{
    sync start height: {}
    sync end height: {}
    blocks scanned: {}
    sapling outputs scanned: {}
    orchard outputs scanned: {}
    ironwood outputs scanned: {}
    percentage total outputs scanned: {}
}}",
            self.sync_start_height,
            self.sync_end_height,
            self.blocks_scanned,
            self.sapling_outputs_scanned,
            self.orchard_outputs_scanned,
            self.ironwood_outputs_scanned,
            self.percentage_total_outputs_scanned,
        )
    }
}

impl From<SyncResult> for json::JsonValue {
    fn from(value: SyncResult) -> Self {
        json::object! {
            "sync_start_height" => u32::from(value.sync_start_height),
            "sync_end_height" => u32::from(value.sync_end_height),
            "blocks_scanned" => value.blocks_scanned,
            "sapling_outputs_scanned" => value.sapling_outputs_scanned,
            "orchard_outputs_scanned" => value.orchard_outputs_scanned,
            "ironwood_outputs_scanned" => value.ironwood_outputs_scanned,
            "percentage_total_outputs_scanned" => value.percentage_total_outputs_scanned,
        }
    }
}

/// Scanning range priority levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScanPriority {
    /// Block ranges that are currently refetching nullifiers.
    RefetchingNullifiers,
    /// Block ranges that are currently being scanned.
    Scanning,
    /// Block ranges that have already been scanned will not be re-scanned.
    Scanned,
    /// Block ranges that have already been scanned. The nullifiers from this range were not mapped after scanning and
    /// spend detection to reduce memory consumption and/or storage for non-linear scanning. These nullifiers will need
    /// to be re-fetched for final spend detection when this range is the lowest unscanned range in the wallet's list
    /// of scan ranges.
    ScannedWithoutMapping,
    /// Block ranges to be scanned to advance the fully-scanned height.
    Historic,
    /// Block ranges adjacent to heights at which the user opened the wallet.
    OpenAdjacent,
    /// Blocks that must be scanned to complete note commitment tree shards adjacent to found notes.
    FoundNote,
    /// Blocks that must be scanned to complete the latest note commitment tree shard.
    ChainTip,
    /// A previously scanned range that must be verified to check it is still in the
    /// main chain, has highest priority.
    Verify,
}

impl ScanPriority {
    /// Whether this priority marks a range whose blocks have been scanned,
    /// including ranges whose nullifiers still await retrieval. Contrast with
    /// equality to [`ScanPriority::Scanned`], which additionally requires the
    /// nullifier work to be finished.
    pub fn is_scanned(self) -> bool {
        matches!(
            self,
            ScanPriority::Scanned
                | ScanPriority::ScannedWithoutMapping
                | ScanPriority::RefetchingNullifiers
        )
    }

    /// Whether this priority marks a range whose blocks have been scanned but
    /// whose nullifiers still await mapping or refetching for final spend
    /// detection.
    pub fn awaits_nullifier_retrieval(self) -> bool {
        matches!(
            self,
            ScanPriority::ScannedWithoutMapping | ScanPriority::RefetchingNullifiers
        )
    }
}

/// A range of blocks to be scanned, along with its associated priority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRange {
    block_range: Range<BlockHeight>,
    priority: ScanPriority,
}

impl std::fmt::Display for ScanRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}({}..{})",
            self.priority, self.block_range.start, self.block_range.end,
        )
    }
}

impl ScanRange {
    /// Constructs a scan range from its constituent parts.
    #[must_use]
    pub fn from_parts(block_range: Range<BlockHeight>, priority: ScanPriority) -> Self {
        assert!(
            block_range.end >= block_range.start,
            "{block_range:?} is invalid for ScanRange({priority:?})",
        );
        ScanRange {
            block_range,
            priority,
        }
    }

    /// Returns the range of block heights to be scanned.
    #[must_use]
    pub fn block_range(&self) -> &Range<BlockHeight> {
        &self.block_range
    }

    /// Returns the priority with which the scan range should be scanned.
    #[must_use]
    pub fn priority(&self) -> ScanPriority {
        self.priority
    }

    /// Returns whether or not the scan range is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.block_range.is_empty()
    }

    /// Returns the number of blocks in the scan range.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::try_from(u32::from(self.block_range.end) - u32::from(self.block_range.start))
            .expect("due to number of max blocks should always be valid usize")
    }

    /// Shifts the start of the block range to the right if `block_height >
    /// self.block_range().start`. Returns `None` if the resulting range would
    /// be empty (or the range was already empty).
    #[must_use]
    pub fn truncate_start(&self, block_height: BlockHeight) -> Option<Self> {
        if block_height >= self.block_range.end || self.is_empty() {
            None
        } else {
            Some(ScanRange {
                block_range: self.block_range.start.max(block_height)..self.block_range.end,
                priority: self.priority,
            })
        }
    }

    /// Shifts the end of the block range to the left if `block_height <
    /// self.block_range().end`. Returns `None` if the resulting range would
    /// be empty (or the range was already empty).
    #[must_use]
    pub fn truncate_end(&self, block_height: BlockHeight) -> Option<Self> {
        if block_height <= self.block_range.start || self.is_empty() {
            None
        } else {
            Some(ScanRange {
                block_range: self.block_range.start..self.block_range.end.min(block_height),
                priority: self.priority,
            })
        }
    }

    /// Splits this scan range at the specified height, such that the provided height becomes the
    /// end of the first range returned and the start of the second. Returns `None` if
    /// `p <= self.block_range().start || p >= self.block_range().end`.
    #[must_use]
    pub fn split_at(&self, p: BlockHeight) -> Option<(Self, Self)> {
        (p > self.block_range.start && p < self.block_range.end).then_some((
            ScanRange {
                block_range: self.block_range.start..p,
                priority: self.priority,
            },
            ScanRange {
                block_range: p..self.block_range.end,
                priority: self.priority,
            },
        ))
    }
}

/// Syncs a wallet to the latest state of the blockchain.
///
/// `sync_mode` is intended to be stored in a struct that owns the wallet(s) (i.e. lightclient) and has a non-atomic
/// counterpart [`crate::wallet::SyncMode`]. The sync engine will set the `sync_mode` to `Running` at the start of sync.
/// However, the consumer is required to set the `sync_mode` back to `NotRunning` when sync is succussful or returns an
/// error. This allows more flexibility and safety with sync task handles etc.
/// `sync_mode` may also be set to `Paused` externally to pause scanning so the wallet lock can be acquired multiple
/// times in quick sucession without the sync engine interrupting.
/// Set `sync_mode` back to `Running` to resume scanning.
/// Set `sync_mode` to `Shutdown` to stop the sync process.
pub async fn sync<C, P, W>(
    client: C,
    consensus_parameters: &P,
    wallet: Arc<RwLock<W>>,
    sync_mode: Arc<AtomicU8>,
    progress: watch::Sender<Option<SyncStatus>>,
    config: SyncConfig,
) -> Result<SyncResult, SyncError<W::Error>>
where
    C: Clone + Indexer + TransparentIndexer + Sync + Send + 'static,
    P: consensus::Parameters + Sync + Send + 'static,
    W: SyncWallet
        + SyncBlocks
        + SyncTransactions
        + SyncNullifiers
        + SyncOutPoints
        + SyncShardTrees
        + Send,
{
    let mut sync_mode_enum = SyncMode::from_atomic_u8(sync_mode.clone())?;
    if sync_mode_enum == SyncMode::NotRunning {
        sync_mode_enum = SyncMode::Running;
        sync_mode.store(sync_mode_enum as u8, atomic::Ordering::Release);
    } else {
        return Err(SyncModeError::SyncAlreadyRunning.into());
    }

    tracing::info!("Starting sync...");

    // create channel for sending fetch requests and launch fetcher task
    let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
    let client_clone = client.clone();
    let fetcher_handle =
        tokio::spawn(
            async move { client::fetch::fetch(fetch_request_receiver, client_clone).await },
        );

    // create channel for receiving mempool transactions and launch mempool monitor
    let (mempool_transaction_sender, mut mempool_transaction_receiver) = mpsc::channel(100);
    let shutdown_mempool = Arc::new(AtomicBool::new(false));
    let shutdown_mempool_clone = shutdown_mempool.clone();
    let unprocessed_mempool_transactions_count = Arc::new(AtomicU32::new(0));
    let unprocessed_mempool_transactions_count_clone =
        unprocessed_mempool_transactions_count.clone();
    let mempool_stream_connected_at = Arc::new(std::sync::OnceLock::new());
    let mempool_stream_connected_at_clone = mempool_stream_connected_at.clone();
    let mempool_handle = tokio::spawn(async move {
        mempool_monitor(
            client,
            mempool_transaction_sender,
            unprocessed_mempool_transactions_count_clone,
            mempool_stream_connected_at_clone,
            shutdown_mempool_clone,
        )
        .await
    });

    // pre-scan initialisation
    let chain_height = client::get_chain_height(fetch_request_sender.clone()).await?;
    if chain_height == 0.into() {
        return Err(SyncError::ServerError(ServerError::GenesisBlockOnly));
    }
    let last_known_chain_height = checked_wallet_height(
        &mut *wallet.write().await,
        chain_height,
        consensus_parameters,
    )?;

    let ufvks = wallet
        .read()
        .await
        .get_unified_full_viewing_keys()
        .map_err(SyncError::WalletError)?;

    transparent::update_addresses_and_scan_targets(
        consensus_parameters,
        wallet.clone(),
        fetch_request_sender.clone(),
        &ufvks,
        last_known_chain_height,
        chain_height,
        config.transparent_address_discovery,
    )
    .await?;

    update_subtree_roots(
        consensus_parameters,
        fetch_request_sender.clone(),
        &mut *wallet.write().await,
    )
    .await?;

    add_initial_frontier(
        consensus_parameters,
        fetch_request_sender.clone(),
        &mut *wallet.write().await,
    )
    .await?;

    let initial_reorg_detection_start_height = state::update_scan_ranges(
        consensus_parameters,
        fetch_request_sender.clone(),
        last_known_chain_height,
        chain_height,
        &mut *wallet.write().await,
    )
    .await?;

    state::set_initial_state(
        consensus_parameters,
        fetch_request_sender.clone(),
        &mut *wallet.write().await,
        chain_height,
    )
    .await?;

    expire_transactions(&mut *wallet.write().await)?;

    publish_sync_status(&*wallet.read().await, &progress).await;

    // create channel for receiving scan results and launch scanner
    let (scan_results_sender, mut scan_results_receiver) = mpsc::unbounded_channel();
    let mut scanner = Scanner::new(
        consensus_parameters.clone(),
        scan_results_sender,
        fetch_request_sender.clone(),
        ufvks.clone(),
    );
    scanner.launch(config.performance_level);

    // TODO: implement an option for continuous scanning where it doesnt exit when complete

    let mut nullifier_map_limit_exceeded = false;
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            Some((scan_range, scan_results)) = scan_results_receiver.recv() => {
                let mut wallet_guard = wallet.write().await;
                process_scan_results(
                    consensus_parameters,
                    &mut *wallet_guard,
                    fetch_request_sender.clone(),
                    &ufvks,
                    scan_range,
                    scan_results,
                    initial_reorg_detection_start_height,
                    config.performance_level,
                    &mut nullifier_map_limit_exceeded,
                )
                .await?;
                wallet_guard.set_save_flag().map_err(SyncError::WalletError)?;
                publish_sync_status(&*wallet_guard, &progress).await;
                drop(wallet_guard);
            }

            Some(raw_transaction) = mempool_transaction_receiver.recv() => {
                let mut wallet_guard = wallet.write().await;
                process_mempool_transaction(
                    consensus_parameters,
                    &ufvks,
                    &mut *wallet_guard,
                    raw_transaction,
                )
                .await?;
                unprocessed_mempool_transactions_count.fetch_sub(1, atomic::Ordering::Release);
                wallet_guard.set_save_flag().map_err(SyncError::WalletError)?;
                drop(wallet_guard);
            }

            _update_scanner = interval.tick() => {
                sync_mode_enum = SyncMode::from_atomic_u8(sync_mode.clone())?;
                match sync_mode_enum {
                    SyncMode::Paused => {
                        let mut pause_interval = tokio::time::interval(Duration::from_secs(1));
                        pause_interval.tick().await;
                        while sync_mode_enum == SyncMode::Paused {
                            pause_interval.tick().await;
                            sync_mode_enum = SyncMode::from_atomic_u8(sync_mode.clone())?;
                        }
                    },
                    SyncMode::Shutdown => {
                        let mut wallet_guard = wallet.write().await;
                        let sync_status = match sync_status(&*wallet_guard).await {
                            Ok(status) => status,
                            Err(SyncStatusError::WalletError(e)) => {
                                return Err(SyncError::WalletError(e));
                            }
                            Err(SyncStatusError::NoSyncData) => {
                                panic!("sync data must exist!");
                            }
                        };
                        wallet_guard
                            .set_save_flag()
                            .map_err(SyncError::WalletError)?;
                        let _ = progress.send(Some(sync_status.clone()));
                        drop(wallet_guard);
                        mempool_handle.abort();
                        fetcher_handle.abort();
                        tracing::info!("Sync successfully shutdown.");

                        return Ok(SyncResult {
                            sync_start_height: sync_status.sync_start_height,
                            sync_end_height: (sync_status
                                .scan_ranges
                                .last()
                                .expect("should be non-empty after syncing")
                                .block_range()
                                .end
                                - 1),
                            blocks_scanned: sync_status.session_blocks_scanned,
                            sapling_outputs_scanned: sync_status.session_sapling_outputs_scanned,
                            orchard_outputs_scanned: sync_status.session_orchard_outputs_scanned,
                            ironwood_outputs_scanned: sync_status.session_ironwood_outputs_scanned,
                            percentage_total_outputs_scanned: sync_status.percentage_total_outputs_scanned,
                        });
                    }
                    SyncMode::Running => (),
                    SyncMode::NotRunning => {
                        panic!("sync mode should not be manually set to NotRunning!");
                    },
                }

                scanner.update(&mut *wallet.write().await, shutdown_mempool.clone(), nullifier_map_limit_exceeded).await?;

                if matches!(scanner.state, ScannerState::Shutdown) {
                    // Drain check on a 25ms cadence instead of the old
                    // unconditional one-second sleep. The policy lives in
                    // [`drain_verdict`]: shutdown requires a drained
                    // scanner AND a mempool stream that has been connected
                    // long enough to have served pre-existing content, so
                    // a first-loop shutdown on a fully synced chain waits
                    // for the subscription instead of closing the session
                    // before the monitor ever connects. The old one-second
                    // ceiling remains the worst case.
                    let shutdown_poll_started = std::time::Instant::now();
                    let mempool_drained = loop {
                        let verdict = drain_verdict(
                            scanner.worker_poolsize(),
                            unprocessed_mempool_transactions_count
                                .load(atomic::Ordering::Acquire),
                            mempool_stream_connected_at.get().map(|at| at.elapsed()),
                            shutdown_poll_started.elapsed(),
                        );
                        match verdict {
                            DrainVerdict::Shutdown => break true,
                            DrainVerdict::Reenter => break false,
                            DrainVerdict::KeepPolling => {
                                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                            }
                        }
                    };
                    if mempool_drained {
                        tracing::info!("Sync successfully shutdown.");
                        break;
                    }
                }
            }
        }
    }
    let mut wallet_guard = wallet.write().await;
    let sync_status = match sync_status(&*wallet_guard).await {
        Ok(status) => status,
        Err(SyncStatusError::WalletError(e)) => {
            return Err(SyncError::WalletError(e));
        }
        Err(SyncStatusError::NoSyncData) => {
            panic!("sync data must exist!");
        }
    };
    // all blocks up to the last known chain height are now scanned, so any transaction still
    // pending past its expiry height is genuinely expired.
    expire_transactions(&mut *wallet_guard)?;
    // once sync is complete, all nullifiers will have been re-fetched so this note metadata can be discarded.
    for transaction in wallet_guard
        .get_wallet_transactions_mut()
        .map_err(SyncError::WalletError)?
        .values_mut()
    {
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
    wallet_guard
        .set_save_flag()
        .map_err(SyncError::WalletError)?;

    drop(wallet_guard);
    drop(scanner);
    drop(fetch_request_sender);

    match mempool_handle.await.expect("task panicked") {
        Ok(()) => (),
        Err(e @ MempoolError::ShutdownWithoutStream) => tracing::warn!("{e}"),
        Err(e) => return Err(e.into()),
    }
    fetcher_handle.await.expect("task panicked");

    Ok(SyncResult {
        sync_start_height: sync_status.sync_start_height,
        sync_end_height: (sync_status
            .scan_ranges
            .last()
            .expect("should be non-empty after syncing")
            .block_range()
            .end
            - 1),
        blocks_scanned: sync_status.session_blocks_scanned,
        sapling_outputs_scanned: sync_status.session_sapling_outputs_scanned,
        orchard_outputs_scanned: sync_status.session_orchard_outputs_scanned,
        ironwood_outputs_scanned: sync_status.session_ironwood_outputs_scanned,
        percentage_total_outputs_scanned: sync_status.percentage_total_outputs_scanned,
    })
}

/// This ensures that the wallet height used to calculate the lower bound for scan range creation is valid.
/// The comparison takes two input heights and uses several constants to select the correct height.
///
/// The input parameter heights are:
///
///   (1) chain_height:
///       * the best block-height reported by the indexer
///   (2) last_known_chain_height
///       * the last max height the wallet recorded from earlier scans
///
/// The constants are:
///   (1) MAX_REORG_ALLOWANCE:
///       * the maximum number of blocks the wallet can truncate during re-org detection
///   (2) Sapling Activation Height:
///       * the lower bound on the wallet birthday
fn checked_wallet_height<W, P>(
    wallet: &mut W,
    chain_height: BlockHeight,
    consensus_parameters: &P,
) -> Result<BlockHeight, SyncError<W::Error>>
where
    W: SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
    P: zcash_protocol::consensus::Parameters,
{
    let sync_state = wallet.get_sync_state().map_err(SyncError::WalletError)?;
    if let Some(last_known_chain_height) = sync_state.last_known_chain_height() {
        if last_known_chain_height > chain_height {
            if last_known_chain_height - chain_height >= MAX_REORG_ALLOWANCE {
                // There's a human attention requiring problem, the wallet supplied
                // last_known_chain_height is more than MAX_REORG_ALLOWANCE **above**
                // the proxy's reported height.
                return Err(SyncError::ChainError(
                    u32::from(last_known_chain_height),
                    MAX_REORG_ALLOWANCE,
                    u32::from(chain_height),
                ));
            }
            // The wallet reported height is above the current proxy height
            // reset to the proxy height.
            truncate_wallet_data(wallet, chain_height)?;
            truncate_scan_ranges(
                chain_height,
                wallet
                    .get_sync_state_mut()
                    .map_err(SyncError::WalletError)?,
            );
            wallet.set_save_flag().map_err(SyncError::WalletError)?;
            return Ok(chain_height);
        }
        // The last wallet reported height is equal or below the proxy height.
        Ok(last_known_chain_height)
    } else {
        // This is the wallet's first sync. Use [birthday - 1] as wallet height.
        let sapling_activation_height = consensus_parameters
            .activation_height(consensus::NetworkUpgrade::Sapling)
            .expect("sapling activation height should always return Some");
        let birthday = wallet.get_birthday().map_err(SyncError::WalletError)?;
        if birthday > chain_height {
            // Human attention requiring error, a birthday *above* the proxy reported
            // chain height has been provided.
            return Err(SyncError::ChainError(
                u32::from(birthday),
                MAX_REORG_ALLOWANCE,
                u32::from(chain_height),
            ));
        } else if birthday < sapling_activation_height {
            return Err(SyncError::BirthdayBelowSapling(
                u32::from(birthday),
                u32::from(sapling_activation_height),
            ));
        }

        Ok(birthday - 1)
    }
}

/// Creates a [`self::SyncStatus`] from the wallet's current [`crate::wallet::SyncState`].
/// If there is still nullifiers to be re-fetched when scanning is complete, the percentages will be overrided to 99%
/// until sync is complete.
///
/// Intended to be called while [`self::sync`] is running in a separate task.
pub async fn sync_status<W>(wallet: &W) -> Result<SyncStatus, SyncStatusError<W::Error>>
where
    W: SyncWallet + SyncBlocks,
{
    /// Sums one per-pool trio of output counts into the pool-agnostic
    /// total. Pure and total: this is the single definition of which
    /// pools participate in scan-progress accounting, so every
    /// consumer (the percentages and the exact u64 ratio) agrees by
    /// construction, and adding a pool touches exactly this function.
    fn output_pool_total(sapling: u32, orchard: u32, ironwood: u32) -> u64 {
        u64::from(sapling) + u64::from(orchard) + u64::from(ironwood)
    }

    /// The scale every scan-progress ratio is reported on.
    const PERCENTAGE_SCALE: f32 = 100.0;
    /// The percentage a fully scanned span reports.
    const COMPLETE_PERCENTAGE: f32 = PERCENTAGE_SCALE;
    /// The percentage an entirely unscanned span reports.
    const NO_PROGRESS_PERCENTAGE: f32 = 0.0;
    /// The smallest whole step a reported percentage moves by.
    const PERCENTAGE_STEP: f32 = 1.0;
    /// The percentage reported while scanning has finished but nullifiers await refetching.
    const NULLIFIER_RETRIEVAL_PERCENTAGE: f32 = COMPLETE_PERCENTAGE - PERCENTAGE_STEP;
    /// The block a span's own first height contributes to the span's inclusive length.
    const INCLUSIVE_SPAN_ADJUSTMENT: u32 = 1;

    /// Reports `scanned` as a percentage of `total`, and reports `None` where a zero denominator leaves that ratio undefined.
    fn percentage_scanned(scanned: u64, total: u64) -> Option<f32> {
        (total != 0).then(|| {
            ((scanned as f32 / total as f32) * PERCENTAGE_SCALE)
                .clamp(NO_PROGRESS_PERCENTAGE, COMPLETE_PERCENTAGE)
        })
    }

    let (
        total_sapling_outputs_scanned,
        total_orchard_outputs_scanned,
        total_ironwood_outputs_scanned,
    ) = state::calculate_scanned_outputs(wallet).map_err(SyncStatusError::WalletError)?;
    let total_outputs_scanned = output_pool_total(
        total_sapling_outputs_scanned,
        total_orchard_outputs_scanned,
        total_ironwood_outputs_scanned,
    );

    let sync_state = wallet
        .get_sync_state()
        .map_err(SyncStatusError::WalletError)?;
    if sync_state.initial_sync_state.sync_start_height == 0.into() {
        return Ok(SyncStatus {
            scan_ranges: sync_state.scan_ranges.clone(),
            sync_start_height: 0.into(),
            session_blocks_scanned: 0,
            total_blocks_scanned: 0,
            percentage_session_blocks_scanned: NO_PROGRESS_PERCENTAGE,
            percentage_total_blocks_scanned: NO_PROGRESS_PERCENTAGE,
            session_sapling_outputs_scanned: 0,
            session_orchard_outputs_scanned: 0,
            session_ironwood_outputs_scanned: 0,
            total_sapling_outputs_scanned: 0,
            total_orchard_outputs_scanned: 0,
            total_ironwood_outputs_scanned: 0,
            percentage_session_outputs_scanned: NO_PROGRESS_PERCENTAGE,
            percentage_total_outputs_scanned: NO_PROGRESS_PERCENTAGE,
            total_outputs_scanned: 0,
            total_outputs: 0,
        });
    }
    let total_blocks_scanned = state::calculate_scanned_blocks(sync_state);

    let birthday = sync_state
        .wallet_birthday()
        .ok_or(SyncStatusError::NoSyncData)?;
    let last_known_chain_height = sync_state
        .last_known_chain_height()
        .ok_or(SyncStatusError::NoSyncData)?;
    let total_blocks = u32::from(last_known_chain_height)
        .saturating_sub(u32::from(birthday))
        .saturating_add(INCLUSIVE_SPAN_ADJUSTMENT);
    let total_sapling_outputs = sync_state
        .initial_sync_state
        .wallet_tree_bounds
        .sapling_final_tree_size
        .saturating_sub(
            sync_state
                .initial_sync_state
                .wallet_tree_bounds
                .sapling_initial_tree_size,
        );
    let total_orchard_outputs = sync_state
        .initial_sync_state
        .wallet_tree_bounds
        .orchard_final_tree_size
        .saturating_sub(
            sync_state
                .initial_sync_state
                .wallet_tree_bounds
                .orchard_initial_tree_size,
        );
    let total_ironwood_outputs = sync_state
        .initial_sync_state
        .wallet_tree_bounds
        .ironwood_final_tree_size
        .saturating_sub(
            sync_state
                .initial_sync_state
                .wallet_tree_bounds
                .ironwood_initial_tree_size,
        );
    let total_outputs = output_pool_total(
        total_sapling_outputs,
        total_orchard_outputs,
        total_ironwood_outputs,
    );

    let session_blocks_scanned = total_blocks_scanned
        .saturating_sub(sync_state.initial_sync_state.previously_scanned_blocks);
    let session_blocks =
        total_blocks.saturating_sub(sync_state.initial_sync_state.previously_scanned_blocks);
    let mut percentage_total_blocks_scanned =
        percentage_scanned(u64::from(total_blocks_scanned), u64::from(total_blocks))
            .unwrap_or(COMPLETE_PERCENTAGE);
    let mut percentage_session_blocks_scanned =
        percentage_scanned(u64::from(session_blocks_scanned), u64::from(session_blocks))
            .unwrap_or(percentage_total_blocks_scanned);

    let session_sapling_outputs_scanned = total_sapling_outputs_scanned.saturating_sub(
        sync_state
            .initial_sync_state
            .previously_scanned_sapling_outputs,
    );
    let session_orchard_outputs_scanned = total_orchard_outputs_scanned.saturating_sub(
        sync_state
            .initial_sync_state
            .previously_scanned_orchard_outputs,
    );
    let session_ironwood_outputs_scanned = total_ironwood_outputs_scanned.saturating_sub(
        sync_state
            .initial_sync_state
            .previously_scanned_ironwood_outputs,
    );
    let session_outputs_scanned = output_pool_total(
        session_sapling_outputs_scanned,
        session_orchard_outputs_scanned,
        session_ironwood_outputs_scanned,
    );
    let previously_scanned_outputs = output_pool_total(
        sync_state
            .initial_sync_state
            .previously_scanned_sapling_outputs,
        sync_state
            .initial_sync_state
            .previously_scanned_orchard_outputs,
        sync_state
            .initial_sync_state
            .previously_scanned_ironwood_outputs,
    );
    let session_outputs = total_outputs.saturating_sub(previously_scanned_outputs);
    let mut percentage_total_outputs_scanned =
        percentage_scanned(total_outputs_scanned, total_outputs).unwrap_or(COMPLETE_PERCENTAGE);
    let mut percentage_session_outputs_scanned =
        percentage_scanned(session_outputs_scanned, session_outputs)
            .unwrap_or(percentage_total_outputs_scanned);

    if sync_state
        .scan_ranges()
        .iter()
        .any(|scan_range| scan_range.priority().awaits_nullifier_retrieval())
    {
        if percentage_session_blocks_scanned == COMPLETE_PERCENTAGE {
            percentage_session_blocks_scanned = NULLIFIER_RETRIEVAL_PERCENTAGE;
        }
        if percentage_total_blocks_scanned == COMPLETE_PERCENTAGE {
            percentage_total_blocks_scanned = NULLIFIER_RETRIEVAL_PERCENTAGE;
        }
        if percentage_session_outputs_scanned == COMPLETE_PERCENTAGE {
            percentage_session_outputs_scanned = NULLIFIER_RETRIEVAL_PERCENTAGE;
        }
        if percentage_total_outputs_scanned == COMPLETE_PERCENTAGE {
            percentage_total_outputs_scanned = NULLIFIER_RETRIEVAL_PERCENTAGE;
        }
    }

    Ok(SyncStatus {
        scan_ranges: sync_state.scan_ranges.clone(),
        sync_start_height: sync_state.initial_sync_state.sync_start_height,
        session_blocks_scanned,
        total_blocks_scanned,
        percentage_session_blocks_scanned,
        percentage_total_blocks_scanned,
        session_sapling_outputs_scanned,
        total_sapling_outputs_scanned,
        session_orchard_outputs_scanned,
        total_orchard_outputs_scanned,
        session_ironwood_outputs_scanned,
        total_ironwood_outputs_scanned,
        percentage_session_outputs_scanned,
        percentage_total_outputs_scanned,
        total_outputs_scanned,
        total_outputs,
    })
}

/// Publishes the wallet's current sync status to the progress channel, ignoring an unreadable status and a closed channel.
async fn publish_sync_status<W>(wallet: &W, progress: &watch::Sender<Option<SyncStatus>>)
where
    W: SyncWallet + SyncBlocks,
{
    if let Ok(status) = sync_status(wallet).await {
        let _ = progress.send(Some(status));
    }
}

/// Scans a pending `transaction` of a given `status`, adding to the wallet and updating output spend statuses.
///
/// Used both internally for scanning mempool transactions and externally for scanning calculated and transmitted
/// transactions during send.
///
/// Panics if `status` is of `Confirmed` variant.
pub fn scan_pending_transaction<W>(
    consensus_parameters: &impl consensus::Parameters,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    wallet: &mut W,
    transaction: Transaction,
    status: ConfirmationStatus,
    datetime: u32,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    if matches!(status, ConfirmationStatus::Confirmed(_)) {
        panic!("this fn is for unconfirmed transactions only");
    }

    let mut pending_transaction_nullifiers = NullifierMap::new();
    let mut pending_transaction_outpoints = BTreeMap::new();
    let transparent_addresses: HashMap<String, TransparentAddressId> = wallet
        .get_transparent_addresses()
        .map_err(SyncError::WalletError)?
        .iter()
        .map(|(id, address)| (address.clone(), *id))
        .collect();
    let pending_transaction = scan_transaction(
        consensus_parameters,
        ufvks,
        transaction.txid(),
        transaction,
        status,
        None,
        &mut pending_transaction_nullifiers,
        &mut pending_transaction_outpoints,
        &transparent_addresses,
        datetime,
    )?;

    let wallet_transactions = wallet
        .get_wallet_transactions()
        .map_err(SyncError::WalletError)?;
    let transparent_output_ids = spend::collect_transparent_output_ids(wallet_transactions);
    let transparent_spend_scan_targets = spend::detect_transparent_spends(
        &mut pending_transaction_outpoints,
        transparent_output_ids,
    );
    let (sapling_derived_nullifiers, orchard_derived_nullifiers, ironwood_derived_nullifiers) =
        spend::collect_derived_nullifiers(wallet_transactions);
    let (sapling_spend_scan_targets, orchard_spend_scan_targets, ironwood_spend_scan_targets) =
        spend::detect_shielded_spends(
            &mut pending_transaction_nullifiers,
            sapling_derived_nullifiers,
            orchard_derived_nullifiers,
            ironwood_derived_nullifiers,
        );

    // return if transaction is not relevant to the wallet
    if pending_transaction.transparent_coins().is_empty()
        && pending_transaction.sapling_notes().is_empty()
        && pending_transaction.orchard_notes().is_empty()
        && pending_transaction.ironwood_notes().is_empty()
        && pending_transaction.outgoing_sapling_notes().is_empty()
        && pending_transaction.outgoing_orchard_notes().is_empty()
        && pending_transaction.outgoing_ironwood_notes().is_empty()
        && transparent_spend_scan_targets.is_empty()
        && sapling_spend_scan_targets.is_empty()
        && orchard_spend_scan_targets.is_empty()
        && ironwood_spend_scan_targets.is_empty()
    {
        return Ok(());
    }

    wallet
        .insert_wallet_transaction(pending_transaction)
        .map_err(SyncError::WalletError)?;
    spend::update_spent_coins(
        wallet
            .get_wallet_transactions_mut()
            .map_err(SyncError::WalletError)?,
        transparent_spend_scan_targets,
    );
    spend::update_spent_notes(
        wallet,
        sapling_spend_scan_targets,
        orchard_spend_scan_targets,
        ironwood_spend_scan_targets,
        false,
    )
    .map_err(SyncError::WalletError)?;

    Ok(())
}

/// API for targetted scanning.
///
/// Allows `scan_targets` to be added externally to the wallet's `sync_state` and be prioritised for scanning. Each
/// scan target must include the block height which will be used to prioritise the block range containing the note
/// commitments to the surrounding orchard shard(s). If the block height is pre-orchard then the surrounding sapling
/// shard(s) will be prioritised instead. The txid in each scan target may be omitted and set to [0u8; 32] in order to
/// prioritise the surrounding blocks for scanning but be ignored when fetching specific relevant transactions to the
/// wallet. However, in the case where a relevant spending transaction at a given height contains no decryptable
/// incoming notes (change), only the nullifier will be mapped and this transaction will be scanned when the
/// transaction containing the spent notes is scanned instead.
pub fn add_scan_targets(sync_state: &mut SyncState, scan_targets: &[ScanTarget]) {
    for scan_target in scan_targets {
        sync_state.scan_targets.insert(*scan_target);
    }
}

/// Resets the spending transaction field of all outputs that were previously spent but became unspent due to a
/// spending transactions becoming invalid.
///
/// `invalid_txids` are the id's of the invalidated spending transactions. Any outputs in the `wallet_transactions`
/// matching these spending transactions will be reset back to `None`.
pub fn reset_spends(
    wallet_transactions: &mut HashMap<TxId, WalletTransaction>,
    invalid_txids: Vec<TxId>,
) {
    wallet_transactions
        .values_mut()
        .flat_map(|transaction| transaction.ironwood_notes_mut())
        .filter(|output| {
            output
                .spending_transaction
                .is_some_and(|spending_txid| invalid_txids.contains(&spending_txid))
        })
        .for_each(|output| {
            output.set_spending_transaction(None);
        });
    wallet_transactions
        .values_mut()
        .flat_map(|transaction| transaction.orchard_notes_mut())
        .filter(|output| {
            output
                .spending_transaction
                .is_some_and(|spending_txid| invalid_txids.contains(&spending_txid))
        })
        .for_each(|output| {
            output.set_spending_transaction(None);
        });
    wallet_transactions
        .values_mut()
        .flat_map(|transaction| transaction.sapling_notes_mut())
        .filter(|output| {
            output
                .spending_transaction
                .is_some_and(|spending_txid| invalid_txids.contains(&spending_txid))
        })
        .for_each(|output| {
            output.set_spending_transaction(None);
        });
    wallet_transactions
        .values_mut()
        .flat_map(|transaction| transaction.transparent_coins_mut())
        .filter(|output| {
            output
                .spending_transaction
                .is_some_and(|spending_txid| invalid_txids.contains(&spending_txid))
        })
        .for_each(|output| {
            output.set_spending_transaction(None);
        });
}

/// Sets transactions associated with list of `failed_txids` in `wallet_transactions` to `Failed` status.
///
/// Sets the `spending_transaction` fields of any outputs spent in these transactions to `None`.
///
/// Transactions with `Confirmed` status are skipped with a warning. A mined transaction
/// cannot fail, since only a reorg can un-mine it, and reorgs are handled by truncation, which
/// reopens the affected scan ranges. For a confirmed transaction the note's
/// `spending_transaction` field is the wallet's only durable record of the on-chain spend
/// (detection consumed the nullifier-map entry when the spending block was scanned), so
/// resetting it here would create a permanent phantom unspent note that no forward sync
/// can correct.
pub fn set_transactions_failed(
    wallet_transactions: &mut HashMap<TxId, WalletTransaction>,
    failed_txids: Vec<TxId>,
) {
    let (confirmed_txids, failable_txids): (Vec<TxId>, Vec<TxId>) =
        failed_txids.into_iter().partition(|txid| {
            wallet_transactions
                .get(txid)
                .is_some_and(|transaction| transaction.status().is_confirmed())
        });
    for confirmed_txid in confirmed_txids {
        tracing::warn!(
            "refusing to fail transaction {confirmed_txid} with `Confirmed` status! \
             a mined transaction can only be invalidated by truncation."
        );
    }
    set_transactions_failed_unchecked(wallet_transactions, failable_txids);
}

/// As [`set_transactions_failed`], without the guard against failing `Confirmed`
/// transactions.
///
/// Only truncation may take this path: it fails transactions in reorged-away blocks and
/// simultaneously reopens the affected scan ranges, so re-scanning is guaranteed to
/// re-detect any spends that are still on the best chain.
pub(crate) fn set_transactions_failed_unchecked(
    wallet_transactions: &mut HashMap<TxId, WalletTransaction>,
    failed_txids: Vec<TxId>,
) {
    for failed_txid in failed_txids.iter() {
        if let Some(transaction) = wallet_transactions.get_mut(failed_txid) {
            let height = transaction.status().get_height();
            transaction.update_status(
                ConfirmationStatus::Failed(height),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .expect("infalliable for such long time periods")
                    .as_secs() as u32,
                true,
            );
        }
    }
    reset_spends(wallet_transactions, failed_txids);
}

/// Returns true if the scanner and mempool are shutdown.
/// Verdict for one pass of the scanner-shutdown drain poll. Pure over a
/// snapshot: the caller loads the atomics and clocks. This only
/// decides, so the whole policy is table-testable without a runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrainVerdict {
    /// Drained and the mempool stream has settled: end the session.
    Shutdown,
    /// Work appeared, or the ceiling expired with workers running:
    /// re-enter the processing loop.
    Reenter,
    /// Undecided: sleep one cadence and poll again.
    KeepPolling,
}

/// The drain policy for scanner shutdown.
///
/// `connected_for` is the age of the mempool stream subscription
/// (`None` until it is established). Connection, not first delivery,
/// is deliberately the grace trigger: an empty mempool never delivers,
/// and a delivery-based grace would hold every such session for the
/// full ceiling, restoring the fixed second c90f8d309 removed. The
/// settle window covers the gap between subscribing and receiving
/// pre-existing mempool content (served within ~100ms of connect).
fn drain_verdict(
    scan_workers: usize,
    unprocessed_mempool_transactions: u32,
    connected_for: Option<Duration>,
    poll_elapsed: Duration,
) -> DrainVerdict {
    use zingo_netutils::time::{MEMPOOL_DRAIN_CEILING, MEMPOOL_DRAIN_SETTLE};

    if unprocessed_mempool_transactions > 0 {
        return DrainVerdict::Reenter;
    }
    if poll_elapsed >= MEMPOOL_DRAIN_CEILING {
        return if scan_workers == 0 {
            DrainVerdict::Shutdown
        } else {
            DrainVerdict::Reenter
        };
    }
    match connected_for {
        Some(age) if age >= MEMPOOL_DRAIN_SETTLE && scan_workers == 0 => DrainVerdict::Shutdown,
        // Settled, but the scanner still holds workers. Polling cannot retire
        // them: only the main loop's `scanner.update()` can, and it runs
        // outside this loop, so waiting here burns the ceiling on a value that
        // cannot move. Re-enter and let the loop make progress instead.
        Some(age) if age >= MEMPOOL_DRAIN_SETTLE => DrainVerdict::Reenter,
        _ => DrainVerdict::KeepPolling,
    }
}

/// Scan post-processing
#[allow(clippy::too_many_arguments)]
async fn process_scan_results<W>(
    consensus_parameters: &impl consensus::Parameters,
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_range: ScanRange,
    scan_results: Result<ScanResults, ScanError>,
    initial_reorg_detection_start_height: BlockHeight,
    performance_level: PerformanceLevel,
    nullifier_map_limit_exceeded: &mut bool,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet
        + SyncBlocks
        + SyncTransactions
        + SyncNullifiers
        + SyncOutPoints
        + SyncShardTrees
        + Send,
{
    match scan_results {
        Ok(results) => {
            let ScanResults {
                mut nullifiers,
                mut outpoints,
                scanned_blocks,
                wallet_transactions,
                sapling_located_trees,
                orchard_located_trees,
                ironwood_located_trees,
            } = results;

            if scan_range.priority() == ScanPriority::ScannedWithoutMapping {
                // add missing block bounds in the case that the load's nullifier budget was reached and the fetch nullifier
                // scan range was split.
                let full_refetching_nullifiers_range = wallet
                    .get_sync_state()
                    .map_err(SyncError::WalletError)?
                    .scan_ranges
                    .iter()
                    .find(|&wallet_scan_range| {
                        wallet_scan_range
                            .block_range()
                            .contains(&scan_range.block_range().start)
                            && wallet_scan_range
                                .block_range()
                                .contains(&(scan_range.block_range().end - 1))
                    })
                    .expect("wallet scan range containing scan range should exist!");
                if scan_range.block_range().start
                    != full_refetching_nullifiers_range.block_range().start
                    || scan_range.block_range().end
                        != full_refetching_nullifiers_range.block_range().end
                {
                    let mut missing_block_bounds = BTreeMap::new();
                    for block_bound in [
                        scan_range.block_range().start - 1,
                        scan_range.block_range().start,
                        scan_range.block_range().end - 1,
                        scan_range.block_range().end,
                    ] {
                        if block_bound < full_refetching_nullifiers_range.block_range().start
                            || block_bound >= full_refetching_nullifiers_range.block_range().end
                        {
                            continue;
                        }
                        if wallet.get_wallet_block(block_bound).is_err() {
                            missing_block_bounds.insert(
                                block_bound,
                                WalletBlock::from_compact_block(
                                    consensus_parameters,
                                    fetch_request_sender.clone(),
                                    &client::get_compact_block(
                                        fetch_request_sender.clone(),
                                        block_bound,
                                    )
                                    .await?,
                                )
                                .await?,
                            );
                        }
                    }
                    if !missing_block_bounds.is_empty() {
                        wallet
                            .append_wallet_blocks(missing_block_bounds)
                            .map_err(SyncError::WalletError)?;
                    }
                }

                let first_unscanned_range = wallet
                    .get_sync_state()
                    .map_err(SyncError::WalletError)?
                    .scan_ranges
                    .iter()
                    .find(|scan_range| scan_range.priority() != ScanPriority::Scanned)
                    .expect("the scan range being processed is not yet set to scanned so at least one unscanned range must exist");
                if !first_unscanned_range
                    .block_range()
                    .contains(&scan_range.block_range().start)
                    || !first_unscanned_range
                        .block_range()
                        .contains(&(scan_range.block_range().end - 1))
                {
                    // in this rare edge case, a scanned `ScannedWithoutMapping` range was the highest priority yet it was not the first unscanned range so it must be discarded to avoid missing spends

                    // reset scan range from `RefetchingNullifiers` to `ScannedWithoutMapping`
                    state::reset_refetching_nullifiers_scan_range(
                        wallet
                            .get_sync_state_mut()
                            .map_err(SyncError::WalletError)?,
                        scan_range.block_range().clone(),
                    );
                    tracing::debug!(
                        "Nullifiers discarded and will be re-fetched to avoid missing spends."
                    );

                    return Ok(());
                }

                spend::update_shielded_spends(
                    consensus_parameters,
                    wallet,
                    fetch_request_sender.clone(),
                    ufvks,
                    &scanned_blocks,
                    Some(&mut nullifiers),
                )
                .await?;

                state::set_scanned_scan_range(
                    wallet
                        .get_sync_state_mut()
                        .map_err(SyncError::WalletError)?,
                    scan_range.block_range().clone(),
                    true, // NOTE: although nullifiers are not actually added to the wallet's nullifier map for efficiency, there is effectively no difference as spends are still updated using the `additional_nullifier_map` and would be removed on the following cleanup (`remove_irrelevant_data`) due to `ScannedWithoutMapping` ranges always being the first non-scanned range and therefore always raise the wallet's fully scanned height after processing.
                );
            } else {
                // nullifiers are not mapped if nullifier map size limit will be exceeded
                if !*nullifier_map_limit_exceeded {
                    let nullifier_map = wallet.get_nullifiers().map_err(SyncError::WalletError)?;
                    if max_nullifier_map_size(performance_level).is_some_and(|max| {
                        nullifier_map.orchard.len()
                            + nullifier_map.sapling.len()
                            + nullifier_map.ironwood.len()
                            + nullifiers.orchard.len()
                            + nullifiers.sapling.len()
                            + nullifiers.ironwood.len()
                            > max
                    }) {
                        *nullifier_map_limit_exceeded = true;
                    }
                }
                let mut map_nullifiers = !*nullifier_map_limit_exceeded;

                // all transparent spend locations are known before scanning so there is no need to map outpoints from untargetted ranges.
                // outpoints of untargetted ranges will still be checked before being discarded.
                let map_outpoints = scan_range.priority() >= ScanPriority::FoundNote;

                // always map nullifiers if scanning the lowest range to be scanned for final spend detection.
                // this will set the range to `Scanned` (as oppose to `ScannedWithoutMapping`) and prevent immediate
                // re-fetching of the nullifiers in this range. these will be immediately cleared after cleanup so will not
                // have an impact on memory or wallet file size.
                // the selected range is not the lowest range to be scanned unless all ranges before it are scanned or
                // scanning.
                for query_scan_range in wallet
                    .get_sync_state()
                    .map_err(SyncError::WalletError)?
                    .scan_ranges()
                {
                    let scan_priority = query_scan_range.priority();
                    if scan_priority != ScanPriority::Scanned
                        && scan_priority != ScanPriority::Scanning
                        && scan_priority != ScanPriority::RefetchingNullifiers
                    {
                        break;
                    }

                    if scan_priority == ScanPriority::Scanning
                        && query_scan_range
                            .block_range()
                            .contains(&scan_range.block_range().start)
                        && query_scan_range
                            .block_range()
                            .contains(&(scan_range.block_range().end - 1))
                    {
                        map_nullifiers = true;
                        break;
                    }
                }

                update_wallet_data(
                    consensus_parameters,
                    wallet,
                    fetch_request_sender.clone(),
                    ufvks,
                    &scan_range,
                    if map_nullifiers {
                        Some(&mut nullifiers)
                    } else {
                        None
                    },
                    if map_outpoints {
                        Some(&mut outpoints)
                    } else {
                        None
                    },
                    wallet_transactions,
                    sapling_located_trees,
                    orchard_located_trees,
                    ironwood_located_trees,
                )
                .await?;
                spend::update_transparent_spends(
                    wallet,
                    if map_outpoints {
                        None
                    } else {
                        Some(&mut outpoints)
                    },
                )
                .map_err(SyncError::WalletError)?;
                spend::update_shielded_spends(
                    consensus_parameters,
                    wallet,
                    fetch_request_sender,
                    ufvks,
                    &scanned_blocks,
                    if map_nullifiers {
                        None
                    } else {
                        Some(&mut nullifiers)
                    },
                )
                .await?;
                add_scanned_blocks(wallet, scanned_blocks, &scan_range)
                    .map_err(SyncError::WalletError)?;

                state::set_scanned_scan_range(
                    wallet
                        .get_sync_state_mut()
                        .map_err(SyncError::WalletError)?,
                    scan_range.block_range().clone(),
                    map_nullifiers,
                );
                state::merge_scan_ranges(
                    wallet
                        .get_sync_state_mut()
                        .map_err(SyncError::WalletError)?,
                    ScanPriority::ScannedWithoutMapping,
                );
            }

            state::merge_scan_ranges(
                wallet
                    .get_sync_state_mut()
                    .map_err(SyncError::WalletError)?,
                ScanPriority::Scanned,
            );
            remove_irrelevant_data(wallet).map_err(SyncError::WalletError)?;
            tracing::debug!("Scan results processed.");
        }
        Err(ScanError::ContinuityError(ContinuityError::HashDiscontinuity { height, .. })) => {
            tracing::warn!("Hash discontinuity detected before block {height}.");
            if height == scan_range.block_range().start
                && scan_range.priority() == ScanPriority::Verify
            {
                tracing::info!("Re-org detected.");
                let sync_state = wallet
                    .get_sync_state_mut()
                    .map_err(SyncError::WalletError)?;
                let last_known_chain_height = sync_state
                    .last_known_chain_height()
                    .expect("scan ranges should be non-empty in this scope");

                // reset scan range from `Scanning` to `Verify`
                state::set_scan_priority(
                    sync_state,
                    scan_range.block_range(),
                    ScanPriority::Verify,
                );

                // extend verification range to VERIFY_BLOCK_RANGE_SIZE blocks below current verification range
                let current_reorg_detection_start_height = state::set_verify_scan_range(
                    sync_state,
                    height - 1,
                    state::VerifyEnd::VerifyHighest,
                )
                .block_range()
                .start;
                state::merge_scan_ranges(sync_state, ScanPriority::Verify);

                if initial_reorg_detection_start_height - current_reorg_detection_start_height
                    > MAX_REORG_ALLOWANCE
                {
                    clear_wallet_data(wallet)?;

                    return Err(ServerError::ChainVerificationError.into());
                }

                truncate_wallet_data(wallet, current_reorg_detection_start_height - 1)?;

                state::set_initial_state(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    wallet,
                    last_known_chain_height,
                )
                .await?;
            } else {
                scan_results?;
            }
        }
        Err(ScanError::IncorrectTreeSize {
            shielded_protocol: PoolType::Shielded(pool),
            height,
            block_metadata_size,
            calculated_size,
        }) => {
            tracing::error!(
                "RESCAN TRIGGERED: at height {height}, {pool:?} history recorded a commitment \
                 tree of {calculated_size} where the chain reports {block_metadata_size}; the \
                 wallet's {pool:?} records are being cleared back to the pool activation height \
                 and the next sync rescans from there."
            );
            return Err(truncate_to_pool_activation_height(
                consensus_parameters,
                fetch_request_sender.clone(),
                wallet,
                pool,
                height,
                block_metadata_size,
                calculated_size,
            )
            .await?);
        }
        Err(e) => return Err(e.into()),
    }

    Ok(())
}

/// Truncates the wallet back to the `target_pool` activation height.
///
/// Wallet blocks, transactions, nullifiers and outpoints are all cleared at or above the target pool's activation
/// height.
///
/// The target pool and any pool that came in a later network upgrade have their shard trees cleared. Any earlier
/// pool's will truncate back to the earliest checkpoint above the target pool's acitvation height. This means that
/// some shard tree data above the target pool's activation height may be retained in the wallet. However, in the
/// case of an older version of the sync engine scanning blocks from a new incompatible pool epoch, all shard tree data
/// for earlier pool's will be correct. Re-insertion of this shard tree data on rescan will not cause any issues.
async fn truncate_to_pool_activation_height<W>(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    wallet: &mut W,
    target_pool: ShieldedPool,
    disagreed_at: BlockHeight,
    block_metadata_size: u32,
    calculated_size: u32,
) -> Result<SyncError<W::Error>, SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    let birthday = wallet.get_birthday().map_err(SyncError::WalletError)?;
    let Some(activation) = PoolActivation::of(consensus_parameters, target_pool) else {
        // A pool the chain never activates cannot have been served, so it
        // cannot be the one that disagreed.
        panic!("{target_pool:?} reported a tree size on a chain that never activates it");
    };
    let rescan_from = activation.max_with(birthday);
    let rescan_targets = wallet
        .get_wallet_transactions()
        .map_err(SyncError::WalletError)?
        .values()
        .filter(|&transaction| transaction.status().is_confirmed_after_or_at(&rescan_from))
        .map(|transaction| ScanTarget {
            block_height: transaction.status().get_height(),
            txid: transaction.txid(),
            narrow_scan_area: true,
        })
        .collect::<Vec<_>>();

    truncate_stores(wallet, rescan_from - 1, false)?;

    let frontiers = client::get_frontiers(fetch_request_sender, birthday).await?;
    let retention = Retention::Checkpoint {
        id: birthday,
        marking: Marking::None,
    };
    let shard_trees = wallet
        .get_shard_trees_mut()
        .map_err(SyncError::WalletError)?;
    for pool in [
        ShieldedPool::Sapling,
        ShieldedPool::Orchard,
        ShieldedPool::Ironwood,
    ] {
        if target_pool >= pool {
            shard_trees.clear_pool(pool);
            match pool {
                ShieldedPool::Sapling => shard_trees
                    .sapling
                    .insert_frontier(frontiers.final_sapling_tree().clone(), retention),
                ShieldedPool::Orchard => shard_trees
                    .orchard
                    .insert_frontier(frontiers.final_orchard_tree().clone(), retention),
                ShieldedPool::Ironwood => shard_trees
                    .ironwood
                    .insert_frontier(frontiers.final_ironwood_tree().clone(), retention),
            }
            .expect("infallible");
        } else {
            match pool {
                ShieldedPool::Sapling => {
                    truncate_tree_to_next_checkpoint(rescan_from - 1, &mut shard_trees.sapling)?;
                }
                ShieldedPool::Orchard => {
                    truncate_tree_to_next_checkpoint(rescan_from - 1, &mut shard_trees.orchard)?;
                }
                ShieldedPool::Ironwood => {
                    truncate_tree_to_next_checkpoint(rescan_from - 1, &mut shard_trees.ironwood)?;
                }
            }
        }
    }

    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    state::reopen_scan_ranges_from(sync_state, rescan_from);
    add_scan_targets(sync_state, &rescan_targets);
    wallet.set_save_flag().map_err(SyncError::WalletError)?;

    Ok(SyncError::PoolHistoryReopened {
        pool: PoolType::Shielded(target_pool),
        rescan_from,
        disagreed_at,
        block_metadata_size,
        calculated_size,
    })
}

/// Truncate the shard tree to the lowest checkpoint equal to or above the `target_height`.
///
/// If all checkpoints are below target height, do not truncate. Shard tree data at the target height is
/// never removed, only data above the target height.
fn truncate_tree_to_next_checkpoint<H, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    target_height: BlockHeight,
    tree: &mut ShardTree<MemoryShardStore<H, BlockHeight>, DEPTH, SHARD_HEIGHT>,
) -> Result<(), shardtree::error::ShardTreeError<Infallible>>
where
    H: incrementalmerkletree::Hashable + Clone + PartialEq,
{
    let mut truncation_height = None;
    tree.store()
        .for_each_checkpoint((MAX_REORG_ALLOWANCE + 1) as usize, |height, _| {
            if truncation_height.is_some() {
                return Ok(());
            }

            if *height >= target_height {
                truncation_height = Some(*height);
            }

            Ok(())
        })
        .expect("infallible");
    if let Some(h) = truncation_height {
        match tree.rollback_to_checkpoint(h)? {
            RollbackOutcome::RolledBack => (),
            RollbackOutcome::NoSuchCheckpoint => panic!("checkpoint must exist in this scope"),
        }
    }

    Ok(())
}

/// Processes mempool transaction.
///
/// Scan the transaction and add to the wallet if relevant.
async fn process_mempool_transaction<W>(
    consensus_parameters: &impl consensus::Parameters,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    wallet: &mut W,
    raw_transaction: RawTransaction,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    // does not use raw transaction height due to a legacy-indexer off-by-one bug and potential to be zero
    let mempool_height = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .last_known_chain_height()
        .expect("wallet height must exist after sync is initialised")
        + 1;

    let transaction = zcash_primitives::transaction::Transaction::read(
        &raw_transaction.data[..],
        consensus::BranchId::for_height(consensus_parameters, mempool_height),
    )
    .map_err(ServerError::InvalidTransaction)?;

    tracing::debug!(
        "mempool received txid {} at height {}",
        transaction.txid(),
        mempool_height
    );

    if let Some(tx) = wallet
        .get_wallet_transactions_mut()
        .map_err(SyncError::WalletError)?
        .get_mut(&transaction.txid())
    {
        // a `Failed` transaction observed in the mempool is demonstrably not failed. fall through
        // to re-scan it, restoring its status and re-marking its spends which were reset when it
        // was marked failed.
        if !tx.status().is_failed() {
            tx.update_status(
                ConfirmationStatus::Mempool(mempool_height),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .expect("infalliable for such long time periods")
                    .as_secs() as u32,
                false,
            );

            return Ok(());
        }
    }

    scan_pending_transaction(
        consensus_parameters,
        ufvks,
        wallet,
        transaction,
        ConfirmationStatus::Mempool(mempool_height),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("infalliable for such long time periods")
            .as_secs() as u32,
    )?;

    Ok(())
}

/// Removes wallet blocks, transactions, nullifiers, outpoints and shard tree data above the given `truncate_height`.
///
/// The decision of what a correct truncation does is made purely by
/// [`truncate::plan_truncation`] from the wallet state, the shard-tree
/// state, and the truncation target. This function only applies the
/// returned plan.
fn truncate_wallet_data<W>(
    wallet: &mut W,
    truncate_height: BlockHeight,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    let wallet_state = truncate::WalletTruncationState {
        birthday: sync_state
            .wallet_birthday()
            .expect("should be non-empty in this scope"),
        highest_scanned_height: sync_state
            .highest_scanned_height()
            .expect("should be non-empty in this scope"),
    };
    match truncate::plan_truncation(wallet_state, truncate_height) {
        truncate::TruncationPlan::NoOp => Ok(()),
        truncate::TruncationPlan::ClearAll => {
            truncate_stores(wallet, consensus::H0, true)?;
            wallet.clear_shard_trees()
        }
        truncate::TruncationPlan::Truncate { height } => {
            truncate_stores(wallet, height, true)?;
            match wallet.truncate_shard_trees(height) {
                Ok(()) => Ok(()),
                Err(SyncError::TruncationError(height, pooltype)) => {
                    clear_wallet_data(wallet)?;

                    Err(SyncError::TruncationError(height, pooltype))
                }
                Err(e) => Err(e),
            }
        }
    }
}

/// Removes wallet blocks, transactions, nullifiers and outpoints above the
/// given `truncate_height`.
///
/// If `set_truncated_transactions_failed` is set, transactions will not be removed from the wallet but their status
/// will be updated to `Failed`.
fn truncate_stores<W>(
    wallet: &mut W,
    truncate_height: BlockHeight,
    set_truncated_transactions_failed: bool,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints,
{
    wallet
        .truncate_wallet_blocks(truncate_height)
        .map_err(SyncError::WalletError)?;
    wallet
        .truncate_wallet_transactions(truncate_height, set_truncated_transactions_failed)
        .map_err(SyncError::WalletError)?;
    wallet
        .truncate_nullifiers(truncate_height)
        .map_err(SyncError::WalletError)?;
    wallet
        .truncate_outpoints(truncate_height)
        .map_err(SyncError::WalletError)?;

    Ok(())
}

fn clear_wallet_data<W>(wallet: &mut W) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    let scan_targets = wallet
        .get_wallet_transactions()
        .map_err(SyncError::WalletError)?
        .values()
        .filter_map(|transaction| {
            transaction
                .status()
                .get_confirmed_height()
                .map(|height| ScanTarget {
                    block_height: height,
                    txid: transaction.txid(),
                    narrow_scan_area: true,
                })
        })
        .collect::<Vec<_>>();
    truncate_wallet_data(wallet, consensus::H0)?;
    truncate_scan_ranges(
        consensus::H0,
        wallet
            .get_sync_state_mut()
            .map_err(SyncError::WalletError)?,
    );
    wallet
        .get_wallet_transactions_mut()
        .map_err(SyncError::WalletError)?
        .clear();
    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    add_scan_targets(sync_state, &scan_targets);
    wallet.set_save_flag().map_err(SyncError::WalletError)?;

    Ok(())
}

/// Updates the wallet with data from `scan_results`
#[allow(clippy::too_many_arguments)]
async fn update_wallet_data<W>(
    consensus_parameters: &impl consensus::Parameters,
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_range: &ScanRange,
    nullifiers: Option<&mut NullifierMap>,
    outpoints: Option<&mut BTreeMap<OutputId, ScanTarget>>,
    mut transactions: HashMap<TxId, WalletTransaction>,
    sapling_located_trees: Vec<LocatedTreeData<sapling_crypto::Node>>,
    orchard_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
    ironwood_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees + Send,
{
    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    let highest_scanned_height = sync_state
        .highest_scanned_height()
        .expect("scan ranges should not be empty in this scope");
    for transaction in transactions.values() {
        state::update_found_note_shard_priority(
            consensus_parameters,
            sync_state,
            ShieldedPool::Sapling,
            transaction,
        );
        state::update_found_note_shard_priority(
            consensus_parameters,
            sync_state,
            ShieldedPool::Orchard,
            transaction,
        );
        state::update_found_note_shard_priority(
            consensus_parameters,
            sync_state,
            ShieldedPool::Ironwood,
            transaction,
        );
    }
    // add all block ranges of scan ranges with `ScannedWithoutMapping` or `RefetchingNullifiers` priority above the
    // current scan range to each note to track which ranges need the nullifiers to be re-fetched before the note is
    // known to be unspent (in addition to all other ranges above the notes height being `Scanned`,
    // `ScannedWithoutMapping` or `RefetchingNullifiers` priority). this information is necessary as these ranges have been scanned but the
    // nullifiers have been discarded so must be re-fetched. if ranges are scanned but the nullifiers are discarded
    // (set to `ScannedWithoutMapping` priority) *after* this note has been added to the wallet, this is sufficient to
    // know this note has not been spent, even if this range is not set to `Scanned` priority.
    let refetch_nullifier_ranges = {
        let block_ranges: Vec<Range<BlockHeight>> = sync_state
            .scan_ranges()
            .iter()
            .filter(|&scan_range| {
                scan_range.priority() == ScanPriority::ScannedWithoutMapping
                    || scan_range.priority() == ScanPriority::RefetchingNullifiers
            })
            .map(|scan_range| scan_range.block_range().clone())
            .collect();

        block_ranges
            [block_ranges.partition_point(|range| range.start < scan_range.block_range().end)..]
            .to_vec()
    };
    for transaction in transactions.values_mut() {
        for note in transaction.sapling_notes.as_mut_slice() {
            note.refetch_nullifier_ranges = refetch_nullifier_ranges.clone();
        }
        for note in transaction.orchard_notes.as_mut_slice() {
            note.refetch_nullifier_ranges = refetch_nullifier_ranges.clone();
        }
        for note in transaction.ironwood_notes.as_mut_slice() {
            note.refetch_nullifier_ranges = refetch_nullifier_ranges.clone();
        }
    }
    for transaction in transactions.values() {
        discover_unified_addresses(wallet, ufvks, transaction).map_err(SyncError::WalletError)?;
    }

    wallet
        .extend_wallet_transactions(transactions)
        .map_err(SyncError::WalletError)?;
    if let Some(nullifiers) = nullifiers {
        wallet
            .append_nullifiers(nullifiers)
            .map_err(SyncError::WalletError)?;
    }
    if let Some(outpoints) = outpoints {
        wallet
            .append_outpoints(outpoints)
            .map_err(SyncError::WalletError)?;
    }
    wallet
        .update_shard_trees(
            fetch_request_sender,
            scan_range,
            highest_scanned_height,
            sapling_located_trees,
            orchard_located_trees,
            ironwood_located_trees,
        )
        .await?;

    Ok(())
}

fn discover_unified_addresses<W>(
    wallet: &mut W,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    transaction: &WalletTransaction,
) -> Result<(), W::Error>
where
    W: SyncWallet,
{
    for note in transaction
        .orchard_notes()
        .iter()
        .filter(|&note| note.key_id().scope == zip32::Scope::External)
    {
        let ivk = ufvks
            .get(&note.key_id().account_id())
            .expect("ufvk must exist to decrypt this note")
            .orchard()
            .expect("fvk must exist to decrypt this note")
            .to_ivk(zip32::Scope::External);

        wallet.add_orchard_address(
            note.key_id().account_id(),
            note.note().recipient(),
            ivk.diversifier_index(&note.note().recipient())
                .expect("must be key used to create this address"),
        )?;
    }
    // Ironwood recipients are orchard receivers, discovered the same way.
    for note in transaction
        .ironwood_notes()
        .iter()
        .filter(|&note| note.key_id().scope == zip32::Scope::External)
    {
        let ivk = ufvks
            .get(&note.key_id().account_id())
            .expect("ufvk must exist to decrypt this note")
            .orchard()
            .expect("fvk must exist to decrypt this note")
            .to_ivk(zip32::Scope::External);

        wallet.add_orchard_address(
            note.key_id().account_id(),
            note.note().recipient(),
            ivk.diversifier_index(&note.note().recipient())
                .expect("must be key used to create this address"),
        )?;
    }
    for note in transaction
        .sapling_notes()
        .iter()
        .filter(|&note| note.key_id().scope == zip32::Scope::External)
    {
        let ivk = ufvks
            .get(&note.key_id().account_id())
            .expect("ufvk must exist to decrypt this note")
            .sapling()
            .expect("fvk must exist to decrypt this note")
            .to_external_ivk();

        wallet.add_sapling_address(
            note.key_id().account_id(),
            note.note().recipient(),
            ivk.decrypt_diversifier(&note.note().recipient())
                .expect("must be key used to create this address"),
        )?;
    }

    Ok(())
}

fn remove_irrelevant_data<W>(wallet: &mut W) -> Result<(), W::Error>
where
    W: SyncWallet + SyncBlocks + SyncOutPoints + SyncNullifiers + SyncTransactions,
{
    let fully_scanned_height = wallet
        .get_sync_state()?
        .fully_scanned_height()
        .expect("scan ranges must be non-empty");

    wallet
        .get_outpoints_mut()?
        .retain(|_, scan_target| scan_target.block_height > fully_scanned_height);
    wallet
        .get_nullifiers_mut()?
        .sapling
        .retain(|_, scan_target| scan_target.block_height > fully_scanned_height);
    wallet
        .get_nullifiers_mut()?
        .orchard
        .retain(|_, scan_target| scan_target.block_height > fully_scanned_height);
    wallet
        .get_nullifiers_mut()?
        .ironwood
        .retain(|_, scan_target| scan_target.block_height > fully_scanned_height);
    wallet
        .get_sync_state_mut()?
        .scan_targets
        .retain(|scan_target| scan_target.block_height > fully_scanned_height);
    remove_irrelevant_blocks(wallet)?;

    Ok(())
}

fn remove_irrelevant_blocks<W>(wallet: &mut W) -> Result<(), W::Error>
where
    W: SyncWallet + SyncBlocks + SyncTransactions,
{
    let sync_state = wallet.get_sync_state()?;
    let highest_scanned_height = sync_state
        .highest_scanned_height()
        .expect("should be non-empty");
    let scanned_range_bounds = sync_state
        .scan_ranges()
        .iter()
        .filter(|scan_range| scan_range.priority().is_scanned())
        .flat_map(|scanned_range| {
            vec![
                scanned_range.block_range().start,
                scanned_range.block_range().end - 1,
            ]
        })
        .collect::<Vec<_>>();
    let wallet_transaction_heights = wallet
        .get_wallet_transactions()?
        .values()
        .filter_map(|tx| tx.status().get_confirmed_height())
        .collect::<Vec<_>>();

    wallet.get_wallet_blocks_mut()?.retain(|height, _| {
        *height >= highest_scanned_height.saturating_sub(MAX_REORG_ALLOWANCE)
            || scanned_range_bounds.contains(height)
            || wallet_transaction_heights.contains(height)
    });

    Ok(())
}

fn add_scanned_blocks<W>(
    wallet: &mut W,
    mut scanned_blocks: BTreeMap<BlockHeight, WalletBlock>,
    scan_range: &ScanRange,
) -> Result<(), W::Error>
where
    W: SyncWallet + SyncBlocks + SyncTransactions,
{
    let sync_state = wallet.get_sync_state()?;
    let highest_scanned_height = sync_state
        .highest_scanned_height()
        .expect("scan ranges must be non-empty");

    let wallet_transaction_heights = wallet
        .get_wallet_transactions()?
        .values()
        .filter_map(|tx| tx.status().get_confirmed_height())
        .collect::<Vec<_>>();

    scanned_blocks.retain(|height, _| {
        *height >= highest_scanned_height.saturating_sub(MAX_REORG_ALLOWANCE)
            || *height == scan_range.block_range().start
            || *height == scan_range.block_range().end - 1
            || wallet_transaction_heights.contains(height)
    });

    wallet.append_wallet_blocks(scanned_blocks)?;

    Ok(())
}

async fn update_subtree_roots<W>(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    wallet: &mut W,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncShardTrees,
{
    // Resume from the stored-root count, except that a newest root which
    // is still bare (never scanned into) is refetched every session: no
    // checkpoint witnesses it, so this refetch is the only mechanism that
    // heals it after a reorg (see `subtree_fetch_start_index`). When a
    // refetch happens, the pool's newest stored shard range is dropped
    // before the fetched roots are accounted, so the range accounting is
    // rebuilt from the refetched root, never duplicated, and corrected
    // if a reorg moved the subtree's completing height.
    let shard_trees = wallet.get_shard_trees().map_err(SyncError::WalletError)?;
    let stored_sapling_roots = witness::stored_subtree_root_count(&shard_trees.sapling);
    let stored_orchard_roots = witness::stored_subtree_root_count(&shard_trees.orchard);
    let stored_ironwood_roots = witness::stored_subtree_root_count(&shard_trees.ironwood);
    let sapling_start_index = witness::subtree_fetch_start_index(&shard_trees.sapling);
    let orchard_start_index = witness::subtree_fetch_start_index(&shard_trees.orchard);
    let ironwood_start_index = witness::subtree_fetch_start_index(&shard_trees.ironwood);
    let (sapling_subtree_roots, orchard_subtree_roots, ironwood_subtree_roots) = futures::join!(
        client::get_subtree_roots(fetch_request_sender.clone(), sapling_start_index, 0, 0),
        client::get_subtree_roots(fetch_request_sender.clone(), orchard_start_index, 1, 0),
        client::get_subtree_roots(fetch_request_sender, ironwood_start_index, 2, 0)
    );

    let sapling_subtree_roots = sapling_subtree_roots?;
    let orchard_subtree_roots = orchard_subtree_roots?;
    // Ironwood subtree roots are requested only where NU6.3 exists, and a
    // server that cannot serve them is tolerated: the shard ranges remain
    // empty and scan prioritisation falls back to the whole-pool range.
    let ironwood_subtree_roots = if consensus_parameters
        .activation_height(consensus::NetworkUpgrade::Nu6_3)
        .is_some()
    {
        ironwood_subtree_roots.unwrap_or_else(|e| {
            tracing::debug!("server does not serve ironwood subtree roots: {e}");
            Vec::new()
        })
    } else {
        Vec::new()
    };

    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    if (sapling_start_index as usize) < stored_sapling_roots && !sapling_subtree_roots.is_empty() {
        state::pop_newest_shard_range(sync_state, ShieldedPool::Sapling);
    }
    state::add_shard_ranges(
        consensus_parameters,
        ShieldedPool::Sapling,
        sync_state,
        &sapling_subtree_roots,
    );
    if (orchard_start_index as usize) < stored_orchard_roots && !orchard_subtree_roots.is_empty() {
        state::pop_newest_shard_range(sync_state, ShieldedPool::Orchard);
    }
    state::add_shard_ranges(
        consensus_parameters,
        ShieldedPool::Orchard,
        sync_state,
        &orchard_subtree_roots,
    );
    if !ironwood_subtree_roots.is_empty() {
        if (ironwood_start_index as usize) < stored_ironwood_roots {
            state::pop_newest_shard_range(sync_state, ShieldedPool::Ironwood);
        }
        state::add_shard_ranges(
            consensus_parameters,
            ShieldedPool::Ironwood,
            sync_state,
            &ironwood_subtree_roots,
        );
    }

    let shard_trees = wallet
        .get_shard_trees_mut()
        .map_err(SyncError::WalletError)?;
    witness::add_subtree_roots(
        sapling_start_index as usize,
        sapling_subtree_roots,
        &mut shard_trees.sapling,
    )?;
    witness::add_subtree_roots(
        orchard_start_index as usize,
        orchard_subtree_roots,
        &mut shard_trees.orchard,
    )?;
    witness::add_subtree_roots(
        ironwood_start_index as usize,
        ironwood_subtree_roots,
        &mut shard_trees.ironwood,
    )?;
    wallet.set_save_flag().map_err(SyncError::WalletError)?;

    Ok(())
}

async fn add_initial_frontier<W>(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    wallet: &mut W,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncShardTrees,
{
    let birthday = wallet.get_birthday().map_err(SyncError::WalletError)?;
    if birthday
        == consensus_parameters
            .activation_height(consensus::NetworkUpgrade::Sapling)
            .expect("sapling activation height should always return Some")
    {
        return Ok(());
    }

    // if the shard store only contains the first checkpoint added on initialisation, add frontiers to complete the
    // shard trees.
    let shard_trees = wallet
        .get_shard_trees_mut()
        .map_err(SyncError::WalletError)?;
    if shard_trees
        .sapling
        .store()
        .checkpoint_count()
        .expect("infallible")
        == 1
    {
        let frontiers = client::get_frontiers(fetch_request_sender, birthday).await?;
        shard_trees
            .sapling
            .insert_frontier(
                frontiers.final_sapling_tree().clone(),
                Retention::Checkpoint {
                    id: birthday,
                    marking: Marking::None,
                },
            )
            .expect("infallible");
        shard_trees
            .orchard
            .insert_frontier(
                frontiers.final_orchard_tree().clone(),
                Retention::Checkpoint {
                    id: birthday,
                    marking: Marking::None,
                },
            )
            .expect("infallible");
        shard_trees
            .ironwood
            .insert_frontier(
                frontiers.final_ironwood_tree().clone(),
                Retention::Checkpoint {
                    id: birthday,
                    marking: Marking::None,
                },
            )
            .expect("infallible");
        wallet.set_save_flag().map_err(SyncError::WalletError)?;
    }

    Ok(())
}

/// Sets up mempool stream.
///
/// If there is some raw transaction, send to be scanned.
/// If the mempool stream message is `None` (a block was mined) or the request failed, setup a new mempool stream.
async fn mempool_monitor<C>(
    mut client: C,
    mempool_transaction_sender: mpsc::Sender<RawTransaction>,
    unprocessed_transactions_count: Arc<AtomicU32>,
    stream_connected_at: Arc<std::sync::OnceLock<std::time::Instant>>,
    shutdown_mempool: Arc<AtomicBool>,
) -> Result<(), MempoolError>
where
    C: Clone + Indexer + TransparentIndexer + Sync + Send + 'static,
{
    // The tick only bounds how quickly the monitor notices the shutdown
    // flag; sync() joins this task at session end, so the tick interval
    // is paid on the critical path of every sync session.
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    'main: loop {
        let response =
            client::get_mempool_transaction_stream(&mut client, shutdown_mempool.clone()).await;

        match response {
            Ok(mut mempool_stream) => {
                // First successful subscription: the drain policy's
                // grace window keys on this instant. Deliberately not
                // reset on reconnect, since any successful connect
                // proves the indexer serves the stream.
                let _already_set = stream_connected_at.set(std::time::Instant::now());
                interval.reset();
                loop {
                    tokio::select! {
                        mempool_stream_message = mempool_stream.message() => {
                            match mempool_stream_message.unwrap_or(None) {
                                Some(raw_transaction) => {
                                     match mempool_transaction_sender
                                        .send(raw_transaction)
                                        .await {
                                            Ok(_) => {
                                                unprocessed_transactions_count.fetch_add(1, atomic::Ordering::Release);
                                            }
                                            Err(_) => {
                                                unprocessed_transactions_count.store(0, atomic::Ordering::Release);
                                                shutdown_mempool.store(true, atomic::Ordering::Release);
                                                break 'main;
                                            }
                                        }
                                }
                                None => {
                                    continue 'main;
                                }
                            }

                        }

                        _ = interval.tick() => {
                            if shutdown_mempool.load(atomic::Ordering::Acquire) {
                                break 'main;
                            }
                        }
                    }
                }
            }
            Err(e @ MempoolError::ShutdownWithoutStream) => return Err(e),
            Err(MempoolError::ServerError(e)) => {
                tracing::warn!(
                    "Mempool stream request failed! Status: {}.\nRetrying...",
                    crate::error::cause_chain_text(&e)
                );
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }

    Ok(())
}

/// Transaction status will be set to `Failed` if it's still unconfirmed when the chain reaches it's expiry height.
///
/// Transactions with an expiry height of 0 never expire (ZIP-203).
///
/// Must only be called after all blocks up to the wallet's last known chain height have been scanned, otherwise a
/// transaction mined near its expiry height would be marked `Failed` before the block containing it is scanned.
fn expire_transactions<W>(wallet: &mut W) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncTransactions,
{
    let last_known_chain_height = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .last_known_chain_height()
        .expect("wallet height must exist after scan ranges have been updated");
    let wallet_transactions = wallet
        .get_wallet_transactions_mut()
        .map_err(SyncError::WalletError)?;

    let expired_txids = wallet_transactions
        .values()
        .filter(|transaction| {
            let expiry_height = transaction.transaction().expiry_height();
            transaction.status().is_pending()
                && expiry_height > BlockHeight::from_u32(0)
                && last_known_chain_height >= expiry_height
        })
        .map(super::wallet::WalletTransaction::txid)
        .collect::<Vec<_>>();
    set_transactions_failed(wallet_transactions, expired_txids);
    wallet.set_save_flag().map_err(SyncError::WalletError)?;

    Ok(())
}

fn max_nullifier_map_size(performance_level: PerformanceLevel) -> Option<usize> {
    match performance_level {
        PerformanceLevel::Low => Some(0),
        PerformanceLevel::Medium => Some(125_000),
        PerformanceLevel::High => Some(2_000_000),
        PerformanceLevel::Maximum => None,
    }
}

#[cfg(test)]
mod test {
    /// The completion contract of [`crate::sync::SyncStatus::is_complete`]:
    /// completion is the sync task's own terminal condition (sync has
    /// started and every scan range is `Scanned`), independent of the
    /// output ratio, so an output-free birthday-to-chain-height range can
    /// complete and a stale `total_outputs` cannot fake completion.
    mod sync_status_completion {
        use zcash_protocol::consensus::BlockHeight;

        use crate::sync::{ScanPriority, ScanRange, SyncStatus};

        /// Builds a status with the given start height and scan-range
        /// priorities. Every counter stays zero: completion must be
        /// decided by the scan ranges alone, never by the output ratio.
        fn status(sync_start_height: u32, priorities: &[ScanPriority]) -> SyncStatus {
            let scan_ranges = priorities
                .iter()
                .enumerate()
                .map(|(index, priority)| {
                    let start = 1_000 + 10 * index as u32;
                    ScanRange::from_parts(
                        BlockHeight::from(start)..BlockHeight::from(start + 10),
                        *priority,
                    )
                })
                .collect();
            SyncStatus {
                scan_ranges,
                sync_start_height: sync_start_height.into(),
                session_blocks_scanned: 0,
                total_blocks_scanned: 0,
                percentage_session_blocks_scanned: 0.0,
                percentage_total_blocks_scanned: 0.0,
                session_sapling_outputs_scanned: 0,
                total_sapling_outputs_scanned: 0,
                session_orchard_outputs_scanned: 0,
                total_orchard_outputs_scanned: 0,
                session_ironwood_outputs_scanned: 0,
                total_ironwood_outputs_scanned: 0,
                percentage_session_outputs_scanned: 0.0,
                percentage_total_outputs_scanned: 0.0,
                total_outputs_scanned: 0,
                total_outputs: 0,
            }
        }

        #[test]
        fn never_started_is_not_complete() {
            assert!(!status(0, &[]).is_complete());
        }

        /// A started sync with no scan ranges left in the state is
        /// vacuously complete: nothing remains tracked, so nothing
        /// awaits scanning or nullifier work.
        #[test]
        fn started_with_no_scan_ranges_is_complete() {
            assert!(status(1_000, &[]).is_complete());
        }

        #[test]
        fn all_ranges_scanned_is_complete() {
            assert!(status(1_000, &[ScanPriority::Scanned, ScanPriority::Scanned]).is_complete());
        }

        /// The empty-range edge: zero shielded outputs from birthday to
        /// chain height must not read as incomplete once the sync task
        /// has run to its terminal state.
        #[test]
        fn output_free_range_is_complete() {
            let status = status(1_000, &[ScanPriority::Scanned]);
            assert_eq!(status.total_outputs, 0);
            assert!(status.is_complete());
        }

        #[test]
        fn nullifier_retrieval_pending_is_not_complete() {
            for pending in [
                ScanPriority::ScannedWithoutMapping,
                ScanPriority::RefetchingNullifiers,
            ] {
                assert!(!status(1_000, &[ScanPriority::Scanned, pending]).is_complete());
            }
        }

        /// The stale-denominator edge: an unscanned range keeps the
        /// status incomplete even if the output counters claim the
        /// initially-computed target was reached.
        #[test]
        fn unscanned_range_is_not_complete() {
            let mut status = status(1_000, &[ScanPriority::Scanned, ScanPriority::Historic]);
            status.total_outputs_scanned = 10_000;
            status.total_outputs = 10_000;
            assert!(!status.is_complete());
        }
    }

    /// The truncation contract of [`crate::sync::truncate_wallet_data`]:
    /// a reorg truncation rolls every store back to the truncate height,
    /// and a shard tree that records nothing above that height (such as
    /// the empty ironwood tree a pre-ironwood (v0) wallet blob migrates
    /// to) is untouched, never classified broken. Only a tree that
    /// holds state above the height and cannot roll back to it forces
    /// the clear-and-rescan path.
    mod truncation {
        use std::collections::BTreeMap;

        use zcash_primitives::block::BlockHash;
        use zcash_protocol::consensus::BlockHeight;

        use crate::mocks::MockWalletBuilder;
        use crate::shardtree_ext::{CheckpointAppendOutcome, ShardTreeExt};
        use crate::sync::{ScanPriority, ScanRange, truncate_wallet_data};
        use crate::wallet::{ShardTrees, SyncState, TreeBounds, WalletBlock, traits::SyncBlocks};

        /// A wallet block carrying only what truncation reads: its height.
        fn block(height: u32) -> WalletBlock {
            WalletBlock {
                block_height: BlockHeight::from_u32(height),
                block_hash: BlockHash([0; 32]),
                prev_hash: BlockHash([0; 32]),
                time: 0,
                txids: Vec::new(),
                tree_bounds: TreeBounds {
                    sapling_initial_tree_size: 0,
                    sapling_final_tree_size: 0,
                    orchard_initial_tree_size: 0,
                    orchard_final_tree_size: 0,
                    ironwood_initial_tree_size: 0,
                    ironwood_final_tree_size: 0,
                },
            }
        }

        /// A wallet synced through height 10 with a birthday of 6: one
        /// fully scanned range, one wallet block per scanned height, and
        /// the given shard trees.
        fn synced_wallet(shard_trees: ShardTrees) -> crate::mocks::MockWallet {
            let wallet_blocks: BTreeMap<_, _> = (6..=10u32)
                .map(|height| (BlockHeight::from_u32(height), block(height)))
                .collect();
            let sync_state = SyncState::new_for_test(vec![ScanRange::from_parts(
                BlockHeight::from_u32(6)..BlockHeight::from_u32(11),
                ScanPriority::Scanned,
            )]);
            MockWalletBuilder::new()
                .birthday(BlockHeight::from_u32(6))
                .sync_state(sync_state)
                .wallet_blocks(wallet_blocks)
                .shard_trees(shard_trees)
                .create_mock_wallet()
        }

        /// A pre-ironwood (v0) wallet blob deserializes to an ironwood
        /// tree holding only the initialization checkpoint at height
        /// zero (`ShardTrees::read`), while sapling and orchard carry
        /// the checkpoints of past scanning. The first routine reorg
        /// truncation after the upgrade must roll sapling and orchard
        /// back and leave the empty ironwood tree untouched, not
        /// classify it broken and wipe the wallet.
        #[test]
        fn migrated_wallet_survives_reorg_truncation() {
            // Sapling and orchard hold checkpoints for the scanned
            // heights; the ironwood tree stays exactly as
            // `ShardTrees::new` built it (its height-zero initialization
            // checkpoint and nothing else), which is also the state
            // `ShardTrees::read` produces for a pre-ironwood blob.
            let mut shard_trees = ShardTrees::new();
            for height in 6..=10u32 {
                assert_eq!(
                    shard_trees
                        .sapling
                        .append_checkpoint(BlockHeight::from_u32(height))
                        .unwrap(),
                    CheckpointAppendOutcome::Appended
                );
                assert_eq!(
                    shard_trees
                        .orchard
                        .append_checkpoint(BlockHeight::from_u32(height))
                        .unwrap(),
                    CheckpointAppendOutcome::Appended
                );
            }
            let mut wallet = synced_wallet(shard_trees);

            // A routine two-block reorg rolls the wallet back to height 8.
            let result = truncate_wallet_data(&mut wallet, BlockHeight::from_u32(8));

            assert!(
                result.is_ok(),
                "reorg truncation wiped a healthy migrated wallet: {result:?}"
            );
            // The blocks at and below the truncate height survive; the
            // clear-and-rescan path leaves none.
            assert!(wallet.get_wallet_block(BlockHeight::from_u32(6)).is_ok());
            assert!(wallet.get_wallet_block(BlockHeight::from_u32(8)).is_ok());
            assert!(wallet.get_wallet_block(BlockHeight::from_u32(9)).is_err());
        }

        /// A tree that records state above the truncate height but holds
        /// no checkpoint at it cannot roll back. The established recovery
        /// (clear all wallet data and rescan) is preserved for it.
        #[test]
        fn unrecoverable_tree_still_clears_wallet_data() {
            // Orchard scanned past the target but its checkpoint at the
            // target height is gone (e.g. pruned): checkpoints exist only
            // above it.
            let mut shard_trees = ShardTrees::new();
            for height in 6..=10u32 {
                assert_eq!(
                    shard_trees
                        .sapling
                        .append_checkpoint(BlockHeight::from_u32(height))
                        .unwrap(),
                    CheckpointAppendOutcome::Appended
                );
            }
            for height in 9..=10u32 {
                assert_eq!(
                    shard_trees
                        .orchard
                        .append_checkpoint(BlockHeight::from_u32(height))
                        .unwrap(),
                    CheckpointAppendOutcome::Appended
                );
            }
            let mut wallet = synced_wallet(shard_trees);

            let result = truncate_wallet_data(&mut wallet, BlockHeight::from_u32(8));

            assert!(matches!(
                result,
                Err(crate::error::SyncError::TruncationError(_, _))
            ));
            // The wallet was cleared for rescan.
            assert!(wallet.get_wallet_block(BlockHeight::from_u32(6)).is_err());
        }
    }

    /// The pool-accounting contract of [`crate::sync::sync_status`]:
    /// every output total on the status (the per-pool u32 fields, the
    /// u64 exact-ratio pair, and the f32 percentage) describes the
    /// same per-pool trio, summed once through `output_pool_total`.
    /// Regression for the skew where the u64 fields re-summed only
    /// sapling and orchard while the percentages included ironwood.
    /// Each pool contributes a distinct count, so dropping any pool
    /// from any consumer changes an asserted value.
    mod sync_status_pool_accounting {
        use std::collections::BTreeMap;

        use zcash_primitives::block::BlockHash;
        use zcash_protocol::consensus::BlockHeight;

        use crate::mocks::MockWalletBuilder;
        use crate::sync::{ScanPriority, ScanRange, sync_status};
        use crate::wallet::{SyncState, TreeBounds, WalletBlock};

        /// A wallet block carrying only what tree-bounds accounting
        /// reads: its height and tree sizes.
        fn block(height: u32, tree_bounds: TreeBounds) -> WalletBlock {
            WalletBlock {
                block_height: BlockHeight::from_u32(height),
                block_hash: BlockHash([0; 32]),
                prev_hash: BlockHash([0; 32]),
                time: 0,
                txids: Vec::new(),
                tree_bounds,
            }
        }

        /// Tree bounds whose initial and final sizes coincide, for
        /// blocks that only mark a boundary of a scanned range.
        fn flat_bounds(sapling: u32, orchard: u32, ironwood: u32) -> TreeBounds {
            TreeBounds {
                sapling_initial_tree_size: sapling,
                sapling_final_tree_size: sapling,
                orchard_initial_tree_size: orchard,
                orchard_final_tree_size: orchard,
                ironwood_initial_tree_size: ironwood,
                ironwood_final_tree_size: ironwood,
            }
        }

        #[tokio::test]
        async fn exact_ratio_fields_agree_with_per_pool_totals() {
            // One fully scanned range whose boundary blocks yield a
            // distinct scanned count per pool: sapling 3, orchard 5,
            // ironwood 7.
            let mut sync_state = SyncState::new_for_test(vec![ScanRange::from_parts(
                BlockHeight::from_u32(1_000)..BlockHeight::from_u32(1_010),
                ScanPriority::Scanned,
            )]);
            sync_state.initial_sync_state.sync_start_height = BlockHeight::from_u32(1_000);
            // Session denominators: 10 sapling + 20 orchard + 40
            // ironwood outputs between the wallet bounds, 70 in all.
            sync_state.initial_sync_state.wallet_tree_bounds = TreeBounds {
                sapling_initial_tree_size: 100,
                sapling_final_tree_size: 110,
                orchard_initial_tree_size: 200,
                orchard_final_tree_size: 220,
                ironwood_initial_tree_size: 300,
                ironwood_final_tree_size: 340,
            };
            sync_state
                .initial_sync_state
                .previously_scanned_sapling_outputs = 1;
            sync_state
                .initial_sync_state
                .previously_scanned_orchard_outputs = 2;
            sync_state
                .initial_sync_state
                .previously_scanned_ironwood_outputs = 3;

            let wallet_blocks = BTreeMap::from([
                (
                    BlockHeight::from_u32(1_000),
                    block(1_000, flat_bounds(100, 200, 300)),
                ),
                (
                    BlockHeight::from_u32(1_009),
                    block(1_009, flat_bounds(103, 205, 307)),
                ),
            ]);

            let wallet = MockWalletBuilder::new()
                .sync_state(sync_state)
                .wallet_blocks(wallet_blocks)
                .create_mock_wallet();

            let status = sync_status(&wallet).await.unwrap();

            assert_eq!(status.total_sapling_outputs_scanned, 3);
            assert_eq!(status.total_orchard_outputs_scanned, 5);
            assert_eq!(status.total_ironwood_outputs_scanned, 7);

            // The regression proper: the u64 exact-ratio fields must
            // equal the sum of the per-pool fields reported beside
            // them. Under the skew, total_outputs_scanned was 8 (no
            // ironwood) and total_outputs was 30.
            assert_eq!(
                status.total_outputs_scanned,
                u64::from(status.total_sapling_outputs_scanned)
                    + u64::from(status.total_orchard_outputs_scanned)
                    + u64::from(status.total_ironwood_outputs_scanned),
            );
            assert_eq!(status.total_outputs_scanned, 15);
            assert_eq!(status.total_outputs, 70);

            // The percentage must describe the same ratio as the exact
            // fields. The skew's observable symptom was these two
            // disagreeing on ironwood chains.
            let expected_percentage =
                (status.total_outputs_scanned as f32 / status.total_outputs as f32) * 100.0;
            assert!(
                (status.percentage_total_outputs_scanned - expected_percentage).abs()
                    < f32::EPSILON,
                "percentage {} disagrees with the exact ratio {}",
                status.percentage_total_outputs_scanned,
                expected_percentage,
            );
        }
    }

    /// The session-progress contract of [`crate::sync::sync_status`]
    /// under a stale initial sync state, whose recorded
    /// previously-scanned counts stand at or above the whole span the
    /// wallet can scan, saturating the session denominator to zero and
    /// obliging the status to report the wallet's total progress
    /// rather than claim completion.
    mod sync_status_stale_initial_state {
        use std::collections::BTreeMap;

        use zcash_primitives::block::BlockHash;
        use zcash_protocol::consensus::BlockHeight;

        use crate::mocks::MockWalletBuilder;
        use crate::sync::{ScanPriority, ScanRange, sync_status};
        use crate::wallet::{SyncState, TreeBounds, WalletBlock};

        /// The scale a ratio is reported on.
        const PERCENTAGE_SCALE: f32 = 100.0;
        /// The percentage a finished sync reports.
        const COMPLETE_PERCENTAGE: f32 = PERCENTAGE_SCALE;

        /// The wallet birthday, and so the first height of the span.
        const BIRTHDAY_HEIGHT: u32 = 1_000;
        /// The number of blocks between the birthday and the chain tip.
        const TOTAL_BLOCKS: u32 = 10;
        /// The number of those blocks this wallet has scanned.
        const SCANNED_BLOCKS: u32 = 5;
        /// The height of the chain tip the wallet last knew.
        const CHAIN_TIP_HEIGHT: u32 = BIRTHDAY_HEIGHT + TOTAL_BLOCKS - 1;
        /// The first height beyond the scanned range.
        const SCANNED_RANGE_END: u32 = BIRTHDAY_HEIGHT + SCANNED_BLOCKS;
        /// The factor by which a stale count exceeds the true span.
        const STALENESS_FACTOR: u32 = 500;
        /// A previously-scanned count left over from a wider span.
        const STALE_PREVIOUSLY_SCANNED_BLOCKS: u32 = TOTAL_BLOCKS * STALENESS_FACTOR;

        /// The sapling tree size at the start of the wallet's span.
        const SAPLING_INITIAL_TREE_SIZE: u32 = 100;
        /// The sapling outputs the whole span holds.
        const SAPLING_TOTAL_OUTPUTS: u32 = 10;
        /// The sapling outputs the wallet has scanned.
        const SAPLING_SCANNED_OUTPUTS: u32 = 3;
        /// The orchard tree size at the start of the wallet's span.
        const ORCHARD_INITIAL_TREE_SIZE: u32 = 200;
        /// The orchard outputs the whole span holds.
        const ORCHARD_TOTAL_OUTPUTS: u32 = 20;
        /// The orchard outputs the wallet has scanned.
        const ORCHARD_SCANNED_OUTPUTS: u32 = 5;
        /// The ironwood tree size at the start of the wallet's span.
        const IRONWOOD_INITIAL_TREE_SIZE: u32 = 300;
        /// The ironwood outputs the whole span holds.
        const IRONWOOD_TOTAL_OUTPUTS: u32 = 40;
        /// The ironwood outputs the wallet has scanned.
        const IRONWOOD_SCANNED_OUTPUTS: u32 = 7;
        /// The outputs the whole span holds, across every pool.
        const TOTAL_OUTPUTS: u32 =
            SAPLING_TOTAL_OUTPUTS + ORCHARD_TOTAL_OUTPUTS + IRONWOOD_TOTAL_OUTPUTS;
        /// The outputs the wallet has scanned, across every pool.
        const SCANNED_OUTPUTS: u32 =
            SAPLING_SCANNED_OUTPUTS + ORCHARD_SCANNED_OUTPUTS + IRONWOOD_SCANNED_OUTPUTS;
        /// A previously-scanned output count left over from a wider span.
        const STALE_PREVIOUSLY_SCANNED_OUTPUTS: u32 = TOTAL_OUTPUTS * STALENESS_FACTOR;

        /// A wallet block carrying only what tree-bounds accounting
        /// reads: its height and tree sizes.
        fn block(height: u32, tree_bounds: TreeBounds) -> WalletBlock {
            WalletBlock {
                block_height: BlockHeight::from_u32(height),
                block_hash: BlockHash([0; 32]),
                prev_hash: BlockHash([0; 32]),
                time: 0,
                txids: Vec::new(),
                tree_bounds,
            }
        }

        /// Tree bounds whose initial and final sizes coincide, for
        /// blocks that only mark a boundary of a scanned range.
        fn flat_bounds(sapling: u32, orchard: u32, ironwood: u32) -> TreeBounds {
            TreeBounds {
                sapling_initial_tree_size: sapling,
                sapling_final_tree_size: sapling,
                orchard_initial_tree_size: orchard,
                orchard_final_tree_size: orchard,
                ironwood_initial_tree_size: ironwood,
                ironwood_final_tree_size: ironwood,
            }
        }

        /// A wallet holding a half-scanned span whose initial sync
        /// state records previously-scanned counts far above that
        /// span, the shape a truncated or rewound wallet leaves
        /// behind.
        fn stale_state_wallet() -> crate::mocks::MockWallet {
            let mut sync_state = SyncState::new_for_test(vec![
                ScanRange::from_parts(
                    BlockHeight::from_u32(BIRTHDAY_HEIGHT)
                        ..BlockHeight::from_u32(SCANNED_RANGE_END),
                    ScanPriority::Scanned,
                ),
                ScanRange::from_parts(
                    BlockHeight::from_u32(SCANNED_RANGE_END)
                        ..BlockHeight::from_u32(CHAIN_TIP_HEIGHT + 1),
                    ScanPriority::Historic,
                ),
            ]);
            sync_state.initial_sync_state.sync_start_height =
                BlockHeight::from_u32(BIRTHDAY_HEIGHT);
            sync_state.initial_sync_state.wallet_tree_bounds = TreeBounds {
                sapling_initial_tree_size: SAPLING_INITIAL_TREE_SIZE,
                sapling_final_tree_size: SAPLING_INITIAL_TREE_SIZE + SAPLING_TOTAL_OUTPUTS,
                orchard_initial_tree_size: ORCHARD_INITIAL_TREE_SIZE,
                orchard_final_tree_size: ORCHARD_INITIAL_TREE_SIZE + ORCHARD_TOTAL_OUTPUTS,
                ironwood_initial_tree_size: IRONWOOD_INITIAL_TREE_SIZE,
                ironwood_final_tree_size: IRONWOOD_INITIAL_TREE_SIZE + IRONWOOD_TOTAL_OUTPUTS,
            };
            sync_state.initial_sync_state.previously_scanned_blocks =
                STALE_PREVIOUSLY_SCANNED_BLOCKS;
            sync_state
                .initial_sync_state
                .previously_scanned_sapling_outputs = STALE_PREVIOUSLY_SCANNED_OUTPUTS;
            sync_state
                .initial_sync_state
                .previously_scanned_orchard_outputs = STALE_PREVIOUSLY_SCANNED_OUTPUTS;
            sync_state
                .initial_sync_state
                .previously_scanned_ironwood_outputs = STALE_PREVIOUSLY_SCANNED_OUTPUTS;

            let wallet_blocks = BTreeMap::from([
                (
                    BlockHeight::from_u32(BIRTHDAY_HEIGHT),
                    block(
                        BIRTHDAY_HEIGHT,
                        flat_bounds(
                            SAPLING_INITIAL_TREE_SIZE,
                            ORCHARD_INITIAL_TREE_SIZE,
                            IRONWOOD_INITIAL_TREE_SIZE,
                        ),
                    ),
                ),
                (
                    BlockHeight::from_u32(SCANNED_RANGE_END - 1),
                    block(
                        SCANNED_RANGE_END - 1,
                        flat_bounds(
                            SAPLING_INITIAL_TREE_SIZE + SAPLING_SCANNED_OUTPUTS,
                            ORCHARD_INITIAL_TREE_SIZE + ORCHARD_SCANNED_OUTPUTS,
                            IRONWOOD_INITIAL_TREE_SIZE + IRONWOOD_SCANNED_OUTPUTS,
                        ),
                    ),
                ),
            ]);

            MockWalletBuilder::new()
                .sync_state(sync_state)
                .wallet_blocks(wallet_blocks)
                .create_mock_wallet()
        }

        /// HYPOTHESIS: a session whose scannable span saturates to zero
        /// never reports a finished sync, so the falsifier is a status
        /// whose session block percentage reads complete while half the
        /// span is unscanned.
        #[tokio::test]
        async fn stale_block_state_reports_total_progress_not_completion() {
            let wallet = stale_state_wallet();

            let status = sync_status(&wallet).await.unwrap();

            let expected_total = (SCANNED_BLOCKS as f32 / TOTAL_BLOCKS as f32) * PERCENTAGE_SCALE;
            assert!(
                (status.percentage_total_blocks_scanned - expected_total).abs() < f32::EPSILON,
                "total block percentage {} disagrees with the scanned ratio {}",
                status.percentage_total_blocks_scanned,
                expected_total,
            );
            assert_ne!(
                status.percentage_session_blocks_scanned, COMPLETE_PERCENTAGE,
                "a session that scanned nothing reported a finished sync",
            );
            assert!(
                (status.percentage_session_blocks_scanned - expected_total).abs() < f32::EPSILON,
                "session block percentage {} disagrees with the total progress {}",
                status.percentage_session_blocks_scanned,
                expected_total,
            );
        }

        /// HYPOTHESIS: the output percentages obey the same rule as the
        /// block percentages, so the falsifier is a status whose
        /// session output percentage reads complete while most of the
        /// span's outputs are unscanned.
        #[tokio::test]
        async fn stale_output_state_reports_total_progress_not_completion() {
            let wallet = stale_state_wallet();

            let status = sync_status(&wallet).await.unwrap();

            let expected_total = (SCANNED_OUTPUTS as f32 / TOTAL_OUTPUTS as f32) * PERCENTAGE_SCALE;
            assert!(
                (status.percentage_total_outputs_scanned - expected_total).abs() < f32::EPSILON,
                "total output percentage {} disagrees with the scanned ratio {}",
                status.percentage_total_outputs_scanned,
                expected_total,
            );
            assert_ne!(
                status.percentage_session_outputs_scanned, COMPLETE_PERCENTAGE,
                "a session that scanned no outputs reported a finished sync",
            );
            assert!(
                (status.percentage_session_outputs_scanned - expected_total).abs() < f32::EPSILON,
                "session output percentage {} disagrees with the total progress {}",
                status.percentage_session_outputs_scanned,
                expected_total,
            );
        }
    }

    /// The drain policy for scanner shutdown, exercised as a table:
    /// pure inputs, no runtime, no clocks.
    mod drain_verdict {
        use std::time::Duration;

        use crate::sync::DrainVerdict::{self, KeepPolling, Reenter, Shutdown};
        use crate::sync::drain_verdict;

        fn ms(millis: u64) -> Duration {
            Duration::from_millis(millis)
        }

        /// One row of the drain-policy table:
        /// (workers, unprocessed, connected_for, poll_elapsed, verdict, label).
        type DrainCase = (usize, u32, Option<u64>, u64, DrainVerdict, &'static str);

        /// HYPOTHESIS: a settled stream whose scanner is still busy, and an
        /// unsettled stream whose scanner is still busy, are two different
        /// states, because only the second one is advanced by waiting.
        /// Falsified if the drain policy answers them alike.
        #[test]
        fn a_settled_and_an_unsettled_busy_scanner_are_not_one_state() {
            let settled = drain_verdict(2, 0, Some(ms(1_400)), ms(500));
            let unsettled = drain_verdict(2, 0, Some(ms(50)), ms(500));

            assert_ne!(
                settled, unsettled,
                "a settled stream waits on `scanner.update()`, which this loop \
                 never calls, while an unsettled one waits on a clock that ticks \
                 by itself; one verdict for both spends the ceiling on the state \
                 that waiting cannot advance"
            );
            assert_eq!(settled, Reenter, "settled and busy: only the loop helps");
            assert_eq!(unsettled, KeepPolling, "unsettled: the clock still runs");
        }

        #[test]
        fn table() {
            let cases: &[DrainCase] = &[
                // The reported bug: first-loop shutdown on a fully
                // synced chain, stream not yet connected. Hold the
                // session open instead of closing it instantly.
                (0, 0, None, 0, KeepPolling, "no stream yet"),
                // Connected but inside the settle window: still hold.
                (0, 0, Some(50), 100, KeepPolling, "settling"),
                // The typical session: stream connected long ago,
                // scanner drained. Immediate shutdown, no added cost.
                (0, 0, Some(1_400), 0, Shutdown, "settled and drained"),
                // Exactly the settle boundary counts as settled.
                (0, 0, Some(200), 0, Shutdown, "settle boundary"),
                // Unprocessed work always re-enters the processing
                // loop, whatever the stream state. No deadline caps
                // the processing itself.
                (0, 3, Some(1_400), 0, Reenter, "unprocessed work"),
                (5, 1, None, 999, Reenter, "work trumps missing stream"),
                // Workers still draining: re-enter at once. Polling here
                // cannot retire a worker, because only the main loop's
                // `scanner.update()` does, so waiting spends the ceiling on
                // a value that cannot move.
                (2, 0, Some(1_400), 500, Reenter, "workers draining"),
                (2, 0, Some(1_400), 1_000, Reenter, "ceiling with workers"),
                // Before the stream settles, polling is still right: the
                // settle window is wall-clock and does elapse.
                (2, 0, Some(50), 500, KeepPolling, "unsettled stream polls"),
                // Ceiling with a stream that never connected: the
                // pre-c90f8d309 semantics. A dead stream must not
                // hold the session open.
                (0, 0, None, 1_000, Shutdown, "ceiling without stream"),
            ];
            for (workers, unprocessed, connected_ms, elapsed_ms, expected, name) in cases {
                assert_eq!(
                    drain_verdict(
                        *workers,
                        *unprocessed,
                        connected_ms.map(ms),
                        ms(*elapsed_ms)
                    ),
                    *expected,
                    "{name}"
                );
            }
        }
    }

    /// The lifecycle of a note's spend mark across spend detection and `reset_spends`.
    ///
    /// The wallet remembers an on-chain spend observation in exactly one durable place: the
    /// note's `spending_transaction` field. The nullifier map is a transient rendezvous
    /// buffer: entries are consumed on detection (`detect_shielded_spends`) and pruned
    /// behind the fully-scanned frontier (`remove_irrelevant_data`). These tests pin the
    /// consequences for `set_transactions_failed`, whose `reset_spends` call erases that
    /// one durable place.
    mod spend_reset_lifecycle {
        use std::collections::HashMap;

        use sapling_crypto::value::NoteValue;
        use zcash_primitives::transaction::TxId;
        use zcash_protocol::{consensus::BlockHeight, memo::Memo};
        use zingo_status::confirmation_status::ConfirmationStatus;

        use crate::{
            mocks::{MockWallet, MockWalletBuilder},
            sync::{set_transactions_failed, spend},
            wallet::{
                NullifierMap, OutputId, ScanTarget, WalletNote, WalletTransaction,
                traits::{SyncNullifiers, SyncTransactions},
            },
        };

        const FUNDING_HEIGHT: BlockHeight = BlockHeight::from_u32(10);
        const SPEND_HEIGHT: BlockHeight = BlockHeight::from_u32(100);
        const FUNDING_TXID: TxId = TxId::from_bytes([1; 32]);
        const SPENDING_TXID: TxId = TxId::from_bytes([2; 32]);
        const NOTE_NULLIFIER: sapling_crypto::Nullifier = sapling_crypto::Nullifier([42; 32]);

        /// The spending transaction's wallet record in the given lifecycle state.
        fn spending_record(status: ConfirmationStatus) -> WalletTransaction {
            WalletTransaction::new_for_test(SPENDING_TXID, status)
        }

        /// A confirmed funding transaction holding one sapling note with a derived
        /// nullifier, optionally already marked spent.
        ///
        /// The crypto-note construction duplicates `zingolib::mocks::SaplingCryptoNoteBuilder`,
        /// which cannot be used here (zingolib depends on this crate). Relocating the note
        /// builders down into this crate is a deferred follow-up.
        fn funding_transaction(spending_transaction: Option<TxId>) -> WalletTransaction {
            let extsk = sapling_crypto::zip32::ExtendedSpendingKey::master(&[0; 32]);
            let (_, recipient) = extsk.default_address();
            let crypto_note = sapling_crypto::Note::from_parts(
                recipient,
                NoteValue::from_raw(100_000),
                sapling_crypto::Rseed::AfterZip212([0; 32]),
            );
            let mut note = WalletNote::new_for_test(
                OutputId::new(FUNDING_TXID, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                crypto_note,
                Memo::Empty,
                None,
            )
            .with_nullifier_for_test(NOTE_NULLIFIER);
            note.spending_transaction = spending_transaction;

            WalletTransaction::new_for_test(
                FUNDING_TXID,
                ConfirmationStatus::Confirmed(FUNDING_HEIGHT),
            )
            .with_sapling_notes_for_test(vec![note])
        }

        fn get_spending_txid(wallet: &MockWallet) -> Option<TxId> {
            wallet
                .get_wallet_transactions()
                .unwrap()
                .get(&FUNDING_TXID)
                .unwrap()
                .sapling_notes()
                .first()
                .unwrap()
                .spending_transaction
        }

        /// The detection pass `update_shielded_spends` performs, minus the network round
        /// trips: match derived note nullifiers against the wallet's nullifier map and
        /// mark the matches spent.
        fn run_spend_detection(wallet: &mut MockWallet) {
            let (sapling_nullifiers, orchard_nullifiers, ironwood_nullifiers) =
                spend::collect_derived_nullifiers(wallet.get_wallet_transactions().unwrap());
            let (sapling_targets, orchard_targets, ironwood_targets) =
                spend::detect_shielded_spends(
                    wallet.get_nullifiers_mut().unwrap(),
                    sapling_nullifiers,
                    orchard_nullifiers,
                    ironwood_nullifiers,
                );
            spend::update_spent_notes(
                wallet,
                sapling_targets,
                orchard_targets,
                ironwood_targets,
                true,
            )
            .unwrap();
        }

        /// A spend reset *before* the spending transaction's block is scanned heals.
        /// When the scanner later reaches the block, `collect_nullifiers` maps the spend
        /// and detection re-marks the note.
        #[test]
        fn reset_before_scan_heals_on_spend_detection() {
            let mut wallet_transactions = HashMap::new();
            wallet_transactions.insert(FUNDING_TXID, funding_transaction(None));
            wallet_transactions.insert(
                SPENDING_TXID,
                spending_record(ConfirmationStatus::Failed(SPEND_HEIGHT)),
            );
            let mut nullifier_map = NullifierMap::new();
            nullifier_map.sapling.insert(
                NOTE_NULLIFIER,
                ScanTarget {
                    block_height: SPEND_HEIGHT,
                    txid: SPENDING_TXID,
                    narrow_scan_area: false,
                },
            );
            let mut wallet = MockWalletBuilder::new()
                .wallet_transactions(wallet_transactions)
                .nullifier_map(nullifier_map)
                .create_mock_wallet();

            run_spend_detection(&mut wallet);

            assert_eq!(get_spending_txid(&wallet), Some(SPENDING_TXID));
            // Detection is one-shot: the observation moved out of the map into the note.
            assert!(wallet.get_nullifiers().unwrap().sapling.is_empty());
        }

        /// Failure-marking a transaction that is `Confirmed` on chain must not
        /// destroy the wallet's knowledge of the spend.
        ///
        /// Once detection has consumed the nullifier-map entry (and cleanup has pruned the
        /// range behind the fully-scanned frontier), the note's `spending_transaction`
        /// field is the wallet's only durable record of the on-chain spend. If
        /// `set_transactions_failed` erased it, the note would become a permanent phantom
        /// unspent note: every future proposal selects it and is rejected as a
        /// double-spend, and no forward sync can correct it.
        ///
        /// The damage is permanent IFF the spending block has already been scanned:
        /// detection consumes the nullifier-map entry at scan time, and scan time is
        /// exactly when the record becomes `Confirmed` (scan ranges complete out of
        /// order, so this can precede the fully-scanned frontier reaching that height).
        /// If the reset lands before that block is scanned, the future scan re-observes
        /// the nullifier and heals the wallet
        /// (see [`reset_before_scan_heals_on_spend_detection`]). This equivalence is why
        /// guarding the failure path on `Confirmed` status is coextensive with the harm.
        ///
        /// `set_transactions_failed` therefore refuses to fail a `Confirmed` transaction:
        /// the record keeps its status and the spend mark survives. Only truncation may
        /// invalidate mined transactions (via `set_transactions_failed_unchecked`),
        /// because it simultaneously reopens the affected scan ranges.
        #[test]
        fn failing_a_confirmed_transaction_must_not_destroy_the_spend_observation() {
            let mut wallet_transactions = HashMap::new();
            wallet_transactions.insert(FUNDING_TXID, funding_transaction(Some(SPENDING_TXID)));
            wallet_transactions.insert(
                SPENDING_TXID,
                spending_record(ConfirmationStatus::Confirmed(SPEND_HEIGHT)),
            );
            // The map entry was consumed by detection and pruned by cleanup: empty map.
            let mut wallet = MockWalletBuilder::new()
                .wallet_transactions(wallet_transactions)
                .create_mock_wallet();

            // A late failure marking (e.g. an expiry decision racing the scanner) targets
            // a transaction that is confirmed on chain.
            set_transactions_failed(
                wallet.get_wallet_transactions_mut().unwrap(),
                vec![SPENDING_TXID],
            );

            // A mined transaction cannot fail: the record keeps its `Confirmed` status.
            assert_eq!(
                wallet
                    .get_wallet_transactions()
                    .unwrap()
                    .get(&SPENDING_TXID)
                    .unwrap()
                    .status(),
                ConfirmationStatus::Confirmed(SPEND_HEIGHT)
            );

            // The spend must remain detectable: after a detection pass the note is still
            // marked spent.
            run_spend_detection(&mut wallet);
            assert_eq!(
                get_spending_txid(&wallet),
                Some(SPENDING_TXID),
                "the on-chain spend observation was destroyed: \
                 the note is now a permanent phantom unspent note"
            );
        }

        /// REPRO: `update_shielded_spends` consumes the nullifier-map entry in
        /// `detect_shielded_spends` before `scan_spending_transactions` awaits the
        /// network. When the spending transaction is not in the wallet and the fetch
        /// fails, the function returns before `update_spent_notes`, so the note is
        /// never marked spent and the map entry that could re-detect it is gone.
        ///
        /// The invariant: after a failed detection pass the spend is still recoverable,
        /// either the note carries the spending txid or the nullifier is still mapped.
        #[tokio::test]
        async fn failed_spending_transaction_fetch_must_not_lose_the_detected_spend() {
            let mut wallet_transactions = HashMap::new();
            wallet_transactions.insert(FUNDING_TXID, funding_transaction(None));
            // The spending transaction evaded trial decryption: it is absent from the
            // wallet, so `scan_spending_transactions` must fetch it.
            let mut nullifier_map = NullifierMap::new();
            nullifier_map.sapling.insert(
                NOTE_NULLIFIER,
                ScanTarget {
                    block_height: SPEND_HEIGHT,
                    txid: SPENDING_TXID,
                    narrow_scan_area: true,
                },
            );
            let mut wallet = MockWalletBuilder::new()
                .wallet_transactions(wallet_transactions)
                .nullifier_map(nullifier_map)
                .create_mock_wallet();

            // The fetcher is gone: every network request fails with `FetcherDropped`.
            let (fetch_request_sender, fetch_request_receiver) =
                tokio::sync::mpsc::unbounded_channel::<crate::client::FetchRequest>();
            drop(fetch_request_receiver);

            let result = spend::update_shielded_spends(
                &zcash_protocol::consensus::MAIN_NETWORK,
                &mut wallet,
                fetch_request_sender,
                &HashMap::new(),
                &std::collections::BTreeMap::new(),
                None,
            )
            .await;
            assert!(result.is_err(), "the fetch was expected to fail");

            let note_marked_spent = get_spending_txid(&wallet) == Some(SPENDING_TXID);
            let nullifier_still_mapped = wallet
                .get_nullifiers()
                .unwrap()
                .sapling
                .contains_key(&NOTE_NULLIFIER);
            assert!(
                note_marked_spent || nullifier_still_mapped,
                "the spend observation was lost: the note is unspent \
                 (spending_transaction = {:?}) and the nullifier is no longer mapped",
                get_spending_txid(&wallet)
            );
        }

        /// A scanned transaction record replaces an existing `Failed` record
        /// wholesale, because `extend_wallet_transactions` merges via `HashMap::extend`.
        /// This is why the reset-before-scan sequence heals completely.
        #[test]
        fn scanned_transaction_overwrites_failed_record() {
            let mut wallet_transactions = HashMap::new();
            wallet_transactions.insert(
                SPENDING_TXID,
                spending_record(ConfirmationStatus::Failed(SPEND_HEIGHT)),
            );
            let mut wallet = MockWalletBuilder::new()
                .wallet_transactions(wallet_transactions)
                .create_mock_wallet();

            wallet
                .extend_wallet_transactions(HashMap::from([(
                    SPENDING_TXID,
                    spending_record(ConfirmationStatus::Confirmed(SPEND_HEIGHT)),
                )]))
                .unwrap();

            assert_eq!(
                wallet
                    .get_wallet_transactions()
                    .unwrap()
                    .get(&SPENDING_TXID)
                    .unwrap()
                    .status(),
                ConfirmationStatus::Confirmed(SPEND_HEIGHT)
            );
        }
    }

    mod checked_height_validation {
        use zcash_protocol::consensus::BlockHeight;
        use zcash_protocol::local_consensus::LocalNetwork;
        const LOCAL_NETWORK: LocalNetwork = LocalNetwork {
            overwinter: Some(BlockHeight::from_u32(1)),
            sapling: Some(BlockHeight::from_u32(3)),
            blossom: Some(BlockHeight::from_u32(3)),
            heartwood: Some(BlockHeight::from_u32(3)),
            canopy: Some(BlockHeight::from_u32(3)),
            nu5: Some(BlockHeight::from_u32(3)),
            nu6: Some(BlockHeight::from_u32(3)),
            nu6_1: Some(BlockHeight::from_u32(3)),
            nu6_2: Some(BlockHeight::from_u32(3)),
            nu6_3: Some(BlockHeight::from_u32(3)),
        };
        use crate::{error::SyncError, mocks::MockWalletError, sync::checked_wallet_height};
        // It's possible an error from an implementor's get_sync_state could bubble up to checked_wallet_height
        // this test shows that such an error is raies wrapped in a WalletError and return as the Err variant
        #[tokio::test]
        async fn get_sync_state_error() {
            let builder = crate::mocks::MockWalletBuilder::new();
            let test_error = "get_sync_state_error";
            let mut test_wallet = builder
                .get_sync_state_patch(Box::new(|_| {
                    Err(MockWalletError::AnErrorVariant(test_error.to_string()))
                }))
                .create_mock_wallet();
            let res =
                checked_wallet_height(&mut test_wallet, BlockHeight::from_u32(1), &LOCAL_NETWORK);
            assert!(matches!(
                res,
                Err(SyncError::WalletError(
                    crate::mocks::MockWalletError::AnErrorVariant(ref s)
                )) if s == test_error
            ));
        }

        mod last_known_chain_height {
            use crate::{
                sync::{MAX_REORG_ALLOWANCE, ScanRange},
                wallet::SyncState,
            };
            const DEFAULT_START_HEIGHT: BlockHeight = BlockHeight::from_u32(1);
            const _DEFAULT_LAST_KNOWN_HEIGHT: BlockHeight = BlockHeight::from_u32(102);
            const DEFAULT_CHAIN_HEIGHT: BlockHeight = BlockHeight::from_u32(110);

            use super::*;
            #[tokio::test]
            async fn above_allowance() {
                const LAST_KNOWN_HEIGHT: BlockHeight = BlockHeight::from_u32(211);
                let lkch = vec![ScanRange::from_parts(
                    DEFAULT_START_HEIGHT..LAST_KNOWN_HEIGHT,
                    crate::sync::ScanPriority::Scanned,
                )];
                let state = SyncState {
                    scan_ranges: lkch,
                    ..Default::default()
                };
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut test_wallet = builder.sync_state(state).create_mock_wallet();
                let res =
                    checked_wallet_height(&mut test_wallet, DEFAULT_CHAIN_HEIGHT, &LOCAL_NETWORK);
                if let Err(e) = res {
                    assert_eq!(
                        e.to_string(),
                        format!(
                            "wallet height {} is more than {} blocks ahead of best chain height {}",
                            LAST_KNOWN_HEIGHT - 1,
                            MAX_REORG_ALLOWANCE,
                            DEFAULT_CHAIN_HEIGHT
                        )
                    );
                } else {
                    panic!()
                }
            }
            #[tokio::test]
            async fn above_chain_height_below_allowance() {
                // The hain_height is received from the proxy
                // truncate uses the wallet scan start height
                // as a
                let lkch = vec![ScanRange::from_parts(
                    BlockHeight::from_u32(6)..BlockHeight::from_u32(10),
                    crate::sync::ScanPriority::Scanned,
                )];
                let state = SyncState {
                    scan_ranges: lkch,
                    ..Default::default()
                };
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut test_wallet = builder.sync_state(state).create_mock_wallet();
                let chain_height = BlockHeight::from_u32(4);
                // This will trigger a call to truncate_wallet_data with
                // chain_height and start_height inferred from the wallet.
                // chain must be greater than by this time which hits the Greater cmp
                // match
                let res = checked_wallet_height(&mut test_wallet, chain_height, &LOCAL_NETWORK);
                assert_eq!(res.unwrap(), BlockHeight::from_u32(4));
            }
            #[ignore = "in progress"]
            #[tokio::test]
            async fn equal_or_below_chain_height_and_above_sapling() {
                let lkch = vec![ScanRange::from_parts(
                    BlockHeight::from_u32(1)..BlockHeight::from_u32(10),
                    crate::sync::ScanPriority::Scanned,
                )];
                let state = SyncState {
                    scan_ranges: lkch,
                    ..Default::default()
                };
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut _test_wallet = builder.sync_state(state).create_mock_wallet();
            }
            #[ignore = "in progress"]
            #[tokio::test]
            async fn equal_or_below_chain_height_and_below_sapling() {
                // This case requires that the wallet have a scan_start_below sapling
                // which is an unexpected state.
                let lkch = vec![ScanRange::from_parts(
                    BlockHeight::from_u32(1)..BlockHeight::from_u32(10),
                    crate::sync::ScanPriority::Scanned,
                )];
                let state = SyncState {
                    scan_ranges: lkch,
                    ..Default::default()
                };
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut _test_wallet = builder.sync_state(state).create_mock_wallet();
            }
            #[ignore = "in progress"]
            #[tokio::test]
            async fn below_sapling() {
                let lkch = vec![ScanRange::from_parts(
                    BlockHeight::from_u32(1)..BlockHeight::from_u32(10),
                    crate::sync::ScanPriority::Scanned,
                )];
                let state = SyncState {
                    scan_ranges: lkch,
                    ..Default::default()
                };
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut _test_wallet = builder.sync_state(state).create_mock_wallet();
            }
        }
        mod no_last_known_chain_height {
            use super::*;
            // If there are know scan_ranges in the SyncState
            #[tokio::test]
            async fn get_bday_error() {
                let test_error = "get_bday_error";
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut test_wallet = builder
                    .get_birthday_patch(Box::new(|_| {
                        Err(crate::mocks::MockWalletError::AnErrorVariant(
                            test_error.to_string(),
                        ))
                    }))
                    .create_mock_wallet();
                let res = checked_wallet_height(
                    &mut test_wallet,
                    BlockHeight::from_u32(1),
                    &LOCAL_NETWORK,
                );
                assert!(matches!(
                    res,
                    Err(SyncError::WalletError(
                        crate::mocks::MockWalletError::AnErrorVariant(ref s)
                    )) if s == test_error
                ));
            }
            #[ignore = "in progress"]
            #[tokio::test]
            async fn raw_bday_above_chain_height() {
                let builder = crate::mocks::MockWalletBuilder::new();
                let mut test_wallet = builder
                    .birthday(BlockHeight::from_u32(15))
                    .create_mock_wallet();
                let res = checked_wallet_height(
                    &mut test_wallet,
                    BlockHeight::from_u32(1),
                    &LOCAL_NETWORK,
                );
                if let Err(e) = res {
                    assert_eq!(
                        e.to_string(),
                        format!(
                            "wallet height is more than {} blocks ahead of best chain height",
                            15 - 1
                        )
                    );
                } else {
                    panic!()
                }
            }
            mod sapling_height {
                use super::*;
                #[tokio::test]
                async fn raw_bday_above() {
                    let builder = crate::mocks::MockWalletBuilder::new();
                    let mut test_wallet = builder
                        .birthday(BlockHeight::from_u32(4))
                        .create_mock_wallet();
                    let res = checked_wallet_height(
                        &mut test_wallet,
                        BlockHeight::from_u32(5),
                        &LOCAL_NETWORK,
                    );
                    assert_eq!(res.unwrap(), BlockHeight::from_u32(4 - 1));
                }
                #[tokio::test]
                async fn raw_bday_equal() {
                    let builder = crate::mocks::MockWalletBuilder::new();
                    let mut test_wallet = builder
                        .birthday(BlockHeight::from_u32(3))
                        .create_mock_wallet();
                    let res = checked_wallet_height(
                        &mut test_wallet,
                        BlockHeight::from_u32(5),
                        &LOCAL_NETWORK,
                    );
                    assert_eq!(res.unwrap(), BlockHeight::from_u32(3 - 1));
                }
                #[tokio::test]
                async fn raw_bday_below() {
                    let builder = crate::mocks::MockWalletBuilder::new();
                    let mut test_wallet = builder
                        .birthday(BlockHeight::from_u32(1))
                        .create_mock_wallet();
                    let res = checked_wallet_height(
                        &mut test_wallet,
                        BlockHeight::from_u32(5),
                        &LOCAL_NETWORK,
                    );
                    assert!(matches!(res, Err(SyncError::BirthdayBelowSapling(1, 3))));
                }
            }
        }
    }

    mod expire_transactions {
        use std::collections::HashMap;

        use zcash_protocol::TxId;
        use zcash_protocol::consensus::BlockHeight;
        use zingo_status::confirmation_status::ConfirmationStatus;

        use crate::mocks::{MockWallet, MockWalletBuilder};
        use crate::sync::{ScanPriority, ScanRange, expire_transactions};
        use crate::wallet::{SyncState, WalletTransaction};

        /// Creates a mock wallet with all blocks scanned up to `chain_height`.
        fn wallet_at_height(chain_height: u32, transactions: Vec<WalletTransaction>) -> MockWallet {
            let sync_state = SyncState {
                scan_ranges: vec![ScanRange::from_parts(
                    BlockHeight::from_u32(1)..BlockHeight::from_u32(chain_height + 1),
                    ScanPriority::Scanned,
                )],
                ..Default::default()
            };
            let wallet_transactions: HashMap<TxId, WalletTransaction> = transactions
                .into_iter()
                .map(|transaction| (transaction.txid(), transaction))
                .collect();

            MockWalletBuilder::new()
                .sync_state(sync_state)
                .wallet_transactions(wallet_transactions)
                .create_mock_wallet()
        }

        fn transaction_status(wallet: &MockWallet, txid: TxId) -> ConfirmationStatus {
            crate::wallet::traits::SyncTransactions::get_wallet_transactions(wallet)
                .unwrap()
                .get(&txid)
                .unwrap()
                .status()
        }

        #[test]
        fn pending_transaction_past_expiry_is_failed() {
            let txid = TxId::from_bytes([1; 32]);
            let transaction = WalletTransaction::new_for_test_with_expiry(
                txid,
                ConfirmationStatus::Mempool(BlockHeight::from_u32(61)),
                BlockHeight::from_u32(100),
            );
            let mut wallet = wallet_at_height(100, vec![transaction]);

            expire_transactions(&mut wallet).unwrap();

            assert!(matches!(
                transaction_status(&wallet, txid),
                ConfirmationStatus::Failed(_)
            ));
        }

        #[test]
        fn pending_transaction_before_expiry_is_untouched() {
            let txid = TxId::from_bytes([1; 32]);
            let transaction = WalletTransaction::new_for_test_with_expiry(
                txid,
                ConfirmationStatus::Mempool(BlockHeight::from_u32(61)),
                BlockHeight::from_u32(101),
            );
            let mut wallet = wallet_at_height(100, vec![transaction]);

            expire_transactions(&mut wallet).unwrap();

            assert!(matches!(
                transaction_status(&wallet, txid),
                ConfirmationStatus::Mempool(_)
            ));
        }

        #[test]
        fn zero_expiry_transaction_never_expires() {
            // ZIP-203: an expiry height of 0 means the transaction never expires.
            let txid = TxId::from_bytes([1; 32]);
            let transaction = WalletTransaction::new_for_test_with_expiry(
                txid,
                ConfirmationStatus::Mempool(BlockHeight::from_u32(61)),
                BlockHeight::from_u32(0),
            );
            let mut wallet = wallet_at_height(1_000_000, vec![transaction]);

            expire_transactions(&mut wallet).unwrap();

            assert!(matches!(
                transaction_status(&wallet, txid),
                ConfirmationStatus::Mempool(_)
            ));
        }

        #[test]
        fn confirmed_transaction_is_untouched() {
            let txid = TxId::from_bytes([1; 32]);
            let transaction = WalletTransaction::new_for_test_with_expiry(
                txid,
                ConfirmationStatus::Confirmed(BlockHeight::from_u32(61)),
                BlockHeight::from_u32(100),
            );
            let mut wallet = wallet_at_height(100, vec![transaction]);

            expire_transactions(&mut wallet).unwrap();

            assert!(matches!(
                transaction_status(&wallet, txid),
                ConfirmationStatus::Confirmed(_)
            ));
        }
    }
}
