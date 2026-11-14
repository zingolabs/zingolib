//! Invariant checks over the wallet's whole record set.

use std::collections::{BTreeSet, HashMap};

use pepper_sync::wallet::{
    IronwoodNote, KeyIdInterface, NoteInterface, OrchardNote, OutputId, OutputInterface,
    SaplingNote, TransparentCoin,
};
use zcash_primitives::transaction::TxId;
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
use zcash_protocol::value::Zatoshis;
use zingo_status::confirmation_status::ConfirmationStatus;

use super::LightWallet;
use super::balance::AccountBalance;

impl LightWallet {
    /// Every violated invariant, by description. Empty means the wallet holds.
    #[must_use]
    pub fn invariant_violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        let pending_spends = pepper_sync::pending_spends(&self.wallet_transactions);

        check_outputs::<TransparentCoin>(self, &pending_spends, &mut violations);
        check_outputs::<SaplingNote>(self, &pending_spends, &mut violations);
        check_outputs::<OrchardNote>(self, &pending_spends, &mut violations);
        check_outputs::<IronwoodNote>(self, &pending_spends, &mut violations);
        check_confirmed_spends_recorded::<SaplingNote>(self, &mut violations);
        check_confirmed_spends_recorded::<OrchardNote>(self, &mut violations);
        check_confirmed_spends_recorded::<IronwoodNote>(self, &mut violations);
        check_confirmed_spends_recorded::<TransparentCoin>(self, &mut violations);
        check_balances::<SaplingNote>(self, &pending_spends, &mut violations);
        check_balances::<OrchardNote>(self, &pending_spends, &mut violations);
        check_balances::<IronwoodNote>(self, &pending_spends, &mut violations);

        violations
    }
}

/// Panics listing every violated invariant, for tests.
pub fn assert_wallet_invariants(wallet: &LightWallet) {
    let violations = wallet.invariant_violations();
    assert!(
        violations.is_empty(),
        "wallet invariants violated:\n{}",
        violations.join("\n")
    );
}

fn check_outputs<Op: OutputInterface>(
    wallet: &LightWallet,
    pending_spends: &HashMap<OutputId, TxId>,
    violations: &mut Vec<String>,
) {
    let mut seen = BTreeSet::new();
    for transaction in wallet.wallet_transactions.values() {
        for output in Op::transaction_outputs(transaction) {
            let output_id = output.output_id();
            if output_id.txid() != transaction.txid() {
                violations.push(format!(
                    "{:?} output {output_id:?} is stored in the record of {}",
                    Op::POOL_TYPE,
                    transaction.txid()
                ));
            }
            if !seen.insert(output_id) {
                violations.push(format!(
                    "{:?} output {output_id:?} appears in two records",
                    Op::POOL_TYPE
                ));
            }
            let Some(spending_txid) = output.spending_transaction() else {
                continue;
            };
            let Some(spending_record) = wallet.wallet_transactions.get(&spending_txid) else {
                violations.push(format!(
                    "{:?} output {output_id:?} names spending transaction {spending_txid}, which the wallet does not hold",
                    Op::POOL_TYPE
                ));
                continue;
            };
            if spending_record.status().is_failed() {
                violations.push(format!(
                    "{:?} output {output_id:?} is marked spent by failed transaction {spending_txid}",
                    Op::POOL_TYPE
                ));
            }
            if spending_record.status().is_confirmed()
                && output.spend_link().is_some_and(|link| {
                    !Op::transaction_inputs(spending_record)
                        .iter()
                        .any(|input| **input == link)
                })
            {
                violations.push(format!(
                    "{:?} output {output_id:?} is marked spent by confirmed transaction {spending_txid}, which carries none of its spend links",
                    Op::POOL_TYPE
                ));
            }
            if !spending_record.status().is_confirmed()
                && pending_spends.get(&output_id) != Some(&spending_txid)
            {
                violations.push(format!(
                    "{:?} output {output_id:?} is marked spent by pending transaction {spending_txid}, which the derivation does not report",
                    Op::POOL_TYPE
                ));
            }
        }
    }
}

/// Every wallet output whose spend link a confirmed transaction carries must
/// carry the recorded spend of that transaction: the scan that found the block
/// records the spend in the same pass.
fn check_confirmed_spends_recorded<Op: OutputInterface>(
    wallet: &LightWallet,
    violations: &mut Vec<String>,
) {
    for spending_record in wallet
        .wallet_transactions
        .values()
        .filter(|transaction| transaction.status().is_confirmed())
    {
        let inputs = Op::transaction_inputs(spending_record);
        if inputs.is_empty() {
            continue;
        }
        for transaction in wallet.wallet_transactions.values() {
            for output in Op::transaction_outputs(transaction) {
                if output
                    .spend_link()
                    .is_some_and(|link| inputs.contains(&&link))
                    && output.spending_transaction().is_none()
                {
                    violations.push(format!(
                        "confirmed transaction {} spends {:?} output {output_id:?} with no recorded spend",
                        spending_record.txid(),
                        Op::POOL_TYPE,
                        output_id = output.output_id()
                    ));
                }
            }
        }
    }
}

fn check_balances<Op>(
    wallet: &LightWallet,
    pending_spends: &HashMap<OutputId, TxId>,
    violations: &mut Vec<String>,
) where
    Op: OutputInterface + NoteInterface,
    Op::KeyId: KeyIdInterface,
{
    let Some(account) = wallet.unified_key_store.keys().copied().next() else {
        return;
    };
    let (mut confirmed, mut unconfirmed) = (0u64, 0u64);
    for transaction in wallet.wallet_transactions.values() {
        for output in Op::transaction_outputs(transaction) {
            if output.key_id().account_id() != account
                || output.value() <= MARGINAL_FEE.into_u64()
                || output.spending_transaction().is_some()
                || pending_spends.contains_key(&output.output_id())
            {
                continue;
            }
            match transaction.status() {
                ConfirmationStatus::Confirmed(_) => confirmed += output.value(),
                status if status.is_pending() => unconfirmed += output.value(),
                _ => (),
            }
        }
    }
    let Ok(balance) = wallet.account_balance(account) else {
        violations.push("account_balance errored".to_string());
        return;
    };
    let (reported_confirmed, reported_unconfirmed) = balance_lines::<Op>(&balance);
    let pool = format!("{:?}", Op::POOL_TYPE);
    if reported_confirmed.is_some()
        && reported_confirmed != Some(Zatoshis::const_from_u64(confirmed))
    {
        violations.push(format!(
            "{pool} confirmed balance reports {reported_confirmed:?}, the raw notes sum to {confirmed}"
        ));
    }
    if reported_unconfirmed.is_some()
        && reported_unconfirmed != Some(Zatoshis::const_from_u64(unconfirmed))
    {
        violations.push(format!(
            "{pool} unconfirmed balance reports {reported_unconfirmed:?}, the raw notes sum to {unconfirmed}"
        ));
    }
}

fn balance_lines<Op: OutputInterface>(
    balance: &AccountBalance,
) -> (Option<Zatoshis>, Option<Zatoshis>) {
    match Op::POOL_TYPE {
        zcash_protocol::PoolType::SAPLING => (
            balance.confirmed_sapling_balance,
            balance.unconfirmed_sapling_balance,
        ),
        zcash_protocol::PoolType::ORCHARD => (
            balance.confirmed_orchard_balance,
            balance.unconfirmed_orchard_balance,
        ),
        zcash_protocol::PoolType::IRONWOOD => (
            balance.confirmed_ironwood_balance,
            balance.unconfirmed_ironwood_balance,
        ),
        zcash_protocol::PoolType::Transparent => (
            balance.confirmed_transparent_balance,
            balance.unconfirmed_transparent_balance,
        ),
    }
}

#[cfg(test)]
mod tests {
    use zcash_primitives::transaction::TxId;
    use zingo_status::confirmation_status::ConfirmationStatus;

    use pepper_sync::wallet::OutputInterface as _;

    use crate::lightclient::LightClient;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use zingo_test_vectors::seeds;

    async fn funded_client() -> LightClient {
        LightClient::new_for_test(
            SyntheticWalletBuilder::new(seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(100_000)
                .build(),
        )
        .await
    }

    #[tokio::test]
    async fn a_healthy_wallet_has_no_violations() {
        let client = funded_client().await;
        let violations = client.wallet().read().await.invariant_violations();
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[tokio::test]
    async fn a_spend_mark_naming_a_missing_transaction_is_a_violation() {
        let client = funded_client().await;
        {
            let mut wallet = client.wallet().write().await;
            for transaction in wallet.wallet_transactions.values_mut() {
                for note in transaction.orchard_notes_mut() {
                    note.set_spending_transaction(Some(TxId::from_bytes([9; 32])));
                }
            }
        }
        let violations = client.wallet().read().await.invariant_violations();
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("does not hold")),
            "{violations:?}"
        );
    }

    #[tokio::test]
    async fn a_spend_mark_held_by_a_failed_transaction_is_a_violation() {
        let client = funded_client().await;
        let failed = TxId::from_bytes([7; 32]);
        {
            let mut wallet = client.wallet().write().await;
            wallet.wallet_transactions.insert(
                failed,
                pepper_sync::wallet::WalletTransaction::new_for_test(
                    failed,
                    ConfirmationStatus::Failed(5.into()),
                ),
            );
            for transaction in wallet.wallet_transactions.values_mut() {
                if transaction.txid() == failed {
                    continue;
                }
                for note in transaction.orchard_notes_mut() {
                    note.set_spending_transaction(Some(failed));
                }
            }
        }
        let violations = client.wallet().read().await.invariant_violations();
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("failed transaction")),
            "{violations:?}"
        );
    }
}
