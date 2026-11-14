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

/// A tree holding a checkpoint above the highest scanned height
/// rolls back to its checkpoint at that height.
#[test]
fn state_above_highest_scanned_rolls_back() {
    assert_eq!(
        plan_unscanned_state_rollback(facts(Some(10), Some(11)), BlockHeight::from_u32(10)),
        Some(BlockHeight::from_u32(10))
    );
}

/// A tree whose newest checkpoint is the highest scanned height is
/// left as it is, so the subtree roots it holds above that height
/// are kept.
#[test]
fn tree_at_highest_scanned_is_left_as_it_is() {
    assert_eq!(
        plan_unscanned_state_rollback(facts(Some(10), Some(10)), BlockHeight::from_u32(10)),
        None
    );
    assert_eq!(
        plan_unscanned_state_rollback(facts(None, Some(0)), BlockHeight::from_u32(10)),
        None
    );
}

/// A tree holding a newer checkpoint and none at the highest scanned
/// height is left as it is, as a pool rescan leaves the trees of the
/// earlier pools.
#[test]
fn tree_without_a_checkpoint_at_highest_scanned_is_left_as_it_is() {
    assert_eq!(
        plan_unscanned_state_rollback(facts(None, Some(12)), BlockHeight::from_u32(10)),
        None
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
