//! Entrypoint for sync engine

use std::cmp;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{self, AtomicBool, AtomicU8};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::{mpsc, Mutex};

use incrementalmerkletree::{Marking, Retention};
use shardtree::store::ShardStore;
use zcash_client_backend::proto::service::RawTransaction;
use zcash_client_backend::ShieldedProtocol;
use zcash_client_backend::{
    data_api::scanning::{ScanPriority, ScanRange},
    proto::service::compact_tx_streamer_client::CompactTxStreamerClient,
};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::{self, BlockHeight};
use zcash_primitives::transaction::Transaction;
use zcash_primitives::zip32::AccountId;

use zingo_status::confirmation_status::ConfirmationStatus;

use crate::client::{self, FetchRequest};
use crate::error::SyncError;
use crate::keys::transparent::TransparentAddressId;
use crate::scan::error::{ContinuityError, ScanError};
use crate::scan::task::{Scanner, ScannerState};
use crate::scan::transactions::scan_transaction;
use crate::scan::ScanResults;
use crate::wallet::traits::{
    SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions, SyncWallet,
};
use crate::wallet::{Locator, NullifierMap, SyncMode, SyncState};
use error::MempoolError;

#[cfg(not(feature = "darkside_test"))]
use crate::witness;

pub mod error;
pub(crate) mod spend;
pub(crate) mod state;
pub(crate) mod transparent;

const VERIFY_BLOCK_RANGE_SIZE: u32 = 10;
pub(crate) const MAX_VERIFICATION_WINDOW: u32 = 100;

/// A snapshot of the current state of sync. Useful for displaying the status of sync to a user / consumer.
///
/// `percentage_outputs_scanned` is a much more accurate indicator of sync completion than `percentage_blocks_scanned`.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct SyncStatus {
    pub scan_ranges: Vec<ScanRange>,
    pub sync_start_height: BlockHeight,
    pub scanned_blocks: u32,
    pub unscanned_blocks: u32,
    pub percentage_blocks_scanned: f32,
    pub scanned_sapling_outputs: u32,
    pub unscanned_sapling_outputs: u32,
    pub scanned_orchard_outputs: u32,
    pub unscanned_orchard_outputs: u32,
    pub percentage_outputs_scanned: f32,
}

// TODO: complete display, scan ranges in raw form are too verbose
impl std::fmt::Display for SyncStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{
                scanned blocks: {}
                percentage complete: {}
            }}",
            self.scanned_blocks, self.percentage_outputs_scanned
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
            "scanned_blocks" => value.scanned_blocks,
            "unscanned_blocks" => value.unscanned_blocks,
            "percentage_blocks_scanned" => value.percentage_blocks_scanned,
            "scanned_sapling_outputs" => value.scanned_sapling_outputs,
            "unscanned_sapling_outputs" => value.unscanned_sapling_outputs,
            "scanned_orchard_outputs" => value.scanned_orchard_outputs,
            "unscanned_orchard_outputs" => value.unscanned_orchard_outputs,
            "percentage_outputs_scanned" => value.percentage_outputs_scanned,
        }
    }
}

/// Returned when [`crate::sync::sync`] successfully completes.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub struct SyncResult {
    pub sync_start_height: BlockHeight,
    pub sync_end_height: BlockHeight,
    pub scanned_blocks: u32,
    pub scanned_sapling_outputs: u32,
    pub scanned_orchard_outputs: u32,
}

impl std::fmt::Display for SyncResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Sync completed succesfully:
{{
    sync start height: {}
    sync end height: {}
    scanned blocks: {}
    scanned sapling outputs: {}
    scanned orchard outputs: {}
}}",
            self.sync_start_height,
            self.sync_end_height,
            self.scanned_blocks,
            self.scanned_sapling_outputs,
            self.scanned_orchard_outputs
        )
    }
}

impl From<SyncResult> for json::JsonValue {
    fn from(value: SyncResult) -> Self {
        json::object! {
            "sync_start_height" => u32::from(value.sync_start_height),
            "sync_end_height" => u32::from(value.sync_end_height),
            "scanned_blocks" => value.scanned_blocks,
            "scanned_sapling_outputs" => value.scanned_sapling_outputs,
            "scanned_orchard_outputs" => value.scanned_orchard_outputs,
        }
    }
}

/// Syncs a wallet to the latest state of the blockchain.
///
/// `sync_mode` is intended to be stored in a struct that owns the wallet(s) (i.e. lightclient) and has a non-atomic
/// counterpart [`crate::wallet::SyncMode`]. The sync engine will set the `sync_mode` to `Running` or `NotRunning`
/// at the start and finish of sync, respectively. `sync_mode` may also be set to `Paused` externally to drop the wallet
/// lock after the next batch is completed and pause scanning. Setting `sync_mode` back to `Running` will resume
/// scanning when the wallet guard is next available.
///
/// If `transparent_address_discovery` is enabled, all transactions with relevant transparent input and/or outputs will
/// be scanned, with the in-use transparent addresses added to the wallet. The number of unused transparent addresses
/// above the in-use address with the highest address index for each scope and account is determined by
/// the address gap limit. If `transparent_address_discovery` is disabled, only transactions
/// with relevant shielded inputs/outputs will be scanned with the transparent addresses currently in the wallet.
// TODO: setting sync_mode to `NotRunning` should kill the sync task immediately.
pub async fn sync<P, W>(
    client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    consensus_parameters: &P,
    wallet: Arc<Mutex<W>>,
    sync_mode: Arc<AtomicU8>,
    transparent_address_discovery: bool,
) -> Result<SyncResult, SyncError>
where
    P: consensus::Parameters + Sync + Send + 'static,
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    let mut sync_mode_enum = SyncMode::from_u8(sync_mode.load(atomic::Ordering::Acquire)).unwrap();
    if sync_mode_enum == SyncMode::NotRunning {
        sync_mode_enum = SyncMode::Running;
        sync_mode.store(sync_mode_enum as u8, atomic::Ordering::Release);
    } else {
        panic!("Sync is already running!");
    }

    tracing::info!("Starting sync...");

    // create channel for sending fetch requests and launch fetcher task
    let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
    let client_clone = client.clone();
    let consensus_parameters_clone = consensus_parameters.clone();
    let fetcher_handle = tokio::spawn(async move {
        client::fetch::fetch(
            fetch_request_receiver,
            client_clone,
            consensus_parameters_clone,
        )
        .await
    });

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
    let mut wallet_guard = wallet.lock().await;

    let mut wallet_height = state::get_wallet_height(consensus_parameters, &*wallet_guard).unwrap();
    let chain_height = client::get_chain_height(fetch_request_sender.clone())
        .await
        .unwrap();
    if wallet_height > chain_height {
        if wallet_height - chain_height > MAX_VERIFICATION_WINDOW {
            panic!(
                "wallet height is more than {} blocks ahead of best chain height!",
                MAX_VERIFICATION_WINDOW
            );
        }
        truncate_wallet_data(&mut *wallet_guard, chain_height).unwrap();
        wallet_height = chain_height;
    }

    let ufvks = wallet_guard.get_unified_full_viewing_keys().unwrap();

    if transparent_address_discovery {
        transparent::update_addresses_and_locators(
            consensus_parameters,
            &mut *wallet_guard,
            fetch_request_sender.clone(),
            &ufvks,
            wallet_height,
            chain_height,
        )
        .await;
    }

    #[cfg(not(feature = "darkside_test"))]
    update_subtree_roots(
        consensus_parameters,
        fetch_request_sender.clone(),
        &mut *wallet_guard,
    )
    .await;

    add_initial_frontier(
        consensus_parameters,
        fetch_request_sender.clone(),
        &mut *wallet_guard,
    )
    .await;

    state::update_scan_ranges(
        consensus_parameters,
        wallet_height,
        chain_height,
        wallet_guard.get_sync_state_mut().unwrap(),
    )
    .await
    .unwrap();

    state::set_initial_state(
        consensus_parameters,
        fetch_request_sender.clone(),
        &mut *wallet_guard,
        chain_height,
    )
    .await;

    drop(wallet_guard);

    // create channel for receiving scan results and launch scanner
    let (scan_results_sender, mut scan_results_receiver) = mpsc::unbounded_channel();
    let mut scanner = Scanner::new(
        consensus_parameters.clone(),
        scan_results_sender,
        fetch_request_sender.clone(),
        ufvks.clone(),
    );
    scanner.launch();

    // TODO: implement an option for continuous scanning where it doesnt exit when complete

    let mut wallet_guard = wallet.lock().await;
    let initial_verification_height = wallet_guard
        .get_sync_state()
        .unwrap()
        .highest_scanned_height()
        .expect("scan ranges must be non-empty")
        + 1;
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            Some((scan_range, scan_results)) = scan_results_receiver.recv() => {
                process_scan_results(
                    consensus_parameters,
                    &mut *wallet_guard,
                    fetch_request_sender.clone(),
                    &ufvks,
                    scan_range,
                    scan_results,
                    initial_verification_height,
                )
                .await
                .unwrap();
                wallet_guard.set_save_flag().unwrap();

                // allow tasks outside the sync engine access to the wallet data
                drop(wallet_guard);

                sync_mode_enum = SyncMode::from_u8(sync_mode.load(atomic::Ordering::Acquire)).unwrap();
                if sync_mode_enum == SyncMode::Paused {
                    let mut pause_interval = tokio::time::interval(Duration::from_secs(1));
                    pause_interval.tick().await;
                    while sync_mode_enum != SyncMode::Running {
                        pause_interval.tick().await;
                        sync_mode_enum = SyncMode::from_u8(sync_mode.load(atomic::Ordering::Acquire)).unwrap();
                    }

                }

                wallet_guard = wallet.lock().await;
            }

            Some(raw_transaction) = mempool_transaction_receiver.recv() => {
                process_mempool_transaction(
                    consensus_parameters,
                    &ufvks,
                    &mut *wallet_guard,
                    raw_transaction,
                )
                .await;
                unprocessed_mempool_transactions_count.fetch_sub(1, atomic::Ordering::Release);
            }

            _update_scanner = interval.tick() => {
                scanner.update(&mut *wallet_guard, shutdown_mempool.clone()).await;

                if matches!(scanner.state, ScannerState::Shutdown) {
                    // wait for mempool monitor to receive mempool transactions
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if sync_complete(&scanner, unprocessed_mempool_transactions_count.clone(), &*wallet_guard) {
                        tracing::info!("Sync complete.");
                        break;
                    }
                }
            }
        }
    }

    let sync_status = sync_status(&*wallet_guard).await;
    wallet_guard.set_save_flag().unwrap();

    drop(wallet_guard);
    drop(scanner);
    drop(fetch_request_sender);

    match mempool_handle.await.unwrap() {
        Ok(_) => (),
        Err(e @ MempoolError::ShutdownWithoutStream) => tracing::warn!("{e}"),
        Err(e) => return Err(e.into()),
    }
    fetcher_handle.await.unwrap().unwrap();
    sync_mode.store(SyncMode::NotRunning as u8, atomic::Ordering::Release);

    Ok(SyncResult {
        sync_start_height: sync_status.sync_start_height,
        sync_end_height: (sync_status
            .scan_ranges
            .last()
            .expect("should be non-empty after syncing")
            .block_range()
            .end
            - 1),
        scanned_blocks: sync_status.scanned_blocks,
        scanned_sapling_outputs: sync_status.scanned_sapling_outputs,
        scanned_orchard_outputs: sync_status.scanned_orchard_outputs,
    })
}

/// Creates a [`self::SyncStatus`] from the wallet's current
/// [`crate::wallet::SyncState`].
///
/// Designed to be called during the sync process with minimal interruption.
pub async fn sync_status<W>(wallet: &W) -> SyncStatus
where
    W: SyncWallet + SyncBlocks,
{
    let sync_state = wallet.get_sync_state().unwrap().clone();

    let unscanned_blocks = sync_state
        .scan_ranges()
        .iter()
        .filter(|scan_range| scan_range.priority() != ScanPriority::Scanned)
        .map(|scan_range| scan_range.block_range())
        .fold(0, |acc, block_range| {
            acc + (block_range.end - block_range.start)
        });
    let scanned_blocks = sync_state
        .initial_sync_state
        .total_blocks_to_scan
        .saturating_sub(unscanned_blocks);
    let percentage_blocks_scanned =
        (scanned_blocks as f32 / sync_state.initial_sync_state.total_blocks_to_scan as f32) * 100.0;

    let (unscanned_sapling_outputs, unscanned_orchard_outputs) =
        state::calculate_unscanned_outputs(wallet);
    let scanned_sapling_outputs = sync_state
        .initial_sync_state
        .total_sapling_outputs_to_scan
        .saturating_sub(unscanned_sapling_outputs);
    let scanned_orchard_outputs = sync_state
        .initial_sync_state
        .total_orchard_outputs_to_scan
        .saturating_sub(unscanned_orchard_outputs);
    let percentage_outputs_scanned = ((scanned_sapling_outputs + scanned_orchard_outputs) as f32
        / (sync_state.initial_sync_state.total_sapling_outputs_to_scan
            + sync_state.initial_sync_state.total_orchard_outputs_to_scan) as f32)
        * 100.0;

    SyncStatus {
        scan_ranges: sync_state.scan_ranges.clone(),
        sync_start_height: sync_state.initial_sync_state.sync_start_height,
        scanned_blocks,
        unscanned_blocks,
        percentage_blocks_scanned,
        scanned_sapling_outputs,
        unscanned_sapling_outputs,
        scanned_orchard_outputs,
        unscanned_orchard_outputs,
        percentage_outputs_scanned,
    }
}

/// Scans a pending `transaction` of a given `status`, adding to the wallet and updating output spend statuses.
///
/// Used both internally for scanning mempool transactions and externally for scanning calculated and transmitted
/// transactions during send.
///
/// Fails if `status` is of `Confirmed` variant.
pub fn scan_pending_transaction<W>(
    consensus_parameters: &impl consensus::Parameters,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    wallet: &mut W,
    transaction: Transaction,
    status: ConfirmationStatus,
    datetime: u32,
) where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints,
{
    if matches!(status, ConfirmationStatus::Confirmed(_)) {
        panic!("this fn is for unconfirmed transactions only");
    }

    let mut pending_transaction_nullifiers = NullifierMap::new();
    let mut pending_transaction_outpoints = BTreeMap::new();
    let transparent_addresses: HashMap<String, TransparentAddressId> = wallet
        .get_transparent_addresses()
        .unwrap()
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
    )
    .unwrap();

    let wallet_transactions = wallet.get_wallet_transactions().unwrap();
    let transparent_output_ids = spend::collect_transparent_output_ids(wallet_transactions);
    let transparent_spend_locators = spend::detect_transparent_spends(
        &mut pending_transaction_outpoints,
        transparent_output_ids,
    );
    let (sapling_derived_nullifiers, orchard_derived_nullifiers) =
        spend::collect_derived_nullifiers(wallet_transactions);
    let (sapling_spend_locators, orchard_spend_locators) = spend::detect_shielded_spends(
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
        && transparent_spend_locators.is_empty()
        && sapling_spend_locators.is_empty()
        && orchard_spend_locators.is_empty()
    {
        return;
    }

    wallet
        .insert_wallet_transaction(pending_transaction)
        .unwrap();
    spend::update_spent_coins(
        wallet.get_wallet_transactions_mut().unwrap(),
        transparent_spend_locators,
    );
    spend::update_spent_notes(
        wallet.get_wallet_transactions_mut().unwrap(),
        sapling_spend_locators,
        orchard_spend_locators,
    );
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
pub fn add_scan_targets(sync_state: &mut SyncState, scan_targets: &[Locator]) {
    for scan_target in scan_targets {
        sync_state.locators.insert(*scan_target);
    }
}

/// Returns true if sync is complete.
///
/// Sync is complete when:
/// - all scan workers have been shutdown
/// - there is no unprocessed mempool transactions
/// - all scan ranges have `Scanned` priority
fn sync_complete<P, W>(
    scanner: &Scanner<P>,
    mempool_unprocessed_transactions_count: Arc<AtomicU8>,
    wallet: &W,
) -> bool
where
    P: consensus::Parameters + Sync + Send + 'static,
    W: SyncWallet,
{
    scanner.worker_poolsize() == 0
        && mempool_unprocessed_transactions_count.load(atomic::Ordering::Acquire) == 0
        && wallet.get_sync_state().unwrap().scan_complete()
}

/// Scan post-processing
async fn process_scan_results<P, W>(
    consensus_parameters: &P,
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_range: ScanRange,
    scan_results: Result<ScanResults, ScanError>,
    initial_verification_height: BlockHeight,
) -> Result<(), SyncError>
where
    P: consensus::Parameters,
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    match scan_results {
        Ok(results) => {
            update_wallet_data(consensus_parameters, wallet, &scan_range, results).unwrap();
            spend::update_transparent_spends(wallet).unwrap();
            spend::update_shielded_spends(
                consensus_parameters,
                wallet,
                fetch_request_sender,
                ufvks,
            )
            .await
            .unwrap();
            state::set_scanned_scan_range(
                wallet.get_sync_state_mut().unwrap(),
                scan_range.block_range().clone(),
            )
            .unwrap();
            remove_irrelevant_data(wallet).unwrap();
            tracing::debug!("Scan results processed.");
        }
        Err(ScanError::ContinuityError(ContinuityError::HashDiscontinuity { height, .. })) => {
            if height == scan_range.block_range().start
                && scan_range.priority() == ScanPriority::Verify
            {
                tracing::info!("Re-org detected.");
                let sync_state = wallet.get_sync_state_mut().unwrap();
                let wallet_height = sync_state
                    .wallet_height()
                    .expect("scan ranges should be non-empty in this scope");

                // reset scan range from `Ignored` to `Verify`
                state::set_scan_priority(
                    sync_state,
                    scan_range.block_range(),
                    ScanPriority::Verify,
                )
                .unwrap();

                // extend verification range to VERIFY_BLOCK_RANGE_SIZE blocks below current verifaction range
                let scan_range_to_verify = state::set_verify_scan_range(
                    sync_state,
                    height - 1,
                    state::VerifyEnd::VerifyHighest,
                );
                state::merge_verification_ranges(sync_state);

                truncate_wallet_data(wallet, scan_range_to_verify.block_range().start - 1).unwrap();

                if initial_verification_height - scan_range_to_verify.block_range().start
                    > MAX_VERIFICATION_WINDOW
                {
                    panic!(
                        "sync failed. re-org of larger than {} blocks detected",
                        MAX_VERIFICATION_WINDOW
                    );
                }

                state::set_initial_state(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    wallet,
                    wallet_height,
                )
                .await;
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
) where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints,
{
    let block_height = BlockHeight::from_u32(u32::try_from(raw_transaction.height).unwrap());
    let transaction = zcash_primitives::transaction::Transaction::read(
        &raw_transaction.data[..],
        consensus::BranchId::for_height(consensus_parameters, block_height),
    )
    .unwrap();

    tracing::debug!(
        "mempool received txid {} at height {}",
        transaction.txid(),
        block_height
    );

    if let Some(tx) = wallet
        .get_wallet_transactions()
        .unwrap()
        .get(&transaction.txid())
    {
        if tx.status().is_confirmed() {
            return;
        }
    }

    scan_pending_transaction(
        consensus_parameters,
        ufvks,
        wallet,
        transaction,
        ConfirmationStatus::Mempool(block_height),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32,
    );

    // TODO: consider logic for pending spent being set back to None when txs are evicted / never make it on chain
    // similar logic to truncate
}

/// Removes all wallet data above the given `truncate_height`.
fn truncate_wallet_data<W>(wallet: &mut W, truncate_height: BlockHeight) -> Result<(), ()>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncShardTrees,
{
    let birthday = wallet
        .get_sync_state()
        .unwrap()
        .wallet_birthday()
        .expect("should be non-empty in this scope");
    let checked_truncate_height = match truncate_height.cmp(&birthday) {
        std::cmp::Ordering::Greater | std::cmp::Ordering::Equal => truncate_height,
        std::cmp::Ordering::Less => birthday,
    };

    wallet
        .truncate_wallet_blocks(checked_truncate_height)
        .unwrap();
    wallet
        .truncate_wallet_transactions(checked_truncate_height)
        .unwrap();
    wallet.truncate_nullifiers(checked_truncate_height).unwrap();
    wallet
        .truncate_shard_trees(checked_truncate_height)
        .unwrap();

    Ok(())
}

/// Updates the wallet with data from `scan_results`
fn update_wallet_data<W>(
    consensus_parameters: &impl consensus::Parameters,
    wallet: &mut W,
    scan_range: &ScanRange,
    scan_results: ScanResults,
) -> Result<(), ()>
where
    W: SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    let ScanResults {
        nullifiers,
        mut outpoints,
        wallet_blocks,
        wallet_transactions,
        sapling_located_trees,
        orchard_located_trees,
    } = scan_results;

    let sync_state = wallet.get_sync_state_mut().unwrap();
    let wallet_height = sync_state
        .wallet_height()
        .expect("scan ranges should not be empty in this scope");
    for transaction in wallet_transactions.values() {
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

    wallet.append_wallet_blocks(wallet_blocks).unwrap();
    wallet
        .extend_wallet_transactions(wallet_transactions)
        .unwrap();
    wallet.append_nullifiers(nullifiers).unwrap();
    wallet.append_outpoints(&mut outpoints).unwrap();
    wallet
        .update_shard_trees(
            scan_range,
            wallet_height,
            sapling_located_trees,
            orchard_located_trees,
        )
        .unwrap();

    Ok(())
}

fn remove_irrelevant_data<W>(wallet: &mut W) -> Result<(), ()>
where
    W: SyncWallet + SyncBlocks + SyncNullifiers + SyncTransactions,
{
    let sync_state = wallet.get_sync_state().unwrap();
    let fully_scanned_height = sync_state
        .fully_scanned_height()
        .expect("scan ranges must be non-empty");
    let highest_scanned_height = sync_state
        .highest_scanned_height()
        .expect("scan ranges must be non-empty");
    let sync_start_height = sync_state.initial_sync_state.sync_start_height;

    let scanned_block_range_bounds = sync_state
        .scan_ranges()
        .iter()
        .filter(|scan_range| {
            scan_range.priority() == ScanPriority::Scanned
                && scan_range.block_range().start >= sync_start_height
        })
        .flat_map(|scan_range| {
            vec![
                scan_range.block_range().start,
                scan_range.block_range().end - 1,
            ]
        })
        .collect::<Vec<_>>();

    let wallet_transaction_heights = wallet
        .get_wallet_transactions()
        .unwrap()
        .values()
        .filter_map(|tx| tx.status().get_confirmed_height())
        .collect::<Vec<_>>();

    wallet.get_wallet_blocks_mut().unwrap().retain(|height, _| {
        *height >= sync_start_height - 1
            || *height >= highest_scanned_height - MAX_VERIFICATION_WINDOW
            || scanned_block_range_bounds.contains(height)
            || wallet_transaction_heights.contains(height)
    });
    wallet
        .get_nullifiers_mut()
        .unwrap()
        .sapling
        .retain(|_, (height, _)| *height > fully_scanned_height);
    wallet
        .get_nullifiers_mut()
        .unwrap()
        .orchard
        .retain(|_, (height, _)| *height > fully_scanned_height);
    wallet
        .get_sync_state_mut()
        .unwrap()
        .locators
        .retain(|(height, _)| *height > fully_scanned_height);

    Ok(())
}

#[cfg(not(feature = "darkside_test"))]
async fn update_subtree_roots<W>(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    wallet: &mut W,
) where
    W: SyncWallet + SyncShardTrees,
{
    let sapling_start_index = wallet
        .get_shard_trees()
        .unwrap()
        .sapling
        .store()
        .get_shard_roots()
        .unwrap()
        .len() as u32;
    let orchard_start_index = wallet
        .get_shard_trees()
        .unwrap()
        .orchard
        .store()
        .get_shard_roots()
        .unwrap()
        .len() as u32;
    let (sapling_subtree_roots, orchard_subtree_roots) = futures::join!(
        client::get_subtree_roots(fetch_request_sender.clone(), sapling_start_index, 0, 0),
        client::get_subtree_roots(fetch_request_sender, orchard_start_index, 1, 0)
    );

    let sapling_subtree_roots = sapling_subtree_roots.unwrap();
    let orchard_subtree_roots = orchard_subtree_roots.unwrap();

    let sync_state = wallet.get_sync_state_mut().unwrap();
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

    let shard_trees = wallet.get_shard_trees_mut().unwrap();
    witness::add_subtree_roots(sapling_subtree_roots, &mut shard_trees.sapling);
    witness::add_subtree_roots(orchard_subtree_roots, &mut shard_trees.orchard);
}

async fn add_initial_frontier<W>(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    wallet: &mut W,
) where
    W: SyncWallet + SyncShardTrees,
{
    let birthday = checked_birthday(consensus_parameters, wallet);
    if birthday
        == consensus_parameters
            .activation_height(consensus::NetworkUpgrade::Sapling)
            .expect("sapling activation height should always return Some")
    {
        return;
    }

    // if the shard store only contains the first checkpoint added on initialisation, add frontiers to complete the
    // shard trees.
    let shard_trees = wallet.get_shard_trees_mut().unwrap();
    if shard_trees
        .sapling
        .store()
        .checkpoint_count()
        .expect("infalliable")
        == 1
    {
        let frontiers = client::get_frontiers(fetch_request_sender, birthday)
            .await
            .unwrap();
        shard_trees
            .sapling
            .insert_frontier(
                frontiers.final_sapling_tree().clone(),
                Retention::Checkpoint {
                    id: birthday,
                    marking: Marking::None,
                },
            )
            .unwrap();
        shard_trees
            .orchard
            .insert_frontier(
                frontiers.final_orchard_tree().clone(),
                Retention::Checkpoint {
                    id: birthday,
                    marking: Marking::None,
                },
            )
            .unwrap();
    }
}

/// Compares the wallet birthday to sapling activation height and returns the highest block height.
fn checked_birthday<W: SyncWallet>(
    consensus_parameters: &impl consensus::Parameters,
    wallet: &W,
) -> BlockHeight {
    let wallet_birthday = wallet.get_birthday().unwrap();
    let sapling_activation_height = consensus_parameters
        .activation_height(consensus::NetworkUpgrade::Sapling)
        .expect("sapling activation height should always return Some");

    match wallet_birthday.cmp(&sapling_activation_height) {
        cmp::Ordering::Greater | cmp::Ordering::Equal => wallet_birthday,
        cmp::Ordering::Less => sapling_activation_height,
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
                                    mempool_transaction_sender
                                        .send(raw_transaction)
                                        .await
                                        .unwrap();
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
            Err(MempoolError::RequestFailed(e)) => {
                tracing::warn!("Mempool stream request failed! Status: {e}.\nRetrying...");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }

    Ok(())
}
