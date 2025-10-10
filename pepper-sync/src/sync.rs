//! Entrypoint for sync engine

use std::cmp;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicBool, AtomicU8};
use std::time::{Duration, SystemTime};

use tokio::sync::{RwLock, mpsc};

use incrementalmerkletree::{Marking, Retention};
use orchard::tree::MerkleHashOrchard;
use shardtree::store::ShardStore;
use zcash_client_backend::proto::service::RawTransaction;
use zcash_client_backend::proto::service::compact_tx_streamer_client::CompactTxStreamerClient;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::transaction::{Transaction, TxId};
use zcash_primitives::zip32::AccountId;
use zcash_protocol::ShieldedProtocol;
use zcash_protocol::consensus::{self, BlockHeight};

use zingo_status::confirmation_status::ConfirmationStatus;

use crate::client::{self, FetchRequest};
use crate::config::SyncConfig;
use crate::error::{
    ContinuityError, MempoolError, ScanError, ServerError, SyncError, SyncModeError,
    SyncStatusError,
};
use crate::keys::transparent::TransparentAddressId;
use crate::scan::ScanResults;
use crate::scan::task::{Scanner, ScannerState};
use crate::scan::transactions::scan_transaction;
use crate::wallet::traits::{
    SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions, SyncWallet,
};
use crate::wallet::{
    KeyIdInterface, NoteInterface, NullifierMap, OutputId, OutputInterface, ScanTarget, SyncMode,
    SyncState, WalletBlock, WalletTransaction,
};
use crate::witness::LocatedTreeData;

#[cfg(not(feature = "darkside_test"))]
use crate::witness;

pub(crate) mod spend;
pub(crate) mod state;
pub(crate) mod transparent;

const MEMPOOL_SPEND_INVALIDATION_THRESHOLD: u32 = 3;
pub(crate) const MAX_VERIFICATION_WINDOW: u32 = 100;
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
    pub percentage_session_outputs_scanned: f32,
    pub percentage_total_outputs_scanned: f32,
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
            "percentage_session_outputs_scanned" => value.percentage_session_outputs_scanned,
            "percentage_total_outputs_scanned" => value.percentage_total_outputs_scanned,
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
    percentage total outputs scanned: {}
}}",
            self.sync_start_height,
            self.sync_end_height,
            self.blocks_scanned,
            self.sapling_outputs_scanned,
            self.orchard_outputs_scanned,
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
            "percentage_total_outputs_scanned" => value.percentage_total_outputs_scanned,
        }
    }
}

/// Scanning range priority levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScanPriority {
    /// Block ranges that are currently being scanned.
    Scanning,
    /// Block ranges that have already been scanned will not be re-scanned.
    Scanned,
    /// Block ranges that have already been scanned. The nullifiers from this range were not mapped after scanning and
    /// spend detection to reduce memory consumption and/or storage for non-linear scanning. These nullifiers will need
    /// to be re-fetched for final spend detection when merging this range is the lowest unscanned range in the
    /// wallet's list of scan ranges.
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
pub async fn sync<P, W>(
    client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    consensus_parameters: &P,
    wallet: Arc<RwLock<W>>,
    sync_mode: Arc<AtomicU8>,
    config: SyncConfig,
) -> Result<SyncResult, SyncError<W::Error>>
where
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
    let unprocessed_mempool_transactions_count = Arc::new(AtomicU8::new(0));
    let unprocessed_mempool_transactions_count_clone =
        unprocessed_mempool_transactions_count.clone();
    let mempool_handle = tokio::spawn(async move {
        mempool_monitor(
            client,
            mempool_transaction_sender,
            unprocessed_mempool_transactions_count_clone,
            shutdown_mempool_clone,
        )
        .await
    });

    // pre-scan initialisation
    let mut wallet_guard = wallet.write().await;

    let mut wallet_height = state::get_wallet_height(consensus_parameters, &*wallet_guard)
        .map_err(SyncError::WalletError)?;
    let chain_height = client::get_chain_height(fetch_request_sender.clone()).await?;
    if chain_height == 0.into() {
        return Err(SyncError::ServerError(ServerError::GenesisBlockOnly));
    }
    if wallet_height > chain_height {
        if wallet_height - chain_height > MAX_VERIFICATION_WINDOW {
            return Err(SyncError::ChainError(MAX_VERIFICATION_WINDOW));
        }
        truncate_wallet_data(&mut *wallet_guard, chain_height)?;
        wallet_height = chain_height;
    }

    let ufvks = wallet_guard
        .get_unified_full_viewing_keys()
        .map_err(SyncError::WalletError)?;

    transparent::update_addresses_and_scan_targets(
        consensus_parameters,
        &mut *wallet_guard,
        fetch_request_sender.clone(),
        &ufvks,
        wallet_height,
        chain_height,
        config.transparent_address_discovery,
    )
    .await?;

    #[cfg(not(feature = "darkside_test"))]
    update_subtree_roots(
        consensus_parameters,
        fetch_request_sender.clone(),
        &mut *wallet_guard,
    )
    .await?;

    add_initial_frontier(
        consensus_parameters,
        fetch_request_sender.clone(),
        &mut *wallet_guard,
    )
    .await?;

    state::update_scan_ranges(
        consensus_parameters,
        wallet_height,
        chain_height,
        wallet_guard
            .get_sync_state_mut()
            .map_err(SyncError::WalletError)?,
    )
    .await;

    state::set_initial_state(
        consensus_parameters,
        fetch_request_sender.clone(),
        &mut *wallet_guard,
        chain_height,
    )
    .await?;

    let initial_verification_height = wallet_guard
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .highest_scanned_height()
        .expect("scan ranges must be non-empty")
        + 1;

    reset_invalid_spends(&mut *wallet_guard)?;

    drop(wallet_guard);

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
                    initial_verification_height,
                )
                .await?;
                wallet_guard.set_save_flag().map_err(SyncError::WalletError)?;
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
                        drop(wallet_guard);
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
                            percentage_total_outputs_scanned: sync_status.percentage_total_outputs_scanned,
                        });
                    }
                    SyncMode::Running => (),
                    SyncMode::NotRunning => {
                        panic!("sync mode should not be manually set to NotRunning!");
                    },
                }

                scanner.update(&mut *wallet.write().await, shutdown_mempool.clone(), config.performance_level).await?;

                if matches!(scanner.state, ScannerState::Shutdown) {
                    // wait for mempool monitor to receive mempool transactions
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if is_shutdown(&scanner, unprocessed_mempool_transactions_count.clone())
                    {
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
        percentage_total_outputs_scanned: sync_status.percentage_total_outputs_scanned,
    })
}

/// Creates a [`self::SyncStatus`] from the wallet's current [`crate::wallet::SyncState`].
///
/// Intended to be called while [`self::sync`] is running in a separate task.
pub async fn sync_status<W>(wallet: &W) -> Result<SyncStatus, SyncStatusError<W::Error>>
where
    W: SyncWallet + SyncBlocks,
{
    let (total_sapling_outputs_scanned, total_orchard_outputs_scanned) =
        state::calculate_scanned_outputs(wallet).map_err(SyncStatusError::WalletError)?;
    let total_outputs_scanned = total_sapling_outputs_scanned + total_orchard_outputs_scanned;

    let sync_state = wallet
        .get_sync_state()
        .map_err(SyncStatusError::WalletError)?;
    if sync_state.initial_sync_state.sync_start_height == 0.into() {
        return Ok(SyncStatus {
            scan_ranges: sync_state.scan_ranges.clone(),
            sync_start_height: 0.into(),
            session_blocks_scanned: 0,
            total_blocks_scanned: 0,
            percentage_session_blocks_scanned: 0.0,
            percentage_total_blocks_scanned: 0.0,
            session_sapling_outputs_scanned: 0,
            session_orchard_outputs_scanned: 0,
            total_sapling_outputs_scanned: 0,
            total_orchard_outputs_scanned: 0,
            percentage_session_outputs_scanned: 0.0,
            percentage_total_outputs_scanned: 0.0,
        });
    }
    let total_blocks_scanned = state::calculate_scanned_blocks(sync_state);

    let birthday = sync_state
        .wallet_birthday()
        .ok_or(SyncStatusError::NoSyncData)?;
    let wallet_height = sync_state
        .wallet_height()
        .ok_or(SyncStatusError::NoSyncData)?;
    let total_blocks = wallet_height - birthday + 1;
    let total_sapling_outputs = sync_state
        .initial_sync_state
        .wallet_tree_bounds
        .sapling_final_tree_size
        - sync_state
            .initial_sync_state
            .wallet_tree_bounds
            .sapling_initial_tree_size;
    let total_orchard_outputs = sync_state
        .initial_sync_state
        .wallet_tree_bounds
        .orchard_final_tree_size
        - sync_state
            .initial_sync_state
            .wallet_tree_bounds
            .orchard_initial_tree_size;
    let total_outputs = total_sapling_outputs + total_orchard_outputs;

    let session_blocks_scanned =
        total_blocks_scanned - sync_state.initial_sync_state.previously_scanned_blocks;
    let percentage_session_blocks_scanned = ((session_blocks_scanned as f32
        / (total_blocks - sync_state.initial_sync_state.previously_scanned_blocks) as f32)
        * 100.0)
        .clamp(0.0, 100.0);
    let percentage_total_blocks_scanned =
        ((total_blocks_scanned as f32 / total_blocks as f32) * 100.0).clamp(0.0, 100.0);

    let session_sapling_outputs_scanned = total_sapling_outputs_scanned
        - sync_state
            .initial_sync_state
            .previously_scanned_sapling_outputs;
    let session_orchard_outputs_scanned = total_orchard_outputs_scanned
        - sync_state
            .initial_sync_state
            .previously_scanned_orchard_outputs;
    let session_outputs_scanned = session_sapling_outputs_scanned + session_orchard_outputs_scanned;
    let previously_scanned_outputs = sync_state
        .initial_sync_state
        .previously_scanned_sapling_outputs
        + sync_state
            .initial_sync_state
            .previously_scanned_orchard_outputs;
    let percentage_session_outputs_scanned = ((session_outputs_scanned as f32
        / (total_outputs - previously_scanned_outputs) as f32)
        * 100.0)
        .clamp(0.0, 100.0);
    let percentage_total_outputs_scanned =
        ((total_outputs_scanned as f32 / total_outputs as f32) * 100.0).clamp(0.0, 100.0);

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
        percentage_session_outputs_scanned,
        percentage_total_outputs_scanned,
    })
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
    let (sapling_derived_nullifiers, orchard_derived_nullifiers) =
        spend::collect_derived_nullifiers(wallet_transactions);
    let (sapling_spend_scan_targets, orchard_spend_scan_targets) = spend::detect_shielded_spends(
        &mut pending_transaction_nullifiers,
        sapling_derived_nullifiers,
        orchard_derived_nullifiers,
    );

    // return if transaction is not relevant to the wallet
    if pending_transaction.transparent_coins().is_empty()
        && pending_transaction.sapling_notes().is_empty()
        && pending_transaction.orchard_notes().is_empty()
        && pending_transaction.outgoing_orchard_notes().is_empty()
        && pending_transaction.outgoing_sapling_notes().is_empty()
        && transparent_spend_scan_targets.is_empty()
        && sapling_spend_scan_targets.is_empty()
        && orchard_spend_scan_targets.is_empty()
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

/// Returns true if the scanner and mempool are shutdown.
fn is_shutdown<P>(
    scanner: &Scanner<P>,
    mempool_unprocessed_transactions_count: Arc<AtomicU8>,
) -> bool
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    scanner.worker_poolsize() == 0
        && mempool_unprocessed_transactions_count.load(atomic::Ordering::Acquire) == 0
}

/// Scan post-processing
async fn process_scan_results<W>(
    consensus_parameters: &impl consensus::Parameters,
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_range: ScanRange,
    scan_results: Result<ScanResults, ScanError>,
    initial_verification_height: BlockHeight,
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
                outpoints,
                scanned_blocks,
                wallet_transactions,
                sapling_located_trees,
                orchard_located_trees,
                map_nullifiers,
            } = results;

            if scan_range.priority() == ScanPriority::ScannedWithoutMapping {
                spend::update_shielded_spends(
                    consensus_parameters,
                    wallet,
                    fetch_request_sender.clone(),
                    ufvks,
                    &scanned_blocks,
                    Some(&mut nullifiers),
                )
                .await?;

                // add missing block bounds in the case that nullifier batch limit was reached and scan range was split
                let mut missing_block_bounds = BTreeMap::new();
                for block_bound in [
                    scan_range.block_range().start,
                    scan_range.block_range().end - 1,
                ] {
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

                state::set_scanned_scan_range(
                    wallet
                        .get_sync_state_mut()
                        .map_err(SyncError::WalletError)?,
                    scan_range.block_range().clone(),
                    true,
                );
            } else {
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
                    outpoints,
                    wallet_transactions,
                    sapling_located_trees,
                    orchard_located_trees,
                )
                .await?;
                spend::update_transparent_spends(wallet).map_err(SyncError::WalletError)?;
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
            if height == scan_range.block_range().start
                && scan_range.priority() == ScanPriority::Verify
            {
                tracing::info!("Re-org detected.");
                let sync_state = wallet
                    .get_sync_state_mut()
                    .map_err(SyncError::WalletError)?;
                let wallet_height = sync_state
                    .wallet_height()
                    .expect("scan ranges should be non-empty in this scope");

                // reset scan range from `Scanning` to `Verify`
                state::set_scan_priority(
                    sync_state,
                    scan_range.block_range(),
                    ScanPriority::Verify,
                );

                // extend verification range to VERIFY_BLOCK_RANGE_SIZE blocks below current verification range
                let scan_range_to_verify = state::set_verify_scan_range(
                    sync_state,
                    height - 1,
                    state::VerifyEnd::VerifyHighest,
                );
                state::merge_scan_ranges(sync_state, ScanPriority::Verify);

                truncate_wallet_data(wallet, scan_range_to_verify.block_range().start - 1)?;

                if initial_verification_height - scan_range_to_verify.block_range().start
                    > MAX_VERIFICATION_WINDOW
                {
                    return Err(ServerError::ChainVerificationError.into());
                }

                state::set_initial_state(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    wallet,
                    wallet_height,
                )
                .await?;
            } else {
                scan_results?;
            }
        }
        Err(e) => return Err(e.into()),
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
    // does not use raw transaction height due to lightwalletd off-by-one bug and potential to be zero
    let mempool_height = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .wallet_height()
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
        .get_wallet_transactions()
        .map_err(SyncError::WalletError)?
        .get(&transaction.txid())
        && (tx.status().is_confirmed() || matches!(tx.status(), ConfirmationStatus::Mempool(_)))
    {
        return Ok(());
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

/// Removes all wallet data above the given `truncate_height`.
fn truncate_wallet_data<W>(
    wallet: &mut W,
    truncate_height: BlockHeight,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncShardTrees,
{
    let birthday = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .wallet_birthday()
        .expect("should be non-empty in this scope");
    let checked_truncate_height = match truncate_height.cmp(&birthday) {
        std::cmp::Ordering::Greater | std::cmp::Ordering::Equal => truncate_height,
        std::cmp::Ordering::Less => birthday,
    };

    wallet
        .truncate_wallet_blocks(checked_truncate_height)
        .map_err(SyncError::WalletError)?;
    wallet
        .truncate_wallet_transactions(checked_truncate_height)
        .map_err(SyncError::WalletError)?;
    wallet
        .truncate_nullifiers(checked_truncate_height)
        .map_err(SyncError::WalletError)?;
    wallet.truncate_shard_trees(checked_truncate_height)?;

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
    mut outpoints: BTreeMap<OutputId, ScanTarget>,
    transactions: HashMap<TxId, WalletTransaction>,
    sapling_located_trees: Vec<LocatedTreeData<sapling_crypto::Node>>,
    orchard_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees + Send,
{
    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    let wallet_height = sync_state
        .wallet_height()
        .expect("scan ranges should not be empty in this scope");
    for transaction in transactions.values() {
        state::update_found_note_shard_priority(
            consensus_parameters,
            sync_state,
            ShieldedProtocol::Sapling,
            transaction,
        );
        state::update_found_note_shard_priority(
            consensus_parameters,
            sync_state,
            ShieldedProtocol::Orchard,
            transaction,
        );
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
    wallet
        .append_outpoints(&mut outpoints)
        .map_err(SyncError::WalletError)?;
    wallet
        .update_shard_trees(
            fetch_request_sender,
            scan_range,
            wallet_height,
            sapling_located_trees,
            orchard_located_trees,
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
        .filter(|scan_range| {
            scan_range.priority() == ScanPriority::Scanned
                || scan_range.priority() == ScanPriority::ScannedWithoutMapping
                || scan_range.priority() == ScanPriority::Scanning
        })
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
        *height >= highest_scanned_height.saturating_sub(MAX_VERIFICATION_WINDOW)
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
        *height >= highest_scanned_height.saturating_sub(MAX_VERIFICATION_WINDOW)
            || *height == scan_range.block_range().start
            || *height == scan_range.block_range().end - 1
            || wallet_transaction_heights.contains(height)
    });

    wallet.append_wallet_blocks(scanned_blocks)?;

    Ok(())
}

#[cfg(not(feature = "darkside_test"))]
async fn update_subtree_roots<W>(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    wallet: &mut W,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncShardTrees,
{
    let sapling_start_index = wallet
        .get_shard_trees()
        .map_err(SyncError::WalletError)?
        .sapling
        .store()
        .get_shard_roots()
        .expect("infallible")
        .len() as u32;
    let orchard_start_index = wallet
        .get_shard_trees()
        .map_err(SyncError::WalletError)?
        .orchard
        .store()
        .get_shard_roots()
        .expect("infallible")
        .len() as u32;
    let (sapling_subtree_roots, orchard_subtree_roots) = futures::join!(
        client::get_subtree_roots(fetch_request_sender.clone(), sapling_start_index, 0, 0),
        client::get_subtree_roots(fetch_request_sender, orchard_start_index, 1, 0)
    );

    let sapling_subtree_roots = sapling_subtree_roots?;
    let orchard_subtree_roots = orchard_subtree_roots?;

    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    state::add_shard_ranges(
        consensus_parameters,
        ShieldedProtocol::Sapling,
        sync_state,
        &sapling_subtree_roots,
    );
    state::add_shard_ranges(
        consensus_parameters,
        ShieldedProtocol::Orchard,
        sync_state,
        &orchard_subtree_roots,
    );

    let shard_trees = wallet
        .get_shard_trees_mut()
        .map_err(SyncError::WalletError)?;
    witness::add_subtree_roots(sapling_subtree_roots, &mut shard_trees.sapling)?;
    witness::add_subtree_roots(orchard_subtree_roots, &mut shard_trees.orchard)?;

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
    let birthday =
        checked_birthday(consensus_parameters, wallet).map_err(SyncError::WalletError)?;
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
    }

    Ok(())
}

/// Compares the wallet birthday to sapling activation height and returns the highest block height.
fn checked_birthday<W: SyncWallet>(
    consensus_parameters: &impl consensus::Parameters,
    wallet: &W,
) -> Result<BlockHeight, W::Error> {
    let wallet_birthday = wallet.get_birthday()?;
    let sapling_activation_height = consensus_parameters
        .activation_height(consensus::NetworkUpgrade::Sapling)
        .expect("sapling activation height should always return Some");

    match wallet_birthday.cmp(&sapling_activation_height) {
        cmp::Ordering::Greater | cmp::Ordering::Equal => Ok(wallet_birthday),
        cmp::Ordering::Less => Ok(sapling_activation_height),
    }
}

/// Sets up mempool stream.
///
/// If there is some raw transaction, send to be scanned.
/// If the mempool stream message is `None` (a block was mined) or the request failed, setup a new mempool stream.
async fn mempool_monitor(
    mut client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    mempool_transaction_sender: mpsc::Sender<RawTransaction>,
    unprocessed_transactions_count: Arc<AtomicU8>,
    shutdown_mempool: Arc<AtomicBool>,
) -> Result<(), MempoolError> {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    'main: loop {
        let response =
            client::get_mempool_transaction_stream(&mut client, shutdown_mempool.clone()).await;

        match response {
            Ok(mut mempool_stream) => {
                interval.reset();
                loop {
                    tokio::select! {
                        mempool_stream_message = mempool_stream.message() => {
                            match mempool_stream_message.unwrap_or(None) {
                                Some(raw_transaction) => {
                                     let _ignore_error = mempool_transaction_sender
                                        .send(raw_transaction)
                                        .await;
                                    unprocessed_transactions_count.fetch_add(1, atomic::Ordering::Release);
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
                tracing::warn!("Mempool stream request failed! Status: {e}.\nRetrying...");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }

    Ok(())
}

fn reset_invalid_spends<W>(wallet: &mut W) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncTransactions,
{
    let wallet_height = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .wallet_height()
        .expect("wallet height must exist after scan ranges have been updated");
    let wallet_transactions = wallet
        .get_wallet_transactions_mut()
        .map_err(SyncError::WalletError)?;

    let invalid_txids = wallet_transactions
        .values()
        .filter(|transaction| {
            matches!(transaction.status(), ConfirmationStatus::Mempool(_))
                && transaction.status().get_height()
                    <= wallet_height - MEMPOOL_SPEND_INVALIDATION_THRESHOLD
        })
        .map(super::wallet::WalletTransaction::txid)
        .chain(
            wallet_transactions
                .values()
                .filter(|transaction| {
                    (matches!(transaction.status(), ConfirmationStatus::Calculated(_))
                        || matches!(transaction.status(), ConfirmationStatus::Transmitted(_)))
                        && wallet_height >= transaction.transaction().expiry_height()
                })
                .map(super::wallet::WalletTransaction::txid),
        )
        .collect::<Vec<_>>();
    reset_spends(wallet_transactions, invalid_txids);

    Ok(())
}
