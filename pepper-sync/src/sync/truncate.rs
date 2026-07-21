//! The truncation contract.
//!
//! [`plan_truncation`] is a pure function that decides what a correct
//! truncation of the wallet does. It receives the three states that
//! govern the decision — the wallet state ([`WalletTruncationState`]:
//! birthday and highest scanned height), the shard-tree state
//! ([`ShardTreeTruncationState`]: per-pool checkpoint facts), and the
//! chain state (the height up to which the chain still agrees with the
//! wallet, i.e. the truncation target) — and returns a
//! [`TruncationPlan`] naming the correct post-truncation state of every
//! store. It mutates nothing; the caller applies the plan.
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
//!   it cannot roll back; only the clear-and-rescan recovery restores
//!   integrity, and the plan says so explicitly.
//!
//! Decisions live here and nowhere else: the applying code performs no
//! judgement of its own beyond reporting a planned rollback that the
//! tree store unexpectedly refuses.

use shardtree::store::ShardStore;
use zcash_protocol::consensus::{self, BlockHeight};

use crate::wallet::ShardTrees;

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
/// A checkpoint records every chain state the tree has absorbed —
/// frontier and subtree insertion both attach checkpoints — so the
/// newest checkpoint bounds the chain state the tree knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeTruncationFacts {
    /// Whether the tree's store holds a checkpoint exactly at the
    /// truncation target.
    pub has_checkpoint_at_target: bool,
    /// The newest checkpoint id the store holds, if any.
    pub newest_checkpoint: Option<BlockHeight>,
}

/// The per-pool shard-tree facts a truncation decision reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardTreeTruncationState {
    /// Facts about the sapling tree.
    pub sapling: TreeTruncationFacts,
    /// Facts about the orchard tree.
    pub orchard: TreeTruncationFacts,
    /// Facts about the ironwood tree.
    pub ironwood: TreeTruncationFacts,
}

impl ShardTreeTruncationState {
    /// Gathers the per-pool facts from the wallet's shard trees at the
    /// given truncation target. Read-only.
    #[must_use]
    pub fn gather(shard_trees: &ShardTrees, truncate_height: BlockHeight) -> Self {
        Self {
            sapling: TreeTruncationFacts {
                has_checkpoint_at_target: shard_trees
                    .sapling
                    .store()
                    .get_checkpoint(&truncate_height)
                    .expect("infallible")
                    .is_some(),
                newest_checkpoint: shard_trees
                    .sapling
                    .store()
                    .max_checkpoint_id()
                    .expect("infallible"),
            },
            orchard: TreeTruncationFacts {
                has_checkpoint_at_target: shard_trees
                    .orchard
                    .store()
                    .get_checkpoint(&truncate_height)
                    .expect("infallible")
                    .is_some(),
                newest_checkpoint: shard_trees
                    .orchard
                    .store()
                    .max_checkpoint_id()
                    .expect("infallible"),
            },
            ironwood: TreeTruncationFacts {
                has_checkpoint_at_target: shard_trees
                    .ironwood
                    .store()
                    .get_checkpoint(&truncate_height)
                    .expect("infallible")
                    .is_some(),
                newest_checkpoint: shard_trees
                    .ironwood
                    .store()
                    .max_checkpoint_id()
                    .expect("infallible"),
            },
        }
    }
}

/// The correct truncation outcome for one shard tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolTruncation {
    /// The tree holds a checkpoint at the target height: roll back to it.
    ToCheckpoint,
    /// The tree records no chain state above the target height (its
    /// newest checkpoint, if any, is at or below it): leave it untouched.
    Untouched,
    /// The tree holds state above the target height but no checkpoint at
    /// it: rollback is impossible, and only a full clear and rescan
    /// restores integrity.
    RequiresRescan,
}

/// The correct truncation outcome for the three shard trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardTreeTruncationPlan {
    /// Outcome for the sapling tree.
    pub sapling: PoolTruncation,
    /// Outcome for the orchard tree.
    pub orchard: PoolTruncation,
    /// Outcome for the ironwood tree.
    pub ironwood: PoolTruncation,
}

/// The correct post-truncation state of every store, decided purely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationPlan {
    /// The target lies above the highest scanned block: every store is
    /// already correct and nothing is removed.
    NoOp,
    /// The target lies below the wallet birthday (or is height zero):
    /// no retained state can anchor the wallet, so every store —
    /// blocks, transactions, nullifiers, outpoints, and all three shard
    /// trees — resets to empty.
    ClearAll,
    /// Roll the wallet back: blocks, transactions, nullifiers, and
    /// outpoints retain exactly the data at or below `height`, and each
    /// shard tree applies its per-pool outcome.
    Truncate {
        /// The height retained; everything strictly above it goes.
        height: BlockHeight,
        /// The per-pool shard-tree outcomes.
        trees: ShardTreeTruncationPlan,
    },
}

/// Decides the correct truncation of the wallet, purely.
///
/// See the module documentation for the contract the returned plan
/// guarantees per store.
#[must_use]
pub fn plan_truncation(
    wallet_state: WalletTruncationState,
    shard_tree_state: ShardTreeTruncationState,
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
        trees: ShardTreeTruncationPlan {
            sapling: plan_pool_truncation(shard_tree_state.sapling, truncate_height),
            orchard: plan_pool_truncation(shard_tree_state.orchard, truncate_height),
            ironwood: plan_pool_truncation(shard_tree_state.ironwood, truncate_height),
        },
    }
}

/// Decides the correct truncation of one shard tree, purely.
fn plan_pool_truncation(
    facts: TreeTruncationFacts,
    truncate_height: BlockHeight,
) -> PoolTruncation {
    if facts.has_checkpoint_at_target {
        PoolTruncation::ToCheckpoint
    } else if facts
        .newest_checkpoint
        .is_none_or(|newest| newest <= truncate_height)
    {
        PoolTruncation::Untouched
    } else {
        PoolTruncation::RequiresRescan
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn wallet_state() -> WalletTruncationState {
        WalletTruncationState {
            birthday: BlockHeight::from_u32(6),
            highest_scanned_height: BlockHeight::from_u32(10),
        }
    }

    fn facts(
        has_checkpoint_at_target: bool,
        newest_checkpoint: Option<u32>,
    ) -> TreeTruncationFacts {
        TreeTruncationFacts {
            has_checkpoint_at_target,
            newest_checkpoint: newest_checkpoint.map(BlockHeight::from_u32),
        }
    }

    fn tree_state(each: TreeTruncationFacts) -> ShardTreeTruncationState {
        ShardTreeTruncationState {
            sapling: each,
            orchard: each,
            ironwood: each,
        }
    }

    /// The migrated pre-ironwood wallet: an empty ironwood tree whose
    /// only checkpoint is the height-zero initialization checkpoint
    /// records nothing above the target, so it is untouched — never
    /// broken.
    #[test]
    fn migrated_empty_tree_is_untouched() {
        let plan = plan_truncation(
            wallet_state(),
            ShardTreeTruncationState {
                sapling: facts(true, Some(10)),
                orchard: facts(true, Some(10)),
                ironwood: facts(false, Some(0)),
            },
            BlockHeight::from_u32(8),
        );
        assert_eq!(
            plan,
            TruncationPlan::Truncate {
                height: BlockHeight::from_u32(8),
                trees: ShardTreeTruncationPlan {
                    sapling: PoolTruncation::ToCheckpoint,
                    orchard: PoolTruncation::ToCheckpoint,
                    ironwood: PoolTruncation::Untouched,
                },
            }
        );
    }

    /// A tree with no checkpoints at all likewise records nothing above
    /// the target.
    #[test]
    fn checkpointless_tree_is_untouched() {
        assert_eq!(
            plan_pool_truncation(facts(false, None), BlockHeight::from_u32(8)),
            PoolTruncation::Untouched
        );
    }

    /// A tree scanned past the target whose checkpoint at the target is
    /// gone cannot roll back: the plan names the rescan recovery.
    #[test]
    fn pruned_checkpoint_requires_rescan() {
        assert_eq!(
            plan_pool_truncation(facts(false, Some(10)), BlockHeight::from_u32(8)),
            PoolTruncation::RequiresRescan
        );
    }

    /// A checkpoint exactly at the target wins over everything else.
    #[test]
    fn checkpoint_at_target_rolls_back() {
        assert_eq!(
            plan_pool_truncation(facts(true, Some(10)), BlockHeight::from_u32(8)),
            PoolTruncation::ToCheckpoint
        );
    }

    /// A target above the highest scanned block removes nothing.
    #[test]
    fn target_above_highest_scanned_is_a_no_op() {
        let plan = plan_truncation(
            wallet_state(),
            tree_state(facts(false, None)),
            BlockHeight::from_u32(11),
        );
        assert_eq!(plan, TruncationPlan::NoOp);
    }

    /// A target below the birthday leaves nothing to anchor the wallet:
    /// every store resets.
    #[test]
    fn target_below_birthday_clears_all() {
        let plan = plan_truncation(
            wallet_state(),
            tree_state(facts(false, None)),
            BlockHeight::from_u32(5),
        );
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
            tree_state(facts(false, None)),
            consensus::H0,
        );
        assert_eq!(plan, TruncationPlan::ClearAll);
    }

    /// The birthday itself is a valid target: the wallet rolls back to
    /// its first block rather than clearing.
    #[test]
    fn target_at_birthday_truncates() {
        let plan = plan_truncation(
            wallet_state(),
            tree_state(facts(true, Some(10))),
            BlockHeight::from_u32(6),
        );
        assert!(matches!(plan, TruncationPlan::Truncate { height, .. }
            if height == BlockHeight::from_u32(6)));
    }

    /// The planner agrees with an independently stated reference model
    /// over its entire input space (heights bounded at `MAX`), and
    /// satisfies the class-level safety property that motivated this
    /// module: a tree recording nothing above the target is never
    /// condemned to rescan. The planner's inputs are small enough to
    /// enumerate exhaustively, so no property-testing dependency is
    /// needed and no case is left to sampling.
    #[test]
    fn exhaustive_sweep_matches_the_reference_model() {
        const MAX: u32 = 6;
        let heights = || (0..=MAX).map(BlockHeight::from_u32);
        let newest_choices = || std::iter::once(None).chain(heights().map(Some));

        for birthday in heights() {
            for highest_scanned_height in heights() {
                for target in heights() {
                    for has_checkpoint_at_target in [false, true] {
                        for newest_checkpoint in newest_choices() {
                            let each = TreeTruncationFacts {
                                has_checkpoint_at_target,
                                newest_checkpoint,
                            };
                            let plan = plan_truncation(
                                WalletTruncationState {
                                    birthday,
                                    highest_scanned_height,
                                },
                                tree_state(each),
                                target,
                            );

                            // The reference model, stated independently
                            // of the implementation.
                            if target == consensus::H0 || target < birthday {
                                assert_eq!(plan, TruncationPlan::ClearAll);
                                continue;
                            }
                            if target > highest_scanned_height {
                                assert_eq!(plan, TruncationPlan::NoOp);
                                continue;
                            }
                            let TruncationPlan::Truncate { height, trees } = plan else {
                                panic!("expected Truncate for target {target:?}, got {plan:?}");
                            };
                            assert_eq!(height, target);

                            let expected = if has_checkpoint_at_target {
                                PoolTruncation::ToCheckpoint
                            } else if newest_checkpoint.is_none_or(|newest| newest <= target) {
                                PoolTruncation::Untouched
                            } else {
                                PoolTruncation::RequiresRescan
                            };
                            for outcome in [trees.sapling, trees.orchard, trees.ironwood] {
                                assert_eq!(outcome, expected);
                                // The safety property behind the migrated-
                                // wallet wipe: nothing-above-target is
                                // never a rescan sentence.
                                if newest_checkpoint.is_none_or(|newest| newest <= target) {
                                    assert_ne!(outcome, PoolTruncation::RequiresRescan);
                                }
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
            assert!(
                shard_trees
                    .orchard
                    .checkpoint(BlockHeight::from_u32(height))
                    .unwrap()
            );
        }

        let at_five = ShardTreeTruncationState::gather(&shard_trees, BlockHeight::from_u32(5));
        assert_eq!(
            at_five.orchard,
            TreeTruncationFacts {
                has_checkpoint_at_target: true,
                newest_checkpoint: Some(BlockHeight::from_u32(5)),
            }
        );
        assert_eq!(
            at_five.sapling,
            TreeTruncationFacts {
                has_checkpoint_at_target: false,
                newest_checkpoint: Some(BlockHeight::from_u32(0)),
            }
        );
        assert_eq!(at_five.ironwood, at_five.sapling);

        let at_four = ShardTreeTruncationState::gather(&shard_trees, BlockHeight::from_u32(4));
        assert_eq!(
            at_four.orchard,
            TreeTruncationFacts {
                has_checkpoint_at_target: false,
                newest_checkpoint: Some(BlockHeight::from_u32(5)),
            }
        );
    }
}
