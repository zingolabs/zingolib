use super::*;
use crate::config::ChainType;
use proptest::prelude::*;
use zcash_protocol::value::COIN;

fn params() -> MigrationParams {
    MigrationParams::provisional(ChainType::Mainnet)
}

/// `MARGINAL_FEE` and `MIN_BUNDLE_ACTIONS` are the only numbers this module
/// still restates, and each has a reason: `Zatoshis` cannot be unwrapped in
/// `const` context, and orchard's `MIN_ACTIONS` is private. Pin both to
/// their sources.
#[test]
fn mirrored_constants_match_the_crates() {
    use orchard::bundle::BundleVersion;
    use zcash_primitives::transaction::fees::zip317;

    assert_eq!(MARGINAL_FEE, zip317::MARGINAL_FEE.into_u64());
    // Every non-empty bundle is padded to the minimum, whatever it holds.
    assert_eq!(
        bundle_actions(BundleVersion::orchard_v3(), 1, 0) as u64,
        MIN_BUNDLE_ACTIONS
    );
    assert_eq!(
        bundle_actions(BundleVersion::ironwood_v3(), 0, 1) as u64,
        MIN_BUNDLE_ACTIONS
    );
}

/// Economic pin for the residual policy: `sweep_min` may carry any
/// safety factor above the ZIP-317 marginal fee, but must never sit
/// below it: below that line a selected note could cost more to spend
/// than the value it provides.
#[test]
fn sweep_min_covers_the_marginal_input_cost() {
    assert!(params().sweep_min >= MARGINAL_FEE);
}

/// The action-count rule the fee model rests on: an Orchard bundle from
/// NU6.3 disables cross-address transfers, so a spend and an output no
/// longer share an action. The crates own that rule and the split fee
/// follows it by asking them. These figures pin the answer.
#[test]
fn note_split_fee_follows_the_activation_boundary() {
    assert_eq!(note_split_fee(1, 1, false), 10_000);
    assert_eq!(note_split_fee(32, 1, false), 160_000);
    assert_eq!(note_split_fee(32, 1, true), 165_000);
}

#[test]
fn part_fee_matches_the_crates() {
    use orchard::bundle::BundleVersion;

    // A part has an Orchard bundle (1 spend, cross-address disabled
    // post-NU6.3) and an Ironwood bundle (1 output, cross-address
    // enabled). The fee covers the sum of both bundles' padded actions.
    assert_eq!(
        CANONICAL_PART_FEE,
        zip317_fee(
            bundle_actions(BundleVersion::orchard_v3(), 1, 0),
            bundle_actions(BundleVersion::ironwood_v3(), 0, 1),
        )
    );
    assert_eq!(CANONICAL_PART_FEE, 20_000);
    assert_eq!(params().part_fee, CANONICAL_PART_FEE);
}

#[test]
fn part_denomination_requires_exact_sizing() {
    let params = params();
    let fee = params.part_fee;
    assert_eq!(part_denomination(1_000_000 + fee, &params), Some(1_000_000));
    assert_eq!(
        part_denomination(100 * COIN + fee, &params),
        Some(100 * COIN)
    );
    assert_eq!(part_denomination(1_000_000, &params), None); // fee missing
    assert_eq!(part_denomination(1_000_000 + fee + 1, &params), None);
    assert_eq!(part_denomination(0, &params), None);
}

#[test]
fn plan_hash_binds_the_plan_content() {
    let plan = plan_migration(&[123_456_789], true, &params());
    let hash = plan_hash(&plan);
    assert_eq!(hash, plan_hash(&plan.clone()));

    let mut altered = plan.clone();
    altered.residual += 1;
    assert_ne!(hash, plan_hash(&altered));
}

/// Runs [`assert_plan_executes_in_era`] in both fee eras and returns the
/// post-activation plan.
fn assert_plan_executes(notes: &[u64], params: &MigrationParams) -> MigrationPlan {
    assert_plan_executes_in_era(notes, false, params);
    assert_plan_executes_in_era(notes, true, params)
}

/// Simulates plan execution: every round's inputs must be present in the
/// wallet's note multiset at that point, and the final multiset must be
/// exactly the part-ready notes. Returns the plan for further checks.
fn assert_plan_executes_in_era(
    notes: &[u64],
    post_activation: bool,
    params: &MigrationParams,
) -> MigrationPlan {
    use std::collections::HashMap;
    let max_notes = side_budget(params);
    let plan = plan_migration(notes, post_activation, params);

    let mut available: HashMap<u64, i64> = HashMap::new();
    for &note in notes {
        if part_denomination(note, params).is_none() && note <= params.sweep_min {
            continue; // residual, never spent
        }
        // With empty splitting + empty parts, pooled notes are residual.
        if plan.split_rounds.is_empty()
            && plan.parts.is_empty()
            && part_denomination(note, params).is_none()
        {
            continue;
        }
        *available.entry(note).or_insert(0) += 1;
    }

    for round in &plan.split_rounds {
        for tx in round {
            assert!((2..=max_notes).contains(&tx.inputs.len()) || tx.inputs.len() == 1);
            assert!(tx.inputs.len() <= max_notes, "too many inputs");
            assert!(!tx.outputs.is_empty() && tx.outputs.len() <= max_notes);
            assert!(
                tx.inputs.len() + tx.outputs.len() <= params.max_actions_per_split_tx,
                "spends plus outputs ({} + {}) exceed the total budget of {}",
                tx.inputs.len(),
                tx.outputs.len(),
                params.max_actions_per_split_tx
            );
            let min_fee = note_split_fee(tx.inputs.len(), tx.outputs.len(), post_activation);
            assert!(tx.fee() >= min_fee, "fee below ZIP-317 conventional");
            for input in &tx.inputs {
                let count = available.entry(*input).or_insert(0);
                assert!(*count > 0, "spends a note the wallet does not hold");
                *count -= 1;
            }
        }
        // Outputs only become spendable after the whole round confirms.
        for tx in round {
            for output in &tx.outputs {
                assert!(*output > params.sweep_min, "note splitting created dust");
                *available.entry(*output).or_insert(0) += 1;
            }
        }
    }

    // What remains must be exactly the parts, each note d + part_fee.
    let mut expected: HashMap<u64, i64> = HashMap::new();
    for &denomination in &plan.parts {
        *expected.entry(denomination + params.part_fee).or_insert(0) += 1;
    }
    available.retain(|_, count| *count != 0);
    expected.retain(|_, count| *count != 0);
    assert_eq!(available, expected, "post-splitting notes ≠ part set");

    plan
}

/// Asserts the selection invariant: no planned transaction spends a note
/// worth at most `sweep_min`, whether it is a wallet note or an
/// intermediate created by an earlier round.
fn assert_planned_inputs_exceed_sweep_min(plan: &MigrationPlan, params: &MigrationParams) {
    for (round, transactions) in plan.split_rounds.iter().enumerate() {
        for tx in transactions {
            for &input in &tx.inputs {
                assert!(
                    input > params.sweep_min,
                    "round {round} spends a {input}-zatoshi note, \
                         at or below sweep_min ({})",
                    params.sweep_min
                );
            }
        }
    }
}

/// ZIP 318's preparation shape is a total budget: a transaction's
/// spends and outputs together must fit
/// [`MigrationParams::max_actions_per_split_tx`]. A pool small enough
/// to skip reduction (at most one spend side's worth of notes) that
/// funds several denominations used to emit one sizing transaction
/// carrying the whole pool beside every target — up to 30 actions
/// post-NU6.3. The planner now consolidates the smallest inputs into
/// one funding note first, and every planned transaction fits the
/// budget (the executes-in-era validator asserts the bound for every
/// test in this module).
#[test]
fn sizing_respects_the_total_action_budget() {
    let params = params();
    // Fifteen unquantized ~11-ZEC notes: reduction is skipped, and the
    // ~165-ZEC total decomposes into several denominations, so the
    // sizing round mixes many spends with many outputs.
    let notes = vec![1_100_000_007u64; 15];
    for post_activation in [false, true] {
        let plan = assert_plan_executes_in_era(&notes, post_activation, &params);
        assert!(
            plan.parts.len() >= 2,
            "the shape under test needs several targets, got {:?}",
            plan.parts
        );
        // The consolidation prepends its own round, and the sizing
        // transaction still mixes multiple spends with multiple
        // outputs — the shape that used to bust the budget.
        assert!(
            plan.split_rounds.len() >= 2,
            "expected consolidation + sizing"
        );
        assert!(
            plan.split_rounds
                .iter()
                .flatten()
                .any(|tx| tx.inputs.len() >= 2 && tx.outputs.len() >= 2),
            "the plan under test must exercise a mixed spend/output transaction"
        );
    }
}

/// The selection boundary is strict: a note worth exactly `sweep_min` is
/// residual, one zatoshi more is selected. Fails whenever the planner's
/// residual filter admits a note at or below the threshold.
#[test]
fn notes_at_or_below_sweep_min_are_never_selected() {
    let params = params();
    let dust = [1, MARGINAL_FEE, params.sweep_min - 1, params.sweep_min];
    let mut notes = dust.to_vec();
    notes.extend([params.sweep_min + 1, 2_000_000, 3_000_000]);
    for post_activation in [false, true] {
        let plan = plan_migration(&notes, post_activation, &params);
        assert_planned_inputs_exceed_sweep_min(&plan, &params);
        assert_eq!(plan.residual, dust.iter().sum::<u64>());
        // The note one zatoshi above the boundary is selected.
        assert!(
            plan.split_rounds[0]
                .iter()
                .any(|tx| tx.inputs.contains(&(params.sweep_min + 1)))
        );
    }
}

/// Guards the one shape where consolidation could violate the selection
/// invariant. Post-activation, a trailing group of exactly two notes,
/// each at most 12_500 zatoshis, would merge to `sum - 15_000` (a pair
/// of 12_000s to 9_000): at or below `sweep_min`. The sizing round
/// would then spend a note the residual policy refuses, and a replan
/// after an interruption would leave it as residual, abandoning the pair's value
/// after fees were paid to create it. The planner instead carries such
/// a group into the next pool unmerged. Every other shape merges clear
/// of the threshold: a group of three yields at least 10_003, and a
/// pre-activation pair at least 10_002, because spends and outputs
/// share actions there.
#[test]
fn consolidation_never_creates_a_sub_sweep_min_intermediate() {
    let params = params();
    // Twelve full consolidation groups (enough to fund a 0.01-ZEC part
    // after the merge fees of the 16-action shape) plus a trailing pair
    // that would merge to 9_000 zatoshis, below the sweep minimum.
    let notes = vec![12_000u64; 12 * side_budget(&params) + 2];
    for post_activation in [false, true] {
        let plan = assert_plan_executes_in_era(&notes, post_activation, &params);
        assert_planned_inputs_exceed_sweep_min(&plan, &params);
    }

    // Post-activation the pair is carried unmerged: the consolidation round
    // merges only the full groups, and the sizing round spends the pair
    // directly.
    let plan = plan_migration(&notes, true, &params);
    assert_eq!(plan.split_rounds[0].len(), 12);
    assert_eq!(
        plan.split_rounds[1][0]
            .inputs
            .iter()
            .filter(|&&value| value == 12_000)
            .count(),
        2
    );
}

#[test]
fn already_split_wallet_skips_splitting() {
    // Notes already sized denomination + fee: no splitting needed.
    let params = params();
    let fee = params.part_fee;
    let notes = vec![COIN + fee, COIN / 10 + fee, 1_000_000 + fee];
    let plan = assert_plan_executes(&notes, &params);
    assert!(plan.is_split());
    assert_eq!(plan.parts, vec![COIN, COIN / 10, 1_000_000]);
    assert_eq!(plan.residual, 0);
}

#[test]
fn dust_only_wallet_leaves_everything_residual() {
    let params = params();
    let notes = vec![5_000, params.sweep_min, 1];
    let plan = assert_plan_executes(&notes, &params);
    assert!(plan.split_rounds.is_empty());
    assert!(plan.parts.is_empty());
    assert_eq!(plan.residual, 5_000 + params.sweep_min + 1);
}

#[test]
fn sub_denomination_pool_is_residual_without_splitting() {
    // Two sweepable notes that cannot fund even the smallest target.
    let notes = vec![20_000, 30_000];
    let plan = assert_plan_executes(&notes, &params());
    assert!(plan.split_rounds.is_empty());
    assert!(plan.parts.is_empty());
    assert_eq!(plan.residual, 50_000);
}

#[test]
fn single_note_is_split_in_one_transaction() {
    let params = params();
    let notes = vec![123_456_789]; // 1.23456789 ZEC
    let plan = assert_plan_executes(&notes, &params);
    assert_eq!(plan.split_rounds.len(), 1);
    assert_eq!(plan.split_rounds[0].len(), 1);
    let tx = &plan.split_rounds[0][0];
    assert_eq!(tx.inputs, vec![123_456_789]);
    // Conservation: everything is either a denomination, a part fee, or
    // a splitting fee.
    let denomination_sum: u64 = plan.parts.iter().sum();
    assert_eq!(
        123_456_789,
        denomination_sum + plan.parts_fee(&params) + plan.split_fee() + plan.residual
    );
}

#[test]
fn fragmented_wallet_consolidates_in_rounds() {
    // 500 notes of 0.0015 ZEC: consolidation (500 → 16 at a budget of 32)
    // then sizing.
    let params = params();
    let notes = vec![150_000u64; 500];
    let plan = assert_plan_executes(&notes, &params);
    assert!(
        plan.split_rounds.len() >= 2,
        "expected consolidation + sizing rounds"
    );
    // First round: one merge per full-or-partial chunk of the spend budget.
    let max_notes = side_budget(&params);
    assert_eq!(plan.split_rounds[0].len(), 500usize.div_ceil(max_notes));
    for tx in &plan.split_rounds[0] {
        assert!(tx.inputs.len() <= max_notes);
        assert_eq!(tx.outputs.len(), 1);
    }
    assert!(!plan.parts.is_empty());
}

#[test]
fn whale_balance_splits_through_intermediates() {
    // 500 000 ZEC in one note: ~50 denominations of 10 000 ZEC.
    let params = params();
    let notes = vec![500_000 * COIN];
    let plan = assert_plan_executes(&notes, &params);
    assert!(plan.parts.len() >= 49);
    for round in &plan.split_rounds {
        for tx in round {
            assert!(tx.outputs.len() <= side_budget(&params));
        }
    }
}

#[test]
fn very_large_balance_recurses_splitting() {
    // 12,000,000 ZEC: ~1,200 targets of 10 000 ZEC → recursive splitting
    // through intermediate levels, within the budget throughout.
    let params = params();
    let notes = vec![12_000_000 * COIN];
    let plan = assert_plan_executes(&notes, &params);
    assert!(plan.parts.len() > params.max_actions_per_split_tx);
    assert!(plan.split_rounds.len() >= 3);
}

#[test]
fn mixed_wallet_leaves_ready_notes_untouched() {
    // One part-ready note + fragments: the ready note is never spent by
    // note splitting (verified by assert_plan_executes) and its
    // denomination appears in the parts.
    let params = params();
    let mut notes = vec![10 * COIN + params.part_fee];
    notes.extend(vec![200_000u64; 50]);
    let plan = assert_plan_executes(&notes, &params);
    assert!(plan.parts.contains(&(10 * COIN)));
}

proptest! {
    // Fundamental invariant: the plan executes (round linkage holds, no
    // dust created, ZIP-317 fees respected, ends exactly part-ready)
    // and value is conserved across the whole migration.
    #[test]
    fn plan_executes_and_conserves_value(
        notes in proptest::collection::vec(1u64..=10_000_000_000, 0..300)
    ) {
        let params = params();
        let plan = assert_plan_executes(&notes, &params);
        let total: u64 = notes.iter().sum();
        let denomination_sum: u64 = plan.parts.iter().sum();
        prop_assert_eq!(
            total,
            denomination_sum + plan.parts_fee(&params) + plan.split_fee() + plan.residual
        );
    }

    // Small-note-heavy wallets (the fragmentation case the consolidation
    // rounds exist for).
    #[test]
    fn fragmented_plans_execute(
        notes in proptest::collection::vec(10_001u64..=500_000, 100..400)
    ) {
        let params = params();
        let plan = assert_plan_executes(&notes, &params);
        let total: u64 = notes.iter().sum();
        let denomination_sum: u64 = plan.parts.iter().sum();
        prop_assert_eq!(
            total,
            denomination_sum + plan.parts_fee(&params) + plan.split_fee() + plan.residual
        );
        // log_K bound: 400 notes at a budget of 32 → one consolidation round
        // plus sizing.
        prop_assert!(plan.split_rounds.len() <= 4);
    }

    // The per-transaction budget is honored whatever its value. The
    // per-tx asserts inside `assert_plan_executes` enforce it.
    #[test]
    fn planner_honors_any_action_budget(
        notes in proptest::collection::vec(10_001u64..=500_000, 0..200),
        budget in prop_oneof![Just(8usize), Just(32usize), Just(100usize)],
    ) {
        let mut params = params();
        params.max_actions_per_split_tx = budget;
        assert_plan_executes(&notes, &params);
    }
}
