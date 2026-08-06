//! The immediate, non-private migration path: move the Orchard pool into
//! Ironwood in one round of transactions.
//!
//! ZIP 318 offers the user two options at the migration entry point:
//! *migrate with privacy*, the scheduled two-phase flow implemented in
//! [`super::split`] and [`super::parts`], and *migrate immediately*, a single
//! transfer with no delay and minimal privacy. This module is the second.
//!
//! A **immediate migration transaction** spends pre-Ironwood (V2) Orchard notes and creates
//! exactly one Ironwood output, wallet-internal, with no change. A ZIP 318 part
//! looks the same, except that an immediate migration takes many inputs rather than one and
//! its output carries the notes' real value rather than a canonical
//! denomination. That value is precisely what makes it non-private: the amount
//! crossing the pool boundary is visible on-chain and does not collide with
//! anyone else's.
//!
//! Immediate migration transactions do not depend on one another. There is no change output and no
//! conditioning round, so when an account holds more notes than fit in one
//! transaction the plan simply chunks them, and every chunk is built and
//! broadcast in the same pass. There are no rounds to drive and nothing to
//! wait for.

use orchard::bundle::BundleVersion;
use zcash_primitives::transaction::TxId;

use super::params::MigrationParams;
use super::split::{MigrationOutputs, bundle_actions, side_budget, zip317_fee};
use crate::wallet::error::WalletError;

/// The ZIP-317 conventional fee of a immediate migration transaction with `n_in` Orchard
/// spends.
///
/// An immediate migration carries two bundles: an Orchard bundle (`n_in` spends, no outputs)
/// and an Ironwood bundle (one output, no spends). Both the padding and the
/// fee come from the crates.
///
/// Unlike [`super::split::note_split_fee`] this is era-independent: the Orchard
/// bundle has no outputs, so whether `orchard_v3` permits a spend and an output
/// to share an action makes no difference to the count (pinned by test).
fn immediate_migration_fee(n_in: usize) -> u64 {
    zip317_fee(
        bundle_actions(BundleVersion::orchard_v3(), n_in, 0),
        bundle_actions(BundleVersion::ironwood_v3(), 0, 1),
    )
}

/// One planned immediate migration transaction. All values are zatoshis. The fee is implied:
/// `sum(inputs) − output`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImmediateMigrationTx {
    /// Values of the Orchard notes this transaction spends.
    pub inputs: Vec<u64>,
    /// Value of the single Ironwood output.
    pub output: u64,
}

impl ImmediateMigrationTx {
    /// The implied fee: `sum(inputs) − output`.
    pub fn fee(&self) -> u64 {
        self.inputs.iter().sum::<u64>() - self.output
    }
}

/// A complete immediate migration plan: every transaction needed to move the account's
/// spendable Orchard balance into Ironwood. Pure data, nothing is signed or
/// sent.
///
/// `migrated + fee + residual` always equals the account's spendable Orchard
/// balance: a plan never loses value silently.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImmediateMigrationPlan {
    /// The transactions to build, each independent of the others.
    pub transactions: Vec<ImmediateMigrationTx>,
    /// Total value that will land in the Ironwood pool, in zatoshis.
    pub migrated: u64,
    /// Total fees across every transaction, in zatoshis.
    pub fee: u64,
    /// Value left behind in the Orchard pool because moving it is not
    /// worthwhile, in zatoshis: notes worth at most the sweep minimum, plus
    /// chunks whose output would not exceed it.
    pub residual: u64,
    /// The eligible broadcast set for the migrate-now path, filled by the client layer.
    pub broadcast_targets: Vec<super::BroadcastTarget>,
}

impl ImmediateMigrationPlan {
    /// True when there is nothing to send.
    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }
}

/// Plans an immediate migration of the Orchard notes with the given values (zatoshis): spend
/// every note worth more than the [`MigrationParams::sweep_min`] Sweep Minimum
/// into one Ironwood output, chunked so each transaction's spends beside its
/// single output fit the [`MigrationParams::max_actions_per_split_tx`] total
/// budget — the same 16-action shape the preparation transactions carry, via
/// the same [`side_budget`] law. The wallet thereby emits one transaction
/// shape family across both migration paths; the fee a larger chunk would
/// save is small even on a very fragmented wallet (the divergence ledger's
/// "retained local" section records the choice). Deterministic and pure.
///
/// Notes worth at most the Sweep Minimum are left as residual rather than selected (see the
/// Sweep Minimum's safety-factor policy). The same floor applies to what a
/// migration creates: a chunk whose output would not exceed the Sweep Minimum is
/// left whole as residual, so an immediate migration never manufactures a note the policy refuses to
/// spend.
pub(crate) fn plan_immediate_migration(
    note_values: &[u64],
    params: &MigrationParams,
) -> ImmediateMigrationPlan {
    let mut plan = ImmediateMigrationPlan::default();
    let sweep_min = params.sweep_min;

    let (spendable, dust): (Vec<u64>, Vec<u64>) =
        note_values.iter().partition(|&&value| value > sweep_min);
    plan.residual = dust.iter().sum();

    for chunk in spendable.chunks(side_budget(params)) {
        let total: u64 = chunk.iter().sum();
        let fee = immediate_migration_fee(chunk.len());
        match total.checked_sub(fee) {
            Some(output) if output > sweep_min => {
                plan.migrated += output;
                plan.fee += fee;
                plan.transactions.push(ImmediateMigrationTx {
                    inputs: chunk.to_vec(),
                    output,
                });
            }
            // A chunk that cannot fund both its fee and an output worth
            // spending is left whole as residual: an Ironwood note at or below
            // the Sweep Minimum is one the residual policy itself refuses. Only
            // reachable when a chunk holds at most three near-`SWEEP_MIN`
            // notes, which the entry filter cannot rule out for small
            // chunks.
            _ => plan.residual += total,
        }
    }

    plan
}

impl crate::wallet::LightWallet {
    /// Plans an immediate migration from the account's spendable pre-Ironwood (V2) Orchard
    /// notes. Pure and deterministic: nothing is signed or sent, so the plan
    /// can be shown to the user for consent first.
    ///
    /// Plans under the provisional [`MigrationParams`] for the wallet's chain:
    /// an immediate migration is planned fresh on every call and records no
    /// consent hash, so no stored parameter set can bind it.
    #[allow(clippy::result_large_err)]
    pub(crate) fn plan_immediate_migration(
        &self,
        account: zip32::AccountId,
    ) -> Result<ImmediateMigrationPlan, WalletError> {
        Ok(plan_immediate_migration(
            &self.migration_note_values(account)?,
            &MigrationParams::provisional(self.chain_type()),
        ))
    }

    /// Builds, proves, signs and records one planned immediate migration transaction
    /// (Orchard→Ironwood). Returns its txid. Broadcast is the caller's step.
    ///
    /// Errors below the NU6.3 activation height: there is no Ironwood pool to
    /// send to.
    #[allow(clippy::result_large_err)]
    pub(crate) fn build_immediate_migration_transaction(
        &mut self,
        account: zip32::AccountId,
        planned: &ImmediateMigrationTx,
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
    /// the recovery path after a partial broadcast.
    #[test]
    fn replanning_covers_only_the_remaining_notes() {
        let all = vec![1_000_000u64, 2_000_000, 3_000_000, 4_000_000];
        let full = plan_immediate_migration(&all, &params());

        // The first two notes were spent by an immediate migration that did broadcast.
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
}
