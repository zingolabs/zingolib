use zcash_primitives::transaction::TxId;
use zip32::AccountId;

use super::fixtures::{
    DUST_NOTE, FUNDING_NOTE, SEED, TIP, bound_note_of, mark_note_spent_by, migration_error,
    migration_of, scheduled_client, scheduled_plan,
};
use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, MigrationError};
use crate::lightclient::migrate::MigrationPlan;
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::error::WalletError;
use crate::wallet::migration::MigrationMode;

async fn immediate_plan(client: &LightClient) -> MigrationPlan {
    client
        .plan_migration(AccountId::ZERO, MigrationMode::Immediate)
        .await
        .expect("planning is pure")
}

#[tokio::test]
async fn migrate_immediately_refuses_a_scheduled_plan() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .tip(TIP)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;

    let result = client.migrate_immediately(AccountId::ZERO, &plan).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::WrongPlanMode
    ));
    assert!(client.wallet().read().await.migration.is_none());
}

#[tokio::test]
async fn migrate_immediately_refuses_a_plan_over_changed_notes() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .tip(TIP)
        .build();
    let bound = bound_note_of(&wallet, FUNDING_NOTE);
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = immediate_plan(&client).await;
    mark_note_spent_by(
        &mut *client.wallet().write().await,
        &bound,
        TxId::from_bytes([9; 32]),
    );

    let result = client.migrate_immediately(AccountId::ZERO, &plan).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::PlanMismatch
    ));
}

#[tokio::test]
async fn migrate_immediately_refuses_an_empty_plan() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(DUST_NOTE)
        .tip(TIP)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = immediate_plan(&client).await;
    let MigrationPlan::Immediate(immediate) = &plan else {
        panic!("immediate mode plans an immediate plan");
    };
    assert!(immediate.is_empty(), "premise: dust alone migrates nothing");
    assert_eq!(immediate.residual, DUST_NOTE);

    let result = client.migrate_immediately(AccountId::ZERO, &plan).await;
    assert!(
        matches!(
            result,
            Err(LightClientError::WalletError(WalletError::NothingToMigrate))
        ),
        "got {result:?}"
    );
}

#[tokio::test]
async fn migrate_immediately_refuses_while_a_scheduled_migration_exists() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .tip(TIP)
        .build();
    let mut client = scheduled_client(wallet, 1).await;
    let before = migration_of(&client).await;
    let plan = immediate_plan(&client).await;

    let result = client.migrate_immediately(AccountId::ZERO, &plan).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::AlreadyInProgress
    ));
    assert_eq!(
        migration_of(&client).await,
        before,
        "the scheduled migration is untouched"
    );
    let wallet = client.wallet().read().await;
    assert!(
        wallet
            .wallet_transactions
            .values()
            .all(|transaction| transaction.status().is_confirmed()),
        "nothing was built"
    );
}

#[tokio::test]
async fn migrate_immediately_refuses_while_a_committed_migration_exists() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .tip(TIP)
        .build();
    let mut client = super::fixtures::committed_client(wallet).await;
    let plan = immediate_plan(&client).await;

    let result = client.migrate_immediately(AccountId::ZERO, &plan).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::AlreadyInProgress
    ));
}

#[tokio::test]
async fn migrate_immediately_completes_a_settled_migration_before_replacing_it() {
    let wallet = super::fixtures::wallet_with_one_confirmed_transfer(TIP, Some(FUNDING_NOTE));
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = immediate_plan(&client).await;
    let MigrationPlan::Immediate(immediate) = &plan else {
        panic!("immediate mode plans an immediate plan");
    };
    assert!(!immediate.is_empty(), "premise: the leftover note migrates");

    let result = client.migrate_immediately(AccountId::ZERO, &plan).await;

    assert!(
        !matches!(
            result,
            Err(LightClientError::MigrationError(
                MigrationError::AlreadyInProgress
            ))
        ),
        "reconciliation completed the settled migration first, got {result:?}"
    );
    assert!(
        matches!(result, Err(LightClientError::Offline)),
        "the build ran and only the offline transmission failed, got {result:?}"
    );
    let wallet = client.wallet().read().await;
    assert!(
        wallet.migration.is_none(),
        "the completed migration was replaced by the immediate one"
    );
    assert!(
        wallet
            .wallet_transactions
            .values()
            .any(|transaction| transaction.status().is_failed()),
        "the immediate transaction was built and failed on transmit"
    );
}
