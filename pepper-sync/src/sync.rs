//! Entrypoint for sync engine

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::ops::{Bound, Range};
use std::sync::atomic::{self, AtomicBool, AtomicU8, AtomicU32};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};

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
use crate::keys;
use crate::keys::transparent::TransparentAddressId;
use crate::scan::ScanResults;
use crate::scan::task::{ScanLoad, Scanner, ScannerState, TaskId};
use crate::scan::transactions::scan_transaction;
use crate::shardtree_ext::{RollbackOutcome, ShardTreeExt};
use crate::sync::state::{ScanResultsStanding, VerifyEnd};
use crate::wallet::traits::{
    SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions, SyncWallet,
};
use crate::wallet::{
    KeyIdInterface, NoteInterface, NullifierMap, OutputId, OutputInterface, PoolActivation,
    ScanTarget, SyncMode, SyncState, WalletBlock, WalletTransaction,
};
use crate::witness::{ANCHOR_RETENTION_INTERVALS, LocatedTreeData};

use crate::witness;

pub(crate) mod spend;
pub(crate) mod state;
pub(crate) mod transparent;
pub mod truncate;

// TODO: investigate a potential case where:
// - a wallet syncs, including the latest incomplete shard
// - the wallet is not opened for some time, the incomplete shard has completed since
// - on next sync, the shard roots *after* the incomplete shard are fetched
// - the wallet can't spend because the incomplete shard is not prioritized to be completed

/// The deepest chain reorganization the wallet tolerates, and the
/// repository's single source of truth for that depth. It mirrors the
/// validator's finalization boundary, zebra's
/// `zebra_state::MAX_BLOCK_REORG_HEIGHT` (100), below which blocks are
/// final and no deeper reorg can occur. Zebra crates are not
/// dependencies of this workspace, so the value is pinned to its
/// upstream by documentation rather than import. If zebra ever moves
/// its boundary, this constant is the one place that follows it.
pub const MAX_REORG_ALLOWANCE: u32 = 100;

/// The maximum number of checkpoints in the rolling window for re-org handling and chain tip anchor spends.
pub const SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE: u32 = MAX_REORG_ALLOWANCE + 1;

/// The maximum total number of checkpoints a shard tree persists.
pub const MAX_SHARDTREE_CHECKPOINTS: u32 =
    SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE + ANCHOR_RETENTION_INTERVALS;

const VERIFY_BLOCK_RANGE_SIZE: u32 = 10;

/// Interval in seconds at which continuous sync checks for newly mined blocks if the mempool stream has not signalled
/// one.
pub const CHECK_NEW_BLOCKS_INTERVAL: u64 = 10;

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
            "Sync result:
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

    /// Returns the priority the wallet holds a range selected at this priority while its scan task is in flight.
    pub fn in_flight(self) -> ScanPriority {
        if self == ScanPriority::ScannedWithoutMapping {
            ScanPriority::RefetchingNullifiers
        } else {
            ScanPriority::Scanning
        }
    }
}

/// Returns true when the two ranges share at least one block.
pub(crate) fn overlaps(first: &Range<BlockHeight>, second: &Range<BlockHeight>) -> bool {
    first.start < second.end && second.start < first.end
}

/// Returns the scan ranges the wallet holds at `priority`, in wallet order.
pub(crate) fn held_at(
    scan_ranges: &[ScanRange],
    priority: ScanPriority,
) -> impl Iterator<Item = &ScanRange> {
    scan_ranges
        .iter()
        .filter(move |scan_range| scan_range.priority() == priority)
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

    /// Returns true when `block_range` lies within this scan range.
    #[must_use]
    pub fn encloses(&self, block_range: &Range<BlockHeight>) -> bool {
        self.block_range.start <= block_range.start && block_range.end <= self.block_range.end
    }

    /// Returns true when this scan range and `block_range` share at least one block.
    #[must_use]
    pub fn overlaps(&self, block_range: &Range<BlockHeight>) -> bool {
        overlaps(&self.block_range, block_range)
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

enum MempoolMessage {
    Transaction(RawTransaction),
    NewBlockMined,
}

/// Sets `shutdown_mempool` when dropped, so the mempool monitor stops on every exit from [`sync`]: a return, an error
/// or a cancelled future.
struct MempoolShutdownGuard(Arc<AtomicBool>);

impl Drop for MempoolShutdownGuard {
    fn drop(&mut self) {
        self.0.store(true, atomic::Ordering::Release);
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
/// With `shutdown_on_completion` set, a scan that reaches the chain tip moves a `Running` engine to `Shutdown` and
/// leaves a `Paused` engine paused, which then shuts down once it is resumed and completes again.
/// Wallet keys must not change while sync is running. Sync must be stoppped and run again after the key material has
/// been updated.
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
    let mut sync_mode_enum = SyncMode::from_atomic_u8(&sync_mode)?;
    if sync_mode_enum == SyncMode::NotRunning {
        sync_mode_enum = SyncMode::Running;
        sync_mode.store(sync_mode_enum as u8, atomic::Ordering::Release);
    } else {
        return Err(SyncModeError::SyncAlreadyRunning.into());
    }

    tracing::info!("Starting sync...");

    // transparent and ironwood data is required in compact blocks so the server must be checked before any tasks are
    // launched.
    let mut client_clone = client.clone();
    client::check_lightwallet_protocol_version(&mut client_clone).await?;

    // create channel for sending fetch requests and launch fetcher task
    let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
    let fetcher_handle =
        tokio::spawn(
            async move { client::fetch::fetch(fetch_request_receiver, client_clone).await },
        );

    // create channel for receiving mempool transactions and launch mempool monitor
    let (mempool_transaction_sender, mut mempool_transaction_receiver) = mpsc::channel(100);
    let shutdown_mempool = Arc::new(AtomicBool::new(false));
    let _mempool_shutdown_guard = MempoolShutdownGuard(shutdown_mempool.clone());
    let shutdown_mempool_clone = shutdown_mempool.clone();
    let unprocessed_mempool_transactions_count = Arc::new(AtomicU32::new(0));
    let unprocessed_mempool_transactions_count_clone =
        unprocessed_mempool_transactions_count.clone();
    let mempool_stream_connected_at = Arc::new(OnceLock::new());
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

    let ufvks = wallet
        .read()
        .await
        .get_unified_full_viewing_keys()
        .map_err(SyncError::WalletError)?;
    let (scan_results_sender, mut scan_results_receiver) = mpsc::unbounded_channel();
    let mut scanner = Scanner::new(
        consensus_parameters.clone(),
        scan_results_sender,
        fetch_request_sender.clone(),
        ufvks.clone(),
        config.transparent_address_discovery.gap_limit as u32,
    );
    scanner.launch(config.performance_level);

    let mut wallet_guard = wallet.write().await;
    state::reset_scan_ranges(
        wallet_guard
            .get_sync_state_mut()
            .map_err(SyncError::WalletError)?,
    );
    rollback_unscanned_tree_states(&mut *wallet_guard)?;
    drop(wallet_guard);

    let mut check_for_new_blocks = false;
    let mut first_verification_complete = false;
    let mut mempool_shutdown_timer = None;
    let mut nullifier_map_limit_exceeded = false;
    let mut continuous_sync_interval =
        tokio::time::interval(Duration::from_secs(CHECK_NEW_BLOCKS_INTERVAL));
    continuous_sync_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    'continuous_sync: loop {
        continuous_sync_interval.reset();
        let mut reorg_occured = false;
        scanner.state.reverify();
        let chain_height = client::get_chain_height(fetch_request_sender.clone()).await?;
        if chain_height == 0.into() {
            return Err(SyncError::ServerError(ServerError::GenesisBlockOnly));
        }

        // hold wallet guard until initial sync state is set to avoid inconsistencies and potential subtraction overflows
        // when calculating sync status.
        let mut wallet_guard = wallet.write().await;
        let last_known_chain_height =
            checked_wallet_height(&mut *wallet_guard, chain_height, consensus_parameters)?;
        let new_blocks_mined = chain_height > last_known_chain_height;
        state::create_scan_range(
            last_known_chain_height,
            chain_height,
            wallet_guard
                .get_sync_state_mut()
                .map_err(SyncError::WalletError)?,
        );
        // only set intiial sync state on first continuous sync loop.
        // otherwise, only extend the wallet tree bounds to include new blocks.
        if first_verification_complete && new_blocks_mined {
            state::update_wallet_tree_bounds(
                consensus_parameters,
                fetch_request_sender.clone(),
                &mut *wallet_guard,
                chain_height,
            )
            .await?;
        } else if !first_verification_complete {
            state::set_initial_state(
                consensus_parameters,
                fetch_request_sender.clone(),
                &mut *wallet_guard,
                chain_height,
            )
            .await?;
        }
        drop(wallet_guard);

        // on the first verification we verify the previous wallet state.
        // afterwards, we only verify the newly mined blocks if they exist.
        let mut initial_reorg_detection_start_height_opt =
            if first_verification_complete && new_blocks_mined {
                Some(last_known_chain_height + 1)
            } else if first_verification_complete {
                None
            } else {
                wallet
                    .read()
                    .await
                    .get_sync_state()
                    .map_err(SyncError::WalletError)?
                    .highest_scanned_height()
                    .map(|highest_scanned_height| highest_scanned_height + 1)
            };

        if let Some(initial_reorg_detection_start_height) = initial_reorg_detection_start_height_opt
        {
            if initial_reorg_detection_start_height <= chain_height {
                state::set_verify_scan_range(
                    wallet
                        .write()
                        .await
                        .get_sync_state_mut()
                        .map_err(SyncError::WalletError)?,
                    initial_reorg_detection_start_height,
                    VerifyEnd::VerifyLowest,
                );
            } else {
                // in this case, no blocks have been mined since last sync session but a re-org may have occured
                let chain_height_server_block =
                    client::get_compact_block(fetch_request_sender.clone(), chain_height).await?;
                let chain_height_wallet_block = wallet
                    .read()
                    .await
                    .get_wallet_block(chain_height)
                    .map_err(SyncError::WalletError)?;
                if chain_height_wallet_block.block_hash().0.to_vec()
                    != chain_height_server_block.hash
                {
                    tracing::info!("Re-org detected.");
                    reorg_occured = true;
                    // hold wallet guard until initial sync state is set to avoid inconsistencies and potential subtraction overflows
                    // when calculating sync status
                    let mut wallet_guard = wallet.write().await;
                    let scan_range_to_verify = state::set_verify_scan_range(
                        wallet_guard
                            .get_sync_state_mut()
                            .map_err(SyncError::WalletError)?,
                        chain_height,
                        VerifyEnd::VerifyHighest,
                    );
                    initial_reorg_detection_start_height_opt =
                        Some(scan_range_to_verify.block_range().start);
                    truncate_wallet_data(
                        &mut *wallet_guard,
                        initial_reorg_detection_start_height_opt
                            .expect("value must exist in this scope")
                            - 1,
                    )?;
                    state::set_initial_state(
                        consensus_parameters,
                        fetch_request_sender.clone(),
                        &mut *wallet_guard,
                        chain_height,
                    )
                    .await?;
                } else {
                    // there are no new blocks to verify and no re-org has occured
                    scanner.state.verified();
                }
            }
        } else {
            // first verification not complete: first time sync, there are no previously synced blocks to verify.
            // first verification complete: no newly mined blocks to verify.
            scanner.state.verified();
        }

        if !first_verification_complete {
            // only perform transparent address discovery on the first continuous sync loop.
            // transparent data in newly mined blocks during the sync session will be scanned in compact blocks.
            // address discovery is still necessary as scanning compact blocks non-linearly may lead to missing funds
            // or requiring rescanning multiple times.
            // this is performed even if no new blocks have been mined since the last sync session as blocks mined during
            // the previous sync session were not covered by transparent address discovery and may not have been
            // scanned. these blocks are above the transparent scan floor of the previous sync session, which
            // address discovery searches from.
            scanner.transparent_gap_addresses.extend(
                transparent::address_discovery(
                    consensus_parameters,
                    wallet.clone(),
                    fetch_request_sender.clone(),
                    &ufvks,
                    last_known_chain_height,
                    chain_height,
                    config.transparent_address_discovery.clone(),
                )
                .await?,
            );

            // transparent address discovery has located all relevant transactions up to the chain height. only the
            // compact block transparent data of blocks mined after this is scanned.
            let mut wallet_guard = wallet.write().await;
            wallet_guard
                .get_sync_state_mut()
                .map_err(SyncError::WalletError)?
                .transparent_scan_floor = Some(chain_height);
            wallet_guard
                .set_save_flag()
                .map_err(SyncError::WalletError)?;
            drop(wallet_guard);
        }

        if new_blocks_mined || reorg_occured || !first_verification_complete {
            // subtree roots are always updated on the first continuous sync loop, even if no new blocks have been mined,
            // so they are added before any note commitments are inserted into the shard trees by scanning.
            // the shard store fills every shard below an inserted shard with an empty shard, and subtree roots are
            // fetched from the number of stored shards, so the subtree roots of these empty shards would never be
            // fetched. for example, a pool rescan (see `truncate_to_pool_activation_height`) clears the pool's shard
            // tree and ends the sync session, relying on the next sync session to fetch the pool's subtree roots.
            update_subtree_roots(
                consensus_parameters,
                fetch_request_sender.clone(),
                &mut *wallet.write().await,
            )
            .await?;

            if !first_verification_complete {
                // frontier is added after subtree roots to retain subtree roots below birthday
                add_initial_frontier(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    &mut *wallet.write().await,
                )
                .await?;
            }

            // now any new transparent scan targets and subtree roots have been added, set ranges to be prioritized
            // for scanning.
            state::prioritize_scan_ranges(
                consensus_parameters,
                chain_height,
                &mut *wallet.write().await,
            )
            .map_err(SyncError::WalletError)?;
        }

        if new_blocks_mined || reorg_occured {
            expire_transactions(&mut *wallet.write().await)?;

            repin_anchor_checkpoints(consensus_parameters, &mut *wallet.write().await)?;
        }

        // publish sync status prior to scanning
        publish_sync_status(&*wallet.read().await, &progress).await?;

        'scan: loop {
            tokio::select! {
                Some((load, scan_results)) = scan_results_receiver.recv() => {
                    let mut wallet_guard = wallet.write().await;
                    let ProcessedScanResults {
                        new_transparent_inuse_addresses,
                        new_transparent_gap_addresses,
                        reorg_detection_start_height,
                    } = process_scan_results(
                        consensus_parameters,
                        &mut *wallet_guard,
                        fetch_request_sender.clone(),
                        &ufvks,
                        load,
                        scanner.in_flight_tasks(),
                        scan_results,
                        initial_reorg_detection_start_height_opt,
                        config.performance_level,
                        &mut nullifier_map_limit_exceeded,
                    )
                    .await?;
                    scanner.retire_finished_tasks(
                        wallet_guard
                            .get_sync_state()
                            .map_err(SyncError::WalletError)?
                            .scan_ranges(),
                    );
                    // only the changes to the gap addresses are applied. scan results without compact block
                    // transparent data, such as re-fetched nullifiers, carry no changes and leave the gap addresses
                    // as they are.
                    // NOTE: this is safe in the current architecture as the correct set of gap addressses will be
                    // determined before scanning begins and these changes will only come from the latest newly mined
                    // block(s). If the sync engine is modified so there are cases where compact blocks may be scanned
                    // for transparent data out-of-order, more checks must be applied here to ensure gap addresses are
                    // not lost and correctly follow on from the wallets current in-use address list.
                    scanner.update_transparent_gap_addresses(
                        &new_transparent_inuse_addresses,
                        new_transparent_gap_addresses,
                    );
                    if let Some(reorg_detection_start_height) = reorg_detection_start_height {
                        // the scanner only scans ranges with `Verify` priority while verifying, and a new continuous
                        // sync loop waits for it to finish.
                        scanner.state.reverify();
                        initial_reorg_detection_start_height_opt = Some(
                            initial_reorg_detection_start_height_opt
                                .map_or(reorg_detection_start_height, |height| {
                                    height.min(reorg_detection_start_height)
                                }),
                        );
                    }
                    expire_transactions(&mut *wallet_guard)?;
                    publish_sync_status(&*wallet_guard, &progress).await?;
                    wallet_guard.set_save_flag().map_err(SyncError::WalletError)?;
                    drop(wallet_guard);
                }

                Some(mempool_message) = mempool_transaction_receiver.recv() => {
                    match mempool_message {
                        MempoolMessage::Transaction(raw_transaction) => {
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
                        MempoolMessage::NewBlockMined => {
                            check_for_new_blocks = true;
                        }
                    }
                }

                _update_scanner = interval.tick() => {
                    sync_mode_enum = SyncMode::from_atomic_u8(&sync_mode)?;
                    match sync_mode_enum {
                        SyncMode::Paused => {
                            let mut pause_interval = tokio::time::interval(Duration::from_secs(1));
                            pause_interval.tick().await;
                            while sync_mode_enum == SyncMode::Paused {
                                pause_interval.tick().await;
                                sync_mode_enum = SyncMode::from_atomic_u8(&sync_mode)?;
                            }
                        },
                        SyncMode::Shutdown => {
                            match mempool_drain_verdict(
                                shutdown_mempool.clone(),
                                unprocessed_mempool_transactions_count.clone(), mempool_shutdown_timer.get_or_insert_with(Instant::now).elapsed(),
                                mempool_stream_connected_at.get().map(|at| at.elapsed())
                            ).await {
                                MempoolDrainVerdict::NotShutdown | MempoolDrainVerdict::ShutdownNotDrained => {
                                    continue 'scan;
                                }
                                MempoolDrainVerdict::ShutdownAndDrainComplete => {
                                    break 'continuous_sync;
                                }
                            }
                        }
                        SyncMode::Running => (),
                        SyncMode::NotRunning => {
                            panic!("sync mode should not be manually set to NotRunning!");
                        },
                    }

                    if check_for_new_blocks && scanner.is_verified() {
                        check_for_new_blocks = false;
                        first_verification_complete = true;
                        continue 'continuous_sync;
                    }

                    scanner.update(&mut *wallet.write().await, nullifier_map_limit_exceeded).await?;

                    if matches!(scanner.state, ScannerState::Complete) && config.shutdown_on_completion {
                        let _ignore_mode_before_completion = SyncMode::apply(&sync_mode, SyncMode::on_completion)?;
                    }
                }

                _check_new_block_mined = continuous_sync_interval.tick() => {
                    // if the mempool has not triggered a new block within the time expected, force a new block check.
                    check_for_new_blocks = true;
                }
            }
        }
    }

    expire_transactions(&mut *wallet.write().await)?;

    // shutdown workers and loader
    while scanner.worker_poolsize() != 0 {
        let worker_id = scanner
            .workers
            .first()
            .expect("non empty in this scope!")
            .id();
        scanner.shutdown_worker(worker_id).await;
    }
    scanner.shutdown_loader().await?;

    let mut wallet_guard = wallet.write().await;
    wallet_guard
        .set_save_flag()
        .map_err(SyncError::WalletError)?;
    let sync_status = match sync_status(&*wallet_guard).await {
        Ok(status) => status,
        Err(SyncStatusError::WalletError(e)) => {
            return Err(SyncError::WalletError(e));
        }
        Err(SyncStatusError::NoSyncData) => {
            panic!("sync data must exist!");
        }
        Err(SyncStatusError::SyncProgressChannelClosed) => {
            panic!("unreachable. outside of the context of progress channel.");
        }
    };
    // error is ignored as correct sync status data will be returned in the SyncResult return
    let _ignore_error = progress.send(Some(sync_status.clone()));

    drop(wallet_guard);
    drop(scanner);
    drop(fetch_request_sender);

    match mempool_handle.await.expect("task panicked") {
        Ok(()) => (),
        Err(e @ MempoolError::ShutdownWithoutStream) => tracing::warn!("{e}"),
        Err(e) => return Err(e.into()),
    }
    fetcher_handle.await.expect("task panicked");
    tracing::info!("Sync successfully shutdown.");

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
            let sync_state = wallet
                .get_sync_state_mut()
                .map_err(SyncError::WalletError)?;
            state::truncate_scan_ranges(chain_height, sync_state);
            // the truncated blocks are scanned again when the chain extends. transparent address discovery is only
            // performed at the start of the sync session so the compact block transparent data of these blocks must
            // be scanned.
            state::lower_transparent_scan_floor(sync_state, chain_height);
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

/// Publishes the wallet's current sync status to the progress channel.
async fn publish_sync_status<W>(
    wallet: &W,
    progress: &watch::Sender<Option<SyncStatus>>,
) -> Result<(), SyncStatusError<W::Error>>
where
    W: SyncWallet + SyncBlocks,
{
    match sync_status(wallet).await {
        Ok(status) => {
            progress
                .send(Some(status))
                .map_err(|_| SyncStatusError::SyncProgressChannelClosed)?;
        }
        Err(e) => {
            return Err(e);
        }
    }

    Ok(())
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
    let transparent_output_ids =
        spend::collect_transparent_output_ids(wallet_transactions.values());
    let transparent_spend_scan_targets =
        spend::detect_spends(&pending_transaction_outpoints, &transparent_output_ids);
    let (sapling_derived_nullifiers, orchard_derived_nullifiers, ironwood_derived_nullifiers) =
        spend::collect_derived_nullifiers(wallet_transactions.values());
    let shielded_spend_scan_targets = spend::detect_shielded_spends(
        &pending_transaction_nullifiers,
        &sapling_derived_nullifiers,
        &orchard_derived_nullifiers,
        &ironwood_derived_nullifiers,
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
        && shielded_spend_scan_targets.is_empty()
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
    spend::update_spent_notes(wallet, shielded_spend_scan_targets, false)
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
/// (the nullifier-map entry is pruned once the fully scanned height passes the spending
/// block), so resetting it here would create a permanent phantom unspent note that no
/// forward sync can correct.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MempoolDrainVerdict {
    ///  The mempool stream has been connected for sufficient duration to be shutdown and all recieved mempool
    /// transactions are processed.
    ShutdownAndDrainComplete,
    ///  The mempool stream has been connected for sufficient duration to be shutdown but not all recieved mempool
    /// transactions have been processed.
    ShutdownNotDrained,
    /// The mempool stream has not been connected for sufficient duration to be shutdown.
    NotShutdown,
}

async fn mempool_drain_verdict(
    shutdown_mempool: Arc<AtomicBool>,
    unprocessed_mempool_transactions_count: Arc<AtomicU32>,
    mempool_shutdown_timer: Duration,
    mempool_stream_connection_timer: Option<Duration>,
) -> MempoolDrainVerdict {
    use zingo_netutils::time::{MEMPOOL_DRAIN_CEILING, MEMPOOL_DRAIN_SETTLE};

    if mempool_shutdown_timer < MEMPOOL_DRAIN_CEILING {
        let Some(mempool_elapsed) = mempool_stream_connection_timer else {
            // if mempool stream has not connected yet, continue scanning unless its timed out
            return MempoolDrainVerdict::NotShutdown;
        };
        // wait if the mempool stream has not been connected for sufficient time to receive mempool transactions
        if let Some(mempool_startup_remaining) = MEMPOOL_DRAIN_SETTLE.checked_sub(mempool_elapsed) {
            tokio::time::sleep(mempool_startup_remaining).await;
        }
    }

    // shutdown mempool monitor
    shutdown_mempool.store(true, atomic::Ordering::Release);

    // continue scanning if mempool transaction have been received but haven't finished being preocessed
    if unprocessed_mempool_transactions_count.load(atomic::Ordering::Acquire) > 0 {
        return MempoolDrainVerdict::ShutdownNotDrained;
    }

    MempoolDrainVerdict::ShutdownAndDrainComplete
}

/// Wallet updates from [`process_scan_results`] that must also be applied to the [`Scanner`].
#[derive(Debug, Default)]
struct ProcessedScanResults {
    /// Transparent gap addresses found in use by scanning.
    new_transparent_inuse_addresses: HashMap<String, TransparentAddressId>,
    /// Transparent gap addresses derived to replace the gap addresses found in use.
    new_transparent_gap_addresses: HashMap<String, TransparentAddressId>,
    /// The height of the first block of a scan range that failed the continuity check with the block below it. The
    /// first blocks of the scan range have been set to `Verify`, so the scanner must return to verifying, and a
    /// re-org they detect is measured from this height.
    reorg_detection_start_height: Option<BlockHeight>,
}

/// Applies the scan results of one load to the wallet and returns the transparent addresses the scan found, after a
/// re-org recovery truncates the wallet, and after stale results are discarded with their part of the scan range
/// reset.
#[allow(clippy::too_many_arguments)]
async fn process_scan_results<W>(
    consensus_parameters: &(impl consensus::Parameters + Sync),
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    load: ScanLoad,
    in_flight_tasks: &BTreeMap<TaskId, ScanRange>,
    scan_results: Result<ScanResults, ScanError>,
    initial_reorg_detection_start_height: Option<BlockHeight>,
    performance_level: PerformanceLevel,
    nullifier_map_limit_exceeded: &mut bool,
) -> Result<ProcessedScanResults, SyncError<W::Error>>
where
    W: SyncWallet
        + SyncBlocks
        + SyncTransactions
        + SyncNullifiers
        + SyncOutPoints
        + SyncShardTrees
        + Send,
{
    let ScanLoad {
        task_id,
        scan_range,
    } = load;
    let task = in_flight_tasks
        .get(&task_id)
        .expect("a task stays in flight until its last load is processed");
    let later_tasks = in_flight_tasks
        .range((Bound::Excluded(task_id), Bound::Unbounded))
        .map(|(_, task)| task.block_range().clone())
        .collect::<Vec<_>>();
    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    match state::scan_results_standing(
        sync_state.scan_ranges(),
        &later_tasks,
        task,
        scan_range.block_range(),
    ) {
        ScanResultsStanding::Current => {}
        ScanResultsStanding::Stale { cause, reset } => {
            state::reset_stale_load(sync_state, task.priority(), &reset);
            tracing::info!("Stale scan results of {scan_range} discarded: {cause:?}.");

            return Ok(ProcessedScanResults::default());
        }
    }

    match scan_results {
        Ok(results) => {
            let ScanResults {
                mut nullifiers,
                mut outpoints,
                scanned_blocks,
                mut wallet_transactions,
                sapling_located_trees,
                orchard_located_trees,
                ironwood_located_trees,
                new_transparent_inuse_addresses,
                new_transparent_gap_addresses,
            } = results;

            if scan_range.priority() == ScanPriority::ScannedWithoutMapping {
                let first_unscanned_range = wallet
                    .get_sync_state()
                    .map_err(SyncError::WalletError)?
                    .scan_ranges
                    .iter()
                    .find(|scan_range| scan_range.priority() != ScanPriority::Scanned)
                    .expect("the scan range being processed is not yet set to scanned so at least one unscanned range must exist");
                if !first_unscanned_range.encloses(scan_range.block_range()) {
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

                    return Ok(ProcessedScanResults {
                        new_transparent_inuse_addresses,
                        new_transparent_gap_addresses,
                        reorg_detection_start_height: None,
                    });
                }

                // the server requests are made before the wallet is updated, so a failed request leaves the wallet
                // as it was before the scan results were processed.
                //
                // fetch missing block bounds in the case that the load's nullifier budget was reached and the fetch nullifier
                // scan range was split.
                let mut missing_block_bounds = BTreeMap::new();
                let full_refetching_nullifiers_range = wallet
                    .get_sync_state()
                    .map_err(SyncError::WalletError)?
                    .scan_ranges
                    .iter()
                    .find(|&wallet_scan_range| wallet_scan_range.encloses(scan_range.block_range()))
                    .expect("wallet scan range containing scan range should exist!");
                if scan_range.block_range().start
                    != full_refetching_nullifiers_range.block_range().start
                    || scan_range.block_range().end
                        != full_refetching_nullifiers_range.block_range().end
                {
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
                }
                let shielded_spend_scan_targets = spend::locate_shielded_spends(
                    consensus_parameters,
                    &*wallet,
                    fetch_request_sender.clone(),
                    ufvks,
                    &missing_block_bounds,
                    &mut wallet_transactions,
                    &nullifiers,
                )
                .await?;

                if !missing_block_bounds.is_empty() {
                    wallet
                        .append_wallet_blocks(missing_block_bounds)
                        .map_err(SyncError::WalletError)?;
                }
                wallet
                    .extend_wallet_transactions(wallet_transactions)
                    .map_err(SyncError::WalletError)?;
                spend::apply_shielded_spends(
                    consensus_parameters,
                    wallet,
                    shielded_spend_scan_targets,
                )
                .map_err(SyncError::WalletError)?;

                state::set_scanned_scan_range(
                    wallet
                        .get_sync_state_mut()
                        .map_err(SyncError::WalletError)?,
                    scan_range.block_range().clone(),
                    true, // NOTE: although nullifiers are not actually added to the wallet's nullifier map for efficiency, there is effectively no difference as spends are still updated using the re-fetched nullifiers and would be removed on the following cleanup (`remove_irrelevant_data`) due to `ScannedWithoutMapping` ranges always being the first non-scanned range and therefore always raise the wallet's fully scanned height after processing.
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
                        && query_scan_range.encloses(scan_range.block_range())
                    {
                        map_nullifiers = true;
                        break;
                    }
                }

                // the server requests that spend detection depends on are made before the wallet is updated, so a
                // failed request leaves the wallet as it was before the scan results were processed. the spends are
                // located with the wallet only read and the scanned data passed in beside it, and the spending
                // transactions that are fetched join the scanned transactions.
                let transparent_spend_scan_targets = spend::locate_transparent_spends(
                    consensus_parameters,
                    &*wallet,
                    fetch_request_sender.clone(),
                    ufvks,
                    &scanned_blocks,
                    &mut wallet_transactions,
                    &outpoints,
                )
                .await?;
                let shielded_spend_scan_targets = spend::locate_shielded_spends(
                    consensus_parameters,
                    &*wallet,
                    fetch_request_sender.clone(),
                    ufvks,
                    &scanned_blocks,
                    &mut wallet_transactions,
                    &nullifiers,
                )
                .await?;

                update_wallet_data(
                    consensus_parameters,
                    wallet,
                    fetch_request_sender,
                    ufvks,
                    &scan_range,
                    if map_nullifiers {
                        Some(&mut nullifiers)
                    } else {
                        None
                    },
                    // compact block transparent inputs at or below the transparent scan floor are not collected
                    // during scanning so all outpoints are mapped.
                    &mut outpoints,
                    wallet_transactions,
                    sapling_located_trees,
                    orchard_located_trees,
                    ironwood_located_trees,
                    &new_transparent_inuse_addresses,
                )
                .await?;
                spend::apply_transparent_spends(wallet, transparent_spend_scan_targets)
                    .map_err(SyncError::WalletError)?;
                spend::apply_shielded_spends(
                    consensus_parameters,
                    wallet,
                    shielded_spend_scan_targets,
                )
                .map_err(SyncError::WalletError)?;
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

            Ok(ProcessedScanResults {
                new_transparent_inuse_addresses,
                new_transparent_gap_addresses,
                reorg_detection_start_height: None,
            })
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

                state::reset_verify_scan_range(sync_state, scan_range.block_range());

                // extend verification range to VERIFY_BLOCK_RANGE_SIZE blocks below current verification range
                let current_reorg_detection_start_height = state::set_verify_scan_range(
                    sync_state,
                    height - 1,
                    state::VerifyEnd::VerifyHighest,
                )
                .block_range()
                .start;
                state::merge_scan_ranges(sync_state, ScanPriority::Verify);

                if initial_reorg_detection_start_height
                    .expect("re-org can only be detected in wallets that have synced previously!")
                    - current_reorg_detection_start_height
                    > MAX_REORG_ALLOWANCE
                {
                    clear_wallet_data(wallet)?;

                    return Err(ServerError::ChainVerificationError.into());
                }

                let reorg_truncate_height = current_reorg_detection_start_height - 1;
                truncate_wallet_data(wallet, reorg_truncate_height)?;
                let sync_state = wallet
                    .get_sync_state_mut()
                    .map_err(SyncError::WalletError)?;
                // truncation removes the wallet data of every block above the truncation height. the verification
                // range is scanned again, and so is any scanned range above it.
                state::reopen_scan_ranges_inner(sync_state, current_reorg_detection_start_height);
                // transparent address discovery is not performed again during this sync session so the compact block
                // transparent data of the re-orged blocks must be scanned.
                state::lower_transparent_scan_floor(sync_state, reorg_truncate_height);

                // the truncated wallet with its scan ranges reopened is complete as it is. the requests made here
                // only feed the initial sync state, which reports the progress of this session and is calculated
                // again at the start of the next one, so a failed request costs nothing that lasts.
                state::set_initial_state(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    wallet,
                    last_known_chain_height,
                )
                .await?;

                Ok(ProcessedScanResults::default())
            } else if height == scan_range.block_range().start {
                // the first block of the scan range does not follow the block below it, which is held by the wallet
                // or was kept by the loader from an earlier scan. a re-org has replaced that block since.
                // the scan range is scanned again with its first blocks set to `Verify`. if the wallet holds the
                // block below, the continuity check fails again and is then handled as a re-org.
                tracing::info!("Verifying {scan_range} again.");
                let sync_state = wallet
                    .get_sync_state_mut()
                    .map_err(SyncError::WalletError)?;
                state::reset_stale_load(
                    sync_state,
                    task.priority(),
                    std::slice::from_ref(scan_range.block_range()),
                );
                state::set_verify_scan_range(sync_state, height, state::VerifyEnd::VerifyLowest);
                state::merge_scan_ranges(sync_state, ScanPriority::Verify);

                Ok(ProcessedScanResults {
                    reorg_detection_start_height: Some(height),
                    ..Default::default()
                })
            } else {
                Err(scan_results
                    .expect_err("must be error variant in this scope")
                    .into())
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
            Err(truncate_to_pool_activation_height(
                consensus_parameters,
                fetch_request_sender.clone(),
                wallet,
                pool,
                height,
                block_metadata_size,
                calculated_size,
            )
            .await?)
        }
        Err(e) => Err(e.into()),
    }
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

    // the server requests are made before the wallet is updated, so a failed request leaves the wallet as it was
    // and the next sync session finds the same disagreement.
    let frontiers =
        client::get_frontiers(fetch_request_sender.clone(), consensus_parameters, birthday).await?;
    let missing_block_bound = state::fetch_reopened_block_bound(
        consensus_parameters,
        fetch_request_sender,
        &*wallet,
        rescan_from,
    )
    .await?;

    // the shard trees of the pools that are kept are rolled back to the rescan height before the records are
    // truncated, as the rollback is the only step of the wallet update that can fail. a failed rollback leaves the
    // records as they were.
    let shard_trees = wallet
        .get_shard_trees_mut()
        .map_err(SyncError::WalletError)?;
    for pool in [
        ShieldedPool::Sapling,
        ShieldedPool::Orchard,
        ShieldedPool::Ironwood,
    ] {
        if pool < target_pool {
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

    truncate_stores(wallet, rescan_from - 1, false)?;

    // the shard trees of the cleared pools are rebuilt from the frontier at the wallet birthday and the subtree roots
    // fetched in the next sync session, so the shard ranges of these pools must also be rebuilt from those subtree
    // roots.
    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    for pool in [
        ShieldedPool::Sapling,
        ShieldedPool::Orchard,
        ShieldedPool::Ironwood,
    ] {
        if pool >= target_pool {
            state::clear_shard_ranges(sync_state, pool);
        }
    }

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
        if pool >= target_pool {
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
        }
    }

    state::reopen_scan_ranges_from(wallet, rescan_from, missing_block_bound)
        .map_err(SyncError::WalletError)?;
    add_scan_targets(
        wallet
            .get_sync_state_mut()
            .map_err(SyncError::WalletError)?,
        &rescan_targets,
    );
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
    let checkpoint_count = tree.store().checkpoint_count().expect("infallible");
    tree.store()
        .for_each_checkpoint(checkpoint_count, |height, _| {
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

/// Rolls the shard trees back to the wallet's highest scanned height where they hold state from blocks above it.
///
/// A sync session that ends partway through a wallet update leaves the note commitments and checkpoints of blocks
/// in the shard trees while the scan ranges of these blocks are still to be scanned. Scanning the blocks again
/// inserts the same note commitments, unless a re-org has replaced the blocks since. The note commitments of the
/// replacing blocks then conflict with the stale ones on every sync session, as re-org handling only truncates
/// blocks the wallet records as scanned.
///
/// A rollback also removes the pool's subtree roots above the highest scanned height. [`update_subtree_roots`]
/// fetches them again in the same sync session.
fn rollback_unscanned_tree_states<W>(wallet: &mut W) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncShardTrees,
{
    let Some(highest_scanned_height) = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .highest_scanned_height()
    else {
        return Ok(());
    };

    for pool in [
        ShieldedPool::Sapling,
        ShieldedPool::Orchard,
        ShieldedPool::Ironwood,
    ] {
        let shard_trees = wallet
            .get_shard_trees_mut()
            .map_err(SyncError::WalletError)?;
        let rolled_back = match pool {
            ShieldedPool::Sapling => {
                rollback_unscanned_tree_state(highest_scanned_height, &mut shard_trees.sapling)
            }
            ShieldedPool::Orchard => {
                rollback_unscanned_tree_state(highest_scanned_height, &mut shard_trees.orchard)
            }
            ShieldedPool::Ironwood => {
                rollback_unscanned_tree_state(highest_scanned_height, &mut shard_trees.ironwood)
            }
        }?;
        if rolled_back {
            tracing::warn!(
                "{pool:?} shard tree held state above the highest scanned height \
                 {highest_scanned_height} and was rolled back to its checkpoint at this height."
            );
            wallet.set_save_flag().map_err(SyncError::WalletError)?;
        }
    }

    Ok(())
}

/// Rolls `tree` back to its checkpoint at `highest_scanned_height` where [`truncate::plan_unscanned_state_rollback`]
/// finds state above this height. Returns whether the tree was rolled back.
///
/// shardtree refuses to roll back to a checkpoint whose note commitment the tree is yet to hold or has pruned into
/// a larger subtree. Such a tree is left as it is.
fn rollback_unscanned_tree_state<H, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    highest_scanned_height: BlockHeight,
    tree: &mut ShardTree<MemoryShardStore<H, BlockHeight>, DEPTH, SHARD_HEIGHT>,
) -> Result<bool, shardtree::error::ShardTreeError<Infallible>>
where
    H: incrementalmerkletree::Hashable + Clone + PartialEq,
{
    let Some(checkpoint) = truncate::plan_unscanned_state_rollback(
        truncate::tree_facts(tree, highest_scanned_height),
        highest_scanned_height,
    ) else {
        return Ok(false);
    };

    match tree.rollback_to_checkpoint(checkpoint) {
        Ok(RollbackOutcome::RolledBack) => Ok(true),
        Ok(RollbackOutcome::NoSuchCheckpoint) => panic!("checkpoint must exist in this scope"),
        Err(shardtree::error::ShardTreeError::Query(
            shardtree::error::QueryError::CheckpointPruned,
        )) => {
            tracing::warn!(
                "shard tree holds state above the highest scanned height {highest_scanned_height} \
                 and cannot roll back to its checkpoint at this height."
            );
            Ok(false)
        }
        Err(e) => Err(e),
    }
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
            truncate_stores(wallet, consensus::H0, false)?;
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
    state::truncate_scan_ranges(
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
    consensus_parameters: &(impl consensus::Parameters + Sync),
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_range: &ScanRange,
    nullifiers: Option<&mut NullifierMap>,
    outpoints: &mut BTreeMap<OutputId, ScanTarget>,
    mut transactions: HashMap<TxId, WalletTransaction>,
    sapling_located_trees: Vec<LocatedTreeData<sapling_crypto::Node>>,
    orchard_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
    ironwood_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
    new_transparent_inuse_addresses: &HashMap<String, TransparentAddressId>,
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
    let highest_scanned_height = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .highest_scanned_height()
        .expect("scan ranges should not be empty in this scope");

    // the updates that may fail are applied before any other, so a failure leaves the wallet without the rest of
    // the scan results. address discovery only adds addresses of the wallet's own keys, and the shard trees fetch
    // every checkpoint they are missing before they are updated.
    let discovered_addresses = discover_unified_addresses(ufvks, transactions.values());
    add_discovered_addresses(wallet, discovered_addresses).map_err(SyncError::WalletError)?;
    wallet
        .update_shard_trees(
            consensus_parameters,
            fetch_request_sender,
            scan_range,
            highest_scanned_height,
            witness::anchor_retention_policy(consensus_parameters),
            sapling_located_trees,
            orchard_located_trees,
            ironwood_located_trees,
        )
        .await?;

    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
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

    wallet
        .extend_wallet_transactions(transactions)
        .map_err(SyncError::WalletError)?;
    if let Some(nullifiers) = nullifiers {
        wallet
            .append_nullifiers(nullifiers)
            .map_err(SyncError::WalletError)?;
    }
    wallet
        .append_outpoints(outpoints)
        .map_err(SyncError::WalletError)?;
    let wallet_transparent_addresses = wallet
        .get_transparent_addresses_mut()
        .map_err(SyncError::WalletError)?;
    for (address, id) in new_transparent_inuse_addresses {
        wallet_transparent_addresses.insert(*id, address.clone());
    }

    Ok(())
}

struct DiscoveredAddresses {
    orchard: Vec<(AccountId, orchard::Address, zip32::DiversifierIndex)>,
    sapling: Vec<(
        AccountId,
        sapling_crypto::PaymentAddress,
        zip32::DiversifierIndex,
    )>,
}

fn discover_unified_addresses<'a>(
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    transactions: impl Iterator<Item = &'a WalletTransaction>,
) -> DiscoveredAddresses {
    let mut discovered = DiscoveredAddresses {
        orchard: Vec::new(),
        sapling: Vec::new(),
    };
    for transaction in transactions {
        discover_into(
            ufvks,
            transaction.orchard_notes(),
            resolve_orchard,
            &mut discovered.orchard,
        );
        // Ironwood recipients are orchard receivers, discovered the same way.
        discover_into(
            ufvks,
            transaction.ironwood_notes(),
            resolve_orchard,
            &mut discovered.orchard,
        );
        discover_into(
            ufvks,
            transaction.sapling_notes(),
            resolve_sapling,
            &mut discovered.sapling,
        );
    }

    discovered
}

fn discover_into<N, Address>(
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    notes: &[N],
    resolve: impl Fn(
        &UnifiedFullViewingKey,
        &N::ZcashNote,
    ) -> Option<(Address, zip32::DiversifierIndex)>,
    discovered: &mut Vec<(AccountId, Address, zip32::DiversifierIndex)>,
) where
    N: NoteInterface<KeyId = keys::KeyId>,
{
    discovered.extend(
        notes
            .iter()
            .filter(|note| note.key_id().scope == zip32::Scope::External)
            .map(|note| {
                let account_id = note.key_id().account_id();
                let ufvk = ufvks
                    .get(&account_id)
                    .expect("ufvk must exist to decrypt this note");
                let (address, index) =
                    resolve(ufvk, note.note()).expect("must be key used to create this address");
                (account_id, address, index)
            }),
    );
}

macro_rules! resolve_shielded {
    (
        $name:ident: $note:ty => $address:ty,
        $pool:ident . $ivk:ident ( $($ivk_arg:expr),* ) . $diversifier:ident
    ) => {
        fn $name(
            ufvk: &UnifiedFullViewingKey,
            note: &$note,
        ) -> Option<($address, zip32::DiversifierIndex)> {
            let address = note.recipient();
            let index = ufvk
                .$pool()
                .expect("fvk must exist to decrypt this note")
                .$ivk($($ivk_arg),*)
                .$diversifier(&address)?;
            Some((address, index))
        }
    };
}

resolve_shielded!(
    resolve_orchard: orchard::Note => orchard::Address,
    orchard.to_ivk(zip32::Scope::External).diversifier_index
);
resolve_shielded!(
    resolve_sapling: sapling_crypto::Note => sapling_crypto::PaymentAddress,
    sapling.to_external_ivk().decrypt_diversifier
);

/// - Adds each discovered orchard address to the wallet's unified address list.
/// - Adds each discovered sapling address to the wallet's unified address list.
fn add_discovered_addresses<W>(
    wallet: &mut W,
    discovered: DiscoveredAddresses,
) -> Result<(), W::Error>
where
    W: SyncWallet,
{
    for (account_id, address, diversifier_index) in discovered.orchard {
        wallet.add_orchard_address(account_id, address, diversifier_index)?;
    }
    for (account_id, address, diversifier_index) in discovered.sapling {
        wallet.add_sapling_address(account_id, address, diversifier_index)?;
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
    // Ironwood subtree roots are only required where NU6.3 exists. A server
    // that does not serve them is an error, as the ironwood subtree roots of
    // any shards below the shards inserted by scanning would never be fetched.
    let ironwood_subtree_roots = if consensus_parameters
        .activation_height(consensus::NetworkUpgrade::Nu6_3)
        .is_some()
    {
        ironwood_subtree_roots?
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

/// Re-derives the pinned anchor-checkpoint set of each shard tree from the retention policy in
/// force for `consensus_parameters`, as of the wallet's newest scanned block.
fn repin_anchor_checkpoints<W>(
    consensus_parameters: &impl consensus::Parameters,
    wallet: &mut W,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncShardTrees,
{
    let Some(policy) = witness::anchor_retention_policy(consensus_parameters) else {
        return Ok(());
    };
    let Some(highest_scanned_height) = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .highest_scanned_height()
    else {
        return Ok(());
    };
    let window = witness::anchor_retention_window(&policy, highest_scanned_height);
    let shard_trees = wallet
        .get_shard_trees_mut()
        .map_err(SyncError::WalletError)?;
    witness::repin_anchor_checkpoints(&policy, &window, shard_trees.sapling.store_mut());
    witness::repin_anchor_checkpoints(&policy, &window, shard_trees.orchard.store_mut());
    witness::repin_anchor_checkpoints(&policy, &window, shard_trees.ironwood.store_mut());

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
        let frontiers =
            client::get_frontiers(fetch_request_sender, consensus_parameters, birthday).await?;
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
/// Returns once `shutdown_mempool` is set.
async fn mempool_monitor<C>(
    mut client: C,
    mempool_transaction_sender: mpsc::Sender<MempoolMessage>,
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
        // checked before every stream request so a refused request is only retried while the sync session is running.
        if shutdown_mempool.load(atomic::Ordering::Acquire) {
            break 'main;
        }

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
                            match mempool_stream_message {
                                Ok(Some(raw_transaction)) => {
                                     // counted before sending so the drain verdict never observes a zero count
                                     // while a transaction is queued in the channel.
                                     unprocessed_transactions_count.fetch_add(1, atomic::Ordering::Release);
                                     match mempool_transaction_sender
                                        .send(MempoolMessage::Transaction(raw_transaction))
                                        .await {
                                            Ok(_) => (),
                                            Err(_) => {
                                                unprocessed_transactions_count.store(0, atomic::Ordering::Release);
                                                shutdown_mempool.store(true, atomic::Ordering::Release);
                                                break 'main;
                                            }
                                        }
                                }
                                Ok(None) => {
                                     match mempool_transaction_sender
                                        .send(MempoolMessage::NewBlockMined)
                                        .await {
                                            Ok(_) => {
                                                continue 'main;
                                            }
                                            Err(_) => {
                                                unprocessed_transactions_count.store(0, atomic::Ordering::Release);
                                                shutdown_mempool.store(true, atomic::Ordering::Release);
                                                break 'main;
                                            }
                                        }
                                }
                                Err(e) => {
                                    tracing::warn!("Mempool error: {e}");
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

/// Transaction status will be set to `Failed` if it's still unconfirmed when the wallet's fully scanned height
/// reaches its expiry height.
///
/// Transactions with an expiry height of 0 never expire (ZIP-203).
///
/// A transaction mined in a block that is not yet scanned is never marked `Failed`, so this is safe to call at any
/// point of a sync session.
fn expire_transactions<W>(wallet: &mut W) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncTransactions,
{
    let fully_scanned_height = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .fully_scanned_height()
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
                && fully_scanned_height >= expiry_height
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
    use zcash_protocol::consensus::BlockHeight;
    use zcash_protocol::local_consensus::LocalNetwork;

    /// A local network with every upgrade active from `height`.
    const fn activated_at(height: u32) -> LocalNetwork {
        let activation = Some(BlockHeight::from_u32(height));
        LocalNetwork {
            overwinter: activation,
            sapling: activation,
            blossom: activation,
            heartwood: activation,
            canopy: activation,
            nu5: activation,
            nu6: activation,
            nu6_1: activation,
            nu6_2: activation,
            nu6_3: activation,
        }
    }

    /// A local network with every upgrade active from block 1.
    const NETWORK: LocalNetwork = activated_at(1);

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

        /// The balance path reads the newest sapling checkpoint as the anchor.
        fn assert_every_tree_holds_a_checkpoint(wallet: &mut crate::mocks::MockWallet) {
            use crate::wallet::traits::SyncShardTrees;
            use shardtree::store::ShardStore as _;

            let shard_trees = wallet.get_shard_trees_mut().unwrap();
            let sapling = shard_trees.sapling.store().max_checkpoint_id().unwrap();
            let orchard = shard_trees.orchard.store().max_checkpoint_id().unwrap();
            let ironwood = shard_trees.ironwood.store().max_checkpoint_id().unwrap();
            assert!(
                sapling.is_some() && orchard.is_some() && ironwood.is_some(),
                "a shard tree lost its last checkpoint: sapling {sapling:?}, orchard {orchard:?}, ironwood {ironwood:?}"
            );
        }

        /// A target below the birthday clears every store but keeps a checkpoint.
        #[test]
        fn clear_all_truncation_leaves_a_checkpoint_in_every_tree() {
            let mut shard_trees = ShardTrees::new();
            for height in 6..=10u32 {
                shard_trees
                    .sapling
                    .append_checkpoint(BlockHeight::from_u32(height))
                    .unwrap();
            }
            let mut wallet = synced_wallet(shard_trees);

            truncate_wallet_data(&mut wallet, BlockHeight::from_u32(3)).unwrap();

            assert!(wallet.get_wallet_block(BlockHeight::from_u32(6)).is_err());
            assert_every_tree_holds_a_checkpoint(&mut wallet);
        }

        /// The rescan recovery must also leave a checkpoint in every tree.
        #[test]
        fn rescan_recovery_leaves_a_checkpoint_in_every_tree() {
            let mut shard_trees = ShardTrees::new();
            for height in 9..=10u32 {
                shard_trees
                    .orchard
                    .append_checkpoint(BlockHeight::from_u32(height))
                    .unwrap();
            }
            let mut wallet = synced_wallet(shard_trees);

            let result = truncate_wallet_data(&mut wallet, BlockHeight::from_u32(8));

            assert!(matches!(
                result,
                Err(crate::error::SyncError::TruncationError(_, _))
            ));
            assert_every_tree_holds_a_checkpoint(&mut wallet);
        }
    }

    /// The repair [`crate::sync::rollback_unscanned_tree_states`] applies at the start of a sync session: a shard
    /// tree holding state from blocks above the highest scanned height rolls back to its checkpoint at this height,
    /// and every other tree is left as it is.
    mod unscanned_shard_tree_state {
        use std::convert::Infallible;

        use incrementalmerkletree::{Marking, Position, Retention};
        use orchard::tree::MerkleHashOrchard;
        use shardtree::error::ShardTreeError;
        use shardtree::store::{Checkpoint, ShardStore as _};
        use zcash_protocol::consensus::BlockHeight;
        use zingo_netutils::lightwallet_protocol::SubtreeRoot;

        use crate::mocks::{MockWallet, MockWalletBuilder};
        use crate::sync::{ScanPriority, ScanRange, rollback_unscanned_tree_states};
        use crate::wallet::{ShardTrees, SyncState, traits::SyncShardTrees};
        use crate::witness;

        const BIRTHDAY: u32 = 6;
        const HIGHEST_SCANNED: u32 = 10;
        const NEXT_BLOCK: u32 = HIGHEST_SCANNED + 1;
        /// Each block holds one orchard note commitment, so the block above the highest scanned height inserts here.
        const NEXT_BLOCK_POSITION: u64 = (NEXT_BLOCK - BIRTHDAY) as u64;

        const SCANNED_CHAIN: u8 = 1;
        const STALE_BLOCK: u8 = 2;
        const REPLACING_BLOCK: u8 = 3;

        /// A note commitment that differs for each `chain_tag`.
        fn note_commitment(chain_tag: u8) -> MerkleHashOrchard {
            let mut bytes = [0; 32];
            bytes[0] = chain_tag;
            MerkleHashOrchard::from_bytes(&bytes).expect("a small value is a valid note commitment")
        }

        /// The note commitment of the block at `height` with the checkpoint scanning gives the last note commitment
        /// of a block.
        fn block_leaf(chain_tag: u8, height: u32) -> (MerkleHashOrchard, Retention<BlockHeight>) {
            (
                note_commitment(chain_tag),
                Retention::Checkpoint {
                    id: BlockHeight::from_u32(height),
                    marking: Marking::None,
                },
            )
        }

        /// Shard trees with the orchard note commitments of every block from the birthday to the highest scanned
        /// height.
        fn scanned_shard_trees() -> ShardTrees {
            let mut shard_trees = ShardTrees::new();
            for height in BIRTHDAY..=HIGHEST_SCANNED {
                let (leaf, retention) = block_leaf(SCANNED_CHAIN, height);
                shard_trees.orchard.append(leaf, retention).unwrap();
            }

            shard_trees
        }

        /// Inserts the orchard note commitment of the block above the highest scanned height as scan results do.
        fn insert_next_block(
            shard_trees: &mut ShardTrees,
            chain_tag: u8,
        ) -> Result<(), ShardTreeError<Infallible>> {
            const LOCATED_TREE_SIZE: usize = 1;
            for located_tree in witness::build_located_trees(
                Position::from(NEXT_BLOCK_POSITION),
                vec![block_leaf(chain_tag, NEXT_BLOCK)],
                LOCATED_TREE_SIZE,
            ) {
                shard_trees
                    .orchard
                    .insert_tree(located_tree.subtree, located_tree.checkpoints)?;
            }

            Ok(())
        }

        /// A wallet scanned from the birthday to the highest scanned height with the block above still to be
        /// scanned.
        fn wallet(shard_trees: ShardTrees) -> MockWallet {
            let sync_state = SyncState::new_for_test(vec![
                ScanRange::from_parts(
                    BlockHeight::from_u32(BIRTHDAY)..BlockHeight::from_u32(NEXT_BLOCK),
                    ScanPriority::Scanned,
                ),
                ScanRange::from_parts(
                    BlockHeight::from_u32(NEXT_BLOCK)..BlockHeight::from_u32(NEXT_BLOCK + 1),
                    ScanPriority::Verify,
                ),
            ]);
            MockWalletBuilder::new()
                .birthday(BlockHeight::from_u32(BIRTHDAY))
                .sync_state(sync_state)
                .shard_trees(shard_trees)
                .create_mock_wallet()
        }

        /// A sync session ended after it inserted the note commitments of the block above the highest scanned
        /// height and before it recorded the block as scanned. A re-org then replaced the block, so scanning it
        /// again conflicts with the stale note commitment. The rollback removes the stale note commitment and its
        /// checkpoint, and the replacing block inserts.
        #[test]
        fn stale_block_is_rolled_back_and_its_replacement_inserts() {
            let mut shard_trees = scanned_shard_trees();
            insert_next_block(&mut shard_trees, STALE_BLOCK).unwrap();
            assert!(
                insert_next_block(&mut shard_trees, REPLACING_BLOCK).is_err(),
                "the replacing block must conflict with the stale block for this test to hold"
            );
            let mut wallet = wallet(shard_trees);

            rollback_unscanned_tree_states(&mut wallet).unwrap();

            let shard_trees = wallet.get_shard_trees_mut().unwrap();
            assert_eq!(
                shard_trees.orchard.max_leaf_position(None).unwrap(),
                Some(Position::from(NEXT_BLOCK_POSITION - 1)),
                "only the note commitments of scanned blocks are kept"
            );
            assert_eq!(
                shard_trees.orchard.store().max_checkpoint_id().unwrap(),
                Some(BlockHeight::from_u32(HIGHEST_SCANNED))
            );
            insert_next_block(shard_trees, REPLACING_BLOCK)
                .expect("the replacing block inserts into the rolled back tree");
        }

        /// A wallet whose shard trees hold only scanned blocks keeps the subtree roots above its highest scanned
        /// height, which a wallet that is behind the chain tip holds.
        #[test]
        fn scanned_shard_trees_are_left_as_they_are() {
            const SUBTREE_ABOVE_HIGHEST_SCANNED: usize = 1;
            let mut shard_trees = scanned_shard_trees();
            witness::add_subtree_roots(
                SUBTREE_ABOVE_HIGHEST_SCANNED,
                vec![SubtreeRoot {
                    root_hash: note_commitment(SCANNED_CHAIN).to_bytes().to_vec(),
                    ..Default::default()
                }],
                &mut shard_trees.orchard,
            )
            .unwrap();
            let mut wallet = wallet(shard_trees);

            rollback_unscanned_tree_states(&mut wallet).unwrap();

            assert_eq!(
                witness::stored_subtree_root_count(&wallet.get_shard_trees_mut().unwrap().orchard),
                SUBTREE_ABOVE_HIGHEST_SCANNED + 1
            );
        }

        /// The orchard pool has no note commitments in the scanned blocks or in the block above them, so the
        /// checkpoints of these blocks point to the last note commitment of a block below the scanned range, which
        /// the tree is yet to hold. shardtree refuses to roll back to such a checkpoint, so the session starts with
        /// the tree as it is.
        #[test]
        fn shard_tree_that_cannot_roll_back_is_left_as_it_is() {
            let mut shard_trees = ShardTrees::new();
            for height in [HIGHEST_SCANNED, NEXT_BLOCK] {
                shard_trees
                    .orchard
                    .store_mut()
                    .add_checkpoint(
                        BlockHeight::from_u32(height),
                        Checkpoint::at_position(Position::from(NEXT_BLOCK_POSITION - 1)),
                    )
                    .unwrap();
            }
            let mut wallet = wallet(shard_trees);

            rollback_unscanned_tree_states(&mut wallet).unwrap();

            let shard_trees = wallet.get_shard_trees_mut().unwrap();
            assert_eq!(
                shard_trees.orchard.store().max_checkpoint_id().unwrap(),
                Some(BlockHeight::from_u32(NEXT_BLOCK))
            );
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

    /// The mode a completed scan leaves behind, exercised as a table on the pure step and once through the atomic.
    mod on_completion {
        use std::sync::atomic::AtomicU8;

        use crate::wallet::SyncMode;

        #[test]
        fn running_sync_is_shutdown() {
            assert_eq!(SyncMode::Running.on_completion(), SyncMode::Shutdown);
        }

        #[test]
        fn paused_sync_stays_paused() {
            assert_eq!(SyncMode::Paused.on_completion(), SyncMode::Paused);
        }

        #[test]
        fn shutdown_requested_by_the_consumer_is_kept() {
            assert_eq!(SyncMode::Shutdown.on_completion(), SyncMode::Shutdown);
        }

        #[test]
        fn sync_that_is_not_running_is_left_alone() {
            assert_eq!(SyncMode::NotRunning.on_completion(), SyncMode::NotRunning);
        }

        #[test]
        fn apply_stores_the_step_and_returns_the_mode_it_replaced() {
            let sync_mode = AtomicU8::new(SyncMode::Running as u8);

            let before = SyncMode::apply(&sync_mode, SyncMode::on_completion).unwrap();

            assert_eq!(before, SyncMode::Running);
            assert_eq!(
                SyncMode::from_atomic_u8(&sync_mode).unwrap(),
                SyncMode::Shutdown
            );
        }

        #[test]
        fn transition_leaves_another_mode_in_place_and_reports_it() {
            let sync_mode = AtomicU8::new(SyncMode::Paused as u8);

            let before =
                SyncMode::transition(&sync_mode, SyncMode::Running, SyncMode::Shutdown).unwrap();

            assert_eq!(before, SyncMode::Paused);
            assert_eq!(
                SyncMode::from_atomic_u8(&sync_mode).unwrap(),
                SyncMode::Paused
            );
        }
    }

    /// The drain policy for scanner shutdown, exercised as a table:
    /// pure inputs, no runtime, no clocks.
    mod drain_verdict {
        use std::{
            sync::{
                Arc,
                atomic::{AtomicBool, AtomicU32},
            },
            time::Duration,
        };

        use zingo_netutils::time::MEMPOOL_DRAIN_CEILING;

        use crate::sync::{MempoolDrainVerdict, mempool_drain_verdict};

        /// One row of the drain-policy table:
        /// (workers, unprocessed, connected_for, poll_elapsed, verdict, label).
        type DrainCase = (
            Arc<AtomicBool>,
            Arc<AtomicU32>,
            Duration,
            Option<Duration>,
            MempoolDrainVerdict,
            &'static str,
        );

        #[tokio::test]
        async fn table() {
            let cases: &[DrainCase] = &[
                // The reported bug: first-loop shutdown on a fully
                // synced chain, stream not yet connected. Hold the
                // session open instead of closing it instantly.
                (
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicU32::new(0)),
                    Duration::from_millis(0),
                    None,
                    MempoolDrainVerdict::NotShutdown,
                    "no stream yet",
                ),
                // Connected but inside the settle window: waits before checking whether there is work to process.
                (
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicU32::new(0)),
                    Duration::from_millis(100),
                    Some(Duration::from_millis(50)),
                    MempoolDrainVerdict::ShutdownAndDrainComplete,
                    "settling",
                ),
                // The typical session: stream connected long ago,
                // scanner drained. Immediate shutdown, no added cost.
                (
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicU32::new(0)),
                    Duration::from_millis(0),
                    Some(Duration::from_millis(1_400)),
                    MempoolDrainVerdict::ShutdownAndDrainComplete,
                    "settled and drained",
                ),
                // Unprocessed work always re-enters the processing
                // loop, whatever the stream state. No deadline caps
                // the processing itself.
                (
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicU32::new(3)),
                    Duration::from_millis(3_000),
                    Some(Duration::from_millis(1_400)),
                    MempoolDrainVerdict::ShutdownNotDrained,
                    "unprocessed work",
                ),
                // Ceiling with a stream that never connected: the
                // pre-c90f8d309 semantics. A dead stream must not
                // hold the session open.
                // Written as the ceiling itself: a literal chosen against
                // one value of it stops testing the ceiling the moment the
                // constant moves.
                (
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicU32::new(0)),
                    MEMPOOL_DRAIN_CEILING,
                    None,
                    MempoolDrainVerdict::ShutdownAndDrainComplete,
                    "ceiling without stream",
                ),
            ];
            for (shutdown_mempool, unprocessed, connected_ms, elapsed_ms, expected, name) in cases {
                assert_eq!(
                    mempool_drain_verdict(
                        shutdown_mempool.clone(),
                        unprocessed.clone(),
                        *connected_ms,
                        *elapsed_ms,
                    )
                    .await,
                    *expected,
                    "{name}"
                );

                // TODO: assert shutdown_mempool is true is cases where mempool should be shutdown
            }
        }
    }

    /// The lifecycle of a note's spend mark across spend detection and `reset_spends`.
    ///
    /// The wallet remembers an on-chain spend observation in exactly one durable place: the
    /// note's `spending_transaction` field. The nullifier map is a transient rendezvous
    /// buffer: detection only reads it, and its entries are pruned behind the
    /// fully-scanned frontier (`remove_irrelevant_data`). These tests pin the
    /// consequences for `set_transactions_failed`, whose `reset_spends` call erases that
    /// one durable place.
    mod spend_reset_lifecycle {
        use std::collections::HashMap;

        use sapling_crypto::value::NoteValue;
        use zcash_keys::keys::UnifiedSpendingKey;
        use zcash_primitives::transaction::TxId;
        use zcash_protocol::{
            consensus::{BlockHeight, MAIN_NETWORK},
            memo::Memo,
        };
        use zingo_status::confirmation_status::ConfirmationStatus;

        use crate::{
            mocks::{MockWallet, MockWalletBuilder},
            sync::{set_transactions_failed, spend},
            wallet::{
                NullifierMap, OutputId, ScanTarget, WalletNote, WalletTransaction,
                traits::{SyncNullifiers, SyncTransactions},
            },
        };

        pub(super) const FUNDING_HEIGHT: BlockHeight = BlockHeight::from_u32(10);
        pub(super) const SPEND_HEIGHT: BlockHeight = BlockHeight::from_u32(100);
        pub(super) const FUNDING_TXID: TxId = TxId::from_bytes([1; 32]);
        pub(super) const SPENDING_TXID: TxId = TxId::from_bytes([2; 32]);
        pub(super) const NOTE_NULLIFIER: sapling_crypto::Nullifier =
            sapling_crypto::Nullifier([42; 32]);

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
        /// The spending key of the wallet's only account, which received the funding note.
        pub(super) fn spending_key() -> UnifiedSpendingKey {
            UnifiedSpendingKey::from_seed(&MAIN_NETWORK, &[0; 32], zip32::AccountId::ZERO).unwrap()
        }

        pub(super) fn funding_transaction(spending_transaction: Option<TxId>) -> WalletTransaction {
            let (_, recipient) = spending_key().sapling().default_address();
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

        /// The detection pass `locate_shielded_spends` and `apply_shielded_spends` perform,
        /// minus the network round trips and the scan prioritisation: match derived note
        /// nullifiers against the wallet's nullifier map and mark the matches spent.
        fn run_spend_detection(wallet: &mut MockWallet) {
            let (sapling_nullifiers, orchard_nullifiers, ironwood_nullifiers) =
                spend::collect_derived_nullifiers(
                    wallet.get_wallet_transactions().unwrap().values(),
                );
            let spend_scan_targets = spend::detect_shielded_spends(
                wallet.get_nullifiers().unwrap(),
                &sapling_nullifiers,
                &orchard_nullifiers,
                &ironwood_nullifiers,
            );
            spend::update_spent_notes(wallet, spend_scan_targets, true).unwrap();
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
            // Detection only reads the map: the entry stays until cleanup prunes it, so a
            // detection pass that fails part way through is repeated on the next scan.
            assert!(
                wallet
                    .get_nullifiers()
                    .unwrap()
                    .sapling
                    .contains_key(&NOTE_NULLIFIER)
            );
        }

        /// Failure-marking a transaction that is `Confirmed` on chain must not
        /// destroy the wallet's knowledge of the spend.
        ///
        /// Once cleanup has pruned the nullifier-map entry behind the fully-scanned
        /// frontier, the note's `spending_transaction` field is the wallet's only durable
        /// record of the on-chain spend. If `set_transactions_failed` erased it, the note
        /// would become a permanent phantom unspent note: every future proposal selects it
        /// and is rejected as a double-spend, and no forward sync can correct it.
        ///
        /// The damage is permanent once the spending block has been scanned and the
        /// fully-scanned frontier has passed it, and the record is `Confirmed` from scan
        /// time on. If the reset lands while the entry is still mapped, the next detection
        /// pass re-observes the nullifier and heals the wallet
        /// (see [`reset_before_scan_heals_on_spend_detection`]). A `Confirmed` record is
        /// therefore the earliest point at which the harm can occur, which is why the
        /// failure path is guarded on `Confirmed` status.
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
            // The map entry was pruned by cleanup behind the fully-scanned frontier: empty map.
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

    /// Transparent spend detection must leave the spending transaction confirmed in the wallet.
    ///
    /// Compact block scanning maps the transparent inputs of every transaction above the transparent scan floor but
    /// only targets a transaction for a full scan when one of its outputs pays the wallet. A transaction that spends
    /// a coin in full to an external recipient is therefore known to the wallet only as the pending record created
    /// when it was sent.
    mod transparent_spend_without_change {
        use std::collections::{BTreeMap, HashMap};

        use tokio::sync::mpsc;
        use zcash_primitives::{block::BlockHash, transaction::TxId};
        use zcash_protocol::{consensus::BlockHeight, value::Zatoshis};
        use zcash_transparent::{address::Script, keys::NonHardenedChildIndex};
        use zingo_netutils::lightwallet_protocol::RawTransaction;
        use zingo_status::confirmation_status::ConfirmationStatus;

        use crate::{
            client::FetchRequest,
            error::SyncError,
            keys::transparent::{TransparentAddressId, TransparentScope},
            mocks::{MockWallet, MockWalletBuilder, MockWalletError},
            sync::spend,
            wallet::{
                OutputId, ScanTarget, TransparentCoin, TreeBounds, WalletBlock, WalletTransaction,
                traits::{SyncOutPoints, SyncTransactions},
            },
        };

        use super::NETWORK;
        const FUNDING_HEIGHT: BlockHeight = BlockHeight::from_u32(10);
        const SPEND_HEIGHT: BlockHeight = BlockHeight::from_u32(100);
        const FUNDING_TXID: TxId = TxId::from_bytes([1; 32]);

        /// The spending transaction's wallet record in the given lifecycle state, keyed by the txid the server
        /// returns it under.
        fn spending_record(status: ConfirmationStatus) -> WalletTransaction {
            let txid = WalletTransaction::new_for_test(TxId::from_bytes([0; 32]), status)
                .transaction()
                .txid();
            WalletTransaction::new_for_test(txid, status)
        }

        /// A wallet holding a confirmed coin, the given record of the transaction that spends it, and the spend's
        /// outpoint as mapped from the compact block at `SPEND_HEIGHT`.
        fn wallet_with_mapped_spend(spending_record: WalletTransaction) -> MockWallet {
            let coin_id = OutputId::new(FUNDING_TXID, 0);
            let coin = TransparentCoin {
                output_id: coin_id,
                key_id: TransparentAddressId::new(
                    zip32::AccountId::ZERO,
                    TransparentScope::External,
                    NonHardenedChildIndex::ZERO,
                ),
                address: String::new(),
                script: Script::default(),
                value: Zatoshis::const_from_u64(100_000),
                spending_transaction: None,
            };
            let funding_transaction = WalletTransaction::new_for_test(
                FUNDING_TXID,
                ConfirmationStatus::Confirmed(FUNDING_HEIGHT),
            )
            .with_transparent_coins_for_test(vec![coin]);
            let spend_scan_target = ScanTarget {
                block_height: SPEND_HEIGHT,
                txid: spending_record.txid(),
                narrow_scan_area: true,
            };

            MockWalletBuilder::new()
                .wallet_transactions(HashMap::from([
                    (FUNDING_TXID, funding_transaction),
                    (spending_record.txid(), spending_record),
                ]))
                .outpoint_map(BTreeMap::from([(coin_id, spend_scan_target)]))
                .create_mock_wallet()
        }

        fn scanned_blocks() -> BTreeMap<BlockHeight, WalletBlock> {
            BTreeMap::from([(
                SPEND_HEIGHT,
                WalletBlock {
                    block_height: SPEND_HEIGHT,
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
                },
            )])
        }

        /// Answers transaction requests with `transaction` mined at `SPEND_HEIGHT`.
        fn spawn_fetcher(transaction: &WalletTransaction) -> mpsc::UnboundedSender<FetchRequest> {
            let mut data = Vec::new();
            transaction.transaction().write(&mut data).unwrap();
            let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                while let Some(fetch_request) = fetch_request_receiver.recv().await {
                    match fetch_request {
                        FetchRequest::Transaction(reply_sender, _txid) => {
                            let _ignore_error = reply_sender.send(Ok(RawTransaction {
                                data: data.clone(),
                                height: u64::from(SPEND_HEIGHT),
                            }));
                        }
                        _ => panic!("unexpected fetch request"),
                    }
                }
            });

            fetch_request_sender
        }

        fn get_coin_spending_txid(wallet: &MockWallet) -> Option<TxId> {
            wallet
                .get_wallet_transactions()
                .unwrap()
                .get(&FUNDING_TXID)
                .unwrap()
                .transparent_coins()
                .first()
                .unwrap()
                .spending_transaction
        }

        /// Locates the transparent spends of the wallet's coins and records them in the wallet, as scan results with
        /// no transactions or outpoints of their own are processed.
        async fn update_transparent_spends(
            wallet: &mut MockWallet,
            fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        ) -> Result<(), SyncError<MockWalletError>> {
            let mut scanned_transactions = HashMap::new();
            let transparent_spend_scan_targets = spend::locate_transparent_spends(
                &NETWORK,
                &*wallet,
                fetch_request_sender,
                &HashMap::new(),
                &scanned_blocks(),
                &mut scanned_transactions,
                &BTreeMap::new(),
            )
            .await?;
            wallet
                .extend_wallet_transactions(scanned_transactions)
                .unwrap();
            spend::apply_transparent_spends(wallet, transparent_spend_scan_targets).unwrap();

            Ok(())
        }

        #[tokio::test]
        async fn pending_spending_transaction_is_fetched_and_confirmed() {
            let spending_record = spending_record(ConfirmationStatus::Mempool(SPEND_HEIGHT));
            let spending_txid = spending_record.txid();
            let fetch_request_sender = spawn_fetcher(&spending_record);
            let mut wallet = wallet_with_mapped_spend(spending_record);

            update_transparent_spends(&mut wallet, fetch_request_sender)
                .await
                .unwrap();

            assert_eq!(
                wallet
                    .get_wallet_transactions()
                    .unwrap()
                    .get(&spending_txid)
                    .unwrap()
                    .status(),
                ConfirmationStatus::Confirmed(SPEND_HEIGHT)
            );
            assert_eq!(get_coin_spending_txid(&wallet), Some(spending_txid));
            // the spend stays mapped until cleanup prunes it behind the fully scanned height
            assert!(
                wallet
                    .get_outpoints()
                    .unwrap()
                    .contains_key(&OutputId::new(FUNDING_TXID, 0))
            );
        }

        /// A spending transaction the scanner already confirmed is marked on the coin without fetching the transaction
        /// from the server again.
        #[tokio::test]
        async fn confirmed_spending_transaction_is_not_fetched() {
            let spending_record = spending_record(ConfirmationStatus::Confirmed(SPEND_HEIGHT));
            let spending_txid = spending_record.txid();
            let mut wallet = wallet_with_mapped_spend(spending_record);
            let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
            // drop receiver so the test fails if the wallet attempts to fetch the transaction again unnecessarily
            drop(fetch_request_receiver);

            update_transparent_spends(&mut wallet, fetch_request_sender)
                .await
                .unwrap();

            assert_eq!(get_coin_spending_txid(&wallet), Some(spending_txid));
        }

        /// The request for the spending transaction fails. The wallet's outpoint map keeps the spend, so the next
        /// sync session detects it again, and the coin is not yet marked as spent.
        #[tokio::test]
        async fn failed_fetch_keeps_the_mapped_spend() {
            let spending_record = spending_record(ConfirmationStatus::Mempool(SPEND_HEIGHT));
            let mut wallet = wallet_with_mapped_spend(spending_record);
            let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
            drop(fetch_request_receiver);

            let result = update_transparent_spends(&mut wallet, fetch_request_sender).await;

            assert!(result.is_err());
            assert!(
                wallet
                    .get_outpoints_mut()
                    .unwrap()
                    .contains_key(&OutputId::new(FUNDING_TXID, 0)),
                "the spend must stay mapped until it is recorded on the coin"
            );
            assert_eq!(get_coin_spending_txid(&wallet), None);
        }
    }

    mod checked_height_validation {
        use zcash_protocol::consensus::BlockHeight;
        use zcash_protocol::local_consensus::LocalNetwork;
        /// Sapling activates above the first block, so a birthday can fall below it.
        const LOCAL_NETWORK: LocalNetwork = LocalNetwork {
            overwinter: Some(BlockHeight::from_u32(1)),
            ..super::activated_at(3)
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
                wallet::{SyncState, traits::SyncWallet as _},
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
            /// Blocks above the chain height are truncated and scanned again when the chain extends. Transparent
            /// address discovery only covers the blocks at or below the transparent scan floor, and is not performed
            /// again during the sync session, so a floor above the chain height is lowered to it for the compact block
            /// transparent data of the truncated blocks to be scanned. A floor at or below the chain height still
            /// covers every block the wallet keeps.
            #[tokio::test]
            async fn above_chain_height_lowers_transparent_scan_floor() {
                const LAST_KNOWN_HEIGHT: BlockHeight = BlockHeight::from_u32(110);
                const CHAIN_HEIGHT: BlockHeight = BlockHeight::from_u32(105);
                const FLOOR_ABOVE_CHAIN_HEIGHT: BlockHeight = BlockHeight::from_u32(108);
                const FLOOR_BELOW_CHAIN_HEIGHT: BlockHeight = BlockHeight::from_u32(100);

                for (floor, expected_floor) in [
                    (FLOOR_ABOVE_CHAIN_HEIGHT, CHAIN_HEIGHT),
                    (CHAIN_HEIGHT, CHAIN_HEIGHT),
                    (FLOOR_BELOW_CHAIN_HEIGHT, FLOOR_BELOW_CHAIN_HEIGHT),
                ] {
                    let state = SyncState {
                        scan_ranges: vec![ScanRange::from_parts(
                            DEFAULT_START_HEIGHT..LAST_KNOWN_HEIGHT + 1,
                            crate::sync::ScanPriority::Scanned,
                        )],
                        transparent_scan_floor: Some(floor),
                        ..Default::default()
                    };
                    let mut test_wallet = crate::mocks::MockWalletBuilder::new()
                        .sync_state(state)
                        .create_mock_wallet();

                    let last_known_chain_height =
                        checked_wallet_height(&mut test_wallet, CHAIN_HEIGHT, &LOCAL_NETWORK)
                            .unwrap();

                    assert_eq!(last_known_chain_height, CHAIN_HEIGHT);
                    assert_eq!(
                        test_wallet.get_sync_state().unwrap().transparent_scan_floor,
                        Some(expected_floor),
                        "floor {floor}"
                    );
                }
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

        const UNSCANNED_START: u32 = 51;
        const UNSCANNED_END: u32 = 91;
        const CHAIN_HEIGHT: u32 = 200;

        /// Creates a mock wallet with all blocks scanned up to `chain_height`.
        fn wallet_at_height(chain_height: u32, transactions: Vec<WalletTransaction>) -> MockWallet {
            wallet_with_scan_ranges(
                vec![ScanRange::from_parts(
                    BlockHeight::from_u32(1)..BlockHeight::from_u32(chain_height + 1),
                    ScanPriority::Scanned,
                )],
                transactions,
            )
        }

        /// Creates a mock wallet whose blocks up to [`CHAIN_HEIGHT`] are scanned, except for the range from
        /// [`UNSCANNED_START`] to [`UNSCANNED_END`].
        fn wallet_with_unscanned_range(transactions: Vec<WalletTransaction>) -> MockWallet {
            wallet_with_scan_ranges(
                vec![
                    ScanRange::from_parts(
                        BlockHeight::from_u32(1)..BlockHeight::from_u32(UNSCANNED_START),
                        ScanPriority::Scanned,
                    ),
                    ScanRange::from_parts(
                        BlockHeight::from_u32(UNSCANNED_START)
                            ..BlockHeight::from_u32(UNSCANNED_END),
                        ScanPriority::Historic,
                    ),
                    ScanRange::from_parts(
                        BlockHeight::from_u32(UNSCANNED_END)
                            ..BlockHeight::from_u32(CHAIN_HEIGHT + 1),
                        ScanPriority::Scanned,
                    ),
                ],
                transactions,
            )
        }

        fn wallet_with_scan_ranges(
            scan_ranges: Vec<ScanRange>,
            transactions: Vec<WalletTransaction>,
        ) -> MockWallet {
            let sync_state = SyncState {
                scan_ranges,
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
        fn pending_transaction_with_expiry_above_an_unscanned_range_is_untouched() {
            let txid = TxId::from_bytes([1; 32]);
            let transaction = WalletTransaction::new_for_test_with_expiry(
                txid,
                ConfirmationStatus::Mempool(BlockHeight::from_u32(UNSCANNED_START - 1)),
                BlockHeight::from_u32(UNSCANNED_END),
            );
            let mut wallet = wallet_with_unscanned_range(vec![transaction]);

            expire_transactions(&mut wallet).unwrap();

            assert!(matches!(
                transaction_status(&wallet, txid),
                ConfirmationStatus::Mempool(_)
            ));
        }

        #[test]
        fn pending_transaction_with_expiry_below_an_unscanned_range_is_failed() {
            let txid = TxId::from_bytes([1; 32]);
            let transaction = WalletTransaction::new_for_test_with_expiry(
                txid,
                ConfirmationStatus::Mempool(BlockHeight::from_u32(1)),
                BlockHeight::from_u32(UNSCANNED_START - 1),
            );
            let mut wallet = wallet_with_unscanned_range(vec![transaction]);

            expire_transactions(&mut wallet).unwrap();

            assert!(matches!(
                transaction_status(&wallet, txid),
                ConfirmationStatus::Failed(_)
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

    /// Stale scan results, whether a scan succeeded or failed, are discarded before they are processed.
    mod stale_scan_results {
        use std::collections::{BTreeMap, HashMap};

        use tokio::sync::mpsc;
        use zcash_primitives::block::BlockHash;
        use zcash_protocol::consensus::BlockHeight;
        use zcash_transparent::keys::NonHardenedChildIndex;

        use crate::{
            config::PerformanceLevel,
            error::{ContinuityError, ScanError},
            keys::transparent::{TransparentAddressId, TransparentScope},
            mocks::{MockWallet, MockWalletBuilder},
            scan::{
                ScanResults,
                task::{ScanLoad, TaskId},
            },
            sync::{ProcessedScanResults, ScanPriority, ScanRange, process_scan_results},
            wallet::{NullifierMap, SyncState, traits::SyncWallet as _},
        };

        use super::NETWORK;
        const BIRTHDAY: u32 = 1;
        /// The chain height the server reported when the scan range was selected.
        const CHAIN_HEIGHT: u32 = 40;

        fn wallet_with_scan_ranges(scan_ranges: Vec<ScanRange>) -> MockWallet {
            MockWalletBuilder::new()
                .sync_state(SyncState {
                    scan_ranges,
                    ..Default::default()
                })
                .create_mock_wallet()
        }

        async fn process(
            wallet: &mut MockWallet,
            scan_range: ScanRange,
            scan_results: Result<ScanResults, ScanError>,
        ) -> ProcessedScanResults {
            let (fetch_request_sender, _) = mpsc::unbounded_channel();
            let task_id = TaskId::first();
            let in_flight_tasks = BTreeMap::from([(task_id, scan_range.clone())]);

            process_scan_results(
                &NETWORK,
                wallet,
                fetch_request_sender,
                &HashMap::new(),
                ScanLoad {
                    task_id,
                    scan_range,
                },
                &in_flight_tasks,
                scan_results,
                None,
                PerformanceLevel::High,
                &mut false,
            )
            .await
            .expect("stale scan results are discarded")
        }

        /// The chain height dropped by one block while the wallet's whole range was being scanned. The scan results
        /// hold a block that has left the chain, so they are discarded with the transparent gap addresses derived
        /// from them, and the range the wallet still holds is set back to the priority it was selected with.
        #[tokio::test]
        async fn truncated_scan_range_is_reset_and_its_scan_results_discarded() {
            let mut wallet = wallet_with_scan_ranges(vec![ScanRange::from_parts(
                BlockHeight::from_u32(BIRTHDAY)..BlockHeight::from_u32(CHAIN_HEIGHT),
                ScanPriority::Scanning,
            )]);
            let gap_address = (
                "gap address".to_string(),
                TransparentAddressId::new(
                    zip32::AccountId::ZERO,
                    TransparentScope::External,
                    NonHardenedChildIndex::ZERO,
                ),
            );

            let processed = process(
                &mut wallet,
                ScanRange::from_parts(
                    BlockHeight::from_u32(BIRTHDAY)..BlockHeight::from_u32(CHAIN_HEIGHT + 1),
                    ScanPriority::ChainTip,
                ),
                Ok(ScanResults {
                    nullifiers: NullifierMap::new(),
                    outpoints: BTreeMap::new(),
                    scanned_blocks: BTreeMap::new(),
                    wallet_transactions: HashMap::new(),
                    sapling_located_trees: Vec::new(),
                    orchard_located_trees: Vec::new(),
                    ironwood_located_trees: Vec::new(),
                    new_transparent_inuse_addresses: HashMap::from([gap_address.clone()]),
                    new_transparent_gap_addresses: HashMap::from([gap_address]),
                }),
            )
            .await;

            assert!(processed.new_transparent_inuse_addresses.is_empty());
            assert!(processed.new_transparent_gap_addresses.is_empty());
            assert_eq!(
                wallet.get_sync_state().unwrap().scan_ranges(),
                [ScanRange::from_parts(
                    BlockHeight::from_u32(BIRTHDAY)..BlockHeight::from_u32(CHAIN_HEIGHT),
                    ScanPriority::ChainTip,
                )]
            );
        }

        /// The chain height dropped below a newly mined block while it was being verified, so the wallet holds none
        /// of its scan range. Its scan failed the continuity check, which is handled as a re-org of the scan range.
        /// The error is discarded as the scan range it would reset has left the wallet.
        #[tokio::test]
        async fn error_of_a_removed_scan_range_is_discarded() {
            let scan_ranges = vec![ScanRange::from_parts(
                BlockHeight::from_u32(BIRTHDAY)..BlockHeight::from_u32(CHAIN_HEIGHT),
                ScanPriority::Scanned,
            )];
            let mut wallet = wallet_with_scan_ranges(scan_ranges.clone());

            process(
                &mut wallet,
                ScanRange::from_parts(
                    BlockHeight::from_u32(CHAIN_HEIGHT)..BlockHeight::from_u32(CHAIN_HEIGHT + 1),
                    ScanPriority::Verify,
                ),
                Err(ScanError::ContinuityError(
                    ContinuityError::HashDiscontinuity {
                        height: BlockHeight::from_u32(CHAIN_HEIGHT),
                        prev_hash: BlockHash([1; 32]),
                        previous_block_hash: BlockHash([2; 32]),
                    },
                )),
            )
            .await;

            assert_eq!(wallet.get_sync_state().unwrap().scan_ranges(), scan_ranges);
        }
    }

    /// The checkpoints [`crate::wallet::traits::SyncShardTrees::update_shard_trees`] adds for the blocks of a scan
    /// range in a pool that has no note commitments in these blocks. The pool's commitment tree is unchanged as of
    /// such a block, so its checkpoint copies the tree state of the checkpoint of the block below, which is
    /// fetched from the server where the shard tree is missing it.
    mod shard_tree_checkpoints {
        use std::sync::Arc;
        use std::sync::atomic::{self, AtomicUsize};

        use incrementalmerkletree::frontier::CommitmentTree;
        use incrementalmerkletree::{Hashable as _, Marking, Position, Retention};
        use orchard::tree::MerkleHashOrchard;
        use shardtree::store::{self, ShardStore as _};
        use tokio::sync::mpsc;
        use zcash_primitives::merkle_tree::write_commitment_tree;
        use zcash_protocol::consensus::BlockHeight;
        use zingo_netutils::lightwallet_protocol::TreeState;

        use crate::{
            client::FetchRequest,
            error::SyncError,
            mocks::{MockWallet, MockWalletBuilder, MockWalletError},
            shardtree_ext::ShardTreeExt,
            sync::{ScanPriority, ScanRange},
            wallet::{ShardTrees, traits::SyncShardTrees},
        };

        use super::NETWORK;
        const HIGHEST_SCANNED: u32 = 10;
        const FIRST_SCANNED_BLOCK: u32 = HIGHEST_SCANNED + 1;
        const SECOND_SCANNED_BLOCK: u32 = FIRST_SCANNED_BLOCK + 1;
        const SHIELDED_POOLS: usize = 3;
        /// The orchard note commitments the chain holds as of the scanned blocks.
        const ORCHARD_TREE_SIZE: u64 = 3;

        /// Updates the wallet's shard trees with the scan of the two blocks above the highest scanned height, which
        /// hold no note commitments of any pool.
        async fn update_shard_trees(
            wallet: &mut MockWallet,
            fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        ) -> Result<(), SyncError<MockWalletError>> {
            wallet
                .update_shard_trees(
                    &NETWORK,
                    fetch_request_sender,
                    &ScanRange::from_parts(
                        BlockHeight::from_u32(FIRST_SCANNED_BLOCK)
                            ..BlockHeight::from_u32(SECOND_SCANNED_BLOCK + 1),
                        ScanPriority::ChainTip,
                    ),
                    BlockHeight::from_u32(HIGHEST_SCANNED),
                    None,
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                )
                .await
        }

        fn wallet(shard_trees: ShardTrees) -> MockWallet {
            MockWalletBuilder::new()
                .shard_trees(shard_trees)
                .create_mock_wallet()
        }

        /// A fetcher whose every request fails.
        fn failing_fetcher() -> mpsc::UnboundedSender<FetchRequest> {
            let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
            drop(fetch_request_receiver);

            fetch_request_sender
        }

        /// Answers tree state requests with an orchard commitment tree of `ORCHARD_TREE_SIZE` note commitments and
        /// empty sapling and ironwood commitment trees, counting the requests.
        fn spawn_counting_fetcher() -> (mpsc::UnboundedSender<FetchRequest>, Arc<AtomicUsize>) {
            const EMPTY_TREE: &str = "000000";
            let mut orchard_tree = CommitmentTree::<
                MerkleHashOrchard,
                { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
            >::empty();
            for _ in 0..ORCHARD_TREE_SIZE {
                orchard_tree
                    .append(MerkleHashOrchard::empty_leaf())
                    .unwrap();
            }
            let mut orchard_tree_bytes = Vec::new();
            write_commitment_tree(&orchard_tree, &mut orchard_tree_bytes).unwrap();
            let orchard_tree = hex::encode(orchard_tree_bytes);

            let request_count = Arc::new(AtomicUsize::new(0));
            let counted_requests = request_count.clone();
            let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                while let Some(fetch_request) = fetch_request_receiver.recv().await {
                    match fetch_request {
                        FetchRequest::TreeState(reply_sender, block_height) => {
                            counted_requests.fetch_add(1, atomic::Ordering::Release);
                            let _ignore_error = reply_sender.send(Ok(TreeState {
                                height: u64::from(block_height),
                                hash: "00".repeat(32),
                                sapling_tree: EMPTY_TREE.to_string(),
                                orchard_tree: orchard_tree.clone(),
                                ironwood_tree: EMPTY_TREE.to_string(),
                                ..Default::default()
                            }));
                        }
                        _ => panic!("unexpected fetch request"),
                    }
                }
            });

            (fetch_request_sender, request_count)
        }

        /// The tree states of the sapling, orchard and ironwood checkpoints at `height`.
        fn checkpoint_tree_states(
            wallet: &mut MockWallet,
            height: u32,
        ) -> [Option<store::TreeState>; SHIELDED_POOLS] {
            let height = BlockHeight::from_u32(height);
            let shard_trees = wallet.get_shard_trees_mut().unwrap();
            [
                shard_trees.sapling.store().get_checkpoint(&height).unwrap(),
                shard_trees.orchard.store().get_checkpoint(&height).unwrap(),
                shard_trees
                    .ironwood
                    .store()
                    .get_checkpoint(&height)
                    .unwrap(),
            ]
            .map(|checkpoint| checkpoint.map(|checkpoint| checkpoint.tree_state()))
        }

        /// Every shard tree holds the checkpoint of the highest scanned block, so the checkpoints of the scanned
        /// blocks copy its tree state with no request to the server.
        #[tokio::test]
        async fn checkpoints_copy_the_block_below() {
            let mut shard_trees = ShardTrees::new();
            let highest_scanned_height = BlockHeight::from_u32(HIGHEST_SCANNED);
            shard_trees
                .sapling
                .append_checkpoint(highest_scanned_height)
                .unwrap();
            shard_trees
                .orchard
                .append(
                    MerkleHashOrchard::empty_leaf(),
                    Retention::Checkpoint {
                        id: highest_scanned_height,
                        marking: Marking::None,
                    },
                )
                .unwrap();
            shard_trees
                .ironwood
                .append_checkpoint(highest_scanned_height)
                .unwrap();
            let mut wallet = wallet(shard_trees);

            update_shard_trees(&mut wallet, failing_fetcher())
                .await
                .unwrap();

            let highest_scanned_tree_states = [
                Some(store::TreeState::Empty),
                Some(store::TreeState::AtPosition(Position::from(0))),
                Some(store::TreeState::Empty),
            ];
            assert_eq!(
                checkpoint_tree_states(&mut wallet, FIRST_SCANNED_BLOCK),
                highest_scanned_tree_states
            );
            assert_eq!(
                checkpoint_tree_states(&mut wallet, SECOND_SCANNED_BLOCK),
                highest_scanned_tree_states
            );
        }

        /// The shard trees are missing the checkpoint of the highest scanned block, so the tree state of the first
        /// scanned block is fetched for each pool. The checkpoint of the second scanned block copies it with no
        /// further request.
        #[tokio::test]
        async fn missing_checkpoint_below_is_fetched_once_for_each_pool() {
            let mut wallet = wallet(ShardTrees::new());
            let (fetch_request_sender, request_count) = spawn_counting_fetcher();

            update_shard_trees(&mut wallet, fetch_request_sender)
                .await
                .unwrap();

            let fetched_tree_states = [
                Some(store::TreeState::Empty),
                Some(store::TreeState::AtPosition(Position::from(
                    ORCHARD_TREE_SIZE - 1,
                ))),
                Some(store::TreeState::Empty),
            ];
            assert_eq!(
                checkpoint_tree_states(&mut wallet, FIRST_SCANNED_BLOCK),
                fetched_tree_states
            );
            assert_eq!(
                checkpoint_tree_states(&mut wallet, SECOND_SCANNED_BLOCK),
                fetched_tree_states
            );
            assert_eq!(
                request_count.load(atomic::Ordering::Acquire),
                SHIELDED_POOLS
            );
        }

        /// The sapling tree holds the checkpoint of the highest scanned block and the orchard tree is missing it.
        /// The request for the orchard tree state fails, so the sapling checkpoint that was determined before the
        /// request is left out of the sapling tree.
        #[tokio::test]
        async fn failed_request_leaves_the_shard_trees_as_they_were() {
            let mut shard_trees = ShardTrees::new();
            shard_trees
                .sapling
                .append_checkpoint(BlockHeight::from_u32(HIGHEST_SCANNED))
                .unwrap();
            let mut wallet = wallet(shard_trees);

            let result = update_shard_trees(&mut wallet, failing_fetcher()).await;

            assert!(result.is_err());
            assert_eq!(
                checkpoint_tree_states(&mut wallet, FIRST_SCANNED_BLOCK),
                [None; SHIELDED_POOLS]
            );
        }
    }

    /// Scan results are added to the wallet together, once every server request the update depends on is answered.
    mod scan_results_wallet_update {
        use std::collections::{BTreeMap, HashMap};

        use incrementalmerkletree::{Hashable as _, Marking, Position, Retention};
        use orchard::tree::MerkleHashOrchard;
        use shardtree::store::ShardStore as _;
        use tokio::sync::mpsc;
        use zcash_primitives::{block::BlockHash, transaction::TxId};
        use zcash_protocol::consensus::BlockHeight;
        use zingo_netutils::lightwallet_protocol::{RawTransaction, TreeState};
        use zingo_status::confirmation_status::ConfirmationStatus;

        use super::spend_reset_lifecycle::{
            FUNDING_HEIGHT, FUNDING_TXID, NOTE_NULLIFIER, SPEND_HEIGHT, funding_transaction,
            spending_key,
        };
        use crate::{
            client::FetchRequest,
            config::PerformanceLevel,
            error::SyncError,
            mocks::{MockWallet, MockWalletBuilder, MockWalletError},
            scan::{
                ScanResults,
                task::{ScanLoad, TaskId},
            },
            sync::{ProcessedScanResults, ScanPriority, ScanRange, process_scan_results},
            wallet::{
                NullifierMap, OutputId, ScanTarget, SyncState, TreeBounds, WalletBlock,
                WalletTransaction,
                traits::{
                    SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions,
                    SyncWallet,
                },
            },
            witness,
        };

        use super::NETWORK;
        const BIRTHDAY: u32 = 1;
        /// A transaction of the wallet that the scan of the block at `SPEND_HEIGHT` found by trial decryption.
        const SCANNED_TXID: TxId = TxId::from_bytes([3; 32]);

        /// The transaction that spends the wallet's note with no change, under the txid the server returns it with.
        fn spending_transaction() -> WalletTransaction {
            let status = ConfirmationStatus::Confirmed(SPEND_HEIGHT);
            let txid = WalletTransaction::new_for_test(TxId::from_bytes([0; 32]), status)
                .transaction()
                .txid();
            WalletTransaction::new_for_test(txid, status)
        }

        /// Answers tree state requests with empty note commitment trees, and transaction requests with
        /// `spending_transaction` mined at `SPEND_HEIGHT`. With no spending transaction, transaction requests fail.
        fn spawn_fetcher(
            spending_transaction: Option<&WalletTransaction>,
        ) -> mpsc::UnboundedSender<FetchRequest> {
            const EMPTY_TREE: &str = "000000";
            let spending_transaction_data = spending_transaction.map(|spending_transaction| {
                let mut data = Vec::new();
                spending_transaction.transaction().write(&mut data).unwrap();
                data
            });
            let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                while let Some(fetch_request) = fetch_request_receiver.recv().await {
                    match fetch_request {
                        FetchRequest::TreeState(reply_sender, block_height) => {
                            let _ignore_error = reply_sender.send(Ok(TreeState {
                                height: u64::from(block_height),
                                hash: "00".repeat(32),
                                sapling_tree: EMPTY_TREE.to_string(),
                                orchard_tree: EMPTY_TREE.to_string(),
                                ironwood_tree: EMPTY_TREE.to_string(),
                                ..Default::default()
                            }));
                        }
                        FetchRequest::Transaction(reply_sender, _txid) => {
                            // the request fails when the reply sender is dropped.
                            if let Some(data) = &spending_transaction_data {
                                let _ignore_error = reply_sender.send(Ok(RawTransaction {
                                    data: data.clone(),
                                    height: u64::from(SPEND_HEIGHT),
                                }));
                            }
                        }
                        _ => panic!("unexpected fetch request"),
                    }
                }
            });

            fetch_request_sender
        }

        /// The block at `SPEND_HEIGHT`, which holds the first orchard note commitment of the chain.
        fn block_at_spend_height() -> WalletBlock {
            WalletBlock {
                block_height: SPEND_HEIGHT,
                block_hash: BlockHash([0; 32]),
                prev_hash: BlockHash([0; 32]),
                time: 0,
                txids: Vec::new(),
                tree_bounds: TreeBounds {
                    sapling_initial_tree_size: 0,
                    sapling_final_tree_size: 0,
                    orchard_initial_tree_size: 0,
                    orchard_final_tree_size: 1,
                    ironwood_initial_tree_size: 0,
                    ironwood_final_tree_size: 0,
                },
            }
        }

        /// The scan ranges of a wallet scanned up to `SPEND_HEIGHT`, with the block at `SPEND_HEIGHT` in the given
        /// priority.
        fn scan_ranges(spend_height_priority: ScanPriority) -> Vec<ScanRange> {
            vec![
                ScanRange::from_parts(
                    BlockHeight::from_u32(BIRTHDAY)..SPEND_HEIGHT,
                    ScanPriority::Scanned,
                ),
                ScanRange::from_parts(SPEND_HEIGHT..SPEND_HEIGHT + 1, spend_height_priority),
            ]
        }

        /// The scan ranges once the block at `SPEND_HEIGHT` is scanned.
        fn scanned_scan_ranges() -> Vec<ScanRange> {
            vec![ScanRange::from_parts(
                BlockHeight::from_u32(BIRTHDAY)..SPEND_HEIGHT + 1,
                ScanPriority::Scanned,
            )]
        }

        /// A wallet holding one unspent sapling note, with the block at `SPEND_HEIGHT` in the given priority.
        fn wallet_builder(spend_height_priority: ScanPriority) -> MockWalletBuilder {
            MockWalletBuilder::new()
                .birthday(BlockHeight::from_u32(BIRTHDAY))
                .sync_state(SyncState::new_for_test(scan_ranges(spend_height_priority)))
                .wallet_transactions(HashMap::from([(FUNDING_TXID, funding_transaction(None))]))
        }

        /// The block at `FUNDING_HEIGHT`, which adds no note commitments.
        fn block_at_funding_height() -> WalletBlock {
            WalletBlock {
                block_height: FUNDING_HEIGHT,
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

        /// The scan ranges of a wallet scanned up to `SPEND_HEIGHT` except for the block at `FUNDING_HEIGHT`, which
        /// is in the given priority.
        fn funding_scan_ranges(funding_height_priority: ScanPriority) -> Vec<ScanRange> {
            vec![
                ScanRange::from_parts(
                    BlockHeight::from_u32(BIRTHDAY)..FUNDING_HEIGHT,
                    ScanPriority::Scanned,
                ),
                ScanRange::from_parts(FUNDING_HEIGHT..FUNDING_HEIGHT + 1, funding_height_priority),
                ScanRange::from_parts(FUNDING_HEIGHT + 1..SPEND_HEIGHT + 1, ScanPriority::Scanned),
            ]
        }

        /// A wallet that scanned the block at `SPEND_HEIGHT` before the block at `FUNDING_HEIGHT`, so it holds the
        /// nullifier of a note it has not yet received, as mapped from the block at `SPEND_HEIGHT`, where
        /// `spending_txid` spends the note.
        fn wallet_with_mapped_spend(spending_txid: TxId) -> MockWallet {
            MockWalletBuilder::new()
                .birthday(BlockHeight::from_u32(BIRTHDAY))
                .sync_state(SyncState::new_for_test(funding_scan_ranges(
                    ScanPriority::Scanning,
                )))
                .nullifier_map(mapped_spend(spending_txid))
                .wallet_blocks(BTreeMap::from([(SPEND_HEIGHT, block_at_spend_height())]))
                .create_mock_wallet()
        }

        /// The scan results of the block at `FUNDING_HEIGHT`, which holds the transaction funding the wallet's
        /// note and nothing else.
        fn funding_scan_results() -> ScanResults {
            ScanResults {
                nullifiers: NullifierMap::new(),
                outpoints: BTreeMap::new(),
                scanned_blocks: BTreeMap::from([(FUNDING_HEIGHT, block_at_funding_height())]),
                wallet_transactions: HashMap::from([(FUNDING_TXID, funding_transaction(None))]),
                sapling_located_trees: Vec::new(),
                orchard_located_trees: Vec::new(),
                ironwood_located_trees: Vec::new(),
                new_transparent_inuse_addresses: HashMap::new(),
                new_transparent_gap_addresses: HashMap::new(),
            }
        }

        /// The nullifier of the wallet's note as mapped from the block at `SPEND_HEIGHT`, where `spending_txid`
        /// spends the note.
        fn mapped_spend(spending_txid: TxId) -> NullifierMap {
            let mut nullifiers = NullifierMap::new();
            nullifiers.sapling.insert(
                NOTE_NULLIFIER,
                ScanTarget {
                    block_height: SPEND_HEIGHT,
                    txid: spending_txid,
                    narrow_scan_area: false,
                },
            );

            nullifiers
        }

        /// The scan results of the block at `SPEND_HEIGHT`. The block holds a transaction of the wallet, one
        /// orchard note commitment, a transparent input and the spend of the wallet's note by `spending_txid`,
        /// which paid no change and so is missing from the scanned transactions.
        fn scan_results(spending_txid: TxId) -> ScanResults {
            const LOCATED_TREE_SIZE: usize = 1;
            ScanResults {
                nullifiers: mapped_spend(spending_txid),
                outpoints: BTreeMap::from([(
                    OutputId::new(SCANNED_TXID, 0),
                    ScanTarget {
                        block_height: SPEND_HEIGHT,
                        txid: spending_txid,
                        narrow_scan_area: true,
                    },
                )]),
                scanned_blocks: BTreeMap::from([(SPEND_HEIGHT, block_at_spend_height())]),
                wallet_transactions: HashMap::from([(
                    SCANNED_TXID,
                    WalletTransaction::new_for_test(
                        SCANNED_TXID,
                        ConfirmationStatus::Confirmed(SPEND_HEIGHT),
                    ),
                )]),
                sapling_located_trees: Vec::new(),
                orchard_located_trees: witness::build_located_trees(
                    Position::from(0),
                    vec![(
                        MerkleHashOrchard::empty_leaf(),
                        Retention::Checkpoint {
                            id: SPEND_HEIGHT,
                            marking: Marking::None,
                        },
                    )],
                    LOCATED_TREE_SIZE,
                ),
                ironwood_located_trees: Vec::new(),
                new_transparent_inuse_addresses: HashMap::new(),
                new_transparent_gap_addresses: HashMap::new(),
            }
        }

        /// Processes the scan results of the block at `SPEND_HEIGHT`, selected for scanning with
        /// `selected_priority`.
        async fn process(
            wallet: &mut MockWallet,
            fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
            selected_priority: ScanPriority,
            scan_results: ScanResults,
        ) -> Result<ProcessedScanResults, SyncError<MockWalletError>> {
            process_block(
                wallet,
                fetch_request_sender,
                SPEND_HEIGHT,
                selected_priority,
                scan_results,
            )
            .await
        }

        /// Processes the scan results of the block at `block_height`, selected for scanning with
        /// `selected_priority`, for the wallet's only account.
        async fn process_block(
            wallet: &mut MockWallet,
            fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
            block_height: BlockHeight,
            selected_priority: ScanPriority,
            scan_results: ScanResults,
        ) -> Result<ProcessedScanResults, SyncError<MockWalletError>> {
            let ufvks = HashMap::from([(
                zip32::AccountId::ZERO,
                spending_key().to_unified_full_viewing_key(),
            )]);
            let task_id = TaskId::first();
            let scan_range =
                ScanRange::from_parts(block_height..block_height + 1, selected_priority);
            let in_flight_tasks = BTreeMap::from([(task_id, scan_range.clone())]);

            process_scan_results(
                &NETWORK,
                wallet,
                fetch_request_sender,
                &ufvks,
                ScanLoad {
                    task_id,
                    scan_range,
                },
                &in_flight_tasks,
                Ok(scan_results),
                None,
                PerformanceLevel::High,
                &mut false,
            )
            .await
        }

        fn note_spending_txid(wallet: &MockWallet) -> Option<TxId> {
            wallet.get_wallet_transactions().unwrap()[&FUNDING_TXID].sapling_notes()[0]
                .spending_transaction
        }

        fn orchard_tree_size(wallet: &mut MockWallet) -> Option<Position> {
            wallet
                .get_shard_trees_mut()
                .unwrap()
                .orchard
                .max_leaf_position(None)
                .unwrap()
        }

        /// The scan of the block at `SPEND_HEIGHT` maps the nullifier of the wallet's note, which a transaction with
        /// no change spent. Every part of the scan results is added to the wallet: the scanned transaction, the
        /// note commitment with a checkpoint in each shard tree, the spend with its fetched spending transaction,
        /// the block and the scanned scan range.
        #[tokio::test]
        async fn scan_results_are_added_to_the_wallet() {
            let spending_transaction = spending_transaction();
            let spending_txid = spending_transaction.txid();
            let mut wallet = wallet_builder(ScanPriority::Scanning).create_mock_wallet();

            let result = process(
                &mut wallet,
                spawn_fetcher(Some(&spending_transaction)),
                ScanPriority::ChainTip,
                scan_results(spending_txid),
            )
            .await;

            assert!(result.is_ok());
            assert_eq!(note_spending_txid(&wallet), Some(spending_txid));
            let wallet_transactions = wallet.get_wallet_transactions().unwrap();
            assert!(wallet_transactions.contains_key(&SCANNED_TXID));
            assert_eq!(
                wallet_transactions[&spending_txid].status(),
                ConfirmationStatus::Confirmed(SPEND_HEIGHT)
            );
            assert_eq!(orchard_tree_size(&mut wallet), Some(Position::from(0)));
            let shard_trees = wallet.get_shard_trees_mut().unwrap();
            let sapling_checkpoint = shard_trees
                .sapling
                .store()
                .get_checkpoint(&SPEND_HEIGHT)
                .unwrap();
            let orchard_checkpoint = shard_trees
                .orchard
                .store()
                .get_checkpoint(&SPEND_HEIGHT)
                .unwrap();
            let ironwood_checkpoint = shard_trees
                .ironwood
                .store()
                .get_checkpoint(&SPEND_HEIGHT)
                .unwrap();
            assert!(
                sapling_checkpoint.is_some()
                    && orchard_checkpoint.is_some()
                    && ironwood_checkpoint.is_some(),
                "every shard tree must hold a checkpoint at the scanned block"
            );
            assert!(wallet.get_wallet_block(SPEND_HEIGHT).is_ok());
            assert_eq!(
                wallet.get_sync_state().unwrap().scan_ranges(),
                scanned_scan_ranges()
            );
            // the wallet is fully scanned, so the mapped nullifiers and outpoints are all removed.
            assert!(wallet.get_nullifiers().unwrap().sapling.is_empty());
            assert!(wallet.get_outpoints().unwrap().is_empty());
        }

        /// The request for the spending transaction fails, so the wallet is left as it was before the scan results
        /// were processed. The scan range is scanned again in the next sync session, where the spend is detected
        /// again.
        #[tokio::test]
        async fn failed_spending_transaction_request_leaves_the_wallet_as_it_was() {
            let mut wallet = wallet_builder(ScanPriority::Scanning).create_mock_wallet();

            let result = process(
                &mut wallet,
                spawn_fetcher(None),
                ScanPriority::ChainTip,
                scan_results(spending_transaction().txid()),
            )
            .await;

            assert!(result.is_err());
            assert!(wallet.get_nullifiers().unwrap().sapling.is_empty());
            assert!(wallet.get_outpoints().unwrap().is_empty());
            assert_eq!(orchard_tree_size(&mut wallet), None);
            assert!(wallet.get_wallet_block(SPEND_HEIGHT).is_err());
            assert_eq!(wallet.get_wallet_transactions().unwrap().len(), 1);
            assert_eq!(note_spending_txid(&wallet), None);
            assert_eq!(
                wallet.get_sync_state().unwrap().scan_ranges(),
                scan_ranges(ScanPriority::Scanning)
            );
        }

        /// The block at `SPEND_HEIGHT` was scanned with its nullifiers discarded, and the nullifiers were then
        /// fetched again. The spend they map is recorded on the wallet's note with its fetched spending
        /// transaction, and the scan range is scanned. The shard trees already hold the block and are left as they
        /// are.
        #[tokio::test]
        async fn refetched_nullifiers_record_the_spend() {
            let spending_transaction = spending_transaction();
            let spending_txid = spending_transaction.txid();
            let mut wallet = wallet_builder(ScanPriority::RefetchingNullifiers)
                .wallet_blocks(BTreeMap::from([(SPEND_HEIGHT, block_at_spend_height())]))
                .create_mock_wallet();
            let scan_results = ScanResults {
                nullifiers: mapped_spend(spending_txid),
                outpoints: BTreeMap::new(),
                scanned_blocks: BTreeMap::new(),
                wallet_transactions: HashMap::new(),
                sapling_located_trees: Vec::new(),
                orchard_located_trees: Vec::new(),
                ironwood_located_trees: Vec::new(),
                new_transparent_inuse_addresses: HashMap::new(),
                new_transparent_gap_addresses: HashMap::new(),
            };

            let result = process(
                &mut wallet,
                spawn_fetcher(Some(&spending_transaction)),
                ScanPriority::ScannedWithoutMapping,
                scan_results,
            )
            .await;

            assert!(result.is_ok());
            assert_eq!(note_spending_txid(&wallet), Some(spending_txid));
            assert_eq!(
                wallet.get_wallet_transactions().unwrap()[&spending_txid].status(),
                ConfirmationStatus::Confirmed(SPEND_HEIGHT)
            );
            assert_eq!(orchard_tree_size(&mut wallet), None);
            assert_eq!(
                wallet.get_sync_state().unwrap().scan_ranges(),
                scanned_scan_ranges()
            );
        }

        /// The request for the spending transaction fails while the re-fetched nullifiers are processed, so the
        /// wallet is left as it was and the nullifiers are fetched again in the next sync session.
        #[tokio::test]
        async fn failed_request_for_refetched_nullifiers_leaves_the_wallet_as_it_was() {
            let mut wallet = wallet_builder(ScanPriority::RefetchingNullifiers)
                .wallet_blocks(BTreeMap::from([(SPEND_HEIGHT, block_at_spend_height())]))
                .create_mock_wallet();
            let scan_results = ScanResults {
                nullifiers: mapped_spend(spending_transaction().txid()),
                outpoints: BTreeMap::new(),
                scanned_blocks: BTreeMap::new(),
                wallet_transactions: HashMap::new(),
                sapling_located_trees: Vec::new(),
                orchard_located_trees: Vec::new(),
                ironwood_located_trees: Vec::new(),
                new_transparent_inuse_addresses: HashMap::new(),
                new_transparent_gap_addresses: HashMap::new(),
            };

            let result = process(
                &mut wallet,
                spawn_fetcher(None),
                ScanPriority::ScannedWithoutMapping,
                scan_results,
            )
            .await;

            assert!(result.is_err());
            assert_eq!(wallet.get_wallet_transactions().unwrap().len(), 1);
            assert_eq!(note_spending_txid(&wallet), None);
            assert_eq!(
                wallet.get_sync_state().unwrap().scan_ranges(),
                scan_ranges(ScanPriority::RefetchingNullifiers)
            );
        }

        /// The block at `SPEND_HEIGHT` was scanned before the block at `FUNDING_HEIGHT`, so the wallet's nullifier
        /// map holds the spend of a note the wallet has not yet received. Scanning the funding block adds the note
        /// and locates its spend in the wallet's map. The spend is recorded on the note with its fetched spending
        /// transaction, and the wallet is fully scanned, so the cleanup drops the spend from the map.
        #[tokio::test]
        async fn mapped_spend_is_recorded_when_the_funding_block_is_scanned() {
            let spending_transaction = spending_transaction();
            let spending_txid = spending_transaction.txid();
            let mut wallet = wallet_with_mapped_spend(spending_txid);

            let result = process_block(
                &mut wallet,
                spawn_fetcher(Some(&spending_transaction)),
                FUNDING_HEIGHT,
                ScanPriority::Historic,
                funding_scan_results(),
            )
            .await;

            assert!(result.is_ok());
            assert_eq!(note_spending_txid(&wallet), Some(spending_txid));
            assert_eq!(
                wallet.get_wallet_transactions().unwrap()[&spending_txid].status(),
                ConfirmationStatus::Confirmed(SPEND_HEIGHT)
            );
            assert!(wallet.get_wallet_block(FUNDING_HEIGHT).is_ok());
            assert_eq!(
                wallet.get_sync_state().unwrap().scan_ranges(),
                scanned_scan_ranges()
            );
            assert!(wallet.get_nullifiers().unwrap().sapling.is_empty());
        }

        /// The shielded counterpart of `transparent_spend_without_change::failed_fetch_keeps_the_mapped_spend`.
        ///
        /// The wallet's nullifier map holds the spend of the note the funding block adds, and the request for the
        /// spending transaction fails. The spend must stay in the wallet's map with the note not yet received, so
        /// the next sync session scans the funding block and locates the spend again. Spend detection used to
        /// remove the spend from the map before the request was made, and the note was then left unspent for good.
        #[tokio::test]
        async fn failed_request_keeps_the_mapped_spend() {
            let spending_txid = spending_transaction().txid();
            let mut wallet = wallet_with_mapped_spend(spending_txid);

            let result = process_block(
                &mut wallet,
                spawn_fetcher(None),
                FUNDING_HEIGHT,
                ScanPriority::Historic,
                funding_scan_results(),
            )
            .await;

            assert!(result.is_err());
            assert_eq!(
                wallet
                    .get_nullifiers()
                    .unwrap()
                    .sapling
                    .get(&NOTE_NULLIFIER)
                    .map(|scan_target| scan_target.txid),
                Some(spending_txid),
                "the spend must stay mapped until it is recorded on the note"
            );
            assert!(wallet.get_wallet_transactions().unwrap().is_empty());
            assert!(wallet.get_wallet_block(FUNDING_HEIGHT).is_err());
            assert_eq!(
                wallet.get_sync_state().unwrap().scan_ranges(),
                funding_scan_ranges(ScanPriority::Scanning)
            );
        }
    }

    /// A scan fails the continuity check when the first block of its scan range does not follow the block below it.
    mod hash_discontinuity {
        use std::collections::{BTreeMap, HashMap};

        use tokio::sync::mpsc;
        use zcash_primitives::block::BlockHash;
        use zcash_protocol::consensus::BlockHeight;
        use zingo_netutils::lightwallet_protocol::TreeState;

        use crate::{
            client::FetchRequest,
            config::PerformanceLevel,
            error::{ContinuityError, ScanError, SyncError},
            mocks::{MockWallet, MockWalletBuilder, MockWalletError},
            scan::task::{ScanLoad, TaskId},
            sync::{ProcessedScanResults, ScanPriority, ScanRange, process_scan_results},
            wallet::{SyncState, TreeBounds, WalletBlock, traits::SyncWallet as _},
        };

        use super::NETWORK;
        const BIRTHDAY: u32 = 1;
        /// The first block of the scan range that fails the continuity check.
        const SCAN_RANGE_START: u32 = 21;
        const SCAN_RANGE_END: u32 = 41;
        const VERIFY_BLOCK_RANGE_SIZE: u32 = crate::sync::VERIFY_BLOCK_RANGE_SIZE;

        fn scan_range(start: u32, end: u32, priority: ScanPriority) -> ScanRange {
            ScanRange::from_parts(
                BlockHeight::from_u32(start)..BlockHeight::from_u32(end),
                priority,
            )
        }

        fn block(height: u32) -> (BlockHeight, WalletBlock) {
            (
                BlockHeight::from_u32(height),
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
                },
            )
        }

        /// Answers tree state requests with empty note commitment trees.
        fn spawn_fetcher() -> mpsc::UnboundedSender<FetchRequest> {
            const EMPTY_TREE: &str = "000000";
            let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                while let Some(fetch_request) = fetch_request_receiver.recv().await {
                    match fetch_request {
                        FetchRequest::TreeState(reply_sender, block_height) => {
                            let _ignore_error = reply_sender.send(Ok(TreeState {
                                height: u64::from(block_height),
                                hash: "00".repeat(32),
                                sapling_tree: EMPTY_TREE.to_string(),
                                orchard_tree: EMPTY_TREE.to_string(),
                                ironwood_tree: EMPTY_TREE.to_string(),
                                ..Default::default()
                            }));
                        }
                        _ => panic!("unexpected fetch request"),
                    }
                }
            });

            fetch_request_sender
        }

        fn wallet(scan_ranges: Vec<ScanRange>, blocks: Vec<u32>) -> MockWallet {
            MockWalletBuilder::new()
                .birthday(BlockHeight::from_u32(BIRTHDAY))
                .sync_state(SyncState {
                    scan_ranges,
                    ..Default::default()
                })
                .wallet_blocks(blocks.into_iter().map(block).collect::<BTreeMap<_, _>>())
                .create_mock_wallet()
        }

        /// Processes the failed scan of `scan_range`, with the continuity check failing at `height`.
        async fn process_hash_discontinuity(
            wallet: &mut MockWallet,
            scan_range: ScanRange,
            height: u32,
            initial_reorg_detection_start_height: Option<BlockHeight>,
        ) -> Result<ProcessedScanResults, SyncError<MockWalletError>> {
            let task_id = TaskId::first();
            let in_flight_tasks = BTreeMap::from([(task_id, scan_range.clone())]);

            process_scan_results(
                &NETWORK,
                wallet,
                spawn_fetcher(),
                &HashMap::new(),
                ScanLoad {
                    task_id,
                    scan_range,
                },
                &in_flight_tasks,
                Err(ScanError::ContinuityError(
                    ContinuityError::HashDiscontinuity {
                        height: BlockHeight::from_u32(height),
                        prev_hash: BlockHash([1; 32]),
                        previous_block_hash: BlockHash([2; 32]),
                    },
                )),
                initial_reorg_detection_start_height,
                PerformanceLevel::High,
                &mut false,
            )
            .await
        }

        /// A scan range selected with a priority other than `Verify` is set back to that priority with its first
        /// blocks set to `Verify`, and the height to detect a re-org from is returned for the scanner to verify them.
        #[tokio::test]
        async fn first_blocks_of_a_scan_range_are_verified_again() {
            let mut wallet = wallet(
                vec![
                    scan_range(BIRTHDAY, SCAN_RANGE_START, ScanPriority::Scanned),
                    scan_range(SCAN_RANGE_START, SCAN_RANGE_END, ScanPriority::Scanning),
                ],
                Vec::new(),
            );

            let processed = process_hash_discontinuity(
                &mut wallet,
                scan_range(SCAN_RANGE_START, SCAN_RANGE_END, ScanPriority::ChainTip),
                SCAN_RANGE_START,
                None,
            )
            .await
            .unwrap();

            assert_eq!(
                processed.reorg_detection_start_height,
                Some(BlockHeight::from_u32(SCAN_RANGE_START))
            );
            assert_eq!(
                wallet.get_sync_state().unwrap().scan_ranges(),
                [
                    scan_range(BIRTHDAY, SCAN_RANGE_START, ScanPriority::Scanned),
                    scan_range(
                        SCAN_RANGE_START,
                        SCAN_RANGE_START + VERIFY_BLOCK_RANGE_SIZE,
                        ScanPriority::Verify
                    ),
                    scan_range(
                        SCAN_RANGE_START + VERIFY_BLOCK_RANGE_SIZE,
                        SCAN_RANGE_END,
                        ScanPriority::ChainTip
                    ),
                ]
            );
        }

        /// A hash discontinuity above the first block of the scan range is between blocks the server served together,
        /// or with a scanned block above the scan range. It ends the sync session.
        #[tokio::test]
        async fn hash_discontinuity_above_the_first_block_is_an_error() {
            let scan_ranges = vec![
                scan_range(BIRTHDAY, SCAN_RANGE_START, ScanPriority::Scanned),
                scan_range(SCAN_RANGE_START, SCAN_RANGE_END, ScanPriority::Scanning),
            ];
            let mut wallet = wallet(scan_ranges.clone(), Vec::new());

            let result = process_hash_discontinuity(
                &mut wallet,
                scan_range(SCAN_RANGE_START, SCAN_RANGE_END, ScanPriority::ChainTip),
                SCAN_RANGE_START + 1,
                None,
            )
            .await;

            assert!(matches!(
                result,
                Err(SyncError::ScanError(ScanError::ContinuityError(
                    ContinuityError::HashDiscontinuity { .. }
                )))
            ));
            assert_eq!(wallet.get_sync_state().unwrap().scan_ranges(), scan_ranges);
        }

        /// A re-org detected by a `Verify` scan range truncates the wallet to below the extended verification range,
        /// which removes the wallet data of every block above it. A scanned range above the verification range is
        /// reopened to be scanned again.
        #[tokio::test]
        async fn reorg_reopens_scanned_ranges_above_the_verification_range() {
            const VERIFY_END: u32 = SCAN_RANGE_START + VERIFY_BLOCK_RANGE_SIZE;
            const TRUNCATE_HEIGHT: u32 = SCAN_RANGE_START - VERIFY_BLOCK_RANGE_SIZE - 1;

            let mut wallet = wallet(
                vec![
                    scan_range(BIRTHDAY, SCAN_RANGE_START, ScanPriority::Scanned),
                    scan_range(SCAN_RANGE_START, VERIFY_END, ScanPriority::Scanning),
                    scan_range(VERIFY_END, SCAN_RANGE_END, ScanPriority::Scanned),
                ],
                // the bounds of the range that is still scanned after truncation
                vec![BIRTHDAY, TRUNCATE_HEIGHT],
            );

            let processed = process_hash_discontinuity(
                &mut wallet,
                scan_range(SCAN_RANGE_START, VERIFY_END, ScanPriority::Verify),
                SCAN_RANGE_START,
                Some(BlockHeight::from_u32(SCAN_RANGE_START)),
            )
            .await
            .unwrap();

            assert_eq!(processed.reorg_detection_start_height, None);
            assert_eq!(
                wallet.get_sync_state().unwrap().scan_ranges(),
                [
                    scan_range(BIRTHDAY, TRUNCATE_HEIGHT + 1, ScanPriority::Scanned),
                    scan_range(TRUNCATE_HEIGHT + 1, VERIFY_END, ScanPriority::Verify),
                    scan_range(VERIFY_END, SCAN_RANGE_END, ScanPriority::Historic),
                ]
            );
        }
    }

    mod pool_rescan {
        use std::collections::{BTreeMap, HashMap};

        use incrementalmerkletree::{Hashable as _, Marking, Position, Retention};
        use orchard::tree::MerkleHashOrchard;
        use tokio::sync::mpsc;
        use zcash_primitives::block::BlockHash;
        use zcash_primitives::transaction::TxId;
        use zcash_protocol::consensus::{self, BlockHeight, Parameters as _};
        use zcash_protocol::local_consensus::LocalNetwork;
        use zcash_protocol::{PoolType, ShieldedPool};
        use zingo_netutils::lightwallet_protocol::{SubtreeRoot, TreeState};
        use zingo_status::confirmation_status::ConfirmationStatus;

        use crate::client::FetchRequest;
        use crate::error::SyncError;
        use crate::mocks::MockWalletBuilder;
        use crate::sync::{ScanPriority, ScanRange, state, truncate_to_pool_activation_height};
        use crate::wallet::{
            ShardTrees, SyncState, TreeBounds, WalletBlock, WalletTransaction,
            traits::{SyncShardTrees, SyncTransactions, SyncWallet},
        };

        /// NU6.3 activates at block 100, so a rescan can straddle the Ironwood boundary.
        const NETWORK: LocalNetwork = LocalNetwork {
            nu6_3: Some(BlockHeight::from_u32(100)),
            ..super::NETWORK
        };

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

        fn subtree_root(completing_block_height: u64) -> SubtreeRoot {
            SubtreeRoot {
                completing_block_height,
                ..Default::default()
            }
        }

        /// Answers tree state requests with empty note commitment trees, omitting the ironwood tree state below the
        /// ironwood activation height.
        fn spawn_fetcher() -> mpsc::UnboundedSender<FetchRequest> {
            const EMPTY_TREE: &str = "000000";
            let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                while let Some(fetch_request) = fetch_request_receiver.recv().await {
                    match fetch_request {
                        FetchRequest::TreeState(reply_sender, block_height) => {
                            let ironwood_tree = if NETWORK
                                .is_nu_active(consensus::NetworkUpgrade::Nu6_3, block_height)
                            {
                                EMPTY_TREE.to_string()
                            } else {
                                String::new()
                            };
                            let _ignore_error = reply_sender.send(Ok(TreeState {
                                height: u64::from(block_height),
                                hash: "00".repeat(32),
                                sapling_tree: EMPTY_TREE.to_string(),
                                orchard_tree: EMPTY_TREE.to_string(),
                                ironwood_tree,
                                ..Default::default()
                            }));
                        }
                        _ => panic!("unexpected fetch request"),
                    }
                }
            });

            fetch_request_sender
        }

        /// A pool rescan clears the pool's shard tree, so the pool's shard ranges are cleared with it and both are
        /// rebuilt from the subtree roots fetched from index 0 in the next sync session. The kept ranges would still
        /// be correct, as they only depend on the chain, but `add_shard_ranges` can only append: against kept ranges
        /// it would reject every refetched root below the newest with an error log each and push a degenerate
        /// one-block range for the newest. Clearing keeps the ranges mirroring the roots the tree holds. The shard
        /// trees and shard ranges of pools activated before the rescanned pool are kept, as their data below the
        /// rescanned pool's activation height is not rescanned.
        #[tokio::test]
        async fn rescan_clears_pool_shard_ranges() {
            let mut sync_state = SyncState::new_for_test(vec![ScanRange::from_parts(
                BlockHeight::from_u32(1)..BlockHeight::from_u32(301),
                ScanPriority::Scanned,
            )]);
            let orchard_shard_ranges = vec![BlockHeight::from_u32(1)..BlockHeight::from_u32(51)];
            sync_state.orchard_shard_ranges = orchard_shard_ranges.clone();
            sync_state.ironwood_shard_ranges = vec![
                BlockHeight::from_u32(100)..BlockHeight::from_u32(251),
                BlockHeight::from_u32(250)..BlockHeight::from_u32(281),
            ];
            let mut shard_trees = ShardTrees::new();
            shard_trees
                .orchard
                .append(
                    MerkleHashOrchard::empty_leaf(),
                    Retention::Checkpoint {
                        id: BlockHeight::from_u32(50),
                        marking: Marking::Marked,
                    },
                )
                .unwrap();
            let mut wallet = MockWalletBuilder::new()
                .birthday(BlockHeight::from_u32(1))
                .sync_state(sync_state)
                .wallet_blocks(BTreeMap::from([(BlockHeight::from_u32(99), block(99))]))
                .shard_trees(shard_trees)
                .create_mock_wallet();

            let result = truncate_to_pool_activation_height(
                &NETWORK,
                spawn_fetcher(),
                &mut wallet,
                ShieldedPool::Ironwood,
                BlockHeight::from_u32(200),
                10,
                5,
            )
            .await;
            assert!(matches!(
                result,
                Ok(SyncError::PoolHistoryReopened {
                    pool: PoolType::Shielded(ShieldedPool::Ironwood),
                    ..
                })
            ));

            assert_eq!(
                wallet
                    .get_shard_trees_mut()
                    .unwrap()
                    .orchard
                    .max_leaf_position(None)
                    .unwrap(),
                Some(Position::from(0)),
                "orchard note commitments below the ironwood activation height must be kept"
            );

            let sync_state = wallet.get_sync_state_mut().unwrap();
            assert!(sync_state.ironwood_shard_ranges.is_empty());
            assert_eq!(sync_state.orchard_shard_ranges, orchard_shard_ranges);

            // the subtree roots fetched from index 0 in the next sync session rebuild the shard ranges
            state::add_shard_ranges(
                &NETWORK,
                ShieldedPool::Ironwood,
                sync_state,
                &[subtree_root(150), subtree_root(250)],
            );
            assert_eq!(
                sync_state.ironwood_shard_ranges,
                vec![
                    BlockHeight::from_u32(100)..BlockHeight::from_u32(151),
                    BlockHeight::from_u32(150)..BlockHeight::from_u32(251),
                ]
            );
        }

        /// The request for the frontier the cleared pool is rebuilt from fails. The wallet keeps the transactions,
        /// shard ranges and scan ranges of the pool's history, so the next sync session finds the same disagreement
        /// and rescans the pool.
        #[tokio::test]
        async fn failed_frontier_request_leaves_the_wallet_as_it_was() {
            const TRANSACTION_HEIGHT: u32 = 200;
            let txid = TxId::from_bytes([1; 32]);
            let scan_ranges = vec![ScanRange::from_parts(
                BlockHeight::from_u32(1)..BlockHeight::from_u32(301),
                ScanPriority::Scanned,
            )];
            let mut sync_state = SyncState::new_for_test(scan_ranges.clone());
            let ironwood_shard_ranges =
                vec![BlockHeight::from_u32(100)..BlockHeight::from_u32(251)];
            sync_state.ironwood_shard_ranges = ironwood_shard_ranges.clone();
            let mut wallet = MockWalletBuilder::new()
                .birthday(BlockHeight::from_u32(1))
                .sync_state(sync_state)
                .wallet_transactions(HashMap::from([(
                    txid,
                    WalletTransaction::new_for_test(
                        txid,
                        ConfirmationStatus::Confirmed(BlockHeight::from_u32(TRANSACTION_HEIGHT)),
                    ),
                )]))
                .create_mock_wallet();
            let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
            drop(fetch_request_receiver);

            let result = truncate_to_pool_activation_height(
                &NETWORK,
                fetch_request_sender,
                &mut wallet,
                ShieldedPool::Ironwood,
                BlockHeight::from_u32(TRANSACTION_HEIGHT),
                10,
                5,
            )
            .await;

            assert!(result.is_err());
            assert!(
                wallet
                    .get_wallet_transactions()
                    .unwrap()
                    .contains_key(&txid),
                "the transaction must be kept while its scan range is still recorded as scanned"
            );
            let sync_state = wallet.get_sync_state().unwrap();
            assert_eq!(sync_state.ironwood_shard_ranges, ironwood_shard_ranges);
            assert_eq!(sync_state.scan_ranges(), scan_ranges);
        }
    }

    /// The geometry of a scan range against a block range, which the engine asks at six sites.
    mod scan_range_geometry {
        use zcash_protocol::consensus::BlockHeight;

        use crate::sync::{ScanPriority, ScanRange};

        fn blocks(start: u32, end: u32) -> std::ops::Range<BlockHeight> {
            BlockHeight::from_u32(start)..BlockHeight::from_u32(end)
        }

        #[test]
        fn encloses_a_block_range_within_its_bounds() {
            let scan_range = ScanRange::from_parts(blocks(21, 41), ScanPriority::Scanning);

            assert!(scan_range.encloses(&blocks(21, 41)));
            assert!(scan_range.encloses(&blocks(21, 31)));
            assert!(scan_range.encloses(&blocks(31, 41)));
            assert!(!scan_range.encloses(&blocks(20, 31)));
            assert!(!scan_range.encloses(&blocks(31, 42)));
            assert!(!scan_range.encloses(&blocks(1, 21)));
        }

        #[test]
        fn overlaps_a_block_range_that_shares_a_block() {
            let scan_range = ScanRange::from_parts(blocks(21, 41), ScanPriority::Scanning);

            assert!(scan_range.overlaps(&blocks(31, 51)));
            assert!(scan_range.overlaps(&blocks(1, 22)));
            assert!(scan_range.overlaps(&blocks(1, 51)));
            assert!(!scan_range.overlaps(&blocks(41, 51)));
            assert!(!scan_range.overlaps(&blocks(1, 21)));
        }
    }

    /// The priority a selected scan range is held at while its task is in flight.
    mod in_flight_priority {
        use crate::sync::ScanPriority;

        #[test]
        fn refetching_nullifiers_for_scanned_without_mapping_and_scanning_for_every_other_selection()
         {
            assert_eq!(
                ScanPriority::ScannedWithoutMapping.in_flight(),
                ScanPriority::RefetchingNullifiers
            );
            for selected in [
                ScanPriority::Historic,
                ScanPriority::OpenAdjacent,
                ScanPriority::FoundNote,
                ScanPriority::ChainTip,
                ScanPriority::Verify,
            ] {
                assert_eq!(selected.in_flight(), ScanPriority::Scanning);
            }
        }
    }

    /// The empty result of a discarded or re-org-handled load.
    mod processed_scan_results {
        use crate::sync::ProcessedScanResults;

        #[test]
        fn default_carries_no_addresses() {
            let processed = ProcessedScanResults::default();

            assert!(processed.new_transparent_inuse_addresses.is_empty());
            assert!(processed.new_transparent_gap_addresses.is_empty());
        }
    }
}
