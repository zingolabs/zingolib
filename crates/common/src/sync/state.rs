use std::{collections::BTreeSet, ops::Range};

use zcash_protocol::{
    ShieldedPool,
    consensus::{self, BlockHeight},
};

use crate::{
    engine::{ScanTargetOps, ScanTargetOrigin, SyncStateOps, VERIFY_BLOCK_RANGE_SIZE, VerifyEnd},
    sync::{InitialSyncState, ScanPriority, ScanRange, SyncState},
    wallet::{ScanTarget, TreeBounds, WalletTransaction},
};

const NARROW_SCAN_AREA: u32 = 1_000;

impl SyncStateOps for SyncState {
    /// Returns true if all scan ranges are scanned.
    fn scan_complete(&self) -> bool {
        self.scan_ranges
            .iter()
            .all(|scan_range| scan_range.priority() == ScanPriority::Scanned)
    }

    /// - Inserts `scan_target`, merged into the target already held for the same block height and txid if any.
    fn insert_scan_target(&mut self, scan_target: ScanTarget) {
        let merged = self
            .scan_targets
            .take(&scan_target)
            .map_or(scan_target, |held| held.merge(scan_target));
        self.scan_targets.insert(merged);
    }

    fn scan_targets(&self) -> &BTreeSet<ScanTarget> {
        &self.scan_targets
    }

    fn initial_sync_state(&self) -> &InitialSyncState {
        &self.initial_sync_state
    }

    fn transparent_scan_floor(&self) -> Option<BlockHeight> {
        self.transparent_scan_floor
    }

    /// - Replaces the initial sync state of the session.
    fn set_initial_sync_state(&mut self, initial_sync_state: InitialSyncState) {
        self.initial_sync_state = initial_sync_state;
    }

    /// - Sets the transparent scan floor to `height`.
    fn set_transparent_scan_floor(&mut self, height: BlockHeight) {
        self.transparent_scan_floor = Some(height);
    }

    /// - Removes every held scan target that `keep` rejects.
    fn retain_scan_targets(&mut self, keep: impl FnMut(&ScanTarget) -> bool) {
        self.scan_targets.retain(keep);
    }

    /// - Replaces the tree bounds of the initial sync state.
    fn set_wallet_tree_bounds(&mut self, wallet_tree_bounds: TreeBounds) {
        self.initial_sync_state.wallet_tree_bounds = wallet_tree_bounds;
    }

    /// Returns the `scan_targets` for a given `block_range` from the wallet's [`crate::sync::SyncState`]
    fn find_scan_targets(&self, block_range: &Range<BlockHeight>) -> BTreeSet<ScanTarget> {
        self.scan_targets
            .range(ScanTarget::new(block_range.start, None)..ScanTarget::new(block_range.end, None))
            .copied()
            .collect()
    }

    /// - Removes every external scan target whose block height is below the wallet birthday or above `chain_height`,
    ///   which no scan range reaches.
    fn drop_external_scan_targets_outside_the_chain(&mut self, chain_height: BlockHeight) {
        let Some(birthday) = self.wallet_birthday() else {
            return;
        };
        self.scan_targets.retain(|scan_target| {
            let reachable = scan_target.origin() == ScanTargetOrigin::Internal
                || (birthday..=chain_height).contains(&scan_target.block_height());
            if !reachable {
                tracing::warn!(
                    "scan target at block {} dropped: outside the wallet's chain range {birthday}..={chain_height}",
                    scan_target.block_height()
                );
            }
            reachable
        });
    }

    /// - Removes every scan target of `sync_state` that is settled at `fully_scanned_height`.
    fn drop_settled_scan_targets(&mut self, fully_scanned_height: BlockHeight) {
        let settled = self
            .scan_targets
            .iter()
            .filter(|scan_target| self.scan_target_is_settled(scan_target, fully_scanned_height))
            .copied()
            .collect::<Vec<_>>();
        for scan_target in settled {
            self.scan_targets.remove(&scan_target);
        }
    }

    /// Merges all adjacent ranges of a given `scan_priority`.
    fn merge_scan_ranges(&mut self, scan_priority: ScanPriority) {
        'main: loop {
            let filtered_ranges = self
                .scan_ranges()
                .iter()
                .cloned()
                .enumerate()
                .filter(|(_, scan_range)| scan_range.priority() == scan_priority)
                .collect::<Vec<_>>();
            if filtered_ranges.is_empty() {
                break;
            }
            let mut peekable_ranges = filtered_ranges.iter().peekable();
            while let Some((index, range)) = peekable_ranges.next() {
                if let Some((next_index, next_range)) = peekable_ranges.peek() {
                    if range.block_range().end == next_range.block_range().start {
                        assert!(*next_index == *index + 1);
                        self.scan_ranges.splice(
                            *index..=*next_index,
                            vec![ScanRange::from_parts(
                                Range {
                                    start: range.block_range().start,
                                    end: next_range.block_range().end,
                                },
                                scan_priority,
                            )],
                        );
                        continue 'main;
                    }
                } else {
                    break 'main;
                }
            }
        }
    }

    /// Create scan range between the wallet height and the chain height from the server.
    fn create_scan_range(
        &mut self,
        last_known_chain_height: BlockHeight,
        chain_height: BlockHeight,
    ) {
        if last_known_chain_height >= chain_height {
            return;
        }

        let new_scan_range = ScanRange::from_parts(
            Range {
                start: last_known_chain_height + 1,
                end: chain_height + 1,
            },
            ScanPriority::Historic,
        );
        self.scan_ranges.push(new_scan_range);
    }

    /// Splits the range containing [`truncate_height` + 1] and removes all ranges containing block heights above
    /// `truncate_height`.
    /// If `truncate_height` is zero, the sync state will be cleared completely.
    fn truncate_scan_ranges(&mut self, truncate_height: BlockHeight) {
        if truncate_height == zcash_protocol::consensus::H0 {
            *self = SyncState::new();
        }
        let Some((index, range_to_split)) = self
            .scan_ranges()
            .iter()
            .cloned()
            .enumerate()
            .find(|(_index, range)| range.block_range().contains(&(truncate_height + 1)))
        else {
            return;
        };

        if let Some((first_segment, second_segment)) = range_to_split.split_at(truncate_height + 1)
        {
            let split_ranges = vec![first_segment, second_segment];
            self.scan_ranges.splice(index..=index, split_ranges);
        }

        let truncated_scan_ranges = self.scan_ranges[..self
            .scan_ranges()
            .partition_point(|range| range.block_range().start <= truncate_height)]
            .to_vec();
        self.scan_ranges = truncated_scan_ranges;
    }

    /// Resets scan ranges to recover from previous sync interruptions.
    ///
    /// A range that was previously scanning when sync was last interrupted is set to `FoundNote` to be prioritised for
    /// scanning.
    /// A range that was previously refetching nullifiers when sync was last interrupted is set to `ScannedWithoutMapping`
    /// so the nullifiers can be fetched again.
    fn reset_scan_ranges(&mut self) {
        let previously_scanning_scan_ranges = self
            .scan_ranges
            .iter()
            .filter(|&range| range.priority() == ScanPriority::Scanning)
            .cloned()
            .collect::<Vec<_>>();
        for scan_range in previously_scanning_scan_ranges {
            self.set_scan_priority(scan_range.block_range(), ScanPriority::FoundNote);
        }

        let previously_refetching_nullifiers_scan_ranges = self
            .scan_ranges
            .iter()
            .filter(|&range| range.priority() == ScanPriority::RefetchingNullifiers)
            .cloned()
            .collect::<Vec<_>>();
        for scan_range in previously_refetching_nullifiers_scan_ranges {
            self.set_scan_priority(
                scan_range.block_range(),
                ScanPriority::ScannedWithoutMapping,
            );
        }
    }

    /// Splits out the highest or lowest `VERIFY_BLOCK_RANGE_SIZE` blocks from the scan range containing the given `block height`
    /// and sets it's priority to `Verify`.
    /// Returns a clone of the scan range to be verified.
    fn set_verify_scan_range(
        &mut self,
        block_height: BlockHeight,
        verify_end: VerifyEnd,
    ) -> ScanRange {
        let (index, scan_range) = self
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

        self.scan_ranges.splice(index..=index, split_ranges);

        scan_range_to_verify
    }

    /// Punches in the chain tip block range with `ScanPriority::ChainTip`.
    ///
    /// Determines the chain tip block range by finding the lowest start height of the latest incomplete shard for each
    /// shielded protocol.
    fn set_chain_tip_scan_range(
        &mut self,
        consensus_parameters: &impl consensus::Parameters,
        chain_height: BlockHeight,
    ) {
        let sapling_incomplete_shard = self.determine_block_range(
            consensus_parameters,
            chain_height,
            Some(ShieldedPool::Sapling),
        );
        let orchard_incomplete_shard = self.determine_block_range(
            consensus_parameters,
            chain_height,
            Some(ShieldedPool::Orchard),
        );
        let ironwood_incomplete_shard = self.determine_block_range(
            consensus_parameters,
            chain_height,
            Some(ShieldedPool::Ironwood),
        );

        let chain_tip = [
            sapling_incomplete_shard,
            orchard_incomplete_shard,
            ironwood_incomplete_shard,
        ]
        .into_iter()
        .min_by_key(|r| r.start)
        .expect("non-empty");

        self.punch_scan_priority(chain_tip, ScanPriority::ChainTip);
    }

    /// Punches in the `shielded_protocol` shard block ranges surrounding each scan target with `ScanPriority::FoundNote`.
    ///
    fn set_found_note_scan_ranges<T: Iterator<Item = ScanTarget>>(
        &mut self,
        consensus_parameters: &impl consensus::Parameters,
        scan_targets: T,
    ) {
        for scan_target in scan_targets {
            // the narrow range around a target whose transaction holds no known note, and the whole shard of each pool
            // the transaction holds notes of, so those notes become spendable.
            let pools = scan_target.pools();
            let shielded_protocols = if pools.is_empty() {
                vec![None]
            } else {
                pools.iter().map(Some).collect()
            };
            for shielded_protocol in shielded_protocols {
                self.set_found_note_scan_range(
                    consensus_parameters,
                    shielded_protocol,
                    scan_target.block_height(),
                );
            }
        }
    }

    fn set_scanned_scan_range(
        &mut self,
        scanned_range: Range<BlockHeight>,
        nullifiers_mapped: bool,
    ) {
        let Some((index, scan_range)) = self
            .scan_ranges
            .iter()
            .enumerate()
            .find(|(_, scan_range)| scan_range.encloses(&scanned_range))
        else {
            panic!("scan range containing scanned range should exist!");
        };

        let split_ranges = split_out_scan_range(
            scan_range.clone(),
            scanned_range.clone(),
            if nullifiers_mapped {
                ScanPriority::Scanned
            } else {
                ScanPriority::ScannedWithoutMapping
            },
        );
        self.scan_ranges.splice(index..=index, split_ranges);
    }

    fn reset_refetching_nullifiers_scan_range(
        &mut self,
        invalid_refetch_range: Range<BlockHeight>,
    ) {
        let Some((index, scan_range)) = self
            .scan_ranges
            .iter()
            .enumerate()
            .find(|(_, scan_range)| scan_range.encloses(&invalid_refetch_range))
        else {
            panic!("scan range containing invalid refetch range should exist!");
        };

        let split_ranges = split_out_scan_range(
            scan_range.clone(),
            invalid_refetch_range.clone(),
            ScanPriority::ScannedWithoutMapping,
        );
        self.scan_ranges.splice(index..=index, split_ranges);
    }

    /// Sets the `reset` blocks of a stale load from `task_priority.in_flight()` back to `task_priority`.
    fn reset_stale_load(&mut self, task_priority: ScanPriority, reset: &[Range<BlockHeight>]) {
        for block_range in reset {
            self.scan_ranges = split_out_overlapping(
                std::mem::take(&mut self.scan_ranges),
                block_range,
                task_priority.in_flight(),
                task_priority,
            );
        }
    }

    /// Sets the blocks of a failed `Verify` load from `Scanning` back to `Verify` within whichever wallet range holds them.
    fn reset_verify_scan_range(&mut self, block_range: &Range<BlockHeight>) {
        self.scan_ranges = split_out_overlapping(
            std::mem::take(&mut self.scan_ranges),
            block_range,
            ScanPriority::Scanning,
            ScanPriority::Verify,
        );
    }

    /// Sets the scan range in `sync_state` with `block_range` to the given `scan_priority`.
    ///
    /// Panics if no scan range is found in `sync_state` with a block range of exactly `block_range`.
    fn set_scan_priority(&mut self, block_range: &Range<BlockHeight>, scan_priority: ScanPriority) {
        if let Some((index, range)) = self
            .scan_ranges
            .iter()
            .enumerate()
            .find(|(_, range)| range.block_range() == block_range)
        {
            self.scan_ranges[index] =
                ScanRange::from_parts(range.block_range().clone(), scan_priority);
        } else {
            panic!("scan range with block range {block_range:?} not found!")
        }
    }

    /// Selects and prepares the next scan range for scanning.
    ///
    /// Sets the range for scanning to `Scanning` priority in the wallet `sync_state` but returns the scan range with its
    /// initial priority for use in scanning and post-scan processing.
    /// If the selected range is of `ScannedWithoutMapping` priority, the range is set to `RefetchingNullifiers` in the
    /// wallet `sync_state` but returns the scan range with priority `ScannedWithoutMapping` for use in scanning and
    /// post-scan processing.
    /// Returns `None` if there are no more ranges to scan.
    ///
    /// Set `nullifier_map_limit_exceeded` to `true` if the nullifiers are not going to be mapped to the wallet's main
    /// nullifier map due to performance constraints.
    fn select_scan_range(
        &mut self,
        consensus_parameters: &impl consensus::Parameters,
        nullifier_map_limit_exceeded: bool,
    ) -> Option<ScanRange> {
        let (first_unscanned_index, first_unscanned_range) = self
            .scan_ranges
            .iter()
            .enumerate()
            .find(|(_, scan_range)| {
                scan_range.priority() != ScanPriority::Scanned
                    && scan_range.priority() != ScanPriority::RefetchingNullifiers
            })?;
        let (selected_index, selected_scan_range) =
            if first_unscanned_range.priority() == ScanPriority::ScannedWithoutMapping {
                // prioritise re-fetching the nullifiers when a range with priority `ScannedWithoutMapping` is the first
                // unscanned range.
                // the `first_unscanned_range` may have `Scanning` priority here as we must select a `ScannedWithoutMapping` range only when all ranges below are `Scanned`. if a `ScannedwithoutMapping` range is selected and completes *before* a lower range that is currently `Scanning`, the nullifiers will need to be discarded and re-fetched afterwards. so this avoids a race condition that results in a sync inefficiency.
                (first_unscanned_index, first_unscanned_range.clone())
            } else {
                // scan ranges are sorted from lowest to highest priority.
                // scan ranges with the same priority are sorted in block height order.
                // the highest priority scan range is then selected from the end of the list, the highest priority with highest
                // starting block height.
                // if the highest priority is `Historic` the range with the lowest starting block height is selected instead.
                // if nullifiers are not being mapped to the wallet's main nullifier map due to performance constraints
                // (`nullifier_map_limit_exceeded` is set `true`) then the range with the highest priority and lowest starting block
                // height is selected to allow notes to be spendable quickly on rescan, otherwise spends would not be detected as nullifiers will be temporarily discarded.
                // TODO: add this documentation of performance levels and order of scanning to pepper-sync doc comments
                // `ScannedWithoutMapping` ranges are only selected above, when they are the first unscanned range.
                let mut scan_ranges_priority_sorted: Vec<(usize, ScanRange)> = self
                    .scan_ranges
                    .iter()
                    .cloned()
                    .enumerate()
                    .filter(|(_, range)| range.priority() != ScanPriority::ScannedWithoutMapping)
                    .collect();
                if nullifier_map_limit_exceeded {
                    scan_ranges_priority_sorted
                        .sort_by_key(|(_, range)| std::cmp::Reverse(range.block_range().start));
                }
                scan_ranges_priority_sorted.sort_by_key(|(_, scan_range)| scan_range.priority());

                scan_ranges_priority_sorted
                    .last()
                    .map(|(index, highest_priority_range)| {
                        if highest_priority_range.priority() == ScanPriority::Historic {
                            if nullifier_map_limit_exceeded {
                                // in this case, scan ranges are already sorted from highest to lowest and we are selecting
                                // the last range (lowest range with historic priority)
                                (*index, highest_priority_range.clone())
                            } else {
                                // in this case, scan ranges are sorted from lowest to highest and we are selecting
                                // the lowest range with historic priority
                                scan_ranges_priority_sorted
                                    .iter()
                                    .find(|(_, range)| range.priority() == ScanPriority::Historic)
                                    .expect("range with Historic priority exists in this scope")
                                    .clone()
                            }
                        } else {
                            // select the last range in the list
                            (*index, highest_priority_range.clone())
                        }
                    })?
            };

        let selected_priority = selected_scan_range.priority();

        if selected_priority == ScanPriority::Scanned
            || selected_priority == ScanPriority::Scanning
            || selected_priority == ScanPriority::RefetchingNullifiers
        {
            return None;
        }

        // historic scan ranges can be larger than a shard block range so must be split out.
        // otherwise, just set the scan priority of selected range to `Scanning` in sync state.
        // if the selected range is of `ScannedWithoutMapping` priority, set the range to `RefetchingNullifiers` priority
        // instead of `Scanning`.
        let selected_block_range = match selected_priority {
            ScanPriority::Historic => {
                let shard_block_range = self.determine_block_range(
                    consensus_parameters,
                    selected_scan_range.block_range().start,
                    Some(ShieldedPool::Ironwood),
                );
                let split_ranges = split_out_scan_range(
                    selected_scan_range,
                    shard_block_range,
                    ScanPriority::Scanning,
                );
                let selected_block_range = split_ranges
                    .first()
                    .expect("split ranges should always be non-empty")
                    .block_range()
                    .clone();
                self.scan_ranges
                    .splice(selected_index..=selected_index, split_ranges);

                selected_block_range
            }
            _ => {
                let selected_scan_range = self
                    .scan_ranges
                    .get_mut(selected_index)
                    .expect("scan range should exist due to previous logic");

                *selected_scan_range = ScanRange::from_parts(
                    selected_scan_range.block_range().clone(),
                    selected_priority.in_flight(),
                );

                selected_scan_range.block_range().clone()
            }
        };

        Some(ScanRange::from_parts(
            selected_block_range,
            selected_priority,
        ))
    }

    /// Lowers the transparent scan floor to `height` if `height` is below the current floor.
    fn lower_transparent_scan_floor(&mut self, height: BlockHeight) {
        if let Some(floor) = self.transparent_scan_floor.as_mut() {
            *floor = (*floor).min(height);
        }
    }

    fn calculate_scanned_blocks(&self) -> u32 {
        self.scan_ranges()
            .iter()
            .filter(|scan_range| scan_range.priority().is_scanned())
            .map(super::ScanRange::block_range)
            .fold(0, |acc, block_range| {
                acc + (block_range.end - block_range.start)
            })
    }

    /// Removes all shard ranges of `shielded_protocol` so they can be rebuilt from the pool's subtree roots through
    /// [`Self::add_shard_ranges`].
    fn clear_shard_ranges(&mut self, shielded_protocol: ShieldedPool) {
        match shielded_protocol {
            ShieldedPool::Sapling => self.sapling_shard_ranges.clear(),
            ShieldedPool::Orchard => self.orchard_shard_ranges.clear(),
            ShieldedPool::Ironwood => self.ironwood_shard_ranges.clear(),
        }
    }

    fn reopen_scan_ranges_inner(&mut self, from_height: BlockHeight) {
        if let Some((index, range_to_split)) = self
            .scan_ranges()
            .iter()
            .cloned()
            .enumerate()
            .find(|(_index, range)| range.block_range().contains(&from_height))
            && let Some((below, above)) = range_to_split.split_at(from_height)
        {
            self.scan_ranges.splice(index..=index, vec![below, above]);
        }

        for scan_range in &mut self.scan_ranges {
            if scan_range.block_range().start >= from_height && scan_range.priority().is_scanned() {
                *scan_range =
                    ScanRange::from_parts(scan_range.block_range().clone(), ScanPriority::Historic);
            }
        }
    }

    /// - Truncates the `shielded_protocol` shard ranges of `sync_state` to `start_index` and appends one range per
    ///   subtree completing height, unless `subtree_completing_heights` is empty.
    fn add_shard_ranges(
        &mut self,
        consensus_parameters: &impl consensus::Parameters,
        shielded_protocol: ShieldedPool,
        start_index: u32,
        subtree_completing_heights: &[BlockHeight],
    ) {
        // A server that serves no root from `start_index` is behind, or a re-org returned the newest recorded shard
        // to incomplete. Neither is told from the other here, so the recorded ranges are kept.
        if subtree_completing_heights.is_empty() {
            return;
        }

        let network_upgrade_activation_height =
            crate::wallet::PoolActivation::of(consensus_parameters, shielded_protocol)
                .expect("activation height should exist for this network upgrade!")
                .height();

        let shard_ranges: &mut Vec<Range<BlockHeight>> = match shielded_protocol {
            ShieldedPool::Sapling => self.sapling_shard_ranges.as_mut(),
            ShieldedPool::Orchard => self.orchard_shard_ranges.as_mut(),
            ShieldedPool::Ironwood => self.ironwood_shard_ranges.as_mut(),
        };

        // The fetch starts at the newest recorded root at the latest (see `crate::sync::subtree_root_fetch_index`), so
        // the ranges from `start_index` are rebuilt from the roots the server holds now. Range `i` then belongs to
        // subtree root `i`, and a range whose completing block a re-org moved is corrected.
        shard_ranges.truncate(start_index as usize);

        let highest_subtree_completing_height = if let Some(shard_range) = shard_ranges.last() {
            shard_range.end - 1
        } else {
            network_upgrade_activation_height
        };

        subtree_completing_heights.iter().copied().fold(
                highest_subtree_completing_height,
                |previous_subtree_completing_height, subtree_completing_height| {
                    if subtree_completing_height > previous_subtree_completing_height {
                        shard_ranges.push(Range {
                            start: previous_subtree_completing_height,
                            end: subtree_completing_height + 1,
                        });

                        subtree_completing_height
                    } else {
                        // A block holds far fewer note commitments than a shard, so no block completes two shards.
                        // A root completing at or below the recorded height contradicts the ranges kept below
                        // `start_index`, which only a re-org reaching below the refetched root can cause. The root
                        // is skipped so the ranges stay contiguous.
                        tracing::warn!(
                            "{:?} subtree root completing at block {} contradicts the recorded shard ranges, whose \
                             newest range completes at block {}",
                            shielded_protocol,
                            subtree_completing_height,
                            previous_subtree_completing_height
                        );

                        previous_subtree_completing_height
                    }
                },
            );

        debug_assert!(
            shard_ranges_are_contiguous(shard_ranges),
            "{shielded_protocol:?} shard ranges must stay contiguous: {shard_ranges:?}"
        );
    }

    /// - Sets the `shielded_protocol` shard range around `wallet_transaction` to `FoundNote` scan priority if the
    ///   transaction holds a note of that pool.
    /// - Records the pool on the transaction's scan target, so the shard is prioritised again at the start of every
    ///   later sync session until the fully scanned height passes the transaction.
    fn update_found_note_shard_priority(
        &mut self,
        consensus_parameters: &impl consensus::Parameters,
        shielded_protocol: ShieldedPool,
        wallet_transaction: &WalletTransaction,
    ) {
        let found_note = match shielded_protocol {
            ShieldedPool::Sapling => !wallet_transaction.sapling_notes().is_empty(),
            ShieldedPool::Orchard => !wallet_transaction.orchard_notes().is_empty(),
            ShieldedPool::Ironwood => !wallet_transaction.ironwood_notes().is_empty(),
        };
        if found_note {
            let block_height = wallet_transaction.status().get_height();
            self.set_found_note_scan_range(
                consensus_parameters,
                Some(shielded_protocol),
                block_height,
            );
            self.insert_scan_target(
                ScanTarget::internal(block_height, wallet_transaction.txid())
                    .with_pool(shielded_protocol),
            );
        }
    }
}

impl SyncState {
    // A scan target is settled once the fully scanned height has passed its block and, for every pool its transaction
    // holds notes of, the block lies below the completing block of the pool's newest recorded shard and every recorded
    // shard range holding the block is scanned through its completing block. A target at or above that completing
    // block lies in the pool's incomplete shard, whose remaining blocks are yet to be mined, and is kept so they are
    // prioritised when they are. A target without a pool has had its transaction scanned, or names none, once the
    // fully scanned height passes it.
    fn scan_target_is_settled(
        &self,
        scan_target: &ScanTarget,
        fully_scanned_height: BlockHeight,
    ) -> bool {
        let block_height = scan_target.block_height();
        block_height <= fully_scanned_height
            && scan_target.pools().iter().all(|pool| {
                let shard_ranges = self.shard_ranges(pool);
                let below_incomplete_shard = shard_ranges
                    .last()
                    .is_some_and(|newest| block_height < newest.end - 1);
                below_incomplete_shard
                    && shard_ranges
                        .iter()
                        .filter(|range| range.contains(&block_height))
                        .all(|range| range.end - 1 <= fully_scanned_height)
            })
    }

    /// Punches in the `shielded_protocol` shard block range surrounding the `block_height` with `ScanPriority::FoundNote`.
    ///
    /// If `shielded_protocol` is `None`, punch in the surrounding `NARROW_SCAN_AREA` blocks starting from the
    /// closest lower multiple of `NARROW_SCAN_AREA` to `block_height`.
    pub(crate) fn set_found_note_scan_range(
        &mut self,
        consensus_parameters: &impl consensus::Parameters,
        shielded_protocol: Option<ShieldedPool>,
        block_height: BlockHeight,
    ) {
        let block_range =
            self.determine_block_range(consensus_parameters, block_height, shielded_protocol);
        self.punch_scan_priority(block_range, ScanPriority::FoundNote);
    }

    /// Sets the blocks of `block_range` to `scan_priority` within every unscanned, idle wallet range of a lower priority, splitting the ranges at the bounds.
    pub(crate) fn punch_scan_priority(
        &mut self,
        block_range: Range<BlockHeight>,
        scan_priority: ScanPriority,
    ) {
        self.scan_ranges = split_out_where(
            std::mem::take(&mut self.scan_ranges),
            &block_range,
            scan_priority,
            |priority| {
                !priority.is_scanned()
                    && priority != ScanPriority::Scanning
                    && priority < scan_priority
            },
        );
    }

    /// Determines the block range which contains all the note commitments for the shard of a given `shielded_protocol` surrounding
    /// the specified `block_height`.
    ///
    /// If no shard range exists for the given `block_height`, return the range of the incomplete shard at the chain tip.
    /// If `block_height` contains note commitments from multiple shards, return the block range of all of those shards combined.
    ///
    /// If `shielded_protocol` is `None`, return the surrounding [`self::NARROW_SCAN_AREA`] blocks starting from the
    /// closest lower multiple of [`self::NARROW_SCAN_AREA`] to `block_height`.
    fn determine_block_range(
        &self,
        consensus_parameters: &impl consensus::Parameters,
        block_height: BlockHeight,
        shielded_protocol: Option<ShieldedPool>,
    ) -> Range<BlockHeight> {
        if let Some(mut shielded_protocol) = shielded_protocol {
            // Walk down to the newest pool already active at `block_height`,
            // judging each pool by its Pool Activation (the single
            // pool-to-upgrade derivation; a missing activation means the pool
            // is not active).
            loop {
                let active =
                    crate::wallet::PoolActivation::of(consensus_parameters, shielded_protocol)
                        .is_some_and(|activation| block_height >= activation.height());
                if active {
                    break;
                }
                shielded_protocol = match shielded_protocol {
                    ShieldedPool::Sapling => panic!("pre-sapling not supported"),
                    ShieldedPool::Orchard => ShieldedPool::Sapling,
                    ShieldedPool::Ironwood => ShieldedPool::Orchard,
                };
            }

            let shard_ranges = self.shard_ranges(shielded_protocol);

            let target_ranges = shard_ranges
                .iter()
                .filter(|range| range.contains(&block_height))
                .cloned()
                .collect::<Vec<_>>();

            if target_ranges.is_empty() {
                let start = if let Some(range) = shard_ranges.last() {
                    range.end - 1
                } else {
                    // With no shard ranges at all (a pool freshly activated, with
                    // no complete shards), fall back to the
                    // pool's own history: the wallet birthday clamped to the
                    // pool's activation height. An unclamped birthday would let
                    // the chain-tip punch flood the entire wallet range.
                    crate::wallet::PoolActivation::of(consensus_parameters, shielded_protocol)
                        .expect("the pool was selected because its upgrade is active")
                        .max_with(
                            self.wallet_birthday()
                                .expect("scan range should not be empty"),
                        )
                };
                let end = self
                    .last_known_chain_height()
                    .expect("scan range should not be empty")
                    + 1;

                let range = Range { start, end };

                assert!(
                    range.contains(&block_height),
                    "block height should always be within the incomplete shard at chain tip when no complete shard range is found!"
                );

                range
            } else {
                Range {
                    start: target_ranges
                        .first()
                        .expect("should not be empty in this scope")
                        .start,
                    end: target_ranges
                        .last()
                        .expect("should not be empty in this scope")
                        .end,
                }
            }
        } else {
            let block_height = u32::from(block_height);
            let lower_bound =
                BlockHeight::from_u32(block_height - (block_height % NARROW_SCAN_AREA));
            let higher_bound = lower_bound + NARROW_SCAN_AREA;

            lower_bound..higher_bound
        }
    }
}

/// Splits every scan range of `from` priority that overlaps `block_range` so the overlap takes `to`, and passes every other range through.
pub(crate) fn split_out_overlapping(
    scan_ranges: Vec<ScanRange>,
    block_range: &Range<BlockHeight>,
    from: ScanPriority,
    to: ScanPriority,
) -> Vec<ScanRange> {
    split_out_where(scan_ranges, block_range, to, |priority| priority == from)
}

/// Splits every scan range whose priority passes `eligible` and overlaps `block_range` so the overlap takes `to`, and passes every other range through.
fn split_out_where(
    scan_ranges: Vec<ScanRange>,
    block_range: &Range<BlockHeight>,
    to: ScanPriority,
    eligible: impl Fn(ScanPriority) -> bool,
) -> Vec<ScanRange> {
    scan_ranges
        .into_iter()
        .flat_map(|scan_range| {
            if eligible(scan_range.priority()) && scan_range.overlaps(block_range) {
                split_out_scan_range(scan_range, block_range.clone(), to)
            } else {
                vec![scan_range]
            }
        })
        .collect()
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
            // `scan_range` is split at the upper and lower bound of `block_range`
            split_ranges.push(ScanRange::from_parts(
                middle_range.block_range().clone(),
                scan_priority,
            ));
            split_ranges.push(higher_range);
        } else {
            // `scan_range` is split only at the lower bound of `block_range`
            split_ranges.push(ScanRange::from_parts(
                higher_range.block_range().clone(),
                scan_priority,
            ));
        }
    } else if let Some((lower_range, higher_range)) = scan_range.split_at(block_range.end) {
        // `scan_range` is split only at the upper bound of `block_range`
        split_ranges.push(ScanRange::from_parts(
            lower_range.block_range().clone(),
            scan_priority,
        ));
        split_ranges.push(higher_range);
    } else {
        // `scan_range` is not split as it is fully contained within `block_range`
        // only scan priority is updated
        assert!(scan_range.block_range().start >= block_range.start);
        assert!(scan_range.block_range().end <= block_range.end);

        split_ranges.push(ScanRange::from_parts(
            scan_range.block_range().clone(),
            scan_priority,
        ));
    }

    split_ranges
}

pub(crate) fn shard_ranges_are_contiguous(shard_ranges: &[Range<BlockHeight>]) -> bool {
    shard_ranges.iter().all(|range| range.end > range.start + 1)
        && shard_ranges
            .windows(2)
            .all(|pair| pair[1].start + 1 == pair[0].end && pair[1].end > pair[0].end)
}

#[cfg(test)]
mod tests;
