//! Module for reading and updating the fields of [`crate::primitives::SyncState`] which tracks the wallet's state of sync.

use std::{
    cmp,
    collections::{BTreeSet, HashMap},
    ops::Range,
};

use tokio::sync::mpsc;

use zcash_client_backend::{
    data_api::scanning::{ScanPriority, ScanRange},
    proto::service::SubtreeRoot,
    ShieldedProtocol,
};
use zcash_primitives::{
    consensus::{self, BlockHeight, NetworkUpgrade},
    transaction::TxId,
};

use crate::{
    client::{self, FetchRequest},
    keys::transparent::TransparentAddressId,
    primitives::{Locator, SyncState, TreeBoundaries, WalletTransaction},
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
    let wallet_height = if let Some(height) = wallet.get_sync_state().unwrap().wallet_height() {
        height
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
fn find_locators(sync_state: &SyncState, block_range: &Range<BlockHeight>) -> BTreeSet<Locator> {
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
    let locators = sync_state.locators().clone();
    set_found_note_scan_ranges(sync_state, ShieldedProtocol::Orchard, locators.into_iter())?;
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

    let split_ranges = split_out_scan_range(
        scan_range.clone(),
        block_range_to_verify,
        ScanPriority::Verify,
    );

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

/// Punches in the `shielded_protocol` shard block ranges surrounding each locator with `ScanPriority::FoundNote`.
pub(super) fn set_found_note_scan_ranges<L: Iterator<Item = Locator>>(
    sync_state: &mut SyncState,
    shielded_protocol: ShieldedProtocol,
    locators: L,
) -> Result<(), ()> {
    for locator in locators {
        set_found_note_scan_range(sync_state, shielded_protocol, locator)?;
    }

    Ok(())
}

/// Punches in the `shielded_protocol` shard block range surrounding the `locator` with `ScanPriority::FoundNote`.
pub(super) fn set_found_note_scan_range(
    sync_state: &mut SyncState,
    shielded_protocol: ShieldedProtocol,
    locator: Locator,
) -> Result<(), ()> {
    let block_height = locator.0;
    let block_range = determine_block_range(sync_state, block_height, shielded_protocol);
    punch_scan_priority(sync_state, block_range, ScanPriority::FoundNote).unwrap();

    Ok(())
}

/// Punches in the chain tip block range with `ScanPriority::ChainTip`.
///
/// Determines the chain tip block range by finding the lowest start height of the latest incomplete shard for each
/// shielded protocol.
fn set_chain_tip_scan_range(
    sync_state: &mut SyncState,
    chain_height: BlockHeight,
) -> Result<(), ()> {
    let sapling_incomplete_shard =
        determine_block_range(sync_state, chain_height, ShieldedProtocol::Sapling);
    let orchard_incomplete_shard =
        determine_block_range(sync_state, chain_height, ShieldedProtocol::Orchard);

    let chain_tip = if sapling_incomplete_shard.start < orchard_incomplete_shard.start {
        sapling_incomplete_shard
    } else {
        orchard_incomplete_shard
    };

    punch_scan_priority(sync_state, chain_tip, ScanPriority::ChainTip).unwrap();

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

/// Punches in a `scan_priority` for a given `block_range`.
///
/// This function will set all scan ranges in `sync_state` with block range boundaries contained by `block_range` to
/// the given `scan_priority`.
/// If any scan ranges in `sync_state` are found to overlap with the given `block_range`, they will be split at the
/// boundary and the new scan ranges contained by `block_range` will be set to `scan_priority`.
/// Any scan ranges that fully contain the `block_range` will be split out with the given `scan_priority`.
/// Any scan ranges with `Ignored` (Scanning) or `Scanned` priority or with higher (or equal) priority than
/// `scan_priority` will be ignored.
fn punch_scan_priority(
    sync_state: &mut SyncState,
    block_range: Range<BlockHeight>,
    scan_priority: ScanPriority,
) -> Result<(), ()> {
    let mut scan_ranges_contained_by_block_range = Vec::new();
    let mut scan_ranges_for_splitting = Vec::new();

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
            scan_range.block_range().contains(&block_range.start),
        ) {
            (true, true, _) => scan_ranges_contained_by_block_range.push(scan_range.clone()),
            (true, false, _) | (false, true, _) => {
                scan_ranges_for_splitting.push((index, scan_range.clone()))
            }
            (false, false, true) => scan_ranges_for_splitting.push((index, scan_range.clone())),
            (false, false, false) => {}
        }
    }

    for scan_range in scan_ranges_contained_by_block_range {
        set_scan_priority(sync_state, scan_range.block_range(), scan_priority).unwrap();
    }

    // split out the scan ranges in reverse order to maintain the correct index for lower scan ranges
    for (index, scan_range) in scan_ranges_for_splitting.into_iter().rev() {
        let split_ranges = split_out_scan_range(scan_range, block_range.clone(), scan_priority);
        sync_state
            .scan_ranges_mut()
            .splice(index..=index, split_ranges);
    }

    Ok(())
}

/// Determines the block range which contains all the note commitments for the shard of a given `shielded_protocol` surrounding
/// the specified `block_height`.
///
/// If no shard range exists for the given `block_height`, return the range of the incomplete shard at the chain tip.
/// If `block_height` contains note commitments from multiple shards, return the block range of all of those shards combined.
fn determine_block_range(
    sync_state: &SyncState,
    block_height: BlockHeight,
    shielded_protocol: ShieldedProtocol,
) -> Range<BlockHeight> {
    let shard_ranges = match shielded_protocol {
        ShieldedProtocol::Sapling => sync_state.sapling_shard_ranges(),
        ShieldedProtocol::Orchard => sync_state.orchard_shard_ranges(),
    };

    let target_ranges = shard_ranges
        .iter()
        .filter(|range| range.contains(&block_height))
        .cloned()
        .collect::<Vec<_>>();

    if target_ranges.is_empty() {
        let start = if let Some(range) = shard_ranges.last() {
            range.end - 1
        } else {
            sync_state
                .wallet_birthday()
                .expect("scan range should not be empty")
        };
        let end = sync_state
            .wallet_height()
            .expect("scan range should not be empty")
            + 1;

        let range = Range { start, end };

        if !range.contains(&block_height) {
            panic!(
                "block height should always be within the incomplete shard at chain tip when no complete shard range is found!"
            );
        }

        range
    } else {
        Range {
            start: target_ranges
                .first()
                .expect("should not be empty in this closure")
                .start,
            end: target_ranges
                .last()
                .expect("should not be empty in this closure")
                .end,
        }
    }
}

/// Takes a `scan_range` and splits it at `block_range.start` and `block_range.end`, returning a vec of scan ranges where
/// the scan range contained within the specified `block_range` has the given `scan_priority`.
///
/// If `block_range` goes beyond the bounds of `scan_range.block_range()` no splitting will occur at the upper and/or
/// lower bound but the priority will still be updated.
///
/// Panics if no blocks in `block_range` are contained within `scan_range.block_range()`.
fn split_out_scan_range(
    scan_range: ScanRange,
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
///
/// Sets the range for scanning to `Ignored` priority in the wallet `sync_state` but returns the scan range with its initial priority.
/// Returns `None` if there are no more ranges to scan.
fn select_scan_range(sync_state: &mut SyncState) -> Option<ScanRange> {
    let scan_ranges = sync_state.scan_ranges_mut();

    // scan ranges are sorted from lowest to highest priority
    // scan ranges with the same priority are sorted in reverse block height order
    // the highest priority scan range is the last in the list, the highest priority with lowest starting block height
    let mut scan_ranges_priority_sorted: Vec<(usize, ScanRange)> =
        scan_ranges.iter().cloned().enumerate().collect();
    scan_ranges_priority_sorted
        .sort_by(|(_, a), (_, b)| b.block_range().start.cmp(&a.block_range().start));
    scan_ranges_priority_sorted.sort_by_key(|(_, scan_range)| scan_range.priority());
    let (index, highest_priority_scan_range) = scan_ranges_priority_sorted
        .pop()
        .expect("scan ranges should be non-empty after pre-scan initialisation");
    if highest_priority_scan_range.priority() == ScanPriority::Scanned
        || highest_priority_scan_range.priority() == ScanPriority::Ignored
    {
        return None;
    }

    let selected_priority = highest_priority_scan_range.priority();
    // TODO: fixed memory batching
    let batch_block_range = Range {
        start: highest_priority_scan_range.block_range().start,
        end: highest_priority_scan_range.block_range().start + BATCH_SIZE,
    };
    let split_ranges = split_out_scan_range(
        highest_priority_scan_range,
        batch_block_range,
        ScanPriority::Ignored,
    );
    let selected_block_range = split_ranges
        .first()
        .expect("split ranges should always be non-empty")
        .block_range()
        .clone();

    sync_state
        .scan_ranges_mut()
        .splice(index..=index, split_ranges);

    // TODO: when this library has its own version of ScanRange this can be simplified and more readable
    Some(ScanRange::from_parts(
        selected_block_range,
        selected_priority,
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

    let initial_sync_state = wallet
        .get_sync_state_mut()
        .unwrap()
        .initial_sync_state_mut();
    initial_sync_state.set_sync_start_height(fully_scanned_height + 1);
    initial_sync_state.set_sync_tree_boundaries(TreeBoundaries {
        sapling_initial_tree_size: sync_start_sapling_tree_size,
        sapling_final_tree_size: chain_tip_sapling_tree_size,
        orchard_initial_tree_size: sync_start_orchard_tree_size,
        orchard_final_tree_size: chain_tip_orchard_tree_size,
    });

    let (total_sapling_outputs_to_scan, total_orchard_outputs_to_scan) =
        calculate_unscanned_outputs(wallet);

    let sync_state = wallet.get_sync_state_mut().unwrap();
    let total_blocks_to_scan = sync_state
        .scan_ranges()
        .iter()
        .filter(|scan_range| scan_range.priority() != ScanPriority::Scanned)
        .map(|scan_range| scan_range.block_range())
        .fold(0, |acc, block_range| {
            acc + (block_range.end - block_range.start)
        });

    let initial_sync_state = sync_state.initial_sync_state_mut();
    initial_sync_state.set_total_blocks_to_scan(total_blocks_to_scan);
    initial_sync_state.set_total_sapling_outputs_to_scan(total_sapling_outputs_to_scan);
    initial_sync_state.set_total_orchard_outputs_to_scan(total_orchard_outputs_to_scan);
}

pub(super) fn calculate_unscanned_outputs<W>(wallet: &W) -> (u32, u32)
where
    W: SyncWallet + SyncBlocks,
{
    let sync_state = wallet.get_sync_state().unwrap();
    let sync_start_height = sync_state.initial_sync_state().sync_start_height();

    let nonlinear_scanned_block_ranges = sync_state
        .scan_ranges()
        .iter()
        .filter(|scan_range| {
            scan_range.priority() == ScanPriority::Scanned
                && scan_range.block_range().start >= sync_start_height
        })
        .map(|scan_range| scan_range.block_range().clone())
        .collect::<Vec<_>>();
    let (nonlinear_scanned_sapling_outputs, nonlinear_scanned_orchard_outputs) =
        nonlinear_scanned_block_ranges
            .iter()
            .map(|block_range| scanned_range_tree_boundaries(wallet, block_range.clone()))
            .fold((0, 0), |acc, tree_boundaries| {
                (
                    acc.0
                        + (tree_boundaries.sapling_final_tree_size
                            - tree_boundaries.sapling_initial_tree_size),
                    acc.1
                        + (tree_boundaries.orchard_final_tree_size
                            - tree_boundaries.orchard_initial_tree_size),
                )
            });

    let initial_sync_state = wallet.get_sync_state().unwrap().initial_sync_state();
    let unscanned_sapling_outputs = initial_sync_state
        .sync_tree_boundaries()
        .sapling_final_tree_size
        - initial_sync_state
            .sync_tree_boundaries()
            .sapling_initial_tree_size
        - nonlinear_scanned_sapling_outputs;
    let unscanned_orchard_outputs = initial_sync_state
        .sync_tree_boundaries()
        .orchard_final_tree_size
        - initial_sync_state
            .sync_tree_boundaries()
            .orchard_initial_tree_size
        - nonlinear_scanned_orchard_outputs;

    (unscanned_sapling_outputs, unscanned_orchard_outputs)
}

/// Gets `block_height` final tree sizes from wallet block if it exists, otherwise from frontiers fetched from server.
async fn final_tree_sizes<W>(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    wallet: &mut W,
    block_height: BlockHeight,
) -> (u32, u32)
where
    W: SyncBlocks,
{
    if let Ok(block) = wallet.get_wallet_block(block_height) {
        (
            block.tree_boundaries().sapling_final_tree_size,
            block.tree_boundaries().orchard_final_tree_size,
        )
    } else {
        // TODO: move this whole block into `client::get_frontiers`
        let sapling_activation_height = consensus_parameters
            .activation_height(NetworkUpgrade::Sapling)
            .expect("should have some sapling activation height");

        match block_height.cmp(&sapling_activation_height) {
            cmp::Ordering::Greater => {
                let frontiers = client::get_frontiers(fetch_request_sender.clone(), block_height)
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
fn scanned_range_tree_boundaries<W>(wallet: &W, scanned_range: Range<BlockHeight>) -> TreeBoundaries
where
    W: SyncBlocks,
{
    let start_block = wallet
        .get_wallet_block(scanned_range.start)
        .expect("scanned range boundary blocks should be retained in the wallet");
    let end_block = wallet
        .get_wallet_block(scanned_range.end - 1)
        .expect("scanned range boundary blocks should be retained in the wallet");

    TreeBoundaries {
        sapling_initial_tree_size: start_block.tree_boundaries().sapling_initial_tree_size,
        sapling_final_tree_size: end_block.tree_boundaries().sapling_final_tree_size,
        orchard_initial_tree_size: start_block.tree_boundaries().orchard_initial_tree_size,
        orchard_final_tree_size: end_block.tree_boundaries().orchard_final_tree_size,
    }
}

/// Creates block ranges that contain all outputs for the shards associated with `subtree_roots` and adds the to
/// `sync_state`.
///
/// The network upgrade activation height for the `shielded_protocol` is the first shard start height for the case
/// where shard ranges in `sync_state` are empty.
pub(super) fn add_shard_ranges(
    consensus_parameters: &impl consensus::Parameters,
    shielded_protocol: ShieldedProtocol,
    sync_state: &mut SyncState,
    subtree_roots: &[SubtreeRoot],
) {
    let network_upgrade_activation_height = match shielded_protocol {
        ShieldedProtocol::Sapling => consensus_parameters
            .activation_height(consensus::NetworkUpgrade::Sapling)
            .expect("activation height should exist for this network upgrade!"),
        ShieldedProtocol::Orchard => consensus_parameters
            .activation_height(consensus::NetworkUpgrade::Nu5)
            .expect("activation height should exist for this network upgrade!"),
    };

    let shard_ranges = match shielded_protocol {
        ShieldedProtocol::Sapling => sync_state.sapling_shard_ranges_mut(),
        ShieldedProtocol::Orchard => sync_state.orchard_shard_ranges_mut(),
    };

    let highest_subtree_completing_height = if let Some(shard_range) = shard_ranges.last() {
        shard_range.end - 1
    } else {
        network_upgrade_activation_height
    };

    subtree_roots
        .iter()
        .map(|subtree_root| {
            BlockHeight::from_u32(
                subtree_root
                    .completing_block_height
                    .try_into()
                    .expect("overflow should never occur"),
            )
        })
        .fold(
            highest_subtree_completing_height,
            |previous_subtree_completing_height, subtree_completing_height| {
                shard_ranges.push(Range {
                    start: previous_subtree_completing_height,
                    end: subtree_completing_height + 1,
                });

                tracing::debug!(
                    "{:?} subtree root height: {}",
                    shielded_protocol,
                    subtree_completing_height
                );

                subtree_completing_height
            },
        );
}

/// Updates the `shielded_protocol` shard range to `FoundNote` scan priority if the `wallet_transaction` contains
/// a note from the corresponding `shielded_protocol`.
pub(super) fn update_found_note_shard_priority(
    sync_state: &mut SyncState,
    shielded_protocol: ShieldedProtocol,
    wallet_transaction: &WalletTransaction,
) {
    let found_note = match shielded_protocol {
        ShieldedProtocol::Sapling => !wallet_transaction.sapling_notes().is_empty(),
        ShieldedProtocol::Orchard => !wallet_transaction.orchard_notes().is_empty(),
    };
    if found_note {
        set_found_note_scan_range(
            sync_state,
            shielded_protocol,
            (
                wallet_transaction.confirmation_status().get_height(),
                wallet_transaction.txid(),
            ),
        )
        .unwrap();
    }
}
