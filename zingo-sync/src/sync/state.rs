//! Module for reading and updating the fields of [`crate::primitives::SyncState`] which tracks the wallet's state of sync.

use std::{cmp, collections::HashMap, ops::Range};

use tokio::sync::mpsc;
use zcash_client_backend::data_api::scanning::{ScanPriority, ScanRange};
use zcash_primitives::{
    consensus::{self, BlockHeight, NetworkUpgrade},
    transaction::TxId,
};

use crate::{
    client::{self, FetchRequest},
    keys::transparent::TransparentAddressId,
    primitives::{Locator, SyncState},
    scan::task::ScanTask,
    traits::{SyncBlocks, SyncWallet},
};

use super::{BATCH_SIZE, VERIFY_BLOCK_RANGE_SIZE};

/// Used to determine which end of the scan range is verified.
pub(super) enum VerifyEnd {
    VerifyHighest,
    VerifyLowest,
}

/// Returns the last known chain height stored in the wallet.
///
/// If no chain height is yet known, returns the highest value of the wallet birthday or sapling activation height.
pub(super) fn get_wallet_height<P, W>(
    consensus_parameters: &P,
    wallet: &W,
) -> Result<BlockHeight, ()>
where
    P: consensus::Parameters,
    W: SyncWallet,
{
    let wallet_height =
        if let Some(highest_range) = wallet.get_sync_state().unwrap().scan_ranges().last() {
            highest_range.block_range().end - 1
        } else {
            let wallet_birthday = wallet.get_birthday().unwrap();
            let sapling_activation_height = consensus_parameters
                .activation_height(consensus::NetworkUpgrade::Sapling)
                .expect("sapling activation height should always return Some");

            let highest = match wallet_birthday.cmp(&sapling_activation_height) {
                cmp::Ordering::Greater | cmp::Ordering::Equal => wallet_birthday,
                cmp::Ordering::Less => sapling_activation_height,
            };
            highest - 1
        };

    Ok(wallet_height)
}

/// Returns the locators for a given `block_range` from the wallet's [`crate::primitives::SyncState`]
// TODO: unit test high priority
fn find_locators(sync_state: &SyncState, block_range: &Range<BlockHeight>) -> Vec<Locator> {
    sync_state
        .locators()
        .range(
            (block_range.start, TxId::from_bytes([0; 32]))
                ..(block_range.end, TxId::from_bytes([0; 32])),
        )
        .cloned()
        .collect()
}

// TODO: remove locators after range is scanned

/// Update scan ranges for scanning
pub(super) async fn update_scan_ranges(
    wallet_height: BlockHeight,
    chain_height: BlockHeight,
    sync_state: &mut SyncState,
) -> Result<(), ()> {
    reset_scan_ranges(sync_state)?;
    create_scan_range(wallet_height, chain_height, sync_state).await?;
    set_found_note_scan_range(sync_state)?;
    set_chain_tip_scan_range(sync_state, chain_height)?;

    // TODO: add logic to merge scan ranges

    Ok(())
}

/// Create scan range between the wallet height and the chain height from the server.
async fn create_scan_range(
    wallet_height: BlockHeight,
    chain_height: BlockHeight,
    sync_state: &mut SyncState,
) -> Result<(), ()> {
    if wallet_height == chain_height {
        return Ok(());
    }

    let scan_ranges = sync_state.scan_ranges_mut();

    let new_scan_range = ScanRange::from_parts(
        Range {
            start: wallet_height + 1,
            end: chain_height + 1,
        },
        ScanPriority::Historic,
    );
    scan_ranges.push(new_scan_range);
    if scan_ranges.is_empty() {
        panic!("scan ranges should never be empty after updating");
    }

    set_verify_scan_range(sync_state, wallet_height + 1, VerifyEnd::VerifyLowest);

    Ok(())
}

/// Resets scan ranges to recover from previous sync interruptions.
///
/// A range that was previously scanning when sync was last interrupted should be set to `Verify` in the case that
/// the scanner may have been in the verification state.
fn reset_scan_ranges(sync_state: &mut SyncState) -> Result<(), ()> {
    let scan_ranges = sync_state.scan_ranges_mut();
    let previously_scanning_scan_ranges = scan_ranges
        .iter()
        .filter(|range| range.priority() == ScanPriority::Ignored)
        .cloned()
        .collect::<Vec<_>>();
    for scan_range in previously_scanning_scan_ranges {
        set_scan_priority(sync_state, scan_range.block_range(), ScanPriority::Verify).unwrap();
    }

    // TODO: determine OpenAdjacent priority ranges from the end block of previous ChainTip ranges

    Ok(())
}

/// Splits out the highest or lowest `VERIFY_BLOCK_RANGE_SIZE` blocks from the scan range containing the given `block height`
/// and sets it's priority to `Verify`.
/// Returns a clone of the scan range to be verified.
pub(super) fn set_verify_scan_range(
    sync_state: &mut SyncState,
    block_height: BlockHeight,
    verify_end: VerifyEnd,
) -> ScanRange {
    let (index, scan_range) = sync_state
        .scan_ranges()
        .iter()
        .enumerate()
        .find(|(_, range)| range.block_range().contains(&block_height))
        .expect("scan range containing given block height should always exist!");

    let block_range_to_verify = match verify_end {
        VerifyEnd::VerifyHighest => Range {
            start: scan_range.block_range().end - VERIFY_BLOCK_RANGE_SIZE,
            end: scan_range.block_range().end,
        },
        VerifyEnd::VerifyLowest => Range {
            start: scan_range.block_range().start,
            end: scan_range.block_range().start + VERIFY_BLOCK_RANGE_SIZE,
        },
    };

    let split_ranges =
        split_out_scan_range(scan_range, &block_range_to_verify, ScanPriority::Verify);

    let scan_range_to_verify = match verify_end {
        VerifyEnd::VerifyHighest => split_ranges
            .last()
            .expect("vec should always be non-empty")
            .clone(),
        VerifyEnd::VerifyLowest => split_ranges
            .first()
            .expect("vec should always be non-empty")
            .clone(),
    };

    sync_state
        .scan_ranges_mut()
        .splice(index..=index, split_ranges);

    scan_range_to_verify
}

/// Punches in the shard block ranges surrounding each locator with `ScanPriority::FoundNote` (TODO).
fn set_found_note_scan_range(sync_state: &mut SyncState) -> Result<(), ()> {
    let locator_heights: Vec<BlockHeight> = sync_state
        .locators()
        .iter()
        .map(|(block_height, _)| *block_height)
        .collect();
    locator_heights.into_iter().for_each(|block_height| {
        // TODO: add protocol info to locators to determine whether the orchard or sapling shard should be scanned
        let block_range = determine_block_range(block_height);
        punch_scan_priority(sync_state, &block_range, ScanPriority::FoundNote).unwrap();
    });

    Ok(())
}

/// Punches in the chain tip block range with `ScanPriority::ChainTip`.
///
/// Determines the chain tip block range by finding the lowest height of the latest completed shard for each shielded
/// protocol (TODO).
fn set_chain_tip_scan_range(
    sync_state: &mut SyncState,
    chain_height: BlockHeight,
) -> Result<(), ()> {
    // TODO: when batching by shards, determine the lowest height that covers both orchard and sapling incomplete shards
    let chain_tip = determine_block_range(chain_height);

    punch_scan_priority(sync_state, &chain_tip, ScanPriority::ChainTip).unwrap();

    Ok(())
}

/// Sets the scan range in `sync_state` with `block_range` to the given `scan_priority`.
///
/// Panics if no scan range is found in `sync_state` with a block range of exactly `block_range`.
pub(super) fn set_scan_priority(
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

/// Punches in a `scan_priority` for given `block_range`.
///
/// This function will set all scan ranges in `sync_state` with block range boundaries contained by `block_range` to
/// the given `scan_priority`.
/// Any scan ranges with `Ignored` (Scanning) or `Scanned` priority or with higher (or equal) priority than
/// `scan_priority` will be ignored.
/// If any scan ranges in `sync_state` are found to overlap with the given `block_range`, they will be split at the
/// boundary and the new scan ranges contained by `block_range` will be set to `scan_priority`.
fn punch_scan_priority(
    sync_state: &mut SyncState,
    block_range: &Range<BlockHeight>,
    scan_priority: ScanPriority,
) -> Result<(), ()> {
    let mut fully_contained_scan_ranges = Vec::new();
    let mut overlapping_scan_ranges = Vec::new();

    for (index, scan_range) in sync_state.scan_ranges().iter().enumerate() {
        if scan_range.priority() == ScanPriority::Scanned
            || scan_range.priority() == ScanPriority::Ignored
            || scan_range.priority() >= scan_priority
        {
            continue;
        }

        match (
            block_range.contains(&scan_range.block_range().start),
            block_range.contains(&scan_range.block_range().end),
        ) {
            (true, true) => fully_contained_scan_ranges.push(scan_range.clone()),
            (true, false) | (false, true) => {
                overlapping_scan_ranges.push((index, scan_range.clone()))
            }
            (false, false) => (),
        }
    }

    for scan_range in fully_contained_scan_ranges {
        set_scan_priority(sync_state, scan_range.block_range(), scan_priority).unwrap();
    }

    // split out the scan ranges in reverse order to maintain the correct index for lower scan ranges
    for (index, scan_range) in overlapping_scan_ranges.into_iter().rev() {
        let split_ranges = split_out_scan_range(&scan_range, block_range, scan_priority);
        sync_state
            .scan_ranges_mut()
            .splice(index..=index, split_ranges);
    }

    Ok(())
}

/// Determines which range of blocks should be scanned for a given `block_height`
fn determine_block_range(block_height: BlockHeight) -> Range<BlockHeight> {
    let start = block_height - (u32::from(block_height) % BATCH_SIZE); // TODO: will be replaced with first block of associated orchard shard
    let end = start + BATCH_SIZE; // TODO: will be replaced with last block of associated orchard shard
    Range { start, end }
}

/// Takes a scan range and splits it at `block_range.start` and `block_range.end`, returning a vec of scan ranges where
/// the scan range with the specified `block_range` has the given `scan_priority`.
///
/// If `block_range` goes beyond the bounds of `scan_range.block_range()` no splitting will occur at the upper and/or
/// lower bound but the priority will still be updated
///
/// Panics if no blocks in `block_range` are contained within `scan_range.block_range()`
fn split_out_scan_range(
    scan_range: &ScanRange,
    block_range: &Range<BlockHeight>,
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

/// Selects and prepares the next scan range for scanning.
///
/// Sets the range for scanning to `Ignored` priority in the wallet `sync_state` but returns the scan range with its initial priority.
/// Returns `None` if there are no more ranges to scan.
fn select_scan_range(sync_state: &mut SyncState) -> Option<ScanRange> {
    let scan_ranges = sync_state.scan_ranges_mut();

    let mut scan_ranges_priority_sorted: Vec<&ScanRange> = scan_ranges.iter().collect();
    scan_ranges_priority_sorted.sort_by(|a, b| b.block_range().start.cmp(&a.block_range().start));
    scan_ranges_priority_sorted.sort_by_key(|scan_range| scan_range.priority());
    let highest_priority_scan_range = scan_ranges_priority_sorted
        .pop()
        .expect("scan ranges should be non-empty after setup")
        .clone();
    if highest_priority_scan_range.priority() == ScanPriority::Scanned
        || highest_priority_scan_range.priority() == ScanPriority::Ignored
    {
        return None;
    }

    let (index, selected_scan_range) = scan_ranges
        .iter_mut()
        .enumerate()
        .find(|(_, scan_range)| {
            scan_range.block_range() == highest_priority_scan_range.block_range()
        })
        .expect("scan range should exist");

    // TODO: batch by shards
    let batch_block_range = Range {
        start: selected_scan_range.block_range().start,
        end: selected_scan_range.block_range().start + BATCH_SIZE,
    };
    let split_ranges = split_out_scan_range(
        selected_scan_range,
        &batch_block_range,
        ScanPriority::Ignored,
    );

    let trimmed_block_range = split_ranges
        .first()
        .expect("vec should always be non-empty")
        .block_range()
        .clone();

    scan_ranges.splice(index..=index, split_ranges);

    // TODO: when this library has its own version of ScanRange this can be simplified and more readable
    Some(ScanRange::from_parts(
        trimmed_block_range,
        highest_priority_scan_range.priority(),
    ))
}

/// Creates a scan task to be sent to a [`crate::scan::task::ScanWorker`] for scanning.
pub(crate) fn create_scan_task<W>(wallet: &mut W) -> Result<Option<ScanTask>, ()>
where
    W: SyncWallet + SyncBlocks,
{
    if let Some(scan_range) = select_scan_range(wallet.get_sync_state_mut().unwrap()) {
        // TODO: disallow scanning without previous wallet block
        let previous_wallet_block = wallet
            .get_wallet_block(scan_range.block_range().start - 1)
            .ok();

        let locators = find_locators(wallet.get_sync_state().unwrap(), scan_range.block_range());
        let transparent_addresses: HashMap<String, TransparentAddressId> = wallet
            .get_transparent_addresses()
            .unwrap()
            .iter()
            .map(|(id, address)| (address.clone(), *id))
            .collect();

        Ok(Some(ScanTask::from_parts(
            scan_range,
            previous_wallet_block,
            locators,
            transparent_addresses,
        )))
    } else {
        Ok(None)
    }
}

/// Sets the `initial_sync_state` field at the start of the sync session
pub(super) async fn set_initial_state<W>(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    wallet: &mut W,
    chain_height: BlockHeight,
) where
    W: SyncWallet + SyncBlocks,
{
    struct ScannedRangeTreeBoundaries {
        sapling_initial_tree_size: u32,
        orchard_initial_tree_size: u32,
        sapling_final_tree_size: u32,
        orchard_final_tree_size: u32,
    }

    /// Gets `block_height` final tree sizes from wallet block if it exists, otherwise from frontiers fetched from server.
    ///
    /// Only used in context of setting initial sync state as can also use the compact blocks to calculate tree sizes
    /// during scanning.
    async fn final_tree_sizes<W>(
        consensus_parameters: &impl consensus::Parameters,
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        wallet: &mut W,
        block_height: BlockHeight,
    ) -> (u32, u32)
    where
        W: SyncWallet + SyncBlocks,
    {
        if let Ok(block) = wallet.get_wallet_block(block_height) {
            (
                block.sapling_final_tree_size(),
                block.orchard_final_tree_size(),
            )
        } else {
            // TODO: move this whole block into `client::get_frontiers`
            let sapling_activation_height = consensus_parameters
                .activation_height(NetworkUpgrade::Sapling)
                .expect("should have some sapling activation height");

            match block_height.cmp(&sapling_activation_height) {
                cmp::Ordering::Greater => {
                    let frontiers =
                        client::get_frontiers(fetch_request_sender.clone(), block_height)
                            .await
                            .unwrap();
                    (
                        frontiers
                            .final_sapling_tree()
                            .tree_size()
                            .try_into()
                            .expect("should not be more than 2^32 note commitments in the tree!"),
                        frontiers
                            .final_orchard_tree()
                            .tree_size()
                            .try_into()
                            .expect("should not be more than 2^32 note commitments in the tree!"),
                    )
                }
                cmp::Ordering::Equal => (0, 0),
                cmp::Ordering::Less => panic!("pre-sapling not supported!"),
            }
        }
    }

    /// Gets the initial and final tree sizes of a `scanned_range`.
    ///
    /// Panics if `scanned_range` boundary wallet blocks are not found in the wallet.
    fn scanned_range_tree_boundaries<W>(
        wallet: &mut W,
        scanned_range: Range<BlockHeight>,
    ) -> ScannedRangeTreeBoundaries
    where
        W: SyncWallet + SyncBlocks,
    {
        let start_block = wallet
            .get_wallet_block(scanned_range.start)
            .expect("scanned range boundary blocks should be retained in the wallet");
        let end_block = wallet
            .get_wallet_block(scanned_range.end - 1)
            .expect("scanned range boundary blocks should be retained in the wallet");

        ScannedRangeTreeBoundaries {
            sapling_initial_tree_size: start_block.sapling_initial_tree_size(),
            orchard_initial_tree_size: start_block.orchard_initial_tree_size(),
            sapling_final_tree_size: end_block.sapling_final_tree_size(),
            orchard_final_tree_size: end_block.orchard_final_tree_size(),
        }
    }

    let fully_scanned_height = wallet.get_sync_state().unwrap().fully_scanned_height();

    let (sync_start_sapling_tree_size, sync_start_orchard_tree_size) = final_tree_sizes(
        consensus_parameters,
        fetch_request_sender.clone(),
        wallet,
        fully_scanned_height,
    )
    .await;
    let (chain_tip_sapling_tree_size, chain_tip_orchard_tree_size) = final_tree_sizes(
        consensus_parameters,
        fetch_request_sender.clone(),
        wallet,
        chain_height,
    )
    .await;

    let sync_state = wallet.get_sync_state().unwrap();
    let total_blocks_to_scan = sync_state
        .scan_ranges()
        .iter()
        .filter(|scan_range| scan_range.priority() != ScanPriority::Scanned)
        .map(|scan_range| scan_range.block_range())
        .fold(0, |acc, block_range| {
            acc + (block_range.end - block_range.start)
        });

    let scanned_block_ranges = sync_state
        .scan_ranges()
        .iter()
        .filter(|scan_range| {
            scan_range.priority() == ScanPriority::Scanned
                && scan_range.block_range().start > fully_scanned_height
        })
        .map(|scan_range| scan_range.block_range().clone())
        .collect::<Vec<_>>();
    let (scanned_sapling_outputs, scanned_orchard_outputs) = scanned_block_ranges
        .iter()
        .map(|block_range| scanned_range_tree_boundaries(wallet, block_range.clone()))
        .fold((0, 0), |acc, tree_sizes| {
            (
                acc.0 + (tree_sizes.sapling_final_tree_size - tree_sizes.sapling_initial_tree_size),
                acc.1 + (tree_sizes.orchard_final_tree_size - tree_sizes.orchard_initial_tree_size),
            )
        });
    let total_sapling_outputs_to_scan =
        chain_tip_sapling_tree_size - sync_start_sapling_tree_size - scanned_sapling_outputs;
    let total_orchard_outputs_to_scan =
        chain_tip_orchard_tree_size - sync_start_orchard_tree_size - scanned_orchard_outputs;

    let initial_sync_state = wallet
        .get_sync_state_mut()
        .unwrap()
        .initial_sync_state_mut();
    initial_sync_state.set_sync_start_height(fully_scanned_height + 1);
    initial_sync_state.set_sync_start_sapling_tree_size(sync_start_sapling_tree_size);
    initial_sync_state.set_sync_start_orchard_tree_size(sync_start_orchard_tree_size);
    initial_sync_state.set_total_blocks_to_scan(total_blocks_to_scan);
    initial_sync_state.set_total_sapling_outputs_to_scan(total_sapling_outputs_to_scan);
    initial_sync_state.set_total_orchard_outputs_to_scan(total_orchard_outputs_to_scan);
}
