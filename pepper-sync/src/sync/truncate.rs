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
//! Every plan variant carries the evidence that justifies it: a rollback
//! carries the checkpoint it found, and a rescan sentence carries the
//! newest checkpoint that condemned the tree. The applier derives its
//! actions and its error messages from that evidence instead of
//! re-deriving or asserting it.
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

use incrementalmerkletree::Hashable;
use shardtree::ShardTree;
use shardtree::store::ShardStore;
use shardtree::store::memory::MemoryShardStore;
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
/// A checkpoint records every *scanned* chain state the tree has
/// absorbed — frontier insertion and scanned-subtree insertion attach
/// checkpoints — so the newest checkpoint bounds the scanned state.
/// Server-fetched subtree roots are the exception: they enter the store
/// with no checkpoint, so this planner cannot see them. They are kept
/// safe by a different mechanism — each sync session refetches the
/// newest still-bare root and replaces it if the chain moved (see
/// `crate::witness::subtree_fetch_start_index`) — so a stale bare root
/// surviving an `Untouched` verdict is healed at the next session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeTruncationFacts {
    /// The checkpoint the tree's store holds exactly at the truncation
    /// target, if any.
    pub checkpoint_at_target: Option<BlockHeight>,
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
            sapling: tree_facts(&shard_trees.sapling, truncate_height),
            orchard: tree_facts(&shard_trees.orchard, truncate_height),
            ironwood: tree_facts(&shard_trees.ironwood, truncate_height),
        }
    }
}

/// Reads one tree's truncation facts from its store.
fn tree_facts<H, const DEPTH: u8, const SHARD_HEIGHT: u8>(
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
        /// The newest checkpoint — above the target — that condemned
        /// the tree.
        newest_checkpoint: BlockHeight,
    },
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

/// Decides the correct truncation of one shard tree, purely, from the
/// evidence in its facts.
fn plan_pool_truncation(
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

#[cfg(test)]
mod test {
    use super::*;
    use crate::shardtree_ext::{CheckpointAppendOutcome, ShardTreeExt};

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
                sapling: facts(Some(8), Some(10)),
                orchard: facts(Some(8), Some(10)),
                ironwood: facts(None, Some(0)),
            },
            BlockHeight::from_u32(8),
        );
        assert_eq!(
            plan,
            TruncationPlan::Truncate {
                height: BlockHeight::from_u32(8),
                trees: ShardTreeTruncationPlan {
                    sapling: PoolTruncation::ToCheckpoint {
                        checkpoint: BlockHeight::from_u32(8),
                    },
                    orchard: PoolTruncation::ToCheckpoint {
                        checkpoint: BlockHeight::from_u32(8),
                    },
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
        let plan = plan_truncation(
            wallet_state(),
            tree_state(facts(None, None)),
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
            tree_state(facts(None, None)),
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
            tree_state(facts(None, None)),
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
            tree_state(facts(Some(6), Some(10))),
            BlockHeight::from_u32(6),
        );
        assert!(matches!(plan, TruncationPlan::Truncate { height, .. }
            if height == BlockHeight::from_u32(6)));
    }

    /// The planner agrees with an independently stated reference model
    /// over its entire input space (heights bounded at `MAX`), and
    /// satisfies the class-level safety property that motivated this
    /// module: a tree recording nothing above the target is never
    /// condemned to rescan. The facts-level inputs are small enough to
    /// enumerate exhaustively, so no case is left to sampling; the
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
                    for checkpoint_at_target in [None, Some(target)] {
                        for newest_checkpoint in newest_choices() {
                            let each = TreeTruncationFacts {
                                checkpoint_at_target,
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
                            for outcome in [trees.sapling, trees.orchard, trees.ironwood] {
                                assert_eq!(outcome, expected);
                                // The safety property behind the migrated-
                                // wallet wipe: nothing-above-target is
                                // never a rescan sentence.
                                if newest_checkpoint.is_none_or(|newest| newest <= target) {
                                    assert!(!matches!(
                                        outcome,
                                        PoolTruncation::RequiresRescan { .. }
                                    ));
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
            assert_eq!(
                shard_trees
                    .orchard
                    .append_checkpoint(BlockHeight::from_u32(height))
                    .unwrap(),
                CheckpointAppendOutcome::Appended
            );
        }

        let at_five = ShardTreeTruncationState::gather(&shard_trees, BlockHeight::from_u32(5));
        assert_eq!(
            at_five.orchard,
            TreeTruncationFacts {
                checkpoint_at_target: Some(BlockHeight::from_u32(5)),
                newest_checkpoint: Some(BlockHeight::from_u32(5)),
            }
        );
        assert_eq!(
            at_five.sapling,
            TreeTruncationFacts {
                checkpoint_at_target: None,
                newest_checkpoint: Some(BlockHeight::from_u32(0)),
            }
        );
        assert_eq!(at_five.ironwood, at_five.sapling);

        let at_four = ShardTreeTruncationState::gather(&shard_trees, BlockHeight::from_u32(4));
        assert_eq!(
            at_four.orchard,
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
            checkpoints in checkpoint_sets(),
            target in 0u32..=1_000,
        ) {
            prop_assume!(checkpoints.last().is_none_or(|&newest| newest <= target));
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
        /// target, every higher target leaves it untouched too — moving
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
            checkpoints in checkpoint_sets(),
        ) {
            let each = facts_for_set(&checkpoints, target);
            let plan = plan_truncation(
                WalletTruncationState {
                    birthday: BlockHeight::from_u32(birthday),
                    highest_scanned_height: BlockHeight::from_u32(highest_scanned_height),
                },
                ShardTreeTruncationState {
                    sapling: each,
                    orchard: each,
                    ironwood: each,
                },
                BlockHeight::from_u32(target),
            );
            if target == 0 || target < birthday {
                prop_assert_eq!(plan, TruncationPlan::ClearAll);
            } else if target > highest_scanned_height {
                prop_assert_eq!(plan, TruncationPlan::NoOp);
            } else {
                prop_assert!(
                    matches!(plan, TruncationPlan::Truncate { height, .. }
                        if height == BlockHeight::from_u32(target)),
                    "expected Truncate at height {}, got {:?}",
                    target,
                    plan,
                );
            }
        }
    }
}
