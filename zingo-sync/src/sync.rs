//! Entrypoint for sync engine

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{self, AtomicBool};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::{mpsc, Mutex};

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
use crate::primitives::{NullifierMap, SyncStatus};
use crate::scan::error::{ContinuityError, ScanError};
use crate::scan::task::Scanner;
use crate::scan::transactions::scan_transaction;
use crate::scan::ScanResults;
use crate::traits::{
    SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions, SyncWallet,
};
use crate::witness;

pub(crate) mod spend;
pub(crate) mod state;
pub(crate) mod transparent;

const VERIFY_BLOCK_RANGE_SIZE: u32 = 10;
pub(crate) const MAX_VERIFICATION_WINDOW: u32 = 100;

/// Syncs a wallet to the latest state of the blockchain
pub async fn sync<P, W>(
    client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    consensus_parameters: &P,
    wallet: Arc<Mutex<W>>,
) -> Result<(), SyncError>
where
    P: consensus::Parameters + Sync + Send + 'static,
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
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
    let (mempool_transaction_sender, mut mempool_transaction_receiver) = mpsc::channel(10);
    let shutdown_mempool = Arc::new(AtomicBool::new(false));
    let shutdown_mempool_clone = shutdown_mempool.clone();
    let mempool_handle = tokio::spawn(async move {
        mempool_monitor(client, mempool_transaction_sender, shutdown_mempool_clone).await
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

    transparent::update_addresses_and_locators(
        consensus_parameters,
        &mut *wallet_guard,
        fetch_request_sender.clone(),
        &ufvks,
        wallet_height,
        chain_height,
    )
    .await;

    update_subtree_roots(
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

    // TODO: invalidate any pending transactions after eviction height (40 below best chain height?)
    // TODO: implement an option for continuous scanning where it doesnt exit when complete

    let mut wallet_guard = wallet.lock().await;
    let initial_verification_height = wallet_guard
        .get_sync_state()
        .unwrap()
        .highest_scanned_height()
        + 1;
    let mut interval = tokio::time::interval(Duration::from_millis(30));
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

                // allow tasks outside the sync engine access to the wallet data
                drop(wallet_guard);
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
            }

            _update_scanner = interval.tick() => {
                scanner.update(&mut *wallet_guard, shutdown_mempool.clone()).await;

                if sync_complete(&scanner, &scan_results_receiver, &*wallet_guard) {
                    tracing::info!("Sync complete.");
                    break;
                }
            }
        }
    }

    drop(wallet_guard);
    drop(scanner);
    drop(fetch_request_sender);
    mempool_handle.await.unwrap().unwrap();
    fetcher_handle.await.unwrap().unwrap();

    Ok(())
}

/// Obtains the mutex guard to the wallet and creates a [`crate::primitives::SyncStatus`] from the wallet's current
/// [`crate::primitives::SyncState`].
///
/// Designed to be called during the sync process with minimal interruption.
pub async fn sync_status<W>(wallet: Arc<Mutex<W>>) -> SyncStatus
where
    W: SyncWallet + SyncBlocks,
{
    let wallet_guard = wallet.lock().await;
    let sync_state = wallet_guard.get_sync_state().unwrap().clone();

    let unscanned_blocks = sync_state
        .scan_ranges()
        .iter()
        .filter(|scan_range| scan_range.priority() != ScanPriority::Scanned)
        .map(|scan_range| scan_range.block_range())
        .fold(0, |acc, block_range| {
            acc + (block_range.end - block_range.start)
        });
    let scanned_blocks = sync_state
        .initial_sync_state()
        .total_blocks_to_scan()
        .saturating_sub(unscanned_blocks);
    let percentage_blocks_scanned = (scanned_blocks as f32
        / sync_state.initial_sync_state().total_blocks_to_scan() as f32)
        * 100.0;

    let (unscanned_sapling_outputs, unscanned_orchard_outputs) =
        state::calculate_unscanned_outputs(&*wallet_guard);
    let scanned_sapling_outputs = sync_state
        .initial_sync_state()
        .total_sapling_outputs_to_scan()
        .saturating_sub(unscanned_sapling_outputs);
    let scanned_orchard_outputs = sync_state
        .initial_sync_state()
        .total_orchard_outputs_to_scan()
        .saturating_sub(unscanned_orchard_outputs);
    let percentage_outputs_scanned = ((scanned_sapling_outputs + scanned_orchard_outputs) as f32
        / (sync_state
            .initial_sync_state()
            .total_sapling_outputs_to_scan()
            + sync_state
                .initial_sync_state()
                .total_orchard_outputs_to_scan()) as f32)
        * 100.0;

    SyncStatus {
        scan_ranges: sync_state.scan_ranges().clone(),
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
    price: Option<f64>,
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
        transaction,
        status,
        None,
        &mut pending_transaction_nullifiers,
        &mut pending_transaction_outpoints,
        &transparent_addresses,
        datetime,
        price,
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

/// Returns true if sync is complete.
///
/// Sync is complete when:
/// - all scan workers have been shutdown
/// - there is no unprocessed scan results in the channel
/// - all scan ranges have `Scanned` priority
fn sync_complete<P, W>(
    scanner: &Scanner<P>,
    scan_results_receiver: &mpsc::UnboundedReceiver<(ScanRange, Result<ScanResults, ScanError>)>,
    wallet: &W,
) -> bool
where
    P: consensus::Parameters + Sync + Send + 'static,
    W: SyncWallet,
{
    scanner.worker_poolsize() == 0
        && scan_results_receiver.is_empty()
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
            update_wallet_data(consensus_parameters, wallet, results).unwrap();
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
            tracing::info!("Re-org detected.");
            if height == scan_range.block_range().start {
                // error handling in case of re-org where first block prev_hash in scan range does not match previous wallet block hash
                let sync_state = wallet.get_sync_state_mut().unwrap();
                state::set_scan_priority(
                    sync_state,
                    scan_range.block_range(),
                    scan_range.priority(),
                )
                .unwrap(); // reset scan range to initial priority in wallet sync state
                let scan_range_to_verify = state::set_verify_scan_range(
                    sync_state,
                    height - 1,
                    state::VerifyEnd::VerifyHighest,
                );
                truncate_wallet_data(wallet, scan_range_to_verify.block_range().start - 1).unwrap();

                if initial_verification_height - scan_range_to_verify.block_range().start
                    > MAX_VERIFICATION_WINDOW
                {
                    panic!(
                        "sync failed. re-org of larger than {} blocks detected",
                        MAX_VERIFICATION_WINDOW
                    );
                }
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
        None,
    );

    // TODO: consider logic for pending spent being set back to None when txs are evicted / never make it on chain
    // similar logic to truncate
}

/// Removes all wallet data above the given `truncate_height`.
fn truncate_wallet_data<W>(wallet: &mut W, truncate_height: BlockHeight) -> Result<(), ()>
where
    W: SyncBlocks + SyncTransactions + SyncNullifiers + SyncShardTrees,
{
    wallet.truncate_wallet_blocks(truncate_height).unwrap();
    wallet
        .truncate_wallet_transactions(truncate_height)
        .unwrap();
    wallet.truncate_nullifiers(truncate_height).unwrap();
    wallet.truncate_shard_trees(truncate_height).unwrap();

    Ok(())
}

/// Updates the wallet with data from `scan_results`
fn update_wallet_data<W>(
    consensus_parameters: &impl consensus::Parameters,
    wallet: &mut W,
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
        .update_shard_trees(sapling_located_trees, orchard_located_trees)
        .unwrap();

    Ok(())
}

fn remove_irrelevant_data<W>(wallet: &mut W) -> Result<(), ()>
where
    W: SyncWallet + SyncBlocks + SyncNullifiers + SyncTransactions,
{
    let sync_state = wallet.get_sync_state().unwrap();
    let fully_scanned_height = sync_state.fully_scanned_height();
    let highest_scanned_height = sync_state.highest_scanned_height();
    let sync_start_height = sync_state.initial_sync_state().sync_start_height();

    let scanned_block_range_boundaries = sync_state
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
            || scanned_block_range_boundaries.contains(height)
            || wallet_transaction_heights.contains(height)
    });
    wallet
        .get_nullifiers_mut()
        .unwrap()
        .sapling_mut()
        .retain(|_, (height, _)| *height > fully_scanned_height);
    wallet
        .get_nullifiers_mut()
        .unwrap()
        .orchard_mut()
        .retain(|_, (height, _)| *height > fully_scanned_height);
    wallet
        .get_sync_state_mut()
        .unwrap()
        .locators_mut()
        .retain(|(height, _)| *height > fully_scanned_height);

    Ok(())
}

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
        .sapling()
        .store()
        .get_shard_roots()
        .unwrap()
        .len() as u32;
    let orchard_start_index = wallet
        .get_shard_trees()
        .unwrap()
        .orchard()
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
    witness::add_subtree_roots(sapling_subtree_roots, shard_trees.sapling_mut());
    witness::add_subtree_roots(orchard_subtree_roots, shard_trees.orchard_mut());
}

/// Sets up mempool stream.
///
/// If there is some raw transaction, send to be scanned.
/// If the response is `None` (a block was mined) or a timeout error occurred, setup a new mempool stream.
async fn mempool_monitor(
    mut client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    mempool_transaction_sender: mpsc::Sender<RawTransaction>,
    shutdown_mempool: Arc<AtomicBool>,
) -> Result<(), ()> {
    let mut mempool_stream = client::get_mempool_transaction_stream(&mut client)
        .await
        .unwrap();
    loop {
        let mut interval = tokio::time::interval(Duration::from_secs(3));
        tokio::select! {
            mempool_stream_response = mempool_stream.message() => {
                match mempool_stream_response.unwrap_or(None) {
                    Some(raw_transaction) => {
                        mempool_transaction_sender
                            .send(raw_transaction)
                            .await
                            .unwrap();
                            interval.reset();
                    }
                    None => {
                        tokio::select! {
                            mempool_stream_response = client::get_mempool_transaction_stream(&mut client) => {
                                mempool_stream = mempool_stream_response.unwrap();
                                tokio::time::sleep(Duration::from_millis(500)).await;
                            }

                            _ = interval.tick() => {
                                if shutdown_mempool.load(atomic::Ordering::Acquire) {
                                    break;
                                }
                            }
                        }
                    }
                }

            }

            _ = interval.tick() => {
                if shutdown_mempool.load(atomic::Ordering::Acquire) {
                    break;
                }
            }
        }
    }

    Ok(())
}
