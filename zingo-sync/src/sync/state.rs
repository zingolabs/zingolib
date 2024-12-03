//! Module for reading and updating the fields of [`crate::primitives::SyncState`] which tracks the wallet's state of sync.

use std::{
    cmp,
    collections::HashMap,
    ops::{Add, Range},
};

use zcash_client_backend::data_api::scanning::{ScanPriority, ScanRange};
use zcash_primitives::{
    consensus::{self, BlockHeight},
    transaction::TxId,
};

use crate::{
    keys::transparent::TransparentAddressId,
    primitives::{Locator, SyncState},
    scan::task::ScanTask,
    traits::{SyncBlocks, SyncWallet},
};

use super::{BATCH_SIZE, VERIFY_BLOCK_RANGE_SIZE};

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
    set_verification_scan_range(sync_state)?;

    // TODO: add logic to merge scan ranges
    // TODO: set chain tip range
    // TODO: set open adjacent range

    Ok(())
}

/// Create scan range between the wallet height and the chain height from the server
async fn create_scan_range(
    wallet_height: BlockHeight,
    chain_height: BlockHeight,
    sync_state: &mut SyncState,
) -> Result<(), ()> {
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

    Ok(())
}

fn reset_scan_ranges(sync_state: &mut SyncState) -> Result<(), ()> {
    let scan_ranges = sync_state.scan_ranges_mut();
    let stale_verify_scan_ranges = scan_ranges
        .iter()
        .filter(|range| range.priority() == ScanPriority::Verify)
        .cloned()
        .collect::<Vec<_>>();
    let previously_scanning_scan_ranges = scan_ranges
        .iter()
        .filter(|range| range.priority() == ScanPriority::Ignored)
        .cloned()
        .collect::<Vec<_>>();
    for scan_range in stale_verify_scan_ranges {
        set_scan_priority(
            sync_state,
            scan_range.block_range(),
            ScanPriority::OpenAdjacent,
        )
        .unwrap();
    }
    // a range that was previously scanning when sync was last interupted should be set to `ChainTip` which is the
    // highest priority that is not `Verify`.
    for scan_range in previously_scanning_scan_ranges {
        set_scan_priority(sync_state, scan_range.block_range(), ScanPriority::ChainTip).unwrap();
    }

    Ok(())
}

fn set_verification_scan_range(sync_state: &mut SyncState) -> Result<(), ()> {
    let scan_ranges = sync_state.scan_ranges_mut();
    // FIXME: this is incorrect, we should verify the start of the newly created range
    if let Some((index, lowest_unscanned_range)) =
        scan_ranges.iter().enumerate().find(|(_, scan_range)| {
            scan_range.priority() != ScanPriority::Ignored
                && scan_range.priority() != ScanPriority::Scanned
        })
    {
        let block_range_to_verify = Range {
            start: lowest_unscanned_range.block_range().start,
            end: lowest_unscanned_range
                .block_range()
                .start
                .add(VERIFY_BLOCK_RANGE_SIZE),
        };
        let split_ranges = split_out_scan_range(
            lowest_unscanned_range,
            block_range_to_verify,
            ScanPriority::Verify,
        );

        sync_state
            .scan_ranges_mut()
            .splice(index..=index, split_ranges);
    }

    Ok(())
}

fn set_found_note_scan_range(sync_state: &mut SyncState) -> Result<(), ()> {
    let locator_heights: Vec<BlockHeight> = sync_state
        .locators()
        .iter()
        .map(|(block_height, _)| *block_height)
        .collect();
    locator_heights.into_iter().for_each(|block_height| {
        update_scan_priority(sync_state, block_height, ScanPriority::FoundNote);
    });

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

/// Splits out a scan range surrounding a given `block_height` with the specified `scan_priority`
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

/// Selects and prepares the next scan range for scanning.
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

    let batch_block_range = Range {
        start: selected_scan_range.block_range().start,
        end: selected_scan_range.block_range().start + BATCH_SIZE,
    };
    let split_ranges = split_out_scan_range(
        selected_scan_range,
        batch_block_range,
        ScanPriority::Ignored,
    );

    let trimmed_block_range = split_ranges
        .first()
        .expect("vec should always be non-empty")
        .block_range()
        .clone();

    scan_ranges.splice(index..=index, split_ranges);

    // TODO: when this library has its own version of ScanRange this can be simpified and more readable
    Some(ScanRange::from_parts(
        trimmed_block_range,
        highest_priority_scan_range.priority(),
    ))
}

/// Splits out the highest VERIFY_BLOCK_RANGE_SIZE blocks from the scan range containing the given `block height`
/// and sets it's priority to `Verify`.
/// Returns a clone of the scan range to be verified.
///
/// Panics if the scan range containing the given block height is not of priority `Scanned`
pub(super) fn verify_scan_range_tip(
    sync_state: &mut SyncState,
    block_height: BlockHeight,
) -> ScanRange {
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
        start: scan_range.block_range().end - VERIFY_BLOCK_RANGE_SIZE,
        end: scan_range.block_range().end,
    };
    let split_ranges =
        split_out_scan_range(scan_range, block_range_to_verify, ScanPriority::Verify);

    let scan_range_to_verify = split_ranges
        .last()
        .expect("vec should always be non-empty")
        .clone();

    sync_state
        .scan_ranges_mut()
        .splice(index..=index, split_ranges);

    scan_range_to_verify
}

pub(crate) fn create_scan_task<W>(wallet: &mut W) -> Result<Option<ScanTask>, ()>
where
    W: SyncWallet + SyncBlocks,
{
    if let Some(scan_range) = select_scan_range(wallet.get_sync_state_mut().unwrap()) {
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
