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
//! transmitted in the same pass. There are no rounds to drive and nothing to
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
    /// (Orchard→Ironwood). Returns its txid. Transmission is the caller's step.
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
mod tests;
