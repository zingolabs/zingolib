//! Entrypoint for sync engine

use std::cmp;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::time::Duration;

use crate::client::fetch::fetch;
use crate::client::{self, FetchRequest};
use crate::error::SyncError;
use crate::primitives::SyncState;
use crate::scan::error::{ContinuityError, ScanError};
use crate::scan::task::{ScanTask, Scanner};
use crate::scan::transactions::scan_transactions;
use crate::scan::{DecryptedNoteData, ScanResults};
use crate::traits::{SyncBlocks, SyncNullifiers, SyncShardTrees, SyncTransactions, SyncWallet};

use tokio::sync::mpsc::error::TryRecvError;
use zcash_client_backend::{
    data_api::scanning::{ScanPriority, ScanRange},
    proto::service::compact_tx_streamer_client::CompactTxStreamerClient,
};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::{BlockHeight, NetworkUpgrade, Parameters};

use futures::future::try_join_all;
use tokio::sync::mpsc;
use zcash_primitives::transaction::TxId;
use zcash_primitives::zip32::AccountId;

// TODO; replace fixed batches with orchard shard ranges (block ranges containing all note commitments to an orchard shard or fragment of a shard)
const BATCH_SIZE: u32 = 1_000;
const VERIFY_BLOCK_RANGE_SIZE: u32 = 10;

/// Syncs a wallet to the latest state of the blockchain
pub async fn sync<P, W>(
    client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>, // TODO: change underlying service for generic
    parameters: &P,
    wallet: &mut W,
) -> Result<(), SyncError>
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
        fetch_request_sender.clone(),
        parameters.clone(),
        ufvks.clone(),
    );
    scanner.spawn_workers();

    let mut interval = tokio::time::interval(Duration::from_millis(30));
    loop {
        interval.tick().await; // TODO: tokio select to recieve scan results before tick

        // if a scan worker is idle, send it a new scan task
        if scanner.is_worker_idle() {
            if let Some(scan_range) = select_scan_range(wallet.get_sync_state_mut().unwrap()) {
                let previous_wallet_block = wallet
                    .get_wallet_block(scan_range.block_range().start - 1)
                    .ok();

                scanner
                    .add_scan_task(ScanTask::from_parts(scan_range, previous_wallet_block))
                    .unwrap();
            } else {
                // when no more ranges are available to scan, break out of the loop
                // TODO: is the case where there is less than WORKER_POOLSIZE ranges left to scan but re-org is hit covered?
                break;
            }
        }

        match scan_results_receiver.try_recv() {
            Ok((scan_range, scan_results)) => {
                process_scan_results(
                    wallet,
                    fetch_request_sender.clone(),
                    parameters,
                    &ufvks,
                    scan_range,
                    scan_results,
                )
                .await
                .unwrap();
            }
            Err(TryRecvError::Empty) => (),
            Err(TryRecvError::Disconnected) => break,
        }
    }

    drop(scanner);
    while let Some((scan_range, scan_results)) = scan_results_receiver.recv().await {
        process_scan_results(
            wallet,
            fetch_request_sender.clone(),
            parameters,
            &ufvks,
            scan_range,
            scan_results,
        )
        .await
        .unwrap();
    }

    drop(fetch_request_sender);
    try_join_all(handles).await.unwrap();

    Ok(())
}

/// Update scan ranges to include blocks between the last known chain height (wallet height) and the chain height from the server
async fn update_scan_ranges<P>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    parameters: &P,
    wallet_birthday: BlockHeight,
    sync_state: &mut SyncState,
) -> Result<(), ()>
where
    P: Parameters,
{
    let chain_height = client::get_chain_height(fetch_request_sender)
        .await
        .unwrap();

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

/// Selects and prepares the next scan range for scanning.
/// Sets the range for scanning to `Ignored` priority in the wallet [sync_state] but returns the scan range with its initial priority.
/// Returns `None` if there are no more ranges to scan.
fn select_scan_range(sync_state: &mut SyncState) -> Option<ScanRange> {
    let scan_ranges = sync_state.scan_ranges_mut();

    // TODO: placeholder for algorythm that determines highest priority range to scan
    let (index, selected_scan_range) = scan_ranges.iter_mut().enumerate().find(|(_, range)| {
        range.priority() != ScanPriority::Scanned && range.priority() != ScanPriority::Ignored
    })?;

    // TODO: replace with new range split/splice helpers
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

/// Scan post-processing
async fn process_scan_results<P, W>(
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_range: ScanRange,
    scan_results: Result<ScanResults, ScanError>,
) -> Result<(), SyncError>
where
    P: Parameters,
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncShardTrees,
{
    match scan_results {
        Ok(results) => {
            update_wallet_data(wallet, results).unwrap();
            link_nullifiers(wallet, fetch_request_sender, parameters, ufvks)
                .await
                .unwrap();
            remove_irrelevant_data(wallet, &scan_range).unwrap();
            set_scan_priority(
                wallet.get_sync_state_mut().unwrap(),
                scan_range.block_range(),
                ScanPriority::Scanned,
            )
            .unwrap();
            // TODO: also combine adjacent scanned ranges together
        }
        Err(ScanError::ContinuityError(ContinuityError::HashDiscontinuity { height, .. })) => {
            if height == scan_range.block_range().start {
                // error handling in case of re-org where first block prev_hash in scan range does not match previous wallet block hash
                let sync_state = wallet.get_sync_state_mut().unwrap();
                set_scan_priority(sync_state, scan_range.block_range(), scan_range.priority())
                    .unwrap(); // reset scan range to initial priority in wallet sync state
                let scan_range_to_verify =
                    verify_scan_range_tip(sync_state, height.saturating_sub(1));
                invalidate_scan_range(wallet, scan_range_to_verify).unwrap();
            } else {
                scan_results?;
            }
        }
        Err(e) => return Err(e.into()),
    }

    Ok(())
}

fn invalidate_scan_range<W>(wallet: &mut W, scan_range_to_verify: ScanRange) -> Result<(), ()>
where
    W: SyncBlocks + SyncTransactions + SyncNullifiers,
{
    wallet
        .remove_wallet_blocks(scan_range_to_verify.block_range())
        .unwrap();
    wallet
        .remove_wallet_transactions(scan_range_to_verify.block_range())
        .unwrap();
    wallet
        .remove_nullifiers(scan_range_to_verify.block_range())
        .unwrap();

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

async fn link_nullifiers<P, W>(
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
) -> Result<(), ()>
where
    P: Parameters,
    W: SyncBlocks + SyncTransactions + SyncNullifiers,
{
    // locate spends
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

    let nullifier_map = wallet.get_nullifiers_mut().unwrap();
    let sapling_spend_locators: BTreeMap<sapling_crypto::Nullifier, (BlockHeight, TxId)> =
        sapling_nullifiers
            .iter()
            .flat_map(|nf| nullifier_map.sapling_mut().remove_entry(nf))
            .collect();
    let orchard_spend_locators: BTreeMap<orchard::note::Nullifier, (BlockHeight, TxId)> =
        orchard_nullifiers
            .iter()
            .flat_map(|nf| nullifier_map.orchard_mut().remove_entry(nf))
            .collect();

    // in the edge case where a spending transaction received no change, scan the transactions that evaded trial decryption
    let mut spending_txids = HashSet::new();
    let mut wallet_blocks = BTreeMap::new();
    for (block_height, txid) in sapling_spend_locators
        .values()
        .chain(orchard_spend_locators.values())
    {
        // skip if transaction already exists in the wallet
        if wallet_txids.contains(txid) {
            continue;
        }

        spending_txids.insert(*txid);
        wallet_blocks.insert(
            *block_height,
            wallet.get_wallet_block(*block_height).unwrap(),
        );
    }
    let spending_transactions = scan_transactions(
        fetch_request_sender,
        parameters,
        ufvks,
        spending_txids,
        DecryptedNoteData::new(),
        &wallet_blocks,
    )
    .await
    .unwrap();
    wallet
        .extend_wallet_transactions(spending_transactions)
        .unwrap();

    // add spending transaction for all spent notes
    let wallet_transactions = wallet.get_wallet_transactions_mut().unwrap();
    wallet_transactions
        .values_mut()
        .flat_map(|tx| tx.sapling_notes_mut())
        .filter(|note| note.spending_transaction().is_none())
        .for_each(|note| {
            if let Some((_, txid)) = note
                .nullifier()
                .and_then(|nf| sapling_spend_locators.get(&nf))
            {
                note.set_spending_transaction(Some(*txid));
            }
        });
    wallet_transactions
        .values_mut()
        .flat_map(|tx| tx.orchard_notes_mut())
        .filter(|note| note.spending_transaction().is_none())
        .for_each(|note| {
            if let Some((_, txid)) = note
                .nullifier()
                .and_then(|nf| orchard_spend_locators.get(&nf))
            {
                note.set_spending_transaction(Some(*txid));
            }
        });

    Ok(())
}

/// Splits out the highest VERIFY_BLOCK_RANGE_SIZE blocks from the scan range containing the given block height
/// and sets it's priority to `verify`.
/// Returns a clone of the scan range to be verified.
///
/// Panics if the scan range containing the given block height is not of priority `Scanned`
fn verify_scan_range_tip(sync_state: &mut SyncState, block_height: BlockHeight) -> ScanRange {
    let (index, scan_range) = sync_state
        .scan_ranges()
        .iter()
        .enumerate()
        .find(|(_, range)| range.block_range().contains(&block_height))
        .expect("scan range containing given block height should always exist!");

    if scan_range.priority() != ScanPriority::Scanned {
        panic!("scan range should always have scan priority `Scanned`!")
    }

    let block_range_to_verify = Range {
        start: scan_range
            .block_range()
            .end
            .saturating_sub(VERIFY_BLOCK_RANGE_SIZE),
        end: scan_range.block_range().end,
    };
    let split_ranges =
        split_out_scan_range(scan_range, block_range_to_verify, ScanPriority::Verify);

    assert!(split_ranges.len() == 2);
    let scan_range_to_verify = split_ranges
        .last()
        .expect("split_ranges should always have exactly 2 elements")
        .clone();

    sync_state
        .scan_ranges_mut()
        .splice(index..=index, split_ranges);

    scan_range_to_verify
}

/// Splits out a scan range surrounding a given block height with the specified priority
#[allow(dead_code)]
fn update_scan_priority(
    sync_state: &mut SyncState,
    block_height: BlockHeight,
    scan_priority: ScanPriority,
) {
    let (index, scan_range) = sync_state
        .scan_ranges()
        .iter()
        .enumerate()
        .find(|(_, range)| range.block_range().contains(&block_height))
        .expect("scan range should always exist for mapped nullifiers");

    // Skip if the given block height is within a range that is scanned or being scanning
    if scan_range.priority() == ScanPriority::Scanned
        || scan_range.priority() == ScanPriority::Ignored
    {
        return;
    }

    let new_block_range = determine_block_range(block_height);
    let split_ranges = split_out_scan_range(scan_range, new_block_range, scan_priority);
    sync_state
        .scan_ranges_mut()
        .splice(index..=index, split_ranges);
}

/// Determines which range of blocks should be scanned for a given block height
fn determine_block_range(block_height: BlockHeight) -> Range<BlockHeight> {
    let start = block_height - (u32::from(block_height) % BATCH_SIZE); // TODO: will be replaced with first block of associated orchard shard
    let end = start + BATCH_SIZE; // TODO: will be replaced with last block of associated orchard shard
    Range { start, end }
}

/// Takes a scan range and splits it at [block_range.start] and [block_range.end], returning a vec of scan ranges where
/// the scan range with the specified [block_range] has the given [scan_priority].
///
/// If [block_range] goes beyond the bounds of [scan_range.block_range()] no splitting will occur at the upper and/or
/// lower bound but the priority will still be updated
///
/// Panics if no blocks in [block_range] are contained within [scan_range.block_range()]
fn split_out_scan_range(
    scan_range: &ScanRange,
    block_range: Range<BlockHeight>,
    scan_priority: ScanPriority,
) -> Vec<ScanRange> {
    let mut split_ranges = Vec::new();
    if let Some((lower_range, higher_range)) = scan_range.split_at(block_range.start) {
        split_ranges.push(lower_range);
        if let Some((middle_range, higher_range)) = higher_range.split_at(block_range.end) {
            // [scan_range] is split at the upper and lower bound of [block_range]
            split_ranges.push(ScanRange::from_parts(
                middle_range.block_range().clone(),
                scan_priority,
            ));
            split_ranges.push(higher_range);
        } else {
            // [scan_range] is split only at the lower bound of [block_range]
            split_ranges.push(ScanRange::from_parts(
                higher_range.block_range().clone(),
                scan_priority,
            ));
        }
    } else if let Some((lower_range, higher_range)) = scan_range.split_at(block_range.end) {
        // [scan_range] is split only at the upper bound of [block_range]
        split_ranges.push(ScanRange::from_parts(
            lower_range.block_range().clone(),
            scan_priority,
        ));
        split_ranges.push(higher_range);
    } else {
        // [scan_range] is not split as it is fully contained within [block_range]
        // only scan priority is updated
        assert!(scan_range.block_range().start >= block_range.start);
        assert!(scan_range.block_range().end <= block_range.end);

        split_ranges.push(ScanRange::from_parts(
            scan_range.block_range().clone(),
            scan_priority,
        ));
    };

    split_ranges
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
        .map(|tx| tx.block_height())
        .collect::<Vec<_>>();
    wallet.get_wallet_blocks_mut().unwrap().retain(|height, _| {
        *height >= scan_range.block_range().end.saturating_sub(1)
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

/// Sets the scan range in [sync_state] with [block_range] to the given [scan_priority].
///
/// Panics if no scan range is found in [sync_state] with a block range of exactly [block_range].
fn set_scan_priority(
    sync_state: &mut SyncState,
    block_range: &Range<BlockHeight>,
    scan_priority: ScanPriority,
) -> Result<(), ()> {
    let scan_ranges = sync_state.scan_ranges_mut();

    if let Some((index, range)) = scan_ranges
        .iter()
        .enumerate()
        .find(|(_, range)| range.block_range() == block_range)
    {
        scan_ranges[index] = ScanRange::from_parts(range.block_range().clone(), scan_priority);
    } else {
        panic!("scan range with block range {:?} not found!", block_range)
    }

    Ok(())
}
