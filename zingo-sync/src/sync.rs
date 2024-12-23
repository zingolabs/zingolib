//! Entrypoint for sync engine

use std::collections::HashMap;
use std::sync::atomic::{self, AtomicBool};
use std::sync::Arc;
use std::time::Duration;

use crate::client::{self, FetchRequest};
use crate::error::SyncError;
use crate::keys::transparent::TransparentAddressId;
use crate::primitives::{NullifierMap, OutPointMap};
use crate::scan::error::{ContinuityError, ScanError};
use crate::scan::task::Scanner;
use crate::scan::transactions::scan_transaction;
use crate::scan::{DecryptedNoteData, ScanResults};
use crate::traits::{
    SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions, SyncWallet,
};

use zcash_client_backend::proto::service::RawTransaction;
use zcash_client_backend::{
    data_api::scanning::{ScanPriority, ScanRange},
    proto::service::compact_tx_streamer_client::CompactTxStreamerClient,
};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::{self, BlockHeight};

use tokio::sync::{mpsc, Mutex};
use zcash_primitives::zip32::AccountId;
use zingo_status::confirmation_status::ConfirmationStatus;

pub(crate) mod spend;
pub(crate) mod state;
pub(crate) mod transparent;

// TODO: move parameters to config module
// TODO; replace fixed batches with orchard shard ranges (block ranges containing all note commitments to an orchard shard or fragment of a shard)
const BATCH_SIZE: u32 = 5_000;
const VERIFY_BLOCK_RANGE_SIZE: u32 = 10;
const MAX_VERIFICATION_WINDOW: u32 = 100; // TODO: fail if re-org goes beyond this window

/// Syncs a wallet to the latest state of the blockchain
pub async fn sync<P, W>(
    client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>, // TODO: change underlying service for generic
    consensus_parameters: &P,
    wallet: Arc<Mutex<W>>,
) -> Result<(), SyncError>
where
    P: consensus::Parameters + Sync + Send + 'static,
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    tracing::info!("Syncing wallet...");

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

    let mut wallet_lock = wallet.lock().await;
    let wallet_height = state::get_wallet_height(consensus_parameters, &*wallet_lock).unwrap();
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
        truncate_wallet_data(&mut *wallet_lock, chain_height).unwrap();
    }
    let ufvks = wallet_lock.get_unified_full_viewing_keys().unwrap();

    // create channel for receiving mempool transactions and launch mempool monitor
    let (mempool_transaction_sender, mut mempool_transaction_receiver) = mpsc::channel(10);
    let shutdown_mempool = Arc::new(AtomicBool::new(false));
    let shutdown_mempool_clone = shutdown_mempool.clone();
    let mempool_handle = tokio::spawn(async move {
        mempool_monitor(client, mempool_transaction_sender, shutdown_mempool_clone).await
    });

    transparent::update_addresses_and_locators(
        consensus_parameters,
        &mut *wallet_lock,
        fetch_request_sender.clone(),
        &ufvks,
        wallet_height,
        chain_height,
    )
    .await;

    state::update_scan_ranges(
        wallet_height,
        chain_height,
        wallet_lock.get_sync_state_mut().unwrap(),
    )
    .await
    .unwrap();
    drop(wallet_lock);

    // create channel for receiving scan results and launch scanner
    let (scan_results_sender, mut scan_results_receiver) = mpsc::unbounded_channel();
    let mut scanner = Scanner::new(
        consensus_parameters.clone(),
        scan_results_sender,
        fetch_request_sender.clone(),
        ufvks.clone(),
    );
    scanner.spawn_workers();

    // TODO: consider what happens when there is no verification range i.e. all ranges already scanned
    // TODO: invalidate any pending transactions after eviction height (40 below best chain height?)
    // TODO: implement an option for continuous scanning where it doesnt exit when complete

    let mut wallet_lock = wallet.lock().await;
    let mut interval = tokio::time::interval(Duration::from_millis(30));
    loop {
        tokio::select! {
            Some((scan_range, scan_results)) = scan_results_receiver.recv() => {
                process_scan_results(
                    consensus_parameters,
                    &mut *wallet_lock,
                    fetch_request_sender.clone(),
                    &ufvks,
                    scan_range,
                    scan_results,
                )
                .await
                .unwrap();

                // allow tasks outside the sync engine access to the wallet data
                drop(wallet_lock);
                wallet_lock = wallet.lock().await;
            }

            Some(raw_transaction) = mempool_transaction_receiver.recv() => {
                process_mempool_transaction(
                    consensus_parameters,
                    &ufvks,
                    &mut *wallet_lock,
                    raw_transaction,
                )
                .await;
            }

            _update_scanner = interval.tick() => {
                scanner.update(&mut *wallet_lock, shutdown_mempool.clone()).await;

                if sync_complete(&scanner, &scan_results_receiver, &*wallet_lock) {
                    tracing::info!("Sync complete.");
                    break;
                }
            }
        }
    }

    drop(wallet_lock);
    drop(scanner);
    drop(fetch_request_sender);
    mempool_handle.await.unwrap().unwrap();
    fetcher_handle.await.unwrap().unwrap();

    Ok(())
}

/// TODO
pub async fn sync_status<W>(wallet: Arc<Mutex<W>>) -> Result<(), SyncError>
where
    W: SyncWallet,
{
    let scan_ranges = wallet
        .lock()
        .await
        .get_sync_state()
        .unwrap()
        .scan_ranges()
        .clone();

    dbg!(scan_ranges);

    Ok(())
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
) -> Result<(), SyncError>
where
    P: consensus::Parameters,
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    match scan_results {
        Ok(results) => {
            update_wallet_data(wallet, results).unwrap();
            spend::update_transparent_spends(wallet).unwrap();
            spend::update_shielded_spends(
                consensus_parameters,
                wallet,
                fetch_request_sender,
                ufvks,
            )
            .await
            .unwrap();
            remove_irrelevant_data(wallet, &scan_range).unwrap();
            state::set_scan_priority(
                wallet.get_sync_state_mut().unwrap(),
                scan_range.block_range(),
                ScanPriority::Scanned,
            )
            .unwrap();
            tracing::info!("Scan results processed.");
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
/// Scan the transaction and add to the wallet if relavent.
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

    tracing::info!(
        "mempool received txid {} at height {}",
        transaction.txid(),
        block_height
    );

    if let Some(tx) = wallet
        .get_wallet_transactions()
        .unwrap()
        .get(&transaction.txid())
    {
        if tx.confirmation_status().is_confirmed() {
            return;
        }
    }

    let confirmation_status = ConfirmationStatus::Mempool(block_height);
    let mut mempool_transaction_nullifiers = NullifierMap::new();
    let mut mempool_transaction_outpoints = OutPointMap::new();
    let transparent_addresses: HashMap<String, TransparentAddressId> = wallet
        .get_transparent_addresses()
        .unwrap()
        .iter()
        .map(|(id, address)| (address.clone(), *id))
        .collect();
    let mempool_transaction = scan_transaction(
        consensus_parameters,
        ufvks,
        transaction,
        confirmation_status,
        &DecryptedNoteData::new(),
        &mut mempool_transaction_nullifiers,
        &mut mempool_transaction_outpoints,
        &transparent_addresses,
    )
    .unwrap();

    let wallet_transactions = wallet.get_wallet_transactions().unwrap();
    let transparent_output_ids = spend::collect_transparent_output_ids(wallet_transactions);
    let transparent_spend_locators = spend::detect_transparent_spends(
        &mut mempool_transaction_outpoints,
        transparent_output_ids,
    );
    let (sapling_derived_nullifiers, orchard_derived_nullifiers) =
        spend::collect_derived_nullifiers(wallet_transactions);
    let (sapling_spend_locators, orchard_spend_locators) = spend::detect_shielded_spends(
        &mut mempool_transaction_nullifiers,
        sapling_derived_nullifiers,
        orchard_derived_nullifiers,
    );

    // return if transaction is not relavent to the wallet
    if mempool_transaction.transparent_coins().is_empty()
        && mempool_transaction.sapling_notes().is_empty()
        && mempool_transaction.orchard_notes().is_empty()
        && mempool_transaction.outgoing_orchard_notes().is_empty()
        && mempool_transaction.outgoing_sapling_notes().is_empty()
        && transparent_spend_locators.is_empty()
        && sapling_spend_locators.is_empty()
        && orchard_spend_locators.is_empty()
    {
        return;
    }

    wallet
        .insert_wallet_transaction(mempool_transaction)
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
fn update_wallet_data<W>(wallet: &mut W, scan_results: ScanResults) -> Result<(), ()>
where
    W: SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    let ScanResults {
        nullifiers,
        outpoints,
        wallet_blocks,
        wallet_transactions,
        sapling_located_trees,
        orchard_located_trees,
    } = scan_results;

    wallet.append_wallet_blocks(wallet_blocks).unwrap();
    wallet
        .extend_wallet_transactions(wallet_transactions)
        .unwrap();
    wallet.append_nullifiers(nullifiers).unwrap();
    wallet.append_outpoints(outpoints).unwrap();
    wallet
        .update_shard_trees(sapling_located_trees, orchard_located_trees)
        .unwrap();
    // TODO: add trait to save wallet data to persistance for in-memory wallets

    Ok(())
}

// TODO: replace this function with a filter on the data added to wallet
fn remove_irrelevant_data<W>(wallet: &mut W, scan_range: &ScanRange) -> Result<(), ()>
where
    W: SyncWallet + SyncBlocks + SyncNullifiers + SyncTransactions,
{
    if scan_range.priority() != ScanPriority::Historic {
        return Ok(());
    }

    let wallet_height = wallet
        .get_sync_state()
        .unwrap()
        .scan_ranges()
        .last()
        .expect("wallet should always have scan ranges after sync has started")
        .block_range()
        .end;

    let wallet_transaction_heights = wallet
        .get_wallet_transactions()
        .unwrap()
        .values()
        .filter_map(|tx| tx.confirmation_status().get_confirmed_height())
        .collect::<Vec<_>>();
    wallet.get_wallet_blocks_mut().unwrap().retain(|height, _| {
        *height >= scan_range.block_range().end - 1
            || *height >= wallet_height - 100
            || wallet_transaction_heights.contains(height)
    });
    wallet
        .get_nullifiers_mut()
        .unwrap()
        .sapling_mut()
        .retain(|_, (height, _)| *height >= scan_range.block_range().end);
    wallet
        .get_nullifiers_mut()
        .unwrap()
        .orchard_mut()
        .retain(|_, (height, _)| *height >= scan_range.block_range().end);

    Ok(())
}

/// Sets up mempool stream.
///
/// If there is some raw transaction, send to be scanned.
/// If the response is `None` (a block was mined) or a timeout error occured, setup a new mempool stream.
async fn mempool_monitor(
    mut client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    mempool_transaction_sender: mpsc::Sender<RawTransaction>,
    shutdown_mempool: Arc<AtomicBool>,
) -> Result<(), ()> {
    let mut mempool_stream = client::get_mempool_transaction_stream(&mut client)
        .await
        .unwrap();
    loop {
        if shutdown_mempool.load(atomic::Ordering::Acquire) {
            break;
        }

        let mempool_stream_response = mempool_stream.message().await;
        match mempool_stream_response.unwrap_or(None) {
            Some(raw_transaction) => {
                mempool_transaction_sender
                    .send(raw_transaction)
                    .await
                    .unwrap();
            }
            None => {
                mempool_stream = client::get_mempool_transaction_stream(&mut client)
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }

    Ok(())
}
