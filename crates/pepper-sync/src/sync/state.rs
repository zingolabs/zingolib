//! Module for reading and updating the fields of [`crate::wallet::SyncState`] which tracks the wallet's state of sync.

use std::{
    cmp,
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Range,
};

use tokio::sync::mpsc;

use zcash_protocol::{
    ShieldedPool,
    consensus::{self, BlockHeight},
};

use crate::engine::{InitialSyncStateOps, SyncStateOps};
use crate::{
    client::{self, FetchRequest},
    error::{ServerError, SyncError},
    keys::transparent::TransparentAddressId,
    scan::task::{ScanTask, TaskId},
    sync::ScanRange,
    wallet::{
        InitialSyncState, SyncState, TreeBounds, WalletBlock,
        traits::{SyncBlocks, SyncNullifiers, SyncWallet},
        wallet_block_from_compact_block,
    },
};

use super::{ScanPriority, held_at, overlaps};

pub(crate) use zingolib_common::engine::VerifyEnd;

use zingolib_netutils::lightwallet_protocol::SubtreeRoot;

/// Prioritize scan ranges for scanning.
pub(super) fn prioritize_scan_ranges<W>(
    consensus_parameters: &impl consensus::Parameters,
    chain_height: BlockHeight,
    wallet: &mut W,
) -> Result<(), W::Error>
where
    W: SyncWallet + SyncBlocks,
{
    let sync_state = wallet.get_sync_state_mut()?;
    sync_state.drop_external_scan_targets_outside_the_chain(chain_height);
    let scan_targets = sync_state.scan_targets().clone();
    sync_state.set_found_note_scan_ranges(consensus_parameters, scan_targets.into_iter());
    sync_state.set_chain_tip_scan_range(consensus_parameters, chain_height);
    sync_state.merge_scan_ranges(ScanPriority::ChainTip);
    wallet.set_save_flag()?;

    Ok(())
}

/// Where the scan results of one load stand once they return to the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScanResultsStanding {
    /// A wallet range of the task's in-flight priority still holds the whole load, and no later task holds a block of it.
    Current,
    /// The load's blocks are stale for `cause`, and `reset` names the blocks the wallet still holds for the task to scan again.
    Stale {
        cause: StaleCause,
        reset: Vec<Range<BlockHeight>>,
    },
}

/// Why the scan results of a load are stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StaleCause {
    /// A task selected after the load's task holds a block of the load.
    Superseded,
    /// The chain height dropped below a block of the load.
    Truncated,
    /// A re-org moved the load's blocks to another priority.
    Reprioritised,
}

/// Classifies the scan results of a `load` of `task` against the wallet's `scan_ranges` and the ranges of the tasks selected after it.
pub(super) fn scan_results_standing(
    scan_ranges: &[ScanRange],
    later_tasks: &[Range<BlockHeight>],
    task: &ScanRange,
    load: &Range<BlockHeight>,
) -> ScanResultsStanding {
    let in_flight = task.priority().in_flight();
    let superseded = later_tasks.iter().any(|later| overlaps(later, load));
    let held_whole = held_at(scan_ranges, in_flight).any(|held| held.encloses(load));
    if held_whole && !superseded {
        return ScanResultsStanding::Current;
    }

    let reset = held_at(scan_ranges, in_flight)
        .filter(|held| held.overlaps(load))
        .map(|held| intersection(held.block_range(), load))
        .flat_map(|held| subtract(held, later_tasks))
        .collect();
    let chain_tip = scan_ranges
        .iter()
        .map(|scan_range| scan_range.block_range().end)
        .max();
    let cause = if superseded {
        StaleCause::Superseded
    } else if chain_tip.is_some_and(|tip| load.end > tip) {
        StaleCause::Truncated
    } else {
        StaleCause::Reprioritised
    };

    ScanResultsStanding::Stale { cause, reset }
}

/// Returns the blocks the two ranges share.
fn intersection(first: &Range<BlockHeight>, second: &Range<BlockHeight>) -> Range<BlockHeight> {
    first.start.max(second.start)..first.end.min(second.end)
}

/// Returns the parts of `range` that lie in none of `cuts`.
fn subtract(range: Range<BlockHeight>, cuts: &[Range<BlockHeight>]) -> Vec<Range<BlockHeight>> {
    cuts.iter().fold(vec![range], |remaining, cut| {
        remaining
            .into_iter()
            .flat_map(|part| {
                [
                    part.start..cut.start.min(part.end),
                    cut.end.max(part.start)..part.end,
                ]
            })
            .filter(|part| !part.is_empty())
            .collect()
    })
}

/// Creates a scan task to be sent to a [`crate::scan::task::ScanWorker`] for scanning.
pub(crate) fn create_scan_task<W>(
    consensus_parameters: &impl consensus::Parameters,
    wallet: &mut W,
    nullifier_map_limit_exceeded: bool,
    transparent_gap_addresses: HashMap<String, TransparentAddressId>,
    task_id: TaskId,
) -> Result<Option<ScanTask>, W::Error>
where
    W: SyncWallet + SyncBlocks + SyncNullifiers,
{
    if let Some(selected_range) = wallet
        .get_sync_state_mut()?
        .select_scan_range(consensus_parameters, nullifier_map_limit_exceeded)
    {
        let transparent_scan_floor = wallet
            .get_sync_state()?
            .transparent_scan_floor()
            .expect("transparent scan floor should be set before scanning");
        let highest_scanned_height = wallet
            .get_sync_state()?
            .highest_scanned_height()
            .expect("the scan ranges hold the selected range");
        let chain_height = wallet
            .get_sync_state()?
            .last_known_chain_height()
            .expect("the chain height is known before a range is selected");
        if selected_range.priority() == ScanPriority::ScannedWithoutMapping {
            // all continuity checks and scanning is already complete, the scan worker will only re-fetch the nullifiers
            // for final spend detection.
            Ok(Some(ScanTask::from_parts(
                task_id,
                selected_range,
                None,
                None,
                BTreeSet::new(),
                HashMap::new(),
                HashMap::new(),
                transparent_scan_floor,
                chain_height,
                highest_scanned_height,
            )))
        } else {
            // in continuous sync there is a case where the range directly below the newly mined block (chain tip) is
            // currently being scanned.
            // sync must be postponed until this range has completed scanning to then reverify in case of re-org.
            if selected_range.priority() == ScanPriority::Verify
                && wallet.get_sync_state()?.scan_ranges().iter().any(|range| {
                    range
                        .block_range()
                        .contains(&(selected_range.block_range().start - 1))
                        && range.priority() == ScanPriority::Scanning
                })
            {
                // reset from scanning priority to verify until chain tip is scanned.
                wallet
                    .get_sync_state_mut()?
                    .set_scan_priority(selected_range.block_range(), ScanPriority::Verify);

                return Ok(None);
            }

            let start_seam_block = wallet
                .get_wallet_block(selected_range.block_range().start - 1)
                .ok();
            let end_seam_block = wallet
                .get_wallet_block(selected_range.block_range().end)
                .ok();

            let scan_targets = wallet
                .get_sync_state()?
                .find_scan_targets(selected_range.block_range());

            // convert transparent addreses to hash map with address as the key for efficient scanning.
            let transparent_inuse_addresses: HashMap<String, TransparentAddressId> = wallet
                .get_transparent_addresses()?
                .iter()
                .map(|(id, address)| (address.clone(), *id))
                .collect();

            Ok(Some(ScanTask::from_parts(
                task_id,
                selected_range,
                start_seam_block,
                end_seam_block,
                scan_targets,
                transparent_inuse_addresses,
                transparent_gap_addresses,
                transparent_scan_floor,
                chain_height,
                highest_scanned_height,
            )))
        }
    } else {
        Ok(None)
    }
}

/// The wallet facts the session's tree bounds are derived from, read under one wallet lock so the server requests
/// that complete them are made with the lock released.
#[derive(Debug, Clone, Copy)]
pub(super) struct TreeBoundsSource {
    birthday: BlockHeight,
    chain_height: BlockHeight,
    /// The initial tree sizes of the wallet's birthday block, where it holds one.
    birthday_initial_tree_sizes: Option<(u32, u32, u32)>,
    /// The final tree sizes of the wallet's block at the chain height, where it holds one.
    chain_tip_final_tree_sizes: Option<(u32, u32, u32)>,
}

impl TreeBoundsSource {
    /// Reads the source from `wallet` for a session whose chain tip is `chain_height`.
    pub(super) fn read<W>(wallet: &W, chain_height: BlockHeight) -> Result<Self, W::Error>
    where
        W: SyncWallet + SyncBlocks,
    {
        let birthday = wallet
            .get_sync_state()?
            .wallet_birthday()
            .expect("scan ranges must be non-empty");
        Ok(Self {
            birthday,
            chain_height,
            birthday_initial_tree_sizes: wallet.get_wallet_block(birthday).ok().map(|block| {
                (
                    block.tree_bounds().sapling_initial_tree_size,
                    block.tree_bounds().orchard_initial_tree_size,
                    block.tree_bounds().ironwood_initial_tree_size,
                )
            }),
            chain_tip_final_tree_sizes: wallet.get_wallet_block(chain_height).ok().map(|block| {
                (
                    block.tree_bounds().sapling_final_tree_size,
                    block.tree_bounds().orchard_final_tree_size,
                    block.tree_bounds().ironwood_final_tree_size,
                )
            }),
        })
    }
}

/// Fetches the tree bounds of a session from `source`: the initial tree sizes of the birthday and the final tree
/// sizes of the chain tip, each from the server where the wallet holds no block for it.
pub(super) async fn fetch_tree_bounds(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    source: TreeBoundsSource,
) -> Result<TreeBounds, ServerError> {
    let (
        birthday_sapling_initial_tree_size,
        birthday_orchard_initial_tree_size,
        birthday_ironwood_initial_tree_size,
    ) = match source.birthday_initial_tree_sizes {
        Some(tree_sizes) => tree_sizes,
        None => {
            final_tree_sizes(
                consensus_parameters,
                fetch_request_sender.clone(),
                source.birthday - 1,
            )
            .await?
        }
    };
    let (
        chain_tip_sapling_final_tree_size,
        chain_tip_orchard_final_tree_size,
        chain_tip_ironwood_final_tree_size,
    ) = match source.chain_tip_final_tree_sizes {
        Some(tree_sizes) => tree_sizes,
        None => {
            final_tree_sizes(
                consensus_parameters,
                fetch_request_sender,
                source.chain_height,
            )
            .await?
        }
    };

    Ok(TreeBounds {
        sapling_initial_tree_size: birthday_sapling_initial_tree_size,
        sapling_final_tree_size: chain_tip_sapling_final_tree_size,
        orchard_initial_tree_size: birthday_orchard_initial_tree_size,
        orchard_final_tree_size: chain_tip_orchard_final_tree_size,
        ironwood_initial_tree_size: birthday_ironwood_initial_tree_size,
        ironwood_final_tree_size: chain_tip_ironwood_final_tree_size,
    })
}

/// Sets the `initial_sync_state` field at the start of the sync session
pub(super) fn set_initial_state<W>(
    wallet: &mut W,
    chain_height: BlockHeight,
    tree_bounds: TreeBounds,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks,
{
    let sync_state = wallet.get_sync_state().map_err(SyncError::WalletError)?;
    let fully_scanned_height = sync_state
        .fully_scanned_height()
        .expect("scan ranges must be non-empty");
    let previously_scanned_blocks = sync_state.calculate_scanned_blocks();
    let (
        previously_scanned_sapling_outputs,
        previously_scanned_orchard_outputs,
        previously_scanned_ironwood_outputs,
    ) = calculate_scanned_outputs(wallet).map_err(SyncError::WalletError)?;

    wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?
        .set_initial_sync_state(InitialSyncState::from_parts(
            if chain_height > fully_scanned_height {
                fully_scanned_height + 1
            } else {
                chain_height
            },
            TreeBounds {
                sapling_initial_tree_size: 0,
                sapling_final_tree_size: 0,
                orchard_initial_tree_size: 0,
                orchard_final_tree_size: 0,
                ironwood_initial_tree_size: 0,
                ironwood_final_tree_size: 0,
            },
            previously_scanned_blocks,
            previously_scanned_sapling_outputs,
            previously_scanned_orchard_outputs,
            previously_scanned_ironwood_outputs,
        ));

    update_wallet_tree_bounds(wallet, tree_bounds)
}

pub(super) fn update_wallet_tree_bounds<W>(
    wallet: &mut W,
    tree_bounds: TreeBounds,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks,
{
    wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?
        .set_wallet_tree_bounds(tree_bounds);

    wallet.set_save_flag().map_err(SyncError::WalletError)?;

    Ok(())
}

pub(super) fn calculate_scanned_outputs<W>(wallet: &W) -> Result<(u32, u32, u32), W::Error>
where
    W: SyncWallet + SyncBlocks,
{
    Ok(wallet
        .get_sync_state()?
        .scan_ranges()
        .iter()
        .filter(|scan_range| scan_range.priority().is_scanned())
        .map(|scanned_range| scanned_range_tree_bounds(wallet, scanned_range.block_range().clone()))
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .fold((0, 0, 0), |acc, tree_bounds| {
            (
                acc.0
                    + (tree_bounds.sapling_final_tree_size - tree_bounds.sapling_initial_tree_size),
                acc.1
                    + (tree_bounds.orchard_final_tree_size - tree_bounds.orchard_initial_tree_size),
                acc.2
                    + (tree_bounds.ironwood_final_tree_size
                        - tree_bounds.ironwood_initial_tree_size),
            )
        }))
}

/// Gets `block_height` final tree sizes from frontiers fetched from server.
async fn final_tree_sizes(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    block_height: BlockHeight,
) -> Result<(u32, u32, u32), ServerError> {
    // TODO: move this whole block into `client::get_frontiers`
    let sapling_activation_height =
        crate::wallet::PoolActivation::of(consensus_parameters, ShieldedPool::Sapling)
            .expect("should have some sapling activation height")
            .height();

    match block_height.cmp(&(sapling_activation_height - 1)) {
        cmp::Ordering::Greater => {
            let frontiers = client::get_frontiers(
                fetch_request_sender.clone(),
                consensus_parameters,
                block_height,
            )
            .await?;
            Ok((
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
                frontiers
                    .final_ironwood_tree()
                    .tree_size()
                    .try_into()
                    .expect("should not be more than 2^32 note commitments in the tree!"),
            ))
        }
        cmp::Ordering::Equal => Ok((0, 0, 0)),
        cmp::Ordering::Less => panic!("pre-sapling not supported!"),
    }
}

/// Gets the initial and final tree sizes of a `scanned_range`.
///
/// Panics if `scanned_range` wallet block bounds are not found in the wallet.
fn scanned_range_tree_bounds<W>(
    wallet: &W,
    scanned_range: Range<BlockHeight>,
) -> Result<TreeBounds, W::Error>
where
    W: SyncBlocks,
{
    let start_block = wallet.get_wallet_block(scanned_range.start)?;
    let end_block = wallet.get_wallet_block(scanned_range.end - 1)?;

    Ok(TreeBounds {
        sapling_initial_tree_size: start_block.tree_bounds().sapling_initial_tree_size,
        sapling_final_tree_size: end_block.tree_bounds().sapling_final_tree_size,
        orchard_initial_tree_size: start_block.tree_bounds().orchard_initial_tree_size,
        orchard_final_tree_size: end_block.tree_bounds().orchard_final_tree_size,
        ironwood_initial_tree_size: start_block.tree_bounds().ironwood_initial_tree_size,
        ironwood_final_tree_size: end_block.tree_bounds().ironwood_final_tree_size,
    })
}

/// Reopens for scanning every scanned range at or above `from_height`,
/// splitting the range that straddles it.
///
/// `Historic` is the priority a range carries when it has never been
/// scanned, which is what a range whose recorded history cannot be trusted
/// has effectively become. Historic ranges are selected lowest first, so the
/// rescan works upward and each range takes its tree-size baseline from a
/// range below that the same rescan has already corrected.
///
/// Ranges being scanned right now are left alone, since their results are
/// already in flight against the bounds they were dispatched with.
///
/// `missing_block_bound` is the block [`fetch_reopened_block_bound`]
/// fetched, which bounds the scanned range below `from_height`.
pub(super) fn reopen_scan_ranges_from<W>(
    wallet: &mut W,
    from_height: BlockHeight,
    missing_block_bound: Option<WalletBlock>,
) -> Result<(), W::Error>
where
    W: SyncWallet + SyncBlocks,
{
    wallet
        .get_sync_state_mut()?
        .reopen_scan_ranges_inner(from_height);
    if let Some(block_bound) = missing_block_bound {
        wallet.append_wallet_blocks(BTreeMap::from([(block_bound.block_height(), block_bound)]))?;
    }

    Ok(())
}

/// Fetches the block below `from_height` where the wallet is missing it,
/// for [`reopen_scan_ranges_from`] to add to the wallet. The wallet is only
/// read, so a failed server request leaves it as it is.
pub(super) async fn fetch_reopened_block_bound<W>(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    wallet: &W,
    from_height: BlockHeight,
) -> Result<Option<WalletBlock>, SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks,
{
    let upper_block_bound_height = from_height - 1;
    if wallet.get_wallet_block(upper_block_bound_height).is_ok() {
        return Ok(None);
    }

    Ok(Some(
        wallet_block_from_compact_block(
            consensus_parameters,
            fetch_request_sender.clone(),
            &client::get_compact_block(fetch_request_sender.clone(), upper_block_bound_height)
                .await?,
        )
        .await?,
    ))
}

/// - Adds the shard ranges of `subtree_roots` to `sync_state` through [`SyncState::add_shard_ranges`].
pub(super) fn add_shard_ranges(
    consensus_parameters: &impl consensus::Parameters,
    shielded_protocol: ShieldedPool,
    sync_state: &mut SyncState,
    start_index: u32,
    subtree_roots: &[SubtreeRoot],
) {
    let subtree_completing_heights = subtree_roots
        .iter()
        .map(|subtree_root| {
            BlockHeight::from_u32(
                subtree_root
                    .completing_block_height
                    .try_into()
                    .expect("overflow should never occur"),
            )
        })
        .collect::<Vec<_>>();
    sync_state.add_shard_ranges(
        consensus_parameters,
        shielded_protocol,
        start_index,
        &subtree_completing_heights,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where a load's scan results stand once they return, decided from the wallet's scan ranges, the tasks selected
    /// after the load's task, the load's task, and the load's own blocks.
    mod scan_results_standing {
        use super::*;

        fn blocks(start: u32, end: u32) -> Range<BlockHeight> {
            BlockHeight::from_u32(start)..BlockHeight::from_u32(end)
        }

        /// The wallet after a `Verify` range was carved out of a task's `Scanning` range and selected as a later task.
        fn wallet_with_a_later_task() -> Vec<ScanRange> {
            vec![
                ScanRange::from_parts(blocks(1, 21), ScanPriority::Scanned),
                ScanRange::from_parts(blocks(21, 31), ScanPriority::Scanning),
                ScanRange::from_parts(blocks(31, 41), ScanPriority::Scanning),
            ]
        }

        #[test]
        fn a_load_of_an_earlier_task_is_stale_where_a_later_task_was_selected_over_it() {
            let earlier_task = ScanRange::from_parts(blocks(21, 41), ScanPriority::ChainTip);

            assert_eq!(
                scan_results_standing(
                    &wallet_with_a_later_task(),
                    &[blocks(31, 41)],
                    &earlier_task,
                    &blocks(31, 41),
                ),
                ScanResultsStanding::Stale {
                    cause: StaleCause::Superseded,
                    reset: Vec::new(),
                },
            );
        }

        #[test]
        fn a_load_of_the_later_task_is_current() {
            let later_task = ScanRange::from_parts(blocks(31, 41), ScanPriority::Verify);

            assert_eq!(
                scan_results_standing(
                    &wallet_with_a_later_task(),
                    &[],
                    &later_task,
                    &blocks(31, 41)
                ),
                ScanResultsStanding::Current,
            );
        }

        #[test]
        fn a_stale_load_resets_only_the_blocks_no_later_task_holds() {
            let earlier_task = ScanRange::from_parts(blocks(21, 41), ScanPriority::ChainTip);

            assert_eq!(
                scan_results_standing(
                    &wallet_with_a_later_task(),
                    &[blocks(31, 41)],
                    &earlier_task,
                    &blocks(28, 41),
                ),
                ScanResultsStanding::Stale {
                    cause: StaleCause::Superseded,
                    reset: vec![blocks(28, 31)],
                },
            );
        }

        #[test]
        fn a_load_past_the_truncated_tip_is_stale_and_resets_the_blocks_the_wallet_still_holds() {
            let wallet = vec![
                ScanRange::from_parts(blocks(1, 21), ScanPriority::Scanned),
                ScanRange::from_parts(blocks(21, 40), ScanPriority::Scanning),
            ];
            let task = ScanRange::from_parts(blocks(21, 41), ScanPriority::ChainTip);

            assert_eq!(
                scan_results_standing(&wallet, &[], &task, &blocks(31, 41)),
                ScanResultsStanding::Stale {
                    cause: StaleCause::Truncated,
                    reset: vec![blocks(31, 40)],
                },
            );
            assert_eq!(
                scan_results_standing(&wallet, &[], &task, &blocks(21, 31)),
                ScanResultsStanding::Current,
            );
        }

        #[test]
        fn a_load_below_a_chain_height_that_dropped_past_it_is_stale_with_nothing_to_reset() {
            let wallet = vec![ScanRange::from_parts(blocks(1, 21), ScanPriority::Scanned)];
            let task = ScanRange::from_parts(blocks(21, 22), ScanPriority::Verify);

            assert_eq!(
                scan_results_standing(&wallet, &[], &task, &blocks(21, 22)),
                ScanResultsStanding::Stale {
                    cause: StaleCause::Truncated,
                    reset: Vec::new(),
                },
            );
        }

        #[test]
        fn a_stale_load_resets_blocks_of_its_own_in_flight_priority_alone() {
            let wallet = vec![
                ScanRange::from_parts(blocks(1, 11), ScanPriority::RefetchingNullifiers),
                ScanRange::from_parts(blocks(11, 22), ScanPriority::Scanning),
            ];
            let refetch_task =
                ScanRange::from_parts(blocks(1, 21), ScanPriority::ScannedWithoutMapping);

            assert_eq!(
                scan_results_standing(&wallet, &[], &refetch_task, &blocks(1, 21)),
                ScanResultsStanding::Stale {
                    cause: StaleCause::Reprioritised,
                    reset: vec![blocks(1, 11)],
                },
            );

            let mut sync_state = SyncState::new_for_test(wallet);
            sync_state.reset_stale_load(refetch_task.priority(), &[blocks(1, 11)]);
            assert_eq!(
                sync_state.scan_ranges(),
                vec![
                    ScanRange::from_parts(blocks(1, 11), ScanPriority::ScannedWithoutMapping),
                    ScanRange::from_parts(blocks(11, 22), ScanPriority::Scanning),
                ],
            );
        }

        #[test]
        fn a_load_of_a_reprioritised_range_is_stale_with_nothing_to_reset() {
            let wallet = vec![ScanRange::from_parts(blocks(21, 41), ScanPriority::Verify)];
            let task = ScanRange::from_parts(blocks(21, 41), ScanPriority::ChainTip);

            assert_eq!(
                scan_results_standing(&wallet, &[], &task, &blocks(21, 41)),
                ScanResultsStanding::Stale {
                    cause: StaleCause::Reprioritised,
                    reset: Vec::new(),
                },
            );
        }
    }
}
