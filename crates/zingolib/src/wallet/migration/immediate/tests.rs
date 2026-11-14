use super::*;
use crate::config::ChainType;
use crate::wallet::migration::split::{CANONICAL_PART_FEE, MARGINAL_FEE, SWEEP_MIN};

/// The provisional parameter set every test plans under, the same set
/// [`crate::wallet::LightWallet::plan_immediate_migration`] resolves.
fn params() -> MigrationParams {
    MigrationParams::provisional(ChainType::Mainnet)
}

/// An immediate migration's Orchard bundle has no outputs, so `orchard_v3`'s ban on
/// cross-address transfers cannot change its action count. That is what
/// lets `immediate_migration_fee` ignore the activation era that `note_split_fee` has to
/// take as an argument.
#[test]
fn immediate_migration_fee_is_era_independent() {
    for n_in in [1usize, 2, 3, 5, side_budget(&params())] {
        assert_eq!(
            bundle_actions(BundleVersion::orchard_v2(), n_in, 0),
            bundle_actions(BundleVersion::orchard_v3(), n_in, 0),
            "a spend-only Orchard bundle changed action count across the NU6.3 boundary"
        );
    }
}

/// A one-input immediate migration transaction is exactly a ZIP 318 part: same two bundles, same
/// padding. The two fee models must agree, or one of them is wrong.
#[test]
fn single_input_immediate_tx_costs_a_canonical_part_fee() {
    assert_eq!(immediate_migration_fee(1), CANONICAL_PART_FEE);
    assert_eq!(immediate_migration_fee(2), CANONICAL_PART_FEE);
    // Beyond the bundle minimum each extra spend costs one marginal fee.
    assert_eq!(
        immediate_migration_fee(3),
        CANONICAL_PART_FEE + MARGINAL_FEE
    );
}

#[test]
fn plan_conserves_value() {
    let cases: [&[u64]; 5] = [
        &[],
        &[100_000],
        &[100_000, 200_000, 300_000],
        // A mix of dust and spendable.
        &[1, 5_000, 10_000, 100_000, 1_000_000],
        // All dust.
        &[1, 2, 3],
    ];
    for note_values in cases {
        let plan = plan_immediate_migration(note_values, &params());
        let total: u64 = note_values.iter().sum();
        assert_eq!(
            plan.migrated + plan.fee + plan.residual,
            total,
            "plan lost value for {note_values:?}: {plan:?}"
        );
        for transaction in &plan.transactions {
            assert_eq!(
                transaction.fee(),
                immediate_migration_fee(transaction.inputs.len())
            );
        }
    }
}

#[test]
fn chunks_at_the_action_bound() {
    let params = params();
    let max = side_budget(&params);
    for (note_count, expected_txs) in [
        (0, 0),
        (1, 1),
        (max, 1),
        (max + 1, 2),
        (max * 2, 2),
        (max * 2 + 1, 3),
    ] {
        let note_values = vec![1_000_000u64; note_count];
        let plan = plan_immediate_migration(&note_values, &params);
        assert_eq!(
            plan.transactions.len(),
            expected_txs,
            "{note_count} notes should chunk into {expected_txs} transaction(s)"
        );
        assert!(
            plan.transactions.iter().all(|transaction| {
                let actions = transaction.inputs.len() + 1;
                actions <= params.max_actions_per_split_tx
            }),
            "a chunk's spends and single output together exceeded the total action budget"
        );
    }
}

#[test]
fn all_dust_leaves_everything_residual() {
    let note_values = vec![SWEEP_MIN, SWEEP_MIN - 1, 1];
    let plan = plan_immediate_migration(&note_values, &params());

    assert!(plan.is_empty());
    assert_eq!(plan.migrated, 0);
    assert_eq!(plan.fee, 0);
    assert_eq!(plan.residual, note_values.iter().sum::<u64>());
}

/// The selection boundary is strict: a note worth exactly `sweep_min` is
/// residual, one zatoshi more is migrated. Fails whenever the migration's
/// residual filter admits a note at or below the threshold.
#[test]
fn notes_at_or_below_sweep_min_are_never_migrated() {
    let dust = [1, SWEEP_MIN - 1, SWEEP_MIN];
    let mut note_values = dust.to_vec();
    note_values.extend([SWEEP_MIN + 1, 1_000_000]);

    let plan = plan_immediate_migration(&note_values, &params());
    assert_eq!(plan.residual, dust.iter().sum::<u64>());
    for transaction in &plan.transactions {
        for &input in &transaction.inputs {
            assert!(
                input > SWEEP_MIN,
                "migration spends a {input}-zatoshi note, at or below sweep_min ({})",
                SWEEP_MIN
            );
        }
    }
    // The note one zatoshi above the boundary is selected.
    assert!(
        plan.transactions
            .iter()
            .any(|tx| tx.inputs.contains(&(SWEEP_MIN + 1)))
    );
}

/// The floor applies to what an immediate migration creates as well as what it spends: a
/// chunk whose output would land at or below `sweep_min` is residual
/// whole, because the immediate migration would pay its fee to manufacture an Ironwood
/// note the residual policy itself refuses to spend. The boundary is
/// strict: an output of exactly `sweep_min` leaves its chunk as residual, one
/// zatoshi more migrates. The window only opens for chunks of at most
/// three near-`sweep_min` notes. From four notes up, a chunk's value
/// outruns its fee by more than the floor.
#[test]
fn output_at_or_below_sweep_min_leaves_the_chunk_residual() {
    // One 25_000-zatoshi note: the fee is 20_000, so the output would be
    // 5_000, at most the sweep minimum.
    let plan = plan_immediate_migration(&[25_000], &params());
    assert!(plan.is_empty());
    assert_eq!(plan.residual, 25_000);
    assert_eq!(plan.fee, 0);

    // An output of exactly `sweep_min` still leaves its chunk as residual.
    let boundary = immediate_migration_fee(1) + SWEEP_MIN;
    let plan = plan_immediate_migration(&[boundary], &params());
    assert!(plan.is_empty());
    assert_eq!(plan.residual, boundary);

    // One zatoshi above the boundary migrates.
    let plan = plan_immediate_migration(&[boundary + 1], &params());
    assert_eq!(plan.transactions.len(), 1);
    assert_eq!(plan.transactions[0].output, SWEEP_MIN + 1);
    assert_eq!(plan.residual, 0);
}

proptest::proptest! {
    /// An immediate migration has no intermediates (every planned input is a wallet
    /// note), so the selection invariant is exact: whatever the wallet
    /// shape, no migrated input is worth at most `sweep_min`.
    #[test]
    fn immediate_migration_inputs_always_exceed_sweep_min(
        note_values in proptest::collection::vec(1u64..=10_000_000_000, 0..300)
    ) {            let plan = plan_immediate_migration(&note_values, &params());
        for transaction in &plan.transactions {
            for &input in &transaction.inputs {
                proptest::prop_assert!(input > SWEEP_MIN);
            }
        }
    }

    /// The creation-side counterpart: whatever the wallet shape, every
    /// planned Ironwood output exceeds `sweep_min`, so an immediate migration never
    /// manufactures a note the policy refuses to spend.
    #[test]
    fn immediate_migration_outputs_always_exceed_sweep_min(
        note_values in proptest::collection::vec(1u64..=10_000_000_000, 0..300)
    ) {            let plan = plan_immediate_migration(&note_values, &params());
        for transaction in &plan.transactions {
            proptest::prop_assert!(transaction.output > SWEEP_MIN);
        }
    }
}

/// An immediate migration re-planned after some of its notes were spent covers exactly
/// the notes that are still free. This is what makes re-calling the immediate migration
/// the recovery path after a partial transmission.
#[test]
fn replanning_covers_only_the_remaining_notes() {
    let all = vec![1_000_000u64, 2_000_000, 3_000_000, 4_000_000];
    let full = plan_immediate_migration(&all, &params());

    // The first two notes were spent by an immediate migration that did transmit.
    let remaining = &all[2..];
    let replan = plan_immediate_migration(remaining, &params());

    assert_eq!(replan.transactions.len(), 1);
    assert_eq!(replan.transactions[0].inputs, remaining.to_vec());
    assert!(replan.migrated < full.migrated);
    assert_eq!(
        replan.migrated + replan.fee + replan.residual,
        remaining.iter().sum::<u64>()
    );
}
