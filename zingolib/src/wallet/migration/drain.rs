//! The immediate, non-private migration path: drain the Orchard pool into
//! Ironwood in one round of transactions.
//!
//! ZIP 318 offers the user two options at the migration entry point:
//! *migrate with privacy*, the scheduled two-phase flow implemented in
//! [`super::split`] and [`super::parts`], and *migrate immediately*, a single
//! transfer with no delay and minimal privacy. This module is the second.
//!
//! A **drain transaction** spends pre-Ironwood (V2) Orchard notes and creates
//! exactly one Ironwood output, wallet-internal, with no change. A ZIP 318 part
//! looks the same, except that a drain takes many inputs rather than one and
//! its output carries the notes' real value rather than a canonical
//! denomination. That value is precisely what makes it non-private: the amount
//! crossing the pool boundary is visible on-chain and does not collide with
//! anyone else's.
//!
//! Drains do not depend on one another. There is no change output and no
//! conditioning round, so when an account holds more notes than fit in one
//! transaction the plan simply chunks them, and every chunk is built and
//! broadcast in the same pass. There are no rounds to drive and nothing to
//! wait for.

use orchard::bundle::BundleVersion;
use zcash_primitives::transaction::TxId;

use super::params::MigrationParams;
use super::split::{MigrationOutputs, bundle_actions, zip317_fee};
use crate::wallet::error::WalletError;

/// The ZIP-317 conventional fee of a drain transaction with `n_in` Orchard
/// spends.
///
/// A drain carries two bundles: an Orchard bundle (`n_in` spends, no outputs)
/// and an Ironwood bundle (one output, no spends). Both the padding and the
/// fee come from the crates.
///
/// Unlike [`super::split::note_split_fee`] this is era-independent: the Orchard
/// bundle has no outputs, so whether `orchard_v3` permits a spend and an output
/// to share an action makes no difference to the count (pinned by test).
pub fn drain_fee(n_in: usize) -> u64 {
    zip317_fee(
        bundle_actions(BundleVersion::orchard_v3(), n_in, 0),
        bundle_actions(BundleVersion::ironwood_v3(), 0, 1),
    )
}

/// One planned drain transaction. All values are zatoshis. The fee is implied:
/// `sum(inputs) − output`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainTx {
    /// Values of the Orchard notes this transaction spends.
    pub inputs: Vec<u64>,
    /// Value of the single Ironwood output.
    pub output: u64,
}

impl DrainTx {
    /// The implied fee: `sum(inputs) − output`.
    pub fn fee(&self) -> u64 {
        self.inputs.iter().sum::<u64>() - self.output
    }
}

/// A complete drain plan: every transaction needed to move the account's
/// spendable Orchard balance into Ironwood. Pure data, nothing is signed or
/// sent.
///
/// `migrated + fee + stranded` always equals the account's spendable Orchard
/// balance: a plan never loses value silently.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DrainPlan {
    /// The transactions to build, each independent of the others.
    pub transactions: Vec<DrainTx>,
    /// Total value that will land in the Ironwood pool, in zatoshis.
    pub migrated: u64,
    /// Total fees across every transaction, in zatoshis.
    pub fee: u64,
    /// Value left behind in the Orchard pool because moving it is not
    /// worthwhile, in zatoshis: notes worth at most the sweep minimum, plus
    /// chunks whose output would not exceed it.
    pub stranded: u64,
}

impl DrainPlan {
    /// True when there is nothing to send.
    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }
}

/// Plans a drain of the Orchard notes with the given values (zatoshis).
/// Deterministic and pure.
///
/// Notes worth at most [`MigrationParams::sweep_min`] are stranded rather
/// than selected (see that field's safety-factor policy). The rest are
/// chunked into groups of at most
/// [`MigrationParams::max_actions_per_split_tx`], one transaction each. The
/// same floor applies to what a drain creates: a chunk whose output would
/// not exceed `sweep_min` is stranded whole, so a drain never manufactures a
/// note the policy refuses to spend.
pub fn plan_drain(note_values: &[u64], params: &MigrationParams) -> DrainPlan {
    let mut plan = DrainPlan::default();

    let (spendable, dust): (Vec<u64>, Vec<u64>) = note_values
        .iter()
        .partition(|&&value| value > params.sweep_min);
    plan.stranded = dust.iter().sum();

    for chunk in spendable.chunks(params.max_actions_per_split_tx.max(1)) {
        let total: u64 = chunk.iter().sum();
        let fee = drain_fee(chunk.len());
        match total.checked_sub(fee) {
            Some(output) if output > params.sweep_min => {
                plan.migrated += output;
                plan.fee += fee;
                plan.transactions.push(DrainTx {
                    inputs: chunk.to_vec(),
                    output,
                });
            }
            // A chunk that cannot fund both its fee and an output worth
            // spending is stranded whole: an Ironwood note at or below
            // `sweep_min` is one the stranding policy itself refuses. Only
            // reachable when a chunk holds at most three near-`sweep_min`
            // notes, which the entry filter cannot rule out for small
            // chunks.
            _ => plan.stranded += total,
        }
    }

    plan
}

impl crate::wallet::LightWallet {
    /// Plans a drain from the account's spendable pre-Ironwood (V2) Orchard
    /// notes. Pure and deterministic: nothing is signed or sent, so the plan
    /// can be shown to the user for consent first.
    #[allow(clippy::result_large_err)]
    pub fn plan_drain(
        &self,
        account: zip32::AccountId,
        params: &MigrationParams,
    ) -> Result<DrainPlan, WalletError> {
        Ok(plan_drain(&self.migration_note_values(account)?, params))
    }

    /// Builds, proves, signs and records one planned drain transaction
    /// (Orchard→Ironwood). Returns its txid. Broadcast is the caller's step.
    ///
    /// Errors below the NU6.3 activation height: there is no Ironwood pool to
    /// send to.
    #[allow(clippy::result_large_err)]
    pub fn build_drain_transaction(
        &mut self,
        account: zip32::AccountId,
        planned: &DrainTx,
    ) -> Result<TxId, WalletError> {
        self.build_migration_transaction_inner(
            account,
            &planned.inputs,
            MigrationOutputs::Ironwood(planned.output),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ChainType;
    use crate::wallet::migration::split::{CANONICAL_PART_FEE, MARGINAL_FEE};

    fn params() -> MigrationParams {
        MigrationParams::provisional(ChainType::Mainnet)
    }

    /// A drain's Orchard bundle has no outputs, so `orchard_v3`'s ban on
    /// cross-address transfers cannot change its action count. That is what
    /// lets `drain_fee` ignore the activation era that `note_split_fee` has to
    /// take as an argument.
    #[test]
    fn drain_fee_is_era_independent() {
        for n_in in [1usize, 2, 3, 5, 32] {
            assert_eq!(
                bundle_actions(BundleVersion::orchard_v2(), n_in, 0),
                bundle_actions(BundleVersion::orchard_v3(), n_in, 0),
                "a spend-only Orchard bundle changed action count across the NU6.3 boundary"
            );
        }
    }

    /// A one-input drain is exactly a ZIP 318 part: same two bundles, same
    /// padding. The two fee models must agree, or one of them is wrong.
    #[test]
    fn single_input_drain_costs_a_canonical_part_fee() {
        assert_eq!(drain_fee(1), CANONICAL_PART_FEE);
        assert_eq!(drain_fee(2), CANONICAL_PART_FEE);
        // Beyond the bundle minimum each extra spend costs one marginal fee.
        assert_eq!(drain_fee(3), CANONICAL_PART_FEE + MARGINAL_FEE);
    }

    #[test]
    fn plan_conserves_value() {
        let params = params();
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
            let plan = plan_drain(note_values, &params);
            let total: u64 = note_values.iter().sum();
            assert_eq!(
                plan.migrated + plan.fee + plan.stranded,
                total,
                "plan lost value for {note_values:?}: {plan:?}"
            );
            for transaction in &plan.transactions {
                assert_eq!(transaction.fee(), drain_fee(transaction.inputs.len()));
            }
        }
    }

    #[test]
    fn chunks_at_the_action_bound() {
        let params = params();
        let max = params.max_actions_per_split_tx;
        for (note_count, expected_txs) in [
            (0, 0),
            (1, 1),
            (max, 1),
            (max + 1, 2),
            (max * 2, 2),
            (max * 2 + 1, 3),
        ] {
            let note_values = vec![1_000_000u64; note_count];
            let plan = plan_drain(&note_values, &params);
            assert_eq!(
                plan.transactions.len(),
                expected_txs,
                "{note_count} notes should chunk into {expected_txs} transaction(s)"
            );
            assert!(
                plan.transactions
                    .iter()
                    .all(|transaction| transaction.inputs.len() <= max),
                "a chunk exceeded the action bound"
            );
        }
    }

    #[test]
    fn all_dust_strands_everything() {
        let params = params();
        let note_values = vec![params.sweep_min, params.sweep_min - 1, 1];
        let plan = plan_drain(&note_values, &params);

        assert!(plan.is_empty());
        assert_eq!(plan.migrated, 0);
        assert_eq!(plan.fee, 0);
        assert_eq!(plan.stranded, note_values.iter().sum::<u64>());
    }

    /// The selection boundary is strict: a note worth exactly `sweep_min` is
    /// stranded, one zatoshi more is drained. Fails whenever the drain's
    /// stranding filter admits a note at or below the threshold.
    #[test]
    fn notes_at_or_below_sweep_min_are_never_drained() {
        let params = params();
        let dust = [1, params.sweep_min - 1, params.sweep_min];
        let mut note_values = dust.to_vec();
        note_values.extend([params.sweep_min + 1, 1_000_000]);

        let plan = plan_drain(&note_values, &params);
        assert_eq!(plan.stranded, dust.iter().sum::<u64>());
        for transaction in &plan.transactions {
            for &input in &transaction.inputs {
                assert!(
                    input > params.sweep_min,
                    "drain spends a {input}-zatoshi note, at or below sweep_min ({})",
                    params.sweep_min
                );
            }
        }
        // The note one zatoshi above the boundary is selected.
        assert!(
            plan.transactions
                .iter()
                .any(|tx| tx.inputs.contains(&(params.sweep_min + 1)))
        );
    }

    /// The floor applies to what a drain creates, not only what it spends: a
    /// chunk whose output would land at or below `sweep_min` is stranded
    /// whole, because the drain would pay its fee to manufacture an Ironwood
    /// note the stranding policy itself refuses to spend. The boundary is
    /// strict: an output of exactly `sweep_min` strands its chunk, one
    /// zatoshi more drains. The window only opens for chunks of at most
    /// three near-`sweep_min` notes; from four notes up, a chunk's value
    /// outruns its fee by more than the floor.
    #[test]
    fn output_at_or_below_sweep_min_strands_the_chunk() {
        let params = params();

        // One 25_000-zatoshi note: the fee is 20_000, so the output would be
        // 5_000, at most the sweep minimum.
        let plan = plan_drain(&[25_000], &params);
        assert!(plan.is_empty());
        assert_eq!(plan.stranded, 25_000);
        assert_eq!(plan.fee, 0);

        // An output of exactly `sweep_min` still strands its chunk.
        let boundary = drain_fee(1) + params.sweep_min;
        let plan = plan_drain(&[boundary], &params);
        assert!(plan.is_empty());
        assert_eq!(plan.stranded, boundary);

        // One zatoshi above the boundary drains.
        let plan = plan_drain(&[boundary + 1], &params);
        assert_eq!(plan.transactions.len(), 1);
        assert_eq!(plan.transactions[0].output, params.sweep_min + 1);
        assert_eq!(plan.stranded, 0);
    }

    proptest::proptest! {
        /// A drain has no intermediates — every planned input is a wallet
        /// note — so the selection invariant is exact: whatever the wallet
        /// shape, no drained input is worth at most `sweep_min`.
        #[test]
        fn drained_inputs_always_exceed_sweep_min(
            note_values in proptest::collection::vec(1u64..=10_000_000_000, 0..300)
        ) {
            let params = params();
            let plan = plan_drain(&note_values, &params);
            for transaction in &plan.transactions {
                for &input in &transaction.inputs {
                    proptest::prop_assert!(input > params.sweep_min);
                }
            }
        }

        /// The creation-side counterpart: whatever the wallet shape, every
        /// planned Ironwood output exceeds `sweep_min`, so a drain never
        /// manufactures a note the policy refuses to spend.
        #[test]
        fn drain_outputs_always_exceed_sweep_min(
            note_values in proptest::collection::vec(1u64..=10_000_000_000, 0..300)
        ) {
            let params = params();
            let plan = plan_drain(&note_values, &params);
            for transaction in &plan.transactions {
                proptest::prop_assert!(transaction.output > params.sweep_min);
            }
        }
    }

    /// A drain re-planned after some of its notes were spent covers exactly
    /// the notes that are still free. This is what makes re-calling the drain
    /// the recovery path after a partial broadcast.
    #[test]
    fn replanning_covers_only_the_remaining_notes() {
        let params = params();
        let all = vec![1_000_000u64, 2_000_000, 3_000_000, 4_000_000];
        let full = plan_drain(&all, &params);

        // The first two notes were spent by a drain that did broadcast.
        let remaining = &all[2..];
        let replan = plan_drain(remaining, &params);

        assert_eq!(replan.transactions.len(), 1);
        assert_eq!(replan.transactions[0].inputs, remaining.to_vec());
        assert!(replan.migrated < full.migrated);
        assert_eq!(
            replan.migrated + replan.fee + replan.stranded,
            remaining.iter().sum::<u64>()
        );
    }
}
