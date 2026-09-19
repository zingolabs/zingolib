use pepper_sync::wallet::WalletTransaction;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zingo_status::confirmation_status::ConfirmationStatus;
use zip32::AccountId;

use super::fixtures::{
    FUNDING_NOTE, SEED, TIP, UNPREPARED_NOTE, bound_note_of, committed_client, creating_txid,
    mark_note_spent_by, migration_error, migration_of, phase_of, preparing_state, scheduled_client,
    scheduled_plan, wallet_with_funding_notes,
};
use crate::lightclient::LightClient;
use crate::lightclient::error::MigrationError;
use crate::lightclient::migrate::MigrationPlan;
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::migration::{MigrationMode, MigrationParams, MigrationPhase, plan_hash};

fn hash_of(plan: &MigrationPlan) -> [u8; 32] {
    match plan {
        MigrationPlan::Scheduled(scheduled) => plan_hash(scheduled),
        MigrationPlan::Immediate(_) => panic!("a scheduled plan was requested"),
    }
}

fn foreign_txid() -> TxId {
    TxId::from_bytes([9; 32])
}

#[tokio::test]
async fn recommit_migration_refreshes_the_consent_in_prepared() {
    let wallet = wallet_with_funding_notes(TIP, 2);
    let spent = bound_note_of(&wallet, FUNDING_NOTE);
    let mut client = committed_client(wallet).await;
    let stale = scheduled_plan(&client).await;
    assert_eq!(phase_of(&client).await, MigrationPhase::Prepared);
    client
        .wallet()
        .write()
        .await
        .migration
        .as_mut()
        .expect("the migration exists")
        .commitment
        .committed_at = 0;
    mark_note_spent_by(&mut *client.wallet().write().await, &spent, foreign_txid());
    let fresh = scheduled_plan(&client).await;
    assert_ne!(
        hash_of(&fresh),
        hash_of(&stale),
        "premise: the notes changed"
    );

    let result = client.recommit_migration(&stale).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::PlanMismatch
    ));
    let untouched = migration_of(&client).await;
    assert_eq!(untouched.commitment.plan_hash, hash_of(&stale));
    assert_eq!(
        untouched.commitment.committed_at, 0,
        "the refusal writes nothing"
    );

    client
        .recommit_migration(&fresh)
        .await
        .expect("the current plan recommits");

    let state = migration_of(&client).await;
    assert_eq!(state.phase, MigrationPhase::Prepared);
    assert_eq!(state.commitment.plan_hash, hash_of(&fresh));
    assert!(
        state.commitment.committed_at > 0,
        "fresh consent is timestamped"
    );
    assert_eq!(state.commitment.params_hash, state.params.params_hash());
    assert!(state.transfers.is_empty());
    let proposed = client
        .propose_schedule(1)
        .await
        .expect("the recommitted migration proposes");
    assert_eq!(
        proposed.transfers.len(),
        1,
        "the spent note is no longer a funding note"
    );
}

#[tokio::test]
async fn recommit_migration_recomputes_the_phase_from_the_current_plan() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .orchard_note(FUNDING_NOTE)
        .tip(TIP)
        .build();
    let unprepared = bound_note_of(&wallet, UNPREPARED_NOTE);
    let mut client = committed_client(wallet).await;
    assert_eq!(phase_of(&client).await, MigrationPhase::Committed);
    mark_note_spent_by(
        &mut *client.wallet().write().await,
        &unprepared,
        foreign_txid(),
    );
    let fresh = scheduled_plan(&client).await;
    let MigrationPlan::Scheduled(scheduled) = &fresh else {
        panic!("scheduled mode plans a scheduled plan");
    };
    assert!(
        scheduled.is_prepared(),
        "premise: only the funding note is left"
    );

    client
        .recommit_migration(&fresh)
        .await
        .expect("the current plan recommits");

    let state = migration_of(&client).await;
    assert_eq!(
        state.phase,
        MigrationPhase::Prepared,
        "a plan with no rounds left lands in Prepared"
    );
    assert_eq!(state.commitment.plan_hash, hash_of(&fresh));
}

#[tokio::test]
async fn recommit_migration_keeps_committed_while_rounds_remain() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .orchard_note(UNPREPARED_NOTE / 2)
        .tip(TIP)
        .build();
    let spent = bound_note_of(&wallet, UNPREPARED_NOTE / 2);
    let mut client = committed_client(wallet).await;
    let stale = scheduled_plan(&client).await;
    mark_note_spent_by(&mut *client.wallet().write().await, &spent, foreign_txid());
    let fresh = scheduled_plan(&client).await;
    assert_ne!(hash_of(&fresh), hash_of(&stale));

    client
        .recommit_migration(&fresh)
        .await
        .expect("the current plan recommits");

    let state = migration_of(&client).await;
    assert_eq!(state.phase, MigrationPhase::Committed);
    assert_eq!(state.commitment.plan_hash, hash_of(&fresh));
    assert_eq!(
        client.wallet().read().await.reserved_output_ids().len(),
        1,
        "the reservation is the live note set, unchanged by the recommit"
    );
}

#[tokio::test]
async fn recommit_migration_refuses_an_immediate_plan() {
    let wallet = wallet_with_funding_notes(TIP, 1);
    let mut client = committed_client(wallet).await;
    let before = migration_of(&client).await;
    let immediate = client
        .plan_migration(AccountId::ZERO, MigrationMode::Immediate)
        .await
        .expect("planning is pure");

    let result = client.recommit_migration(&immediate).await;

    assert!(matches!(
        migration_error(result),
        MigrationError::WrongPlanMode
    ));
    assert_eq!(migration_of(&client).await, before);
}

#[tokio::test]
async fn recommit_migration_refuses_without_a_migration_and_once_scheduled() {
    let wallet = wallet_with_funding_notes(TIP, 1);
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;
    let result = client.recommit_migration(&plan).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::NoMigration
    ));

    let wallet = wallet_with_funding_notes(TIP, 1);
    let mut client = scheduled_client(wallet, 1).await;
    let before = migration_of(&client).await;
    let plan = scheduled_plan(&client).await;
    let result = client.recommit_migration(&plan).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::AlreadyPrepared
    ));
    assert_eq!(migration_of(&client).await, before);
}

#[tokio::test]
async fn recommit_migration_refuses_while_a_round_is_in_flight() {
    let mut wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let params = MigrationParams::provisional(wallet.chain_type());
    let in_flight = TxId::from_bytes([0x11; 32]);
    wallet.wallet_transactions.insert(
        in_flight,
        WalletTransaction::new_for_test(
            in_flight,
            ConfirmationStatus::Transmitted(BlockHeight::from_u32(TIP + 1)),
        ),
    );
    let phase = MigrationPhase::Preparing {
        round: 0,
        pending_txids: vec![in_flight],
    };
    wallet.migration = Some(preparing_state(params, phase.clone()));
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;

    let result = client.recommit_migration(&plan).await;

    assert!(matches!(
        migration_error(result),
        MigrationError::RoundPending { ref txids } if *txids == vec![in_flight]
    ));
    assert_eq!(phase_of(&client).await, phase);
}

#[tokio::test]
async fn recommit_migration_accepts_a_preparing_migration_whose_round_failed() {
    let mut wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let params = MigrationParams::provisional(wallet.chain_type());
    let failed = TxId::from_bytes([0x11; 32]);
    wallet.wallet_transactions.insert(
        failed,
        WalletTransaction::new_for_test(
            failed,
            ConfirmationStatus::Failed(BlockHeight::from_u32(TIP - 1)),
        ),
    );
    wallet.migration = Some(preparing_state(
        params,
        MigrationPhase::Preparing {
            round: 0,
            pending_txids: vec![failed],
        },
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;

    client
        .recommit_migration(&plan)
        .await
        .expect("a failed round leaves nothing in flight");

    let state = migration_of(&client).await;
    assert_eq!(
        state.phase,
        MigrationPhase::Committed,
        "the unprepared note still needs its round"
    );
    assert_eq!(state.commitment.plan_hash, hash_of(&plan));
}

#[tokio::test]
async fn recommit_migration_accepts_a_preparing_migration_whose_round_confirmed_and_anchored() {
    let mut wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .tip(TIP)
        .build();
    let confirmed = creating_txid(&wallet, FUNDING_NOTE);
    let params = MigrationParams::provisional(wallet.chain_type());
    wallet.migration = Some(preparing_state(
        params,
        MigrationPhase::Preparing {
            round: 0,
            pending_txids: vec![confirmed],
        },
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    let plan = scheduled_plan(&client).await;

    client
        .recommit_migration(&plan)
        .await
        .expect("a confirmed and anchored round leaves nothing in flight");

    assert_eq!(phase_of(&client).await, MigrationPhase::Prepared);
}

#[tokio::test]
async fn reconciliation_demotes_a_prepared_migration_whose_plan_needs_preparation_again() {
    let mut wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let receipt_txid = creating_txid(&wallet, UNPREPARED_NOTE);
    let receipt = wallet
        .wallet_transactions
        .remove(&receipt_txid)
        .expect("the unprepared note's transaction is in the wallet");
    let mut client = committed_client(wallet).await;
    let prepared = migration_of(&client).await;
    assert_eq!(prepared.phase, MigrationPhase::Prepared, "premise");

    client
        .wallet()
        .write()
        .await
        .wallet_transactions
        .insert(receipt_txid, receipt);
    let replanned = scheduled_plan(&client).await;
    let MigrationPlan::Scheduled(scheduled) = &replanned else {
        panic!("scheduled mode plans a scheduled plan");
    };
    assert!(
        !scheduled.is_prepared(),
        "premise: the receipt needs a preparation round"
    );

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    let state = migration_of(&client).await;
    assert_eq!(
        state.phase,
        MigrationPhase::Committed,
        "the migration is back to needing preparation"
    );
    assert_eq!(
        state.commitment.plan_hash,
        hash_of(&replanned),
        "the plan hash is refreshed to the replan"
    );
    assert_ne!(state.commitment.plan_hash, prepared.commitment.plan_hash);
    let result = client.propose_schedule(1).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::NotPrepared
    ));
    client
        .recommit_migration(&replanned)
        .await
        .expect("the replan is the current plan");
    assert_eq!(
        client.wallet().read().await.reserved_output_ids().len(),
        2,
        "the receipt is reserved with the rest"
    );
}
