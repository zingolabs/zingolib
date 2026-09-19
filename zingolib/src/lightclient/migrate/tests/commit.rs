use zcash_primitives::transaction::TxId;
use zip32::AccountId;

use super::fixtures::{
    DUST_NOTE, FUNDING_NOTE, RESIDUAL_NOTE, SEED, TIP, UNPREPARED_NOTE, bound_note_of,
    committed_client, migration_error, migration_of, phase_of, scheduled_plan, scheduled_state,
    sorted, unspent_v2_output_ids, wallet_with_one_confirmed_transfer,
};
use crate::lightclient::LightClient;
use crate::lightclient::error::MigrationError;
use crate::lightclient::migrate::{MigrationPlan, MigrationProgress};
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::migration::{MigrationMode, MigrationParams, MigrationPhase, plan_hash};

#[tokio::test]
async fn commit_migration_accepts_the_plan_just_returned() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .orchard_note(RESIDUAL_NOTE)
        .tip(TIP)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;
    let MigrationPlan::Scheduled(scheduled) = &plan else {
        panic!("scheduled mode plans a scheduled plan");
    };
    assert!(scheduled.is_prepared(), "premise: nothing to prepare");
    assert_eq!(
        scheduled.residual, RESIDUAL_NOTE,
        "the residual note is booked as residual"
    );

    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("the plan just returned commits over the unchanged wallet");

    let state = migration_of(&client).await;
    assert_eq!(state.commitment.plan_hash, plan_hash(scheduled));
    assert_eq!(state.mode, MigrationMode::Scheduled);
    assert_eq!(state.account, AccountId::ZERO);
    assert!(
        state.transfers.is_empty(),
        "no transfer is bound before the schedule is committed"
    );
}

#[tokio::test]
async fn commit_migration_refuses_a_plan_over_changed_notes() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .tip(TIP)
        .build();
    let bound = bound_note_of(&wallet, FUNDING_NOTE);
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;

    super::fixtures::mark_note_spent_by(
        &mut *client.wallet().write().await,
        &bound,
        TxId::from_bytes([9; 32]),
    );

    let result = client.commit_migration(AccountId::ZERO, &plan).await;
    assert!(
        matches!(migration_error(result), MigrationError::PlanMismatch),
        "a plan over notes that changed is refused"
    );
    assert!(
        client.wallet().read().await.migration.is_none(),
        "a refusal writes nothing"
    );
}

#[tokio::test]
async fn commit_migration_refuses_an_immediate_plan() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .tip(TIP)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = client
        .plan_migration(AccountId::ZERO, MigrationMode::Immediate)
        .await
        .expect("planning is pure");

    let result = client.commit_migration(AccountId::ZERO, &plan).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::WrongPlanMode
    ));
    assert!(client.wallet().read().await.migration.is_none());
}

#[tokio::test]
async fn commit_migration_refuses_while_a_migration_is_in_progress() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .tip(TIP)
        .build();
    let mut client = committed_client(wallet).await;
    let before = migration_of(&client).await;
    let plan = scheduled_plan(&client).await;

    let result = client.commit_migration(AccountId::ZERO, &plan).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::AlreadyInProgress
    ));
    assert_eq!(
        migration_of(&client).await,
        before,
        "the refusal leaves the committed migration untouched"
    );
}

#[tokio::test]
async fn commit_migration_replaces_a_completed_migration() {
    let mut wallet = wallet_with_one_confirmed_transfer(TIP, Some(FUNDING_NOTE));
    wallet
        .migration
        .as_mut()
        .expect("the fixture carries a migration")
        .phase = MigrationPhase::Complete {
        residual: FUNDING_NOTE,
    };
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;
    let MigrationPlan::Scheduled(scheduled) = &plan else {
        panic!("scheduled mode plans a scheduled plan");
    };
    assert_eq!(
        scheduled.transfers,
        vec![super::fixtures::NOTE_VALUE],
        "premise: the leftover funding note plans a fresh transfer"
    );

    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("a completed migration is history and is replaced");

    let state = migration_of(&client).await;
    assert_eq!(state.phase, MigrationPhase::Prepared);
    assert!(state.transfers.is_empty(), "the old transfers are gone");
    assert_eq!(state.commitment.plan_hash, plan_hash(scheduled));
}

#[tokio::test]
async fn commit_migration_of_a_plan_without_preparation_rounds_lands_in_prepared() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .tip(TIP)
        .build();
    let client = committed_client(wallet).await;
    assert_eq!(phase_of(&client).await, MigrationPhase::Prepared);
}

#[tokio::test]
async fn commit_migration_refuses_a_plan_with_nothing_to_migrate() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(DUST_NOTE)
        .tip(TIP)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;

    let result = client.commit_migration(AccountId::ZERO, &plan).await;

    assert!(
        matches!(
            result,
            Err(crate::lightclient::error::LightClientError::WalletError(
                crate::wallet::error::WalletError::NothingToMigrate
            ))
        ),
        "a plan with no transfers and no rounds is refused, got {result:?}"
    );
    assert!(client.wallet().read().await.migration.is_none());
}

#[tokio::test]
async fn commit_migration_of_a_plan_with_preparation_rounds_lands_in_committed() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;
    let MigrationPlan::Scheduled(scheduled) = &plan else {
        panic!("scheduled mode plans a scheduled plan");
    };
    assert!(
        !scheduled.is_prepared(),
        "premise: the note needs preparation"
    );

    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("an unprepared plan commits");

    let state = migration_of(&client).await;
    assert_eq!(state.phase, MigrationPhase::Committed);
    assert!(state.transfers.is_empty());
}

#[tokio::test]
async fn commit_migration_reserves_every_orchard_note_of_the_account() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .orchard_note(RESIDUAL_NOTE)
        .orchard_note(UNPREPARED_NOTE)
        .ironwood_note(77_777)
        .tip(TIP)
        .build();
    let expected = unspent_v2_output_ids(&wallet);
    assert_eq!(expected.len(), 3, "premise: three pre-Ironwood notes");
    let mut client = LightClient::new_for_test(wallet).await;
    assert!(
        client
            .wallet()
            .read()
            .await
            .reserved_output_ids()
            .is_empty(),
        "nothing is reserved before the commit"
    );
    let plan = scheduled_plan(&client).await;

    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("the plan commits");

    let wallet = client.wallet().read().await;
    assert_eq!(
        sorted(wallet.reserved_output_ids()),
        expected,
        "every unspent V2 Orchard note of the account is reserved after the commit"
    );
    let summaries = wallet.note_summaries::<pepper_sync::wallet::OrchardNote>(false);
    assert!(
        summaries.iter().all(|note| note.reserved),
        "every Orchard note summary carries the reservation"
    );
    let ironwood = wallet.note_summaries::<pepper_sync::wallet::IronwoodNote>(false);
    assert!(
        ironwood.iter().all(|note| !note.reserved),
        "an Ironwood note is never reserved"
    );
}

#[tokio::test]
async fn commit_migration_broadcasts_nothing() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let transactions_before = wallet.wallet_transactions.len();
    let client = committed_client(wallet).await;

    let wallet = client.wallet().read().await;
    assert_eq!(
        wallet.wallet_transactions.len(),
        transactions_before,
        "no transaction is built"
    );
    assert!(
        wallet
            .wallet_transactions
            .values()
            .all(|transaction| transaction.status().is_confirmed()),
        "every wallet transaction is the fabricated confirmed one"
    );
    assert_eq!(
        *client.migration_progress().borrow(),
        MigrationProgress::Idle
    );
}

#[tokio::test]
async fn commit_migration_records_the_provisional_parameters() {
    let wallet = super::fixtures::wallet_with_funding_notes(TIP, 1);
    let params = MigrationParams::provisional(wallet.chain_type());
    let client = committed_client(wallet).await;

    let state = migration_of(&client).await;
    assert_eq!(state.params, params);
    assert_eq!(state.commitment.params_hash, params.params_hash());
    assert!(
        state.commitment.committed_at > 0,
        "the commit time is recorded"
    );
}

#[tokio::test]
async fn commit_migration_refuses_while_a_scheduled_migration_holds_transfers() {
    let (mut wallet, bound_note) = super::fixtures::wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    wallet.migration = Some(scheduled_state(
        params,
        vec![super::fixtures::assigned_transfer(0, bound_note, 0)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;

    let result = client.commit_migration(AccountId::ZERO, &plan).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::AlreadyInProgress
    ));
}

#[tokio::test]
async fn commit_migration_completes_a_settled_migration_before_replacing_it() {
    let wallet = wallet_with_one_confirmed_transfer(TIP, Some(FUNDING_NOTE));
    assert_eq!(
        wallet
            .migration
            .as_ref()
            .expect("the fixture carries a migration")
            .phase,
        MigrationPhase::Scheduled,
        "premise: the settled migration was never marked complete"
    );
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;
    let MigrationPlan::Scheduled(scheduled) = &plan else {
        panic!("scheduled mode plans a scheduled plan");
    };

    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("reconciliation completes the settled migration, so the commit replaces it");

    let state = migration_of(&client).await;
    assert_eq!(state.phase, MigrationPhase::Prepared);
    assert_eq!(state.commitment.plan_hash, plan_hash(scheduled));
    assert!(
        state.transfers.is_empty(),
        "the settled transfers are history"
    );
    let status = client.migration_status().await.expect("status reads");
    assert_eq!(status.transfers_total, 1);
    assert_eq!(
        status.value_migrated, 0,
        "the new migration starts from nothing"
    );
}
