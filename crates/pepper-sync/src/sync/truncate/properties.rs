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
