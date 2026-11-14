//! Entrypoint for sync engine

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::future::Future;
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
use zip32::AccountId;
use zl_netutils::lightwallet_protocol::{RawTransaction, SubtreeRoot};
use zl_netutils::{Indexer, TransparentIndexer};

use zl_common::status::ConfirmationStatus;

use crate::client::{self, FetchRequest};
use crate::config::{PerformanceLevel, SyncConfig};
use crate::engine::{
    InitialSyncStateOps, ScanTargetOps, ShardTreesOps, SyncStateOps, WalletTransactionOps,
};
use crate::error::{
    ContinuityError, MempoolError, ScanError, ServerError, SyncError, SyncModeError,
    SyncStatusError,
};
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
    ScanTarget, SyncMode, WalletBlock, WalletTransaction,
};
use crate::witness::{ANCHOR_RETENTION_INTERVALS, LocatedTreeData};

use crate::witness;

pub(crate) mod spend;
pub(crate) mod state;
pub(crate) mod transparent;
pub mod truncate;

pub(crate) use zl_common::engine::overlaps;
pub use zl_common::sync::{
    MAX_REORG_ALLOWANCE, SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE, ScanPriority, ScanRange,
    add_scan_targets,
};

// TODO: investigate a potential case where:
// - a wallet syncs, including the latest incomplete shard
// - the wallet is not opened for some time, the incomplete shard has completed since
// - on next sync, the shard roots *after* the incomplete shard are fetched
// - the wallet can't spend because the incomplete shard is not prioritized to be completed

/// The maximum total number of checkpoints a shard tree persists.
pub const MAX_SHARDTREE_CHECKPOINTS: u32 =
    SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE + ANCHOR_RETENTION_INTERVALS;

/// Interval in seconds at which continuous sync checks for newly mined blocks if the mempool stream has not signalled
/// one.
pub const CHECK_NEW_BLOCKS_INTERVAL: u64 = 10;

/// A snapshot of the current state of sync. Useful for displaying the status of sync to a user / consumer.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct SyncStatus {
    pub scan_ranges: Vec<ScanRange>,
    pub sync_start_height: BlockHeight,
    pub session_blocks_scanned: u32,
    pub total_blocks_scanned: u32,
    pub total_blocks: u32,
    pub session_sapling_outputs_scanned: u32,
    pub total_sapling_outputs_scanned: u32,
    pub session_orchard_outputs_scanned: u32,
    pub total_orchard_outputs_scanned: u32,
    pub session_ironwood_outputs_scanned: u32,
    pub total_ironwood_outputs_scanned: u32,
    /// Numerator of the exact scan-progress ratio: outputs scanned so far
    /// across every shielded pool. Never exceeds `total_outputs`: the sync
    /// engine extends the tree bounds to each newly mined block under the
    /// wallet lock before it scans that block.
    pub total_outputs_scanned: u64,
    /// Denominator of the exact scan-progress ratio: outputs in the chain
    /// between the wallet birthday and the last known chain height, across
    /// every shielded pool. Zero when sync has never started, and also when
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

    /// ```
    /// use pepper_sync::sync::{ScanPriority, ScanRange, SyncStatus};
    /// use zcash_protocol::consensus::BlockHeight;
    ///
    /// let mut status = SyncStatus {
    ///     scan_ranges: vec![ScanRange::from_parts(
    ///         BlockHeight::from_u32(1)..BlockHeight::from_u32(2),
    ///         ScanPriority::ScannedWithoutMapping,
    ///     )],
    ///     sync_start_height: BlockHeight::from_u32(1),
    ///     session_blocks_scanned: 1,
    ///     total_blocks_scanned: 1,
    ///     total_blocks: 1,
    ///     session_sapling_outputs_scanned: 0,
    ///     total_sapling_outputs_scanned: 0,
    ///     session_orchard_outputs_scanned: 0,
    ///     total_orchard_outputs_scanned: 0,
    ///     session_ironwood_outputs_scanned: 0,
    ///     total_ironwood_outputs_scanned: 0,
    ///     total_outputs_scanned: 0,
    ///     total_outputs: 0,
    /// };
    /// assert!(status.nullifier_retrieval_pending());
    /// assert!(!status.is_complete());
    ///
    /// status.scan_ranges = vec![ScanRange::from_parts(
    ///     BlockHeight::from_u32(1)..BlockHeight::from_u32(2),
    ///     ScanPriority::Scanned,
    /// )];
    /// assert!(!status.nullifier_retrieval_pending());
    /// assert!(status.is_complete());
    /// ```
    pub fn nullifier_retrieval_pending(&self) -> bool {
        self.scan_ranges
            .iter()
            .map(ScanRange::priority)
            .any(ScanPriority::awaits_nullifier_retrieval)
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
    pub total_outputs_scanned: u64,
    pub total_outputs: u64,
    pub nullifier_retrieval_pending: bool,
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

/// How often a session-start request checks the sync mode for a shutdown request.
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Awaits `request` unless `sync_mode` is set to `Shutdown` first, in which case `request` is dropped and `None` is
/// returned.
async fn unless_shutdown<F>(
    sync_mode: &AtomicU8,
    request: F,
) -> Result<Option<F::Output>, SyncModeError>
where
    F: Future,
{
    let mut interval = tokio::time::interval(SHUTDOWN_POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tokio::pin!(request);
    loop {
        tokio::select! {
            output = &mut request => return Ok(Some(output)),
            _ = interval.tick() => {
                if SyncMode::from_atomic_u8(sync_mode)? == SyncMode::Shutdown {
                    return Ok(None);
                }
            }
        }
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
    wallet_guard
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?
        .reset_scan_ranges();
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
        tracing::debug!("Sync stage: requesting the chain height.");
        // every server request made before scanning begins is abandoned when shutdown is requested, so a stop is
        // not held behind a slow server. the requests are made with the wallet lock released.
        let Some(chain_height) = unless_shutdown(
            &sync_mode,
            client::get_chain_height(fetch_request_sender.clone()),
        )
        .await?
        else {
            break 'continuous_sync;
        };
        let chain_height = chain_height?;
        if chain_height == 0.into() {
            return Err(SyncError::ServerError(ServerError::GenesisBlockOnly));
        }

        let mut wallet_guard = wallet.write().await;
        let last_known_chain_height =
            checked_wallet_height(&mut *wallet_guard, chain_height, consensus_parameters)?;
        let new_blocks_mined = chain_height > last_known_chain_height;
        wallet_guard
            .get_sync_state_mut()
            .map_err(SyncError::WalletError)?
            .create_scan_range(last_known_chain_height, chain_height);
        // the initial sync state is set on the first continuous sync loop. afterwards, the wallet tree bounds are
        // only extended when new blocks have been mined. the wallet lock is released while the tree bounds are
        // fetched: `sync_status` saturates, so an initial sync state that lags the new scan range for the length of
        // the request under-reports the session's progress and nothing more.
        let tree_bounds_source = (!first_verification_complete || new_blocks_mined)
            .then(|| state::TreeBoundsSource::read(&*wallet_guard, chain_height))
            .transpose()
            .map_err(SyncError::WalletError)?;
        drop(wallet_guard);
        if let Some(tree_bounds_source) = tree_bounds_source {
            tracing::debug!("Sync stage: requesting the tree bounds.");
            let Some(tree_bounds) = unless_shutdown(
                &sync_mode,
                state::fetch_tree_bounds(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    tree_bounds_source,
                ),
            )
            .await?
            else {
                break 'continuous_sync;
            };
            let tree_bounds = tree_bounds?;
            let mut wallet_guard = wallet.write().await;
            if first_verification_complete {
                state::update_wallet_tree_bounds(&mut *wallet_guard, tree_bounds)?;
            } else {
                state::set_initial_state(&mut *wallet_guard, chain_height, tree_bounds)?;
            }
        }

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
                wallet
                    .write()
                    .await
                    .get_sync_state_mut()
                    .map_err(SyncError::WalletError)?
                    .set_verify_scan_range(
                        initial_reorg_detection_start_height,
                        VerifyEnd::VerifyLowest,
                    );
            } else {
                // in this case, no blocks have been mined since last sync session but a re-org may have occured
                let Some(chain_height_server_block) = unless_shutdown(
                    &sync_mode,
                    client::get_compact_block(fetch_request_sender.clone(), chain_height),
                )
                .await?
                else {
                    break 'continuous_sync;
                };
                let chain_height_server_block = chain_height_server_block?;
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
                    let mut wallet_guard = wallet.write().await;
                    let scan_range_to_verify = wallet_guard
                        .get_sync_state_mut()
                        .map_err(SyncError::WalletError)?
                        .set_verify_scan_range(chain_height, VerifyEnd::VerifyHighest);
                    initial_reorg_detection_start_height_opt =
                        Some(scan_range_to_verify.block_range().start);
                    truncate_wallet_data(
                        &mut *wallet_guard,
                        initial_reorg_detection_start_height_opt
                            .expect("value must exist in this scope")
                            - 1,
                    )?;
                    // the re-orged block at the chain height is truncated, so its tree sizes are fetched. the wallet
                    // lock is released for the request as it is above.
                    let tree_bounds_source =
                        state::TreeBoundsSource::read(&*wallet_guard, chain_height)
                            .map_err(SyncError::WalletError)?;
                    drop(wallet_guard);
                    let Some(tree_bounds) = unless_shutdown(
                        &sync_mode,
                        state::fetch_tree_bounds(
                            consensus_parameters,
                            fetch_request_sender.clone(),
                            tree_bounds_source,
                        ),
                    )
                    .await?
                    else {
                        break 'continuous_sync;
                    };
                    state::set_initial_state(
                        &mut *wallet.write().await,
                        chain_height,
                        tree_bounds?,
                    )?;
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
            tracing::debug!("Sync stage: discovering transparent addresses.");
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
                .set_transparent_scan_floor(chain_height);
            wallet_guard
                .set_save_flag()
                .map_err(SyncError::WalletError)?;
            drop(wallet_guard);
        }

        if new_blocks_mined || reorg_occured || !first_verification_complete {
            // subtree roots are always updated on the first continuous sync loop, even if no new blocks have been mined,
            // so they are added before any note commitments are inserted into the shard trees by scanning.
            // the shard store fills every shard below an inserted shard with an empty shard, and the shard tree resumes
            // fetching from the number of stored shards, so the subtree roots of these empty shards would never be
            // fetched. for example, a pool rescan (see `truncate_to_pool_activation_height`) clears the pool's shard
            // tree and ends the sync session, relying on the next sync session to fetch the pool's subtree roots.
            tracing::debug!("Sync stage: requesting the subtree roots.");
            let fetch_start = subtree_root_fetch_start(&*wallet.read().await)?;
            let Some(subtree_roots) = unless_shutdown(
                &sync_mode,
                fetch_subtree_roots(fetch_request_sender.clone(), fetch_start),
            )
            .await?
            else {
                break 'continuous_sync;
            };
            let subtree_roots = subtree_roots?;
            add_fetched_subtree_roots(
                consensus_parameters,
                &mut *wallet.write().await,
                subtree_roots,
            )?;

            if !first_verification_complete {
                // frontier is added after subtree roots to retain subtree roots below birthday.
                // the height is bound first: a wallet guard in an `if let` scrutinee lives through the body, and
                // the body takes the write lock.
                let initial_frontier_height =
                    initial_frontier_height(consensus_parameters, &*wallet.read().await)?;
                if let Some(birthday) = initial_frontier_height {
                    let Some(frontiers) = unless_shutdown(
                        &sync_mode,
                        client::get_frontiers(
                            fetch_request_sender.clone(),
                            consensus_parameters,
                            birthday,
                        ),
                    )
                    .await?
                    else {
                        break 'continuous_sync;
                    };
                    let frontiers = frontiers?;
                    insert_initial_frontier(&mut *wallet.write().await, birthday, &frontiers)?;
                }
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
        tracing::debug!("Sync stage: scanning.");

        'scan: loop {
            tokio::select! {
                Some((load, scan_results)) = scan_results_receiver.recv() => {
                    for (load, scan_results) in scanner.admit_scan_results(load, scan_results) {
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
                            last_known_chain_height,
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
                            tracing::debug!("Sync stage: the mempool monitor reported a new block.");
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
                            let drain_verdict = mempool_drain_verdict(
                                shutdown_mempool.clone(),
                                unprocessed_mempool_transactions_count.clone(), mempool_shutdown_timer.get_or_insert_with(Instant::now).elapsed(),
                                mempool_stream_connected_at.get().map(|at| at.elapsed())
                            ).await;
                            tracing::debug!("Sync stage: shutdown requested, mempool drain verdict {drain_verdict:?}.");
                            match drain_verdict {
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
                        tracing::debug!("Sync stage: leaving the scan to check for new blocks.");
                        check_for_new_blocks = false;
                        first_verification_complete = true;
                        continue 'continuous_sync;
                    }

                    scanner.update(&mut *wallet.write().await, nullifier_map_limit_exceeded).await?;

                    if matches!(scanner.state, ScannerState::Complete) && config.shutdown_on_completion {
                        tracing::debug!("Sync stage: the scan is complete, requesting shutdown.");
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

    tracing::debug!("Sync stage: the scan loop ended, stopping the workers.");
    // shutdown workers and loader
    while scanner.worker_poolsize() != 0 {
        let worker_id = scanner
            .workers
            .first()
            .expect("non empty in this scope!")
            .id();
        scanner.shutdown_worker(worker_id).await;
    }
    tracing::debug!("Sync stage: stopping the loader.");
    scanner.shutdown_loader().await?;
    tracing::debug!("Sync stage: publishing the final sync status.");

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

    // a session shut down before scanning began has not asked the mempool monitor to stop.
    shutdown_mempool.store(true, atomic::Ordering::Release);
    tracing::debug!("Sync stage: joining the mempool monitor.");
    match mempool_handle.await.expect("task panicked") {
        Ok(()) => (),
        Err(e @ MempoolError::ShutdownWithoutStream) => tracing::warn!("{e}"),
        Err(e) => return Err(e.into()),
    }
    // the fetcher is aborted rather than joined: the only request it can still be serving is one that a shutdown
    // abandoned, whose reply nobody awaits, and a slow server must not hold the shutdown for it.
    tracing::debug!("Sync stage: joining the fetcher.");
    fetcher_handle.abort();
    if let Err(join_error) = fetcher_handle.await
        && join_error.is_panic()
    {
        std::panic::resume_unwind(join_error.into_panic());
    }
    tracing::info!("Sync successfully shutdown.");

    // a session shut down before its first scan range was created ends where it started.
    let sync_end_height = sync_status
        .scan_ranges
        .last()
        .map_or(sync_status.sync_start_height, |scan_range| {
            scan_range.block_range().end - 1
        });

    Ok(SyncResult {
        sync_start_height: sync_status.sync_start_height,
        sync_end_height,
        blocks_scanned: sync_status.session_blocks_scanned,
        sapling_outputs_scanned: sync_status.session_sapling_outputs_scanned,
        orchard_outputs_scanned: sync_status.session_orchard_outputs_scanned,
        ironwood_outputs_scanned: sync_status.session_ironwood_outputs_scanned,
        total_outputs_scanned: sync_status.total_outputs_scanned,
        total_outputs: sync_status.total_outputs,
        nullifier_retrieval_pending: sync_status.nullifier_retrieval_pending(),
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
            sync_state.truncate_scan_ranges(chain_height);
            // the truncated blocks are scanned again when the chain extends. transparent address discovery is only
            // performed at the start of the sync session so the compact block transparent data of these blocks must
            // be scanned.
            sync_state.lower_transparent_scan_floor(chain_height);
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
///
/// Intended to be called while [`self::sync`] is running in a separate task.
pub async fn sync_status<W>(wallet: &W) -> Result<SyncStatus, SyncStatusError<W::Error>>
where
    W: SyncWallet + SyncBlocks,
{
    /// Sums one per-pool trio of output counts into the pool-agnostic
    /// total. Pure and total: this is the single definition of which
    /// pools participate in scan-progress accounting, so every
    /// consumer (the per-pool counts and the exact u64 ratio) agrees by
    /// construction, and adding a pool touches exactly this function.
    fn output_pool_total(sapling: u32, orchard: u32, ironwood: u32) -> u64 {
        u64::from(sapling) + u64::from(orchard) + u64::from(ironwood)
    }

    /// The block a span's own first height contributes to the span's inclusive length.
    const INCLUSIVE_SPAN_ADJUSTMENT: u32 = 1;

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
    if sync_state.initial_sync_state().sync_start_height() == 0.into() {
        return Ok(SyncStatus {
            scan_ranges: sync_state.scan_ranges().to_vec(),
            sync_start_height: 0.into(),
            session_blocks_scanned: 0,
            total_blocks_scanned: 0,
            total_blocks: 0,
            session_sapling_outputs_scanned: 0,
            session_orchard_outputs_scanned: 0,
            session_ironwood_outputs_scanned: 0,
            total_sapling_outputs_scanned: 0,
            total_orchard_outputs_scanned: 0,
            total_ironwood_outputs_scanned: 0,
            total_outputs_scanned: 0,
            total_outputs: 0,
        });
    }
    let total_blocks_scanned = sync_state.calculate_scanned_blocks();

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
        .initial_sync_state()
        .wallet_tree_bounds()
        .sapling_final_tree_size
        .saturating_sub(
            sync_state
                .initial_sync_state()
                .wallet_tree_bounds()
                .sapling_initial_tree_size,
        );
    let total_orchard_outputs = sync_state
        .initial_sync_state()
        .wallet_tree_bounds()
        .orchard_final_tree_size
        .saturating_sub(
            sync_state
                .initial_sync_state()
                .wallet_tree_bounds()
                .orchard_initial_tree_size,
        );
    let total_ironwood_outputs = sync_state
        .initial_sync_state()
        .wallet_tree_bounds()
        .ironwood_final_tree_size
        .saturating_sub(
            sync_state
                .initial_sync_state()
                .wallet_tree_bounds()
                .ironwood_initial_tree_size,
        );
    let total_outputs = output_pool_total(
        total_sapling_outputs,
        total_orchard_outputs,
        total_ironwood_outputs,
    );

    let session_blocks_scanned = total_blocks_scanned
        .saturating_sub(sync_state.initial_sync_state().previously_scanned_blocks());

    let session_sapling_outputs_scanned = total_sapling_outputs_scanned.saturating_sub(
        sync_state
            .initial_sync_state()
            .previously_scanned_sapling_outputs(),
    );
    let session_orchard_outputs_scanned = total_orchard_outputs_scanned.saturating_sub(
        sync_state
            .initial_sync_state()
            .previously_scanned_orchard_outputs(),
    );
    let session_ironwood_outputs_scanned = total_ironwood_outputs_scanned.saturating_sub(
        sync_state
            .initial_sync_state()
            .previously_scanned_ironwood_outputs(),
    );
    Ok(SyncStatus {
        scan_ranges: sync_state.scan_ranges().to_vec(),
        sync_start_height: sync_state.initial_sync_state().sync_start_height(),
        session_blocks_scanned,
        total_blocks_scanned,
        total_blocks,
        session_sapling_outputs_scanned,
        total_sapling_outputs_scanned,
        session_orchard_outputs_scanned,
        total_orchard_outputs_scanned,
        session_ironwood_outputs_scanned,
        total_ironwood_outputs_scanned,
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

    let transparent_addresses: HashMap<String, TransparentAddressId> = wallet
        .get_transparent_addresses()
        .map_err(SyncError::WalletError)?
        .iter()
        .map(|(id, address)| (address.clone(), *id))
        .collect();
    let mut pending_transaction_nullifiers = NullifierMap::new();
    let mut pending_transaction_outpoints = BTreeMap::new();
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

    // return if transaction is not relevant to the wallet
    let spends_wallet_outputs =
        !spend::collect_transaction_spends(&pending_transaction, wallet_transactions).is_empty();
    if pending_transaction.transparent_coins().is_empty()
        && pending_transaction.sapling_notes().is_empty()
        && pending_transaction.orchard_notes().is_empty()
        && pending_transaction.ironwood_notes().is_empty()
        && pending_transaction.outgoing_sapling_notes().is_empty()
        && pending_transaction.outgoing_orchard_notes().is_empty()
        && pending_transaction.outgoing_ironwood_notes().is_empty()
        && !spends_wallet_outputs
    {
        return Ok(());
    }

    wallet
        .insert_wallet_transaction(pending_transaction)
        .map_err(SyncError::WalletError)?;

    Ok(())
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
    for transaction in wallet_transactions.values_mut() {
        reset_invalid_spends(transaction.ironwood_notes_mut(), &invalid_txids);
        reset_invalid_spends(transaction.orchard_notes_mut(), &invalid_txids);
        reset_invalid_spends(transaction.sapling_notes_mut(), &invalid_txids);
        reset_invalid_spends(transaction.transparent_coins_mut(), &invalid_txids);
    }
}

/// - Sets the spending transaction of each of `outputs` spent by one of `invalid_txids` to `None`.
fn reset_invalid_spends<O: OutputInterface>(outputs: Vec<&mut O>, invalid_txids: &[TxId]) {
    for output in outputs {
        if output
            .spending_transaction()
            .is_some_and(|spending_txid| invalid_txids.contains(&spending_txid))
        {
            output.set_spending_transaction(None);
        }
    }
}

/// Sets transactions associated with list of `failed_txids` in `wallet_transactions` to `Failed` status.
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
    use zl_netutils::time::{MEMPOOL_DRAIN_CEILING, MEMPOOL_DRAIN_SETTLE};

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
///
/// `previous_chain_height` is the chain height the wallet's scanned state was formed against: the chain tip of
/// the previous session, or the one before the blocks a continuous session found newly mined. The validator
/// finalizes every block more than the re-org allowance below its tip, so a hash that disagrees that far below
/// it is a different chain and clears the wallet with the chain error, as a re-org deeper than the allowance
/// does.
#[allow(clippy::too_many_arguments)]
async fn process_scan_results<W>(
    consensus_parameters: &(impl consensus::Parameters + Sync),
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    load: ScanLoad,
    in_flight_tasks: &BTreeMap<TaskId, ScanRange>,
    scan_results: Result<ScanResults, ScanError>,
    previous_chain_height: BlockHeight,
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
        chain_height,
        highest_scanned_height,
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
            sync_state.reset_stale_load(task.priority(), &reset);
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
                dropped_scan_targets,
                sapling_located_trees,
                orchard_located_trees,
                ironwood_located_trees,
                new_transparent_inuse_addresses,
                new_transparent_gap_addresses,
            } = results;

            wallet
                .get_sync_state_mut()
                .map_err(SyncError::WalletError)?
                .retain_scan_targets(|scan_target| !dropped_scan_targets.contains(scan_target));

            if scan_range.priority() == ScanPriority::ScannedWithoutMapping {
                let first_unscanned_range = wallet
                    .get_sync_state()
                    .map_err(SyncError::WalletError)?
                    .scan_ranges()
                    .iter()
                    .find(|scan_range| scan_range.priority() != ScanPriority::Scanned)
                    .expect("the scan range being processed is not yet set to scanned so at least one unscanned range must exist");
                if !first_unscanned_range.encloses(scan_range.block_range()) {
                    // in this rare edge case, a scanned `ScannedWithoutMapping` range was the highest priority yet it was not the first unscanned range so it must be discarded to avoid missing spends

                    // reset scan range from `RefetchingNullifiers` to `ScannedWithoutMapping`
                    wallet
                        .get_sync_state_mut()
                        .map_err(SyncError::WalletError)?
                        .reset_refetching_nullifiers_scan_range(scan_range.block_range().clone());
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
                    .scan_ranges()
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
                                crate::wallet::wallet_block_from_compact_block(
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

                wallet
                    .get_sync_state_mut()
                    .map_err(SyncError::WalletError)?
                    .set_scanned_scan_range(
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
                    chain_height,
                    highest_scanned_height,
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

                wallet
                    .get_sync_state_mut()
                    .map_err(SyncError::WalletError)?
                    .set_scanned_scan_range(scan_range.block_range().clone(), map_nullifiers);
                wallet
                    .get_sync_state_mut()
                    .map_err(SyncError::WalletError)?
                    .merge_scan_ranges(ScanPriority::ScannedWithoutMapping);
            }

            wallet
                .get_sync_state_mut()
                .map_err(SyncError::WalletError)?
                .merge_scan_ranges(ScanPriority::Scanned);
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

                sync_state.reset_verify_scan_range(scan_range.block_range());

                // extend verification range to VERIFY_BLOCK_RANGE_SIZE blocks below current verification range
                let current_reorg_detection_start_height = sync_state
                    .set_verify_scan_range(height - 1, state::VerifyEnd::VerifyHighest)
                    .block_range()
                    .start;
                sync_state.merge_scan_ranges(ScanPriority::Verify);

                // a block more than the allowance below the chain height the wallet's state was formed against is
                // final on the wallet's chain, whatever its highest scanned block, so a disagreement there is a
                // different chain rather than a re-org to truncate
                if previous_chain_height - current_reorg_detection_start_height
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
                sync_state.reopen_scan_ranges_inner(current_reorg_detection_start_height);
                // transparent address discovery is not performed again during this sync session so the compact block
                // transparent data of the re-orged blocks must be scanned.
                sync_state.lower_transparent_scan_floor(reorg_truncate_height);

                // the truncated wallet with its scan ranges reopened is complete as it is. the requests made here
                // only feed the initial sync state, which reports the progress of this session and is calculated
                // again at the start of the next one, so a failed request costs nothing that lasts.
                let tree_bounds_source =
                    state::TreeBoundsSource::read(&*wallet, last_known_chain_height)
                        .map_err(SyncError::WalletError)?;
                let tree_bounds = state::fetch_tree_bounds(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    tree_bounds_source,
                )
                .await?;
                state::set_initial_state(wallet, last_known_chain_height, tree_bounds)?;

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
                sync_state.reset_stale_load(
                    task.priority(),
                    std::slice::from_ref(scan_range.block_range()),
                );
                sync_state.set_verify_scan_range(height, state::VerifyEnd::VerifyLowest);
                sync_state.merge_scan_ranges(ScanPriority::Verify);

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
    // the wallet's records of the pool are wrong, not the chain's: these transactions were served and are
    // targeted as the sync engine's own, so one the server no longer serves is reported as an inconsistency.
    let rescan_targets = wallet
        .get_wallet_transactions()
        .map_err(SyncError::WalletError)?
        .values()
        .filter(|&transaction| transaction.status().is_confirmed_after_or_at(&rescan_from))
        .map(|transaction| {
            ScanTarget::internal(transaction.status().get_height(), transaction.txid())
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
            sync_state.clear_shard_ranges(pool);
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
    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    for rescan_target in rescan_targets {
        sync_state.insert_scan_target(rescan_target);
    }
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
/// A rollback also removes the pool's subtree roots above the highest scanned height. [`fetch_subtree_roots`]
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
        // to re-scan it, restoring its status.
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

/// Removes wallet blocks, transactions, nullifiers, outpoints and scan targets above the
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
    // a target above the truncation names a transaction in a block the wallet no longer holds. scanning the
    // replacing blocks finds the transaction again where the chain still holds it.
    wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?
        .retain_scan_targets(|scan_target| scan_target.block_height() <= truncate_height);

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
                .map(|height| ScanTarget::new(height, Some(transaction.txid())))
        })
        .collect::<Vec<_>>();
    truncate_wallet_data(wallet, consensus::H0)?;
    wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?
        .truncate_scan_ranges(consensus::H0);
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
    chain_height: BlockHeight,
    highest_scanned_height: BlockHeight,
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
    // the updates that may fail are applied before any other, so a failure leaves the wallet without the rest of
    // the scan results. address discovery only adds addresses of the wallet's own keys, and the shard trees fetch
    // every checkpoint they are missing before they are updated.
    for transaction in transactions.values() {
        discover_unified_addresses(wallet, ufvks, transaction).map_err(SyncError::WalletError)?;
    }
    wallet
        .update_shard_trees(
            consensus_parameters,
            fetch_request_sender,
            scan_range,
            chain_height,
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
        sync_state.update_found_note_shard_priority(
            consensus_parameters,
            ShieldedPool::Sapling,
            transaction,
        );
        sync_state.update_found_note_shard_priority(
            consensus_parameters,
            ShieldedPool::Orchard,
            transaction,
        );
        sync_state.update_found_note_shard_priority(
            consensus_parameters,
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
        transaction.set_refetch_nullifier_ranges(&refetch_nullifier_ranges);
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
        .retain(|_, scan_target| scan_target.block_height() > fully_scanned_height);
    wallet
        .get_nullifiers_mut()?
        .sapling
        .retain(|_, scan_target| scan_target.block_height() > fully_scanned_height);
    wallet
        .get_nullifiers_mut()?
        .orchard
        .retain(|_, scan_target| scan_target.block_height() > fully_scanned_height);
    wallet
        .get_nullifiers_mut()?
        .ironwood
        .retain(|_, scan_target| scan_target.block_height() > fully_scanned_height);
    wallet
        .get_sync_state_mut()?
        .drop_settled_scan_targets(fully_scanned_height);
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

/// Where each pool's subtree root fetch resumes, read from the shard trees and the recorded shard ranges.
#[derive(Debug, Clone, Copy)]
struct SubtreeRootFetchStart {
    sapling_start_index: u32,
    orchard_start_index: u32,
    ironwood_start_index: u32,
}

/// The subtree roots fetched from the server, with the fetch start they resume from.
#[derive(Debug)]
struct FetchedSubtreeRoots {
    fetch_start: SubtreeRootFetchStart,
    sapling: Vec<SubtreeRoot>,
    orchard: Vec<SubtreeRoot>,
    ironwood: Vec<SubtreeRoot>,
}

fn subtree_root_fetch_start<W>(wallet: &W) -> Result<SubtreeRootFetchStart, SyncError<W::Error>>
where
    W: SyncWallet + SyncShardTrees,
{
    let shard_trees = wallet.get_shard_trees().map_err(SyncError::WalletError)?;
    let sync_state = wallet.get_sync_state().map_err(SyncError::WalletError)?;
    Ok(SubtreeRootFetchStart {
        sapling_start_index: subtree_root_fetch_index(
            &shard_trees.sapling,
            sync_state.sapling_shard_ranges(),
        ),
        orchard_start_index: subtree_root_fetch_index(
            &shard_trees.orchard,
            sync_state.orchard_shard_ranges(),
        ),
        ironwood_start_index: subtree_root_fetch_index(
            &shard_trees.ironwood,
            sync_state.ironwood_shard_ranges(),
        ),
    })
}

async fn fetch_subtree_roots(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    fetch_start: SubtreeRootFetchStart,
) -> Result<FetchedSubtreeRoots, ServerError> {
    let (sapling_subtree_roots, orchard_subtree_roots, ironwood_subtree_roots) = futures::join!(
        client::get_subtree_roots(
            fetch_request_sender.clone(),
            fetch_start.sapling_start_index,
            0,
            0
        ),
        client::get_subtree_roots(
            fetch_request_sender.clone(),
            fetch_start.orchard_start_index,
            1,
            0
        ),
        client::get_subtree_roots(fetch_request_sender, fetch_start.ironwood_start_index, 2, 0)
    );

    Ok(FetchedSubtreeRoots {
        fetch_start,
        sapling: sapling_subtree_roots?,
        orchard: orchard_subtree_roots?,
        ironwood: ironwood_subtree_roots?,
    })
}

fn add_fetched_subtree_roots<W>(
    consensus_parameters: &impl consensus::Parameters,
    wallet: &mut W,
    subtree_roots: FetchedSubtreeRoots,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncShardTrees,
{
    let FetchedSubtreeRoots {
        fetch_start:
            SubtreeRootFetchStart {
                sapling_start_index,
                orchard_start_index,
                ironwood_start_index,
            },
        sapling: sapling_subtree_roots,
        orchard: orchard_subtree_roots,
        ironwood: ironwood_subtree_roots,
    } = subtree_roots;

    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    for (pool, start_index, subtree_roots) in [
        (
            ShieldedPool::Sapling,
            sapling_start_index,
            &sapling_subtree_roots,
        ),
        (
            ShieldedPool::Orchard,
            orchard_start_index,
            &orchard_subtree_roots,
        ),
        (
            ShieldedPool::Ironwood,
            ironwood_start_index,
            &ironwood_subtree_roots,
        ),
    ] {
        state::add_shard_ranges(
            consensus_parameters,
            pool,
            sync_state,
            start_index,
            subtree_roots,
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

fn subtree_root_fetch_index<H, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    shard_tree: &ShardTree<MemoryShardStore<H, BlockHeight>, DEPTH, SHARD_HEIGHT>,
    shard_ranges: &[Range<BlockHeight>],
) -> u32
where
    H: incrementalmerkletree::Hashable + Clone + PartialEq,
{
    // The shard tree resumes from its stored shard count, which scanning advances past the roots the server
    // holds, so the shard ranges resume from their own count instead: the newest recorded root is fetched again
    // every session, which corrects a range whose completing block a re-org moved. The lower index serves both.
    // The ranges are rebuilt from it (see `state::add_shard_ranges`) and the tree replaces only the shards that
    // scanning has not populated (see `witness::add_subtree_roots`).
    let newest_recorded_root = shard_ranges.len().saturating_sub(1) as u32;
    witness::subtree_fetch_start_index(shard_tree).min(newest_recorded_root)
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

/// The birthday whose frontiers complete the shard trees, where the shard store only holds the first checkpoint
/// added on initialisation and the birthday is above sapling activation.
fn initial_frontier_height<W>(
    consensus_parameters: &impl consensus::Parameters,
    wallet: &W,
) -> Result<Option<BlockHeight>, SyncError<W::Error>>
where
    W: SyncWallet + SyncShardTrees,
{
    let birthday = wallet.get_birthday().map_err(SyncError::WalletError)?;
    if birthday
        == consensus_parameters
            .activation_height(consensus::NetworkUpgrade::Sapling)
            .expect("sapling activation height should always return Some")
    {
        return Ok(None);
    }

    let shard_trees = wallet.get_shard_trees().map_err(SyncError::WalletError)?;
    Ok((shard_trees
        .sapling
        .store()
        .checkpoint_count()
        .expect("infallible")
        == 1)
        .then_some(birthday))
}

fn insert_initial_frontier<W>(
    wallet: &mut W,
    birthday: BlockHeight,
    frontiers: &witness::Frontiers,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncShardTrees,
{
    let shard_trees = wallet
        .get_shard_trees_mut()
        .map_err(SyncError::WalletError)?;
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
                tracing::debug!("Mempool monitor: the stream is connected.");
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
                                     tracing::debug!("Mempool monitor: the stream ended, reporting a new block.");
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

    tracing::debug!("Mempool monitor: stopped.");
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
    let Some(fully_scanned_height) = wallet
        .get_sync_state()
        .map_err(SyncError::WalletError)?
        .fully_scanned_height()
    else {
        return Ok(());
    };
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
mod test;
