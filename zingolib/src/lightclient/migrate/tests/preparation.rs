use std::num::NonZeroU32;

use zcash_primitives::transaction::TxId;
use zingo_status::confirmation_status::ConfirmationStatus;
use zip32::AccountId;

use super::fixtures::{
    FUNDING_NOTE, NOTE_VALUE, RESIDUAL_NOTE, SEED, TIP, UNPREPARED_NOTE, bound_note_of,
    committed_client, creating_txid, mark_note_spent_by, migration_error, migration_of, on_disk,
    phase_of, preparing_state, scheduled_client, scheduled_plan, transaction_status,
    wallet_with_migration_note,
};
use crate::lightclient::LightClient;
use crate::lightclient::error::MigrationError;
use crate::lightclient::migrate::{MAX_ROUNDS, MigrationPlan};
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::LightWallet;
use crate::wallet::migration::{MigrationParams, MigrationPhase};

#[tokio::test]
async fn broadcast_preparation_round_refuses_without_a_migration() {
    let (wallet, _) = wallet_with_migration_note(TIP);
    let mut client = LightClient::new_for_test(wallet).await;
    let result = client.broadcast_preparation_round().await;
    assert!(matches!(
        migration_error(result),
        MigrationError::NoMigration
    ));
}

#[tokio::test]
async fn broadcast_preparation_round_refuses_once_prepared() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .tip(TIP)
        .build();
    let mut client = committed_client(wallet).await;
    assert_eq!(phase_of(&client).await, MigrationPhase::Prepared);

    let result = client.broadcast_preparation_round().await;
    assert!(matches!(
        migration_error(result),
        MigrationError::AlreadyPrepared
    ));
    assert_eq!(phase_of(&client).await, MigrationPhase::Prepared);
}

#[tokio::test]
async fn broadcast_preparation_round_refuses_once_scheduled() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .tip(TIP)
        .build();
    let mut client = scheduled_client(wallet, 1).await;
    assert_eq!(phase_of(&client).await, MigrationPhase::Scheduled);

    let result = client.broadcast_preparation_round().await;
    assert!(matches!(
        migration_error(result),
        MigrationError::AlreadyPrepared
    ));
    assert_eq!(phase_of(&client).await, MigrationPhase::Scheduled);
}

#[tokio::test]
async fn broadcast_preparation_round_reports_the_pending_round_while_it_is_unconfirmed() {
    let (mut wallet, _) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let in_flight = TxId::from_bytes([9; 32]);
    let phase = MigrationPhase::Preparing {
        round: 0,
        pending_txids: vec![in_flight],
    };
    wallet.migration = Some(preparing_state(params, phase.clone()));
    let mut client = LightClient::new_for_test(wallet).await;

    let result = client.broadcast_preparation_round().await;
    assert!(
        matches!(
            migration_error(result),
            MigrationError::RoundPending { ref txids } if *txids == vec![in_flight]
        ),
        "the unconfirmed round is reported pending"
    );
    assert_eq!(phase_of(&client).await, phase, "the refusal writes nothing");
}

#[tokio::test]
async fn broadcast_preparation_round_reports_a_confirmed_round_above_the_anchor_as_pending() {
    let mut wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(NOTE_VALUE)
        .orchard_note(2 * NOTE_VALUE)
        .tip(TIP)
        .build();
    wallet.wallet_settings.min_confirmations = NonZeroU32::new(TIP).expect("non-zero literal");
    let confirmed = creating_txid(&wallet, 2 * NOTE_VALUE);
    let params = MigrationParams::provisional(wallet.chain_type());
    wallet.migration = Some(preparing_state(
        params,
        MigrationPhase::Preparing {
            round: 0,
            pending_txids: vec![confirmed],
        },
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    let result = client.broadcast_preparation_round().await;
    assert!(
        matches!(
            migration_error(result),
            MigrationError::RoundPending { ref txids } if *txids == vec![confirmed]
        ),
        "a round confirmed above the anchor is still pending"
    );
}

#[tokio::test]
async fn broadcast_preparation_round_refuses_round_zero_when_the_notes_changed() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let bound = bound_note_of(&wallet, UNPREPARED_NOTE);
    let mut client = committed_client(wallet).await;
    assert_eq!(phase_of(&client).await, MigrationPhase::Committed);
    mark_note_spent_by(
        &mut *client.wallet().write().await,
        &bound,
        TxId::from_bytes([9; 32]),
    );

    let result = client.broadcast_preparation_round().await;
    assert!(matches!(
        migration_error(result),
        MigrationError::PlanMismatch
    ));
    assert_eq!(phase_of(&client).await, MigrationPhase::Committed);
}

#[tokio::test]
async fn broadcast_preparation_round_aborts_at_the_round_bound() {
    let mut wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let confirmed = creating_txid(&wallet, UNPREPARED_NOTE);
    let params = MigrationParams::provisional(wallet.chain_type());
    wallet.migration = Some(preparing_state(
        params,
        MigrationPhase::Preparing {
            round: u32::try_from(MAX_ROUNDS - 1).expect("bound fits u32"),
            pending_txids: vec![confirmed],
        },
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    let result = client.broadcast_preparation_round().await;
    assert!(matches!(
        migration_error(result),
        MigrationError::PreparationDidNotConverge(MAX_ROUNDS)
    ));
}

#[tokio::test]
async fn reconciliation_moves_a_confirmed_and_anchored_round_to_prepared() {
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

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    assert_eq!(phase_of(&client).await, MigrationPhase::Prepared);
    let result = client.broadcast_preparation_round().await;
    assert!(matches!(
        migration_error(result),
        MigrationError::AlreadyPrepared
    ));
}

#[tokio::test]
async fn reconciliation_finishes_preparation_when_the_leftover_is_residual() {
    let mut wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .orchard_note(RESIDUAL_NOTE)
        .tip(TIP)
        .build();
    let params = MigrationParams::provisional(wallet.chain_type());
    wallet.migration = Some(preparing_state(
        params,
        MigrationPhase::Preparing {
            round: 0,
            pending_txids: Vec::new(),
        },
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    assert_eq!(
        phase_of(&client).await,
        MigrationPhase::Prepared,
        "a residual leftover does not keep the migration preparing"
    );
}

#[tokio::test]
async fn broadcast_preparation_round_persists_the_round_before_broadcast_and_fails_it_on_transmit_failure()
 {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let chain_type = wallet.chain_type();
    let mut client = LightClient::new_for_test(wallet).await;
    client.save_task().await;
    let plan = scheduled_plan(&client).await;
    match &plan {
        MigrationPlan::Scheduled(plan) => {
            assert!(!plan.is_prepared(), "premise: the note needs a round");
        }
        MigrationPlan::Immediate(_) => panic!("a scheduled plan was requested"),
    }
    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("the plan commits");

    let error = client
        .broadcast_preparation_round()
        .await
        .expect_err("a client with no indexer cannot broadcast the round");
    eprintln!("broadcast_preparation_round returned: {error:?}");

    let reloaded = on_disk(&client, chain_type).await;
    let state = reloaded
        .migration
        .as_ref()
        .expect("the committed migration is on disk");
    let MigrationPhase::Preparing {
        round,
        pending_txids,
    } = &state.phase
    else {
        panic!("the persisted phase is the attempted round, got {state:?}");
    };
    assert_eq!(*round, 0);
    assert!(
        !pending_txids.is_empty(),
        "the round's transactions are recorded"
    );
    for txid in pending_txids {
        assert!(
            matches!(
                transaction_status(&reloaded, txid),
                Some(ConfirmationStatus::Failed(_))
            ),
            "round transaction {txid} is marked failed on disk"
        );
    }
    assert_eq!(
        migration_of(&client).await.phase,
        state.phase,
        "memory and disk agree"
    );
    client
        .shutdown_save_task()
        .await
        .expect("the save task stops");
}

#[tokio::test]
async fn broadcast_preparation_round_replans_after_a_failed_round() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let mut client = committed_client(wallet).await;
    client
        .broadcast_preparation_round()
        .await
        .expect_err("a client with no indexer cannot broadcast the round");
    let MigrationPhase::Preparing {
        pending_txids: first_round,
        ..
    } = phase_of(&client).await
    else {
        panic!("the failed round is recorded");
    };

    let result = client.broadcast_preparation_round().await;
    assert!(
        !matches!(
            result,
            Err(crate::lightclient::error::LightClientError::MigrationError(
                MigrationError::RoundPending { .. }
            ))
        ),
        "a failed round is not pending: {result:?}"
    );
    let MigrationPhase::Preparing {
        round,
        pending_txids: second_round,
    } = phase_of(&client).await
    else {
        panic!("the replanned round is recorded");
    };
    assert_eq!(round, 0, "a failed round does not count toward the bound");
    assert!(
        second_round.iter().all(|txid| !first_round.contains(txid)),
        "the replanned round builds fresh transactions"
    );
}

#[tokio::test]
async fn relaunch_after_a_failed_round_replans() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let chain_type = wallet.chain_type();
    let mut client = LightClient::new_for_test(wallet).await;
    client.save_task().await;
    let plan = scheduled_plan(&client).await;
    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("the plan commits");
    client
        .broadcast_preparation_round()
        .await
        .expect_err("a client with no indexer cannot broadcast the round");
    client
        .shutdown_save_task()
        .await
        .expect("the save task stops");

    let bytes = std::fs::read(client.wallet_path()).expect("the wallet file exists");
    drop(client);
    let reloaded = LightWallet::read(bytes.as_slice(), chain_type).expect("the file reads back");
    let mut relaunched = LightClient::new_for_test(reloaded).await;

    let result = relaunched.broadcast_preparation_round().await;
    assert!(
        !matches!(
            result,
            Err(crate::lightclient::error::LightClientError::MigrationError(
                MigrationError::RoundPending { .. }
            ))
        ),
        "the relaunched wallet must not wait on transactions that never left: {result:?}"
    );
}

mod planning {
    use zip32::AccountId;

    use super::super::fixtures::{
        SEED, TIP, UNPREPARED_NOTE, scheduled_plan, set_tip_with_unscanned_gap,
    };
    use crate::lightclient::LightClient;
    use crate::lightclient::migrate::MigrationPlan;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::error::WalletError;
    use crate::wallet::migration::MigrationParams;
    use crate::wallet::migration::preparation::note_preparation_fee;

    #[tokio::test]
    async fn plan_residual_equals_the_value_that_never_reaches_ironwood() {
        const NOTE: u64 = 1_999_999;
        let wallet = SyntheticWalletBuilder::new(SEED)
            .orchard_note(NOTE)
            .tip(TIP)
            .build();
        let params = MigrationParams::provisional(wallet.chain_type());
        let client = LightClient::new_for_test(wallet).await;
        let plan = scheduled_plan(&client).await;
        let MigrationPlan::Scheduled(scheduled) = &plan else {
            panic!("scheduled mode yields a scheduled plan");
        };
        let preparation_fees: u64 = scheduled
            .preparation_rounds
            .iter()
            .flatten()
            .map(|tx| note_preparation_fee(tx.inputs.len(), tx.outputs.len(), true))
            .sum();
        let reaching_ironwood: u64 = scheduled.transfers.iter().sum();
        assert_eq!(
            reaching_ironwood
                + scheduled.transfers_fee(&params)
                + preparation_fees
                + plan.residual(),
            NOTE,
            "value reaching Ironwood, fees and residual account for the whole note"
        );
    }

    #[test]
    fn scheduled_planner_errors_when_the_horizon_withholds_every_note() {
        let mut wallet = SyntheticWalletBuilder::new(SEED)
            .orchard_note(UNPREPARED_NOTE)
            .tip(TIP)
            .build();
        set_tip_with_unscanned_gap(&mut wallet, TIP, 600);

        let planned = wallet.plan_ironwood_migration_now(AccountId::ZERO);
        assert!(
            matches!(planned, Err(WalletError::SyncIncomplete)),
            "a stalled sync must not read as an empty wallet, got {planned:?}"
        );
    }

    #[test]
    fn immediate_planner_errors_when_the_horizon_withholds_every_note() {
        let mut wallet = SyntheticWalletBuilder::new(SEED)
            .orchard_note(UNPREPARED_NOTE)
            .tip(TIP)
            .build();
        set_tip_with_unscanned_gap(&mut wallet, TIP, 600);

        let planned = wallet.plan_immediate_migration(AccountId::ZERO);
        assert!(
            matches!(planned, Err(WalletError::SyncIncomplete)),
            "a stalled sync must not read as an empty wallet, got {planned:?}"
        );
    }

    #[test]
    fn an_empty_wallet_plans_empty_under_a_stalled_sync() {
        let mut wallet = SyntheticWalletBuilder::new(SEED).tip(TIP).build();
        set_tip_with_unscanned_gap(&mut wallet, TIP, 600);

        let plan = wallet
            .plan_ironwood_migration_now(AccountId::ZERO)
            .expect("an empty note set is not a sync failure");
        assert!(plan.is_prepared() && plan.transfers.is_empty());
    }
}

#[tokio::test]
async fn a_failed_round_at_the_round_bound_replays_with_the_same_round_number() {
    use pepper_sync::wallet::WalletTransaction;
    use zcash_protocol::consensus::BlockHeight;

    let mut wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let failed = TxId::from_bytes([0x11; 32]);
    wallet.wallet_transactions.insert(
        failed,
        WalletTransaction::new_for_test(
            failed,
            ConfirmationStatus::Failed(BlockHeight::from_u32(TIP - 1)),
        ),
    );
    let params = MigrationParams::provisional(wallet.chain_type());
    let last_round = u32::try_from(MAX_ROUNDS - 1).expect("bound fits u32");
    wallet.migration = Some(preparing_state(
        params,
        MigrationPhase::Preparing {
            round: last_round,
            pending_txids: vec![failed],
        },
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    let result = client.broadcast_preparation_round().await;

    assert!(
        !matches!(
            result,
            Err(crate::lightclient::error::LightClientError::MigrationError(
                MigrationError::PreparationDidNotConverge(_)
            ))
        ),
        "a failed round never counted, so the bound is not reached: {result:?}"
    );
    let MigrationPhase::Preparing {
        round,
        pending_txids,
    } = phase_of(&client).await
    else {
        panic!("the replayed round is recorded");
    };
    assert_eq!(round, last_round, "the replay keeps the round number");
    assert!(
        !pending_txids.contains(&failed) && !pending_txids.is_empty(),
        "the replay built fresh transactions: {pending_txids:?}"
    );
}
