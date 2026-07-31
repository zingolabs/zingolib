//! The truncation contract: two pure decision rules, applied where the
//! data lives.
//!
//! [`plan_truncation`] routes the wallet-level decision, from the
//! wallet state ([`WalletTruncationState`]: birthday and highest
//! scanned height) and the truncation target, it returns whether the
//! truncation is a no-op, a full clear, or a rollback to the target.
//! `plan_pool_truncation` is the per-tree rule: from one tree's
//! checkpoint facts ([`TreeTruncationFacts`]) it returns that tree's
//! outcome, and the shard-tree applier derives each tree's outcome
//! through it at the point of application. Both rules are pure. All
//! mutation stays with the appliers.
//!
//! Every per-pool outcome carries the evidence that justifies it: a
//! rollback carries the checkpoint it found, and a rescan sentence
//! carries the newest checkpoint that condemned the tree. The applier
//! derives its actions and its error messages from that evidence
//! instead of re-deriving or asserting it.
//!
//! The contract the plan guarantees, per store:
//!
//! - Wallet blocks, transactions, nullifiers, and outpoints retain
//!   exactly the data at or below the plan's height.
//! - A shard tree holding a checkpoint at the height rolls back to it.
//! - A shard tree recording no chain state above the height is left
//!   untouched: it has nothing to remove, so it cannot be "broken" with
//!   respect to this truncation. This is the state of the empty ironwood
//!   tree a pre-ironwood (v0) wallet blob migrates to, whose only
//!   checkpoint is the height-zero initialization checkpoint.
//! - A shard tree holding state above the height with no checkpoint at
//!   it cannot roll back. Only the clear-and-rescan recovery restores
//!   integrity, and the plan says so explicitly.
//!
//! Decisions live here and nowhere else: the applying code performs no
//! judgement of its own beyond reporting a planned rollback that the
//! tree store unexpectedly refuses.

use incrementalmerkletree::Hashable;
use shardtree::ShardTree;
use shardtree::store::ShardStore;
use shardtree::store::memory::MemoryShardStore;
use zcash_protocol::consensus::{self, BlockHeight};

use crate::error::SyncError;
use crate::wallet::{
    ScanTarget,
    traits::{
        SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions, SyncWallet,
    },
};

use super::state::truncate_scan_ranges;

/// The wallet state a truncation decision reads: where scanning began
/// and how far it has reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalletTruncationState {
    /// The wallet birthday. A target below it means no retained state
    /// can anchor the wallet, so every store resets.
    pub birthday: BlockHeight,
    /// The highest block height that has been scanned. A target above
    /// it leaves nothing to remove.
    pub highest_scanned_height: BlockHeight,
}

/// What the planner reads of one shard tree, gathered at the truncation
/// target.
///
/// A checkpoint records every *scanned* chain state the tree has
/// absorbed (frontier insertion and scanned-subtree insertion attach
/// checkpoints), so the newest checkpoint bounds the scanned state.
/// Server-fetched subtree roots are the exception: they enter the store
/// with no checkpoint, so this planner cannot see them. They are kept
/// safe by a different mechanism: each sync session refetches the
/// newest still-bare root and replaces it if the chain moved (see
/// `crate::witness::subtree_fetch_start_index`), so a stale bare root
/// surviving an `Untouched` verdict is healed at the next session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeTruncationFacts {
    /// The checkpoint the tree's store holds exactly at the truncation
    /// target, if any.
    pub checkpoint_at_target: Option<BlockHeight>,
    /// The newest checkpoint id the store holds, if any.
    pub newest_checkpoint: Option<BlockHeight>,
}

/// Reads one tree's truncation facts from its store.
pub(crate) fn tree_facts<H, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    tree: &ShardTree<MemoryShardStore<H, BlockHeight>, DEPTH, SHARD_HEIGHT>,
    truncate_height: BlockHeight,
) -> TreeTruncationFacts
where
    H: Hashable + Clone + PartialEq,
{
    TreeTruncationFacts {
        checkpoint_at_target: tree
            .store()
            .get_checkpoint(&truncate_height)
            .expect("infallible")
            .map(|_| truncate_height),
        newest_checkpoint: tree.store().max_checkpoint_id().expect("infallible"),
    }
}

/// The correct truncation outcome for one shard tree, carrying the
/// evidence that justifies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolTruncation {
    /// The tree holds this checkpoint at the target height: roll back
    /// to it.
    ToCheckpoint {
        /// The checkpoint found at the target height.
        checkpoint: BlockHeight,
    },
    /// The tree records no chain state above the target height (its
    /// newest checkpoint, if any, is at or below it): leave it untouched.
    Untouched,
    /// The tree holds state above the target height but no checkpoint at
    /// it: rollback is impossible, and only a full clear and rescan
    /// restores integrity.
    RequiresRescan {
        /// The newest checkpoint above the target that condemned
        /// the tree.
        newest_checkpoint: BlockHeight,
    },
}

/// The correct post-truncation state of every store, decided purely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationPlan {
    /// The target lies above the highest scanned block: every store is
    /// already correct and nothing is removed.
    NoOp,
    /// The target lies below the wallet birthday (or is height zero):
    /// no retained state can anchor the wallet, so every store (blocks,
    /// transactions, nullifiers, outpoints, and all three shard
    /// trees) resets to empty.
    ClearAll,
    /// Roll the wallet back: blocks, transactions, nullifiers, and
    /// outpoints retain exactly the data at or below `height`, and each
    /// shard tree applies the per-pool outcome `plan_pool_truncation`
    /// derives at the point of application.
    Truncate {
        /// The height retained. Everything strictly above it goes.
        height: BlockHeight,
    },
}

/// Decides the correct truncation of the wallet, purely.
///
/// See the module documentation for the contract the returned plan
/// guarantees per store.
#[must_use]
pub fn plan_truncation(
    wallet_state: WalletTruncationState,
    truncate_height: BlockHeight,
) -> TruncationPlan {
    if truncate_height == consensus::H0 || truncate_height < wallet_state.birthday {
        return TruncationPlan::ClearAll;
    }
    if truncate_height > wallet_state.highest_scanned_height {
        return TruncationPlan::NoOp;
    }
    TruncationPlan::Truncate {
        height: truncate_height,
    }
}

/// Decides the correct truncation of one shard tree, purely, from the
/// evidence in its facts.
pub(crate) fn plan_pool_truncation(
    facts: TreeTruncationFacts,
    truncate_height: BlockHeight,
) -> PoolTruncation {
    if let Some(checkpoint) = facts.checkpoint_at_target {
        return PoolTruncation::ToCheckpoint { checkpoint };
    }
    match facts.newest_checkpoint {
        Some(newest_checkpoint) if newest_checkpoint > truncate_height => {
            PoolTruncation::RequiresRescan { newest_checkpoint }
        }
        _ => PoolTruncation::Untouched,
    }
}

/// Truncates all wallet stores, derived state, and scan-range accounting to
/// the targeted height.
///
/// This is the paired entry point: every flow in which the wallet's claimed
/// height itself is wrong rewinds range accounting together with the data
/// through this function:
/// 1. the wallet-above-chain reset
/// 2. healing unqualified written data
/// 3. the clear-all recovery
pub(super) fn targeted_truncate_wallet_height<W>(
    wallet: &mut W,
    truncate_height: BlockHeight,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    truncate_wallet_data(wallet, truncate_height)?;
    truncate_scan_ranges(
        truncate_height,
        wallet
            .get_sync_state_mut()
            .map_err(SyncError::WalletError)?,
    );

    Ok(())
}

/// Truncates wallet data alone, leaving scan-range accounting untouched.
///
/// Solely for reorg fork-finding: the caller has already re-prioritised the
/// affected span for verification, so range coverage must survive the data
/// rewind. Every other flow pairs the range rewind with the data through
/// [`targeted_truncate_wallet_height`].
pub(super) fn truncate_data_for_verification<W>(
    wallet: &mut W,
    truncate_height: BlockHeight,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    truncate_wallet_data(wallet, truncate_height)
}

fn truncate_wallet_data<W>(
    wallet: &mut W,
    truncate_height: BlockHeight,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    let wallet_state = WalletTruncationState {
        birthday: sync_state
            .wallet_birthday()
            .expect("should be non-empty in this scope"),
        highest_scanned_height: sync_state
            .highest_scanned_height()
            .expect("should be non-empty in this scope"),
    };
    match plan_truncation(wallet_state, truncate_height) {
        TruncationPlan::NoOp => Ok(()),
        TruncationPlan::ClearAll => {
            truncate_stores(wallet, consensus::H0)?;
            wallet.clear_shard_trees()
        }
        TruncationPlan::Truncate { height } => {
            truncate_stores(wallet, height)?;
            match wallet.truncate_shard_trees(height) {
                Ok(()) => Ok(()),
                Err(SyncError::TruncationError(height, pooltype)) => {
                    clear_wallet_data(wallet)?;

                    Err(SyncError::TruncationError(height, pooltype))
                }
                Err(e) => Err(e),
            }
        }
    }
}

/// Removes wallet blocks, transactions, nullifiers and outpoints above the
/// given `truncate_height`.
fn truncate_stores<W>(
    wallet: &mut W,
    truncate_height: BlockHeight,
) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints,
{
    wallet
        .truncate_wallet_blocks(truncate_height)
        .map_err(SyncError::WalletError)?;
    wallet
        .truncate_wallet_transactions(truncate_height)
        .map_err(SyncError::WalletError)?;
    wallet
        .truncate_nullifiers(truncate_height)
        .map_err(SyncError::WalletError)?;
    wallet
        .truncate_outpoints(truncate_height)
        .map_err(SyncError::WalletError)?;

    Ok(())
}

/// Resets every store to empty while retaining the confirmed transactions'
/// heights and txids as scan targets, so the forced rescan can find them.
pub(super) fn clear_wallet_data<W>(wallet: &mut W) -> Result<(), SyncError<W::Error>>
where
    W: SyncWallet + SyncBlocks + SyncTransactions + SyncNullifiers + SyncOutPoints + SyncShardTrees,
{
    let scan_targets = wallet
        .get_wallet_transactions()
        .map_err(SyncError::WalletError)?
        .values()
        .filter_map(|transaction| {
            transaction
                .status()
                .get_confirmed_height()
                .map(|height| ScanTarget {
                    block_height: height,
                    txid: transaction.txid(),
                    narrow_scan_area: true,
                })
        })
        .collect::<Vec<_>>();
    targeted_truncate_wallet_height(wallet, consensus::H0)?;
    wallet
        .get_wallet_transactions_mut()
        .map_err(SyncError::WalletError)?
        .clear();
    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
    super::add_scan_targets(sync_state, &scan_targets);
    wallet.set_save_flag().map_err(SyncError::WalletError)?;

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::shardtree_ext::{CheckpointAppendOutcome, ShardTreeExt};
    use crate::wallet::ShardTrees;

    fn wallet_state() -> WalletTruncationState {
        WalletTruncationState {
            birthday: BlockHeight::from_u32(6),
            highest_scanned_height: BlockHeight::from_u32(10),
        }
    }

    /// Facts for a tree whose checkpoint at the target is present when
    /// `at_target` is `Some` (carrying the target height itself).
    fn facts(at_target: Option<u32>, newest_checkpoint: Option<u32>) -> TreeTruncationFacts {
        TreeTruncationFacts {
            checkpoint_at_target: at_target.map(BlockHeight::from_u32),
            newest_checkpoint: newest_checkpoint.map(BlockHeight::from_u32),
        }
    }

    /// The migrated pre-ironwood wallet: sapling and orchard roll back to
    /// their target checkpoints, and the empty ironwood tree whose only
    /// checkpoint is the height-zero initialization checkpoint records
    /// nothing above the target, so it is untouched, never broken.
    #[test]
    fn migrated_empty_tree_is_untouched() {
        assert_eq!(
            plan_truncation(wallet_state(), BlockHeight::from_u32(8)),
            TruncationPlan::Truncate {
                height: BlockHeight::from_u32(8),
            }
        );
        assert_eq!(
            plan_pool_truncation(facts(Some(8), Some(10)), BlockHeight::from_u32(8)),
            PoolTruncation::ToCheckpoint {
                checkpoint: BlockHeight::from_u32(8),
            }
        );
        assert_eq!(
            plan_pool_truncation(facts(None, Some(0)), BlockHeight::from_u32(8)),
            PoolTruncation::Untouched
        );
    }

    /// A tree with no checkpoints at all likewise records nothing above
    /// the target.
    #[test]
    fn checkpointless_tree_is_untouched() {
        assert_eq!(
            plan_pool_truncation(facts(None, None), BlockHeight::from_u32(8)),
            PoolTruncation::Untouched
        );
    }

    /// A tree scanned past the target whose checkpoint at the target is
    /// gone cannot roll back: the plan names the rescan recovery and the
    /// checkpoint that condemned the tree.
    #[test]
    fn pruned_checkpoint_requires_rescan() {
        assert_eq!(
            plan_pool_truncation(facts(None, Some(10)), BlockHeight::from_u32(8)),
            PoolTruncation::RequiresRescan {
                newest_checkpoint: BlockHeight::from_u32(10),
            }
        );
    }

    /// A checkpoint exactly at the target wins over everything else, and
    /// the outcome carries the checkpoint it found.
    #[test]
    fn checkpoint_at_target_rolls_back() {
        assert_eq!(
            plan_pool_truncation(facts(Some(8), Some(10)), BlockHeight::from_u32(8)),
            PoolTruncation::ToCheckpoint {
                checkpoint: BlockHeight::from_u32(8),
            }
        );
    }

    /// A target above the highest scanned block removes nothing.
    #[test]
    fn target_above_highest_scanned_is_a_no_op() {
        let plan = plan_truncation(wallet_state(), BlockHeight::from_u32(11));
        assert_eq!(plan, TruncationPlan::NoOp);
    }

    /// A target below the birthday leaves nothing to anchor the wallet:
    /// every store resets.
    #[test]
    fn target_below_birthday_clears_all() {
        let plan = plan_truncation(wallet_state(), BlockHeight::from_u32(5));
        assert_eq!(plan, TruncationPlan::ClearAll);
    }

    /// Height zero is the explicit clear-everything request, birthday
    /// notwithstanding.
    #[test]
    fn height_zero_clears_all() {
        let plan = plan_truncation(
            WalletTruncationState {
                birthday: consensus::H0,
                highest_scanned_height: BlockHeight::from_u32(10),
            },
            consensus::H0,
        );
        assert_eq!(plan, TruncationPlan::ClearAll);
    }

    /// The birthday itself is a valid target: the wallet rolls back to
    /// its first block rather than clearing.
    #[test]
    fn target_at_birthday_truncates() {
        let plan = plan_truncation(wallet_state(), BlockHeight::from_u32(6));
        assert!(matches!(plan, TruncationPlan::Truncate { height }
            if height == BlockHeight::from_u32(6)));
    }

    /// The planner agrees with an independently stated reference model
    /// over its entire input space (heights bounded at `MAX`), and
    /// satisfies the class-level safety property that motivated this
    /// module: a tree recording nothing above the target is never
    /// condemned to rescan. The facts-level inputs are small enough to
    /// enumerate exhaustively, so no case is left to sampling. The
    /// `properties` module complements this with generated checkpoint
    /// sets.
    #[test]
    fn exhaustive_sweep_matches_the_reference_model() {
        const MAX: u32 = 6;
        let heights = || (0..=MAX).map(BlockHeight::from_u32);
        let newest_choices = || std::iter::once(None).chain(heights().map(Some));

        for birthday in heights() {
            for highest_scanned_height in heights() {
                for target in heights() {
                    let plan = plan_truncation(
                        WalletTruncationState {
                            birthday,
                            highest_scanned_height,
                        },
                        target,
                    );

                    // The wallet-level reference model, stated
                    // independently of the implementation.
                    if target == consensus::H0 || target < birthday {
                        assert_eq!(plan, TruncationPlan::ClearAll);
                        continue;
                    }
                    if target > highest_scanned_height {
                        assert_eq!(plan, TruncationPlan::NoOp);
                        continue;
                    }
                    assert_eq!(plan, TruncationPlan::Truncate { height: target });

                    // The per-pool reference model at this target.
                    for checkpoint_at_target in [None, Some(target)] {
                        for newest_checkpoint in newest_choices() {
                            let outcome = plan_pool_truncation(
                                TreeTruncationFacts {
                                    checkpoint_at_target,
                                    newest_checkpoint,
                                },
                                target,
                            );
                            let expected = if let Some(checkpoint) = checkpoint_at_target {
                                PoolTruncation::ToCheckpoint { checkpoint }
                            } else {
                                match newest_checkpoint {
                                    Some(newest_checkpoint) if newest_checkpoint > target => {
                                        PoolTruncation::RequiresRescan { newest_checkpoint }
                                    }
                                    _ => PoolTruncation::Untouched,
                                }
                            };
                            assert_eq!(outcome, expected);
                            // The safety property behind the migrated-
                            // wallet wipe: nothing-above-target is never
                            // a rescan sentence.
                            if newest_checkpoint.is_none_or(|newest| newest <= target) {
                                assert!(!matches!(outcome, PoolTruncation::RequiresRescan { .. }));
                            }
                        }
                    }
                }
            }
        }
    }

    /// `gather` reports the store's ground truth: the checkpoint facts it
    /// returns match what the shard-tree stores actually hold, per pool.
    /// This guards the proxy seam between fact-gathering and planning.
    #[test]
    fn gather_reports_store_ground_truth() {
        // Each tree starts with the height-zero initialization
        // checkpoint; orchard alone gains checkpoints at 3 and 5.
        let mut shard_trees = ShardTrees::new();
        for height in [3u32, 5] {
            assert_eq!(
                shard_trees
                    .orchard
                    .append_checkpoint(BlockHeight::from_u32(height))
                    .unwrap(),
                CheckpointAppendOutcome::Appended
            );
        }

        assert_eq!(
            tree_facts(&shard_trees.orchard, BlockHeight::from_u32(5)),
            TreeTruncationFacts {
                checkpoint_at_target: Some(BlockHeight::from_u32(5)),
                newest_checkpoint: Some(BlockHeight::from_u32(5)),
            }
        );
        assert_eq!(
            tree_facts(&shard_trees.sapling, BlockHeight::from_u32(5)),
            TreeTruncationFacts {
                checkpoint_at_target: None,
                newest_checkpoint: Some(BlockHeight::from_u32(0)),
            }
        );
        assert_eq!(
            tree_facts(&shard_trees.ironwood, BlockHeight::from_u32(5)),
            tree_facts(&shard_trees.sapling, BlockHeight::from_u32(5)),
        );

        assert_eq!(
            tree_facts(&shard_trees.orchard, BlockHeight::from_u32(4)),
            TreeTruncationFacts {
                checkpoint_at_target: None,
                newest_checkpoint: Some(BlockHeight::from_u32(5)),
            }
        );
    }
}

/// Property tests over generated checkpoint sets, complementing the
/// exhaustive facts-level sweep in `test`: the facts here are derived
/// from a whole generated set, as `gather` would derive them from a
/// store, so the properties quantify over tree histories rather than
/// pre-digested facts.
#[cfg(test)]
mod properties {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;

    /// The facts `gather` would report for a tree holding exactly
    /// `checkpoints`.
    fn facts_for_set(checkpoints: &BTreeSet<u32>, target: u32) -> TreeTruncationFacts {
        TreeTruncationFacts {
            checkpoint_at_target: checkpoints
                .contains(&target)
                .then(|| BlockHeight::from_u32(target)),
            newest_checkpoint: checkpoints
                .last()
                .map(|&height| BlockHeight::from_u32(height)),
        }
    }

    fn checkpoint_sets() -> impl Strategy<Value = BTreeSet<u32>> {
        proptest::collection::btree_set(0u32..=1_000, 0..16)
    }

    proptest! {
        /// Agreement with a brute-force model computed directly from the
        /// checkpoint set.
        #[test]
        fn pool_outcome_agrees_with_the_set_model(
            checkpoints in checkpoint_sets(),
            target in 0u32..=1_000,
        ) {
            let outcome = plan_pool_truncation(
                facts_for_set(&checkpoints, target),
                BlockHeight::from_u32(target),
            );
            let expected = if checkpoints.contains(&target) {
                PoolTruncation::ToCheckpoint {
                    checkpoint: BlockHeight::from_u32(target),
                }
            } else {
                match checkpoints.last() {
                    Some(&newest) if newest > target => PoolTruncation::RequiresRescan {
                        newest_checkpoint: BlockHeight::from_u32(newest),
                    },
                    _ => PoolTruncation::Untouched,
                }
            };
            prop_assert_eq!(outcome, expected);
        }

        /// The class property behind the migrated-wallet wipe: a tree
        /// with no checkpoint above the target is never condemned to
        /// rescan.
        #[test]
        fn nothing_above_target_is_never_condemned(
            candidates in checkpoint_sets(),
            target in 0u32..=1_000,
        ) {
            // Constructed, not assumed: keeping only checkpoints at or
            // below the target samples the property's whole domain
            // without rejects (a prop_assume here starved the runner).
            let checkpoints: BTreeSet<u32> = candidates
                .into_iter()
                .filter(|&height| height <= target)
                .collect();
            let outcome = plan_pool_truncation(
                facts_for_set(&checkpoints, target),
                BlockHeight::from_u32(target),
            );
            prop_assert!(
                !matches!(outcome, PoolTruncation::RequiresRescan { .. }),
                "a tree with nothing above target {} was condemned to rescan: {:?}",
                target,
                outcome,
            );
        }

        /// Monotonicity in the target: once a tree is `Untouched` at a
        /// target, every higher target leaves it untouched too, since moving
        /// the truncation point up can never invent state above it.
        #[test]
        fn untouched_is_upward_closed(
            checkpoints in checkpoint_sets(),
            target in 0u32..1_000,
        ) {
            let outcome_at = |t: u32| {
                plan_pool_truncation(facts_for_set(&checkpoints, t), BlockHeight::from_u32(t))
            };
            if matches!(outcome_at(target), PoolTruncation::Untouched) {
                prop_assert!(matches!(outcome_at(target + 1), PoolTruncation::Untouched));
            }
        }

        /// Totality: every input falls in exactly the region the
        /// wallet-state rules name, and the planner never panics.
        #[test]
        fn planner_is_total_over_its_regions(
            birthday in 0u32..=1_000,
            highest_scanned_height in 0u32..=1_000,
            target in 0u32..=1_000,
        ) {
            let plan = plan_truncation(
                WalletTruncationState {
                    birthday: BlockHeight::from_u32(birthday),
                    highest_scanned_height: BlockHeight::from_u32(highest_scanned_height),
                },
                BlockHeight::from_u32(target),
            );
            if target == 0 || target < birthday {
                prop_assert_eq!(plan, TruncationPlan::ClearAll);
            } else if target > highest_scanned_height {
                prop_assert_eq!(plan, TruncationPlan::NoOp);
            } else {
                prop_assert!(
                    matches!(plan, TruncationPlan::Truncate { height }
                        if height == BlockHeight::from_u32(target)),
                    "expected Truncate at height {}, got {:?}",
                    target,
                    plan,
                );
            }
        }
    }
}
