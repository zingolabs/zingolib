use std::collections::BTreeSet;

use pepper_sync::wallet::{
    IronwoodNote, KeyIdInterface, NoteInterface, OrchardNote, OutputInterface, SaplingNote,
};
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;

use crate::testutils::mock_indexer::PoolNullifier;
use crate::wallet::LightWallet;

/// The reconciliation invariant: no note the wallet counts as spendable has a
/// nullifier the chain has revealed. `revealed` is the mock chain's ground
/// truth; local data cannot support this check.
#[must_use]
pub fn wallet_reconciliation_violations(
    wallet: &LightWallet,
    revealed: &BTreeSet<PoolNullifier>,
) -> Vec<String> {
    let mut sapling = BTreeSet::new();
    let mut orchard = BTreeSet::new();
    let mut ironwood = BTreeSet::new();
    for pool_nullifier in revealed {
        match pool_nullifier {
            PoolNullifier::Sapling(nullifier) => {
                sapling.insert(nullifier.to_vec());
            }
            PoolNullifier::Orchard(nullifier) => {
                orchard.insert(nullifier.to_bytes().to_vec());
            }
            PoolNullifier::Ironwood(nullifier) => {
                ironwood.insert(nullifier.to_bytes().to_vec());
            }
        }
    }

    let pending_spends = pepper_sync::pending_spends(&wallet.wallet_transactions);
    let mut violations = Vec::new();
    let Some(account) = wallet.unified_key_store.keys().copied().next() else {
        return violations;
    };
    for transaction in wallet.wallet_transactions.values() {
        if transaction.status().is_failed() {
            continue;
        }
        for note in SaplingNote::transaction_outputs(transaction) {
            let counted = note.spending_transaction().is_none()
                && !pending_spends.contains_key(&note.output_id())
                && note.key_id().account_id() == account
                && note.value() > MARGINAL_FEE.into_u64()
                && note
                    .nullifier()
                    .is_some_and(|nullifier| sapling.contains(&nullifier.to_vec()));
            if counted {
                violations.push(format!(
                    "sapling note {:?} is counted spendable while its nullifier is revealed on chain",
                    note.output_id()
                ));
            }
        }
        for note in OrchardNote::transaction_outputs(transaction) {
            let counted = note.spending_transaction().is_none()
                && !pending_spends.contains_key(&note.output_id())
                && note.key_id().account_id() == account
                && note.value() > MARGINAL_FEE.into_u64()
                && note
                    .nullifier()
                    .is_some_and(|nullifier| orchard.contains(nullifier.to_bytes().as_ref()));
            if counted {
                violations.push(format!(
                    "orchard note {:?} is counted spendable while its nullifier is revealed on chain",
                    note.output_id()
                ));
            }
        }
        for note in IronwoodNote::transaction_outputs(transaction) {
            let counted = note.spending_transaction().is_none()
                && !pending_spends.contains_key(&note.output_id())
                && note.key_id().account_id() == account
                && note.value() > MARGINAL_FEE.into_u64()
                && note
                    .nullifier()
                    .is_some_and(|nullifier| ironwood.contains(nullifier.to_bytes().as_ref()));
            if counted {
                violations.push(format!(
                    "ironwood note {:?} is counted spendable while its nullifier is revealed on chain",
                    note.output_id()
                ));
            }
        }
    }
    violations
}
