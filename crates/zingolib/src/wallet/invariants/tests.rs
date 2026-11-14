use zcash_primitives::transaction::TxId;
use zl_common::status::ConfirmationStatus;

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
