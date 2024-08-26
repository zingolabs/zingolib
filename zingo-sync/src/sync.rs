//! Entrypoint for sync engine

use std::cmp;
use std::collections::HashSet;
use std::ops::Range;
use std::time::Duration;

use crate::client::FetchRequest;
use crate::client::{fetch::fetch, get_chain_height};
use crate::primitives::SyncState;
use crate::scan::task::{ScanTask, Scanner};
use crate::scan::ScanResults;
use crate::traits::{SyncBlocks, SyncNullifiers, SyncShardTrees, SyncTransactions, SyncWallet};

use tokio::sync::mpsc::error::TryRecvError;
use zcash_client_backend::{
    data_api::scanning::{ScanPriority, ScanRange},
    proto::service::compact_tx_streamer_client::CompactTxStreamerClient,
};
use zcash_primitives::consensus::{BlockHeight, NetworkUpgrade, Parameters};

use futures::future::try_join_all;
use tokio::sync::mpsc;

const BATCH_SIZE: u32 = 10;
// const BATCH_SIZE: u32 = 1_000;

/// Syncs a wallet to the latest state of the blockchain
pub async fn sync<P, W>(
    client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>, // TODO: change underlying service for generic
    parameters: &P,
    wallet: &mut W,
) -> Result<(), ()>
where
    P: Parameters + Sync + Send + 'static,
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncShardTrees,
{
    tracing::info!("Syncing wallet...");

    let mut handles = Vec::new();

    // create channel for sending fetch requests and launch fetcher task
    let (fetch_request_sender, fetch_request_receiver) = mpsc::unbounded_channel();
    let fetcher_handle = tokio::spawn(fetch(fetch_request_receiver, client, parameters.clone()));
    handles.push(fetcher_handle);

    update_scan_ranges(
        fetch_request_sender.clone(),
        parameters,
        wallet.get_birthday().unwrap(),
        wallet.get_sync_state_mut().unwrap(),
    )
    .await
    .unwrap();

    // create channel for receiving scan results and launch scanner
    let (scan_results_sender, mut scan_results_receiver) = mpsc::unbounded_channel();
    let ufvks = wallet.get_unified_full_viewing_keys().unwrap();
    let mut scanner = Scanner::new(
        scan_results_sender,
        fetch_request_sender,
        parameters.clone(),
        ufvks,
    );
    scanner.spawn_workers();

    let mut interval = tokio::time::interval(Duration::from_millis(30));
    loop {
        interval.tick().await; // TODO: tokio select to recieve scan results before tick

        // if a scan worker is idle, send it a new scan task
        if scanner.is_worker_idle() {
            if let Some(scan_range) = prepare_next_scan_range(wallet.get_sync_state_mut().unwrap())
            {
                let previous_wallet_block = wallet
                    .get_wallet_block(scan_range.block_range().start - 1)
                    .ok();

                scanner
                    .add_scan_task(ScanTask::from_parts(scan_range, previous_wallet_block))
                    .unwrap();
            } else {
                // when no more ranges are available to scan, break out of the loop
                break;
            }
        }

        match scan_results_receiver.try_recv() {
            Ok((scan_range, scan_results)) => {
                process_scan_results(wallet, scan_range, scan_results).unwrap()
            }
            Err(TryRecvError::Empty) => (),
            Err(TryRecvError::Disconnected) => break,
        }
    }

    drop(scanner);
    while let Some((scan_range, scan_results)) = scan_results_receiver.recv().await {
        process_scan_results(wallet, scan_range, scan_results).unwrap();
    }

    try_join_all(handles).await.unwrap();

    Ok(())
}

// update scan_ranges to include blocks between the last known chain height (wallet height) and the chain height from the server
async fn update_scan_ranges<P>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    parameters: &P,
    wallet_birthday: BlockHeight,
    sync_state: &mut SyncState,
) -> Result<(), ()>
where
    P: Parameters,
{
    let chain_height = get_chain_height(fetch_request_sender).await.unwrap();

    let scan_ranges = sync_state.scan_ranges_mut();

    let wallet_height = if scan_ranges.is_empty() {
        let sapling_activation_height = parameters
            .activation_height(NetworkUpgrade::Sapling)
            .expect("sapling activation height should always return Some");

        match wallet_birthday.cmp(&sapling_activation_height) {
            cmp::Ordering::Greater | cmp::Ordering::Equal => wallet_birthday,
            cmp::Ordering::Less => sapling_activation_height,
        }
    } else {
        scan_ranges
            .last()
            .expect("Vec should not be empty")
            .block_range()
            .end
    };

    if wallet_height > chain_height {
        panic!("wallet is ahead of server!")
    }

    let chain_tip_scan_range = ScanRange::from_parts(
        Range {
            start: wallet_height,
            end: chain_height + 1,
        },
        ScanPriority::Historic,
    );
    scan_ranges.push(chain_tip_scan_range);

    if scan_ranges.is_empty() {
        panic!("scan ranges should never be empty after updating")
    }

    // TODO: add logic to combine chain tip scan range with wallet tip scan range
    // TODO: add scan priority logic
    // TODO: replace `ignored` (a.k.a scanning) priority with `verify` to prioritise ranges that were being scanned when sync was interrupted

    Ok(())
}

// returns `None` if there are no more ranges to scan
fn prepare_next_scan_range(sync_state: &mut SyncState) -> Option<ScanRange> {
    let scan_ranges = sync_state.scan_ranges_mut();

    // placeholder for algorythm that determines highest priority range to scan
    let (index, selected_scan_range) = scan_ranges.iter_mut().enumerate().find(|(_, range)| {
        range.priority() != ScanPriority::Scanned && range.priority() != ScanPriority::Ignored
    })?;

    // if scan range is larger than BATCH_SIZE, split off and return a batch from the lower end and update scan ranges
    if let Some((lower_range, higher_range)) = selected_scan_range
        .split_at(selected_scan_range.block_range().start + BlockHeight::from_u32(BATCH_SIZE))
    {
        let lower_range_ignored =
            ScanRange::from_parts(lower_range.block_range().clone(), ScanPriority::Ignored);
        scan_ranges.splice(index..=index, vec![lower_range_ignored, higher_range]);

        Some(lower_range)
    } else {
        let selected_scan_range = selected_scan_range.clone();
        let selected_range_ignored = ScanRange::from_parts(
            selected_scan_range.block_range().clone(),
            ScanPriority::Ignored,
        );
        scan_ranges.splice(index..=index, vec![selected_range_ignored]);

        Some(selected_scan_range.clone())
    }
}

fn process_scan_results<W>(
    wallet: &mut W,
    scan_range: ScanRange,
    scan_results: ScanResults,
) -> Result<(), ()>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncShardTrees,
{
    update_wallet_data(wallet, scan_results).unwrap();
    link_nullifiers(wallet).unwrap();
    remove_irrelevant_data(wallet, &scan_range).unwrap();
    mark_scanned(scan_range, wallet.get_sync_state_mut().unwrap()).unwrap();

    Ok(())
}

fn update_wallet_data<W>(wallet: &mut W, scan_results: ScanResults) -> Result<(), ()>
where
    W: SyncBlocks + SyncTransactions + SyncNullifiers + SyncShardTrees,
{
    let ScanResults {
        nullifiers,
        wallet_blocks,
        wallet_transactions,
        shard_tree_data,
    } = scan_results;

    wallet.append_wallet_blocks(wallet_blocks).unwrap();
    wallet
        .extend_wallet_transactions(wallet_transactions)
        .unwrap();
    wallet.append_nullifiers(nullifiers).unwrap();
    // TODO: pararellise shard tree, this is currently the bottleneck on sync
    wallet.update_shard_trees(shard_tree_data).unwrap();
    // TODO: add trait to save wallet data to persistance for in-memory wallets

    Ok(())
}

fn link_nullifiers<W>(wallet: &mut W) -> Result<(), ()>
where
    W: SyncWallet + SyncTransactions + SyncNullifiers,
{
    let wallet_transactions = wallet.get_wallet_transactions().unwrap();
    let wallet_txids = wallet_transactions.keys().copied().collect::<HashSet<_>>();
    let sapling_nullifiers = wallet_transactions
        .values()
        .flat_map(|tx| tx.sapling_notes())
        .flat_map(|note| note.nullifier())
        .collect::<Vec<_>>();
    let orchard_nullifiers = wallet_transactions
        .values()
        .flat_map(|tx| tx.orchard_notes())
        .flat_map(|note| note.nullifier())
        .collect::<Vec<_>>();

    let mut spend_locations = Vec::new();
    let nullifier_map = wallet.get_nullifiers_mut().unwrap();
    spend_locations.extend(
        sapling_nullifiers
            .iter()
            .flat_map(|nf| nullifier_map.sapling_mut().remove(&nf)),
    );
    spend_locations.extend(
        orchard_nullifiers
            .iter()
            .flat_map(|nf| nullifier_map.orchard_mut().remove(&nf)),
    );

    for (block_height, txid) in spend_locations {
        // skip found spend if transaction already exists in the wallet
        if wallet_txids.get(&txid).is_some() {
            continue;
        }

        let scan_ranges = wallet.get_sync_state_mut().unwrap().scan_ranges_mut();
        let (range_index, scan_range) = scan_ranges
            .iter()
            .enumerate()
            .find(|(_, range)| range.block_range().contains(&block_height))
            .expect("scan range should always exist for mapped nullifiers");

        // if the scan range with the found spend is already scanned, the wallet blocks will already be stored and the transaction can be scanned
        // if the scan range is currently being scanned (has `Ignored` priority), TODO: explain how the txid is stored for future scanning
        // otherwise, create a scan range with `FoundNote` priority TODO: explain how txid is stored here also
        if scan_range.priority() == ScanPriority::Scanned {
            // TODO: scan tx
        } else if scan_range.priority() == ScanPriority::Ignored {
            // TODO: store txid for scanning
        } else {
            // TODO: store txid for scanning
            // TODO: create found note range
        }
    }

    Ok(())
}

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
        .map(|tx| tx.block_height())
        .collect::<Vec<_>>();
    wallet.get_wallet_blocks_mut().unwrap().retain(|height, _| {
        *height >= scan_range.block_range().end
            || *height >= wallet_height.saturating_sub(100)
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

fn mark_scanned(scan_range: ScanRange, sync_state: &mut SyncState) -> Result<(), ()> {
    let scan_ranges = sync_state.scan_ranges_mut();

    if let Some((index, range)) = scan_ranges
        .iter()
        .enumerate()
        .find(|(_, range)| range.block_range() == scan_range.block_range())
    {
        scan_ranges[index] =
            ScanRange::from_parts(range.block_range().clone(), ScanPriority::Scanned);
    } else {
        panic!("scanned range not found!")
    }
    // TODO: also combine adjacent scanned ranges together

    Ok(())
}
