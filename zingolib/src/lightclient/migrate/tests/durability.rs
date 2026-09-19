use std::time::Duration;

use zcash_primitives::transaction::TxId;
use zip32::AccountId;

use super::fixtures::{
    FUNDING_NOTE, RESIDUAL_NOTE, SEED, TIP, client_with_a_scheduled_transfer, committed_client,
    current_bucket_of, migration_of, mixnet_receipt, on_disk, scheduled_plan, scheduled_state,
    signed_transfer, wallet_with_funding_notes, wallet_with_migration_note, window_end_of,
};
use crate::lightclient::LightClient;
use crate::lightclient::migrate::{TransferBroadcastResult, TransferOutcome};
use crate::mocks::transmission::MockBroadcastClient;
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::migration::{MigrationParams, MigrationPhase, TransferId, TransferState};

fn block_the_wallet_file(client: &LightClient) -> std::path::PathBuf {
    let blocker = client.wallet_path().with_extension("dat.tmp");
    std::fs::create_dir(&blocker).expect("the blocker is creatable");
    blocker
}

#[tokio::test]
async fn commit_migration_is_on_disk_before_the_call_returns() {
    let wallet = wallet_with_funding_notes(TIP, 1);
    let chain_type = wallet.chain_type();
    let mut client = LightClient::new_for_test(wallet).await;
    client.save_task().await;
    let plan = scheduled_plan(&client).await;

    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("the plan commits");

    let reloaded = on_disk(&client, chain_type).await;
    assert_eq!(
        reloaded.migration,
        Some(migration_of(&client).await),
        "the file holds the committed migration"
    );
    assert!(
        !client.wallet().read().await.save_required,
        "the write cleared the dirty flag"
    );
    client
        .shutdown_save_task()
        .await
        .expect("the save task stops");
}

#[tokio::test]
async fn commit_schedule_is_on_disk_before_the_call_returns() {
    let wallet = wallet_with_funding_notes(TIP, 2);
    let chain_type = wallet.chain_type();
    let mut client = committed_client(wallet).await;
    client.save_task().await;
    let proposed = client.propose_schedule(1).await.expect("proposes");

    client.commit_schedule(&proposed).await.expect("commits");

    let reloaded = on_disk(&client, chain_type).await;
    let state = reloaded.migration.expect("the file holds the migration");
    assert_eq!(state.phase, MigrationPhase::Scheduled);
    assert_eq!(state.transfers, migration_of(&client).await.transfers);
    client
        .shutdown_save_task()
        .await
        .expect("the save task stops");
}

#[tokio::test]
async fn release_transfer_is_on_disk_before_the_call_returns() {
    let (mut client, chain_type) = client_with_a_scheduled_transfer(TIP, 4).await;
    client.save_task().await;

    client
        .release_transfer(TransferId(0))
        .await
        .expect("releases");

    let reloaded = on_disk(&client, chain_type).await;
    assert_eq!(
        reloaded.migration.expect("the migration stands").transfers[0].state,
        TransferState::Released
    );
    client
        .shutdown_save_task()
        .await
        .expect("the save task stops");
}

#[tokio::test]
async fn cancel_migration_is_on_disk_before_the_call_returns() {
    let (mut client, chain_type) = client_with_a_scheduled_transfer(TIP, 4).await;
    client.save_task().await;

    client.cancel_migration().await.expect("cancels");

    assert!(
        on_disk(&client, chain_type).await.migration.is_none(),
        "the file no longer holds the migration"
    );
    client
        .shutdown_save_task()
        .await
        .expect("the save task stops");
}

#[tokio::test]
async fn reconciliation_is_on_disk_before_the_call_returns() {
    let (mut client, chain_type) = client_with_a_scheduled_transfer(TIP, 0).await;
    client.save_task().await;

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    let reloaded = on_disk(&client, chain_type).await;
    let transfer = &reloaded.migration.expect("the migration stands").transfers[0];
    assert_eq!(transfer.missed_windows, 1, "the reschedule is on disk");
    assert!(transfer.bucket_index.is_some_and(|bucket| bucket > 0));
    assert_eq!(
        *transfer,
        migration_of(&client).await.transfers[0],
        "memory and disk agree"
    );
    client
        .shutdown_save_task()
        .await
        .expect("the save task stops");
}

#[tokio::test]
async fn a_broadcast_is_on_disk_before_the_call_returns() {
    const OPEN_TIP: u32 = 300;
    let (mut wallet, bound_note) = wallet_with_migration_note(OPEN_TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let window_end = u32::from(window_end_of(current_bucket, &params));
    let own_txid = TxId::from_bytes([7; 32]);
    let transfer = signed_transfer(
        0,
        bound_note,
        current_bucket,
        own_txid,
        window_end,
        Some(vec![0xAB; 64]),
    );
    let chain_type = wallet.chain_type();
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    let mut client = LightClient::new_for_test(wallet).await;
    client.save_task().await;
    let mock = MockBroadcastClient::default();

    let report = client
        .broadcast_due_transfers_with(&mock, Duration::ZERO)
        .await
        .expect("the batch runs");
    assert_eq!(
        report.outcomes,
        vec![TransferOutcome {
            transfer: TransferId(0),
            denomination: super::fixtures::NOTE_VALUE,
            result: TransferBroadcastResult::Sent(mixnet_receipt(own_txid)),
        }]
    );

    let reloaded = on_disk(&client, chain_type).await;
    let transfer = &reloaded.migration.expect("the migration stands").transfers[0];
    assert_eq!(transfer.state, TransferState::Broadcast);
    assert_eq!(transfer.txid, Some(own_txid));
    assert_eq!(transfer.attempts, 1);
    client
        .shutdown_save_task()
        .await
        .expect("the save task stops");
}

#[tokio::test]
async fn without_the_save_task_nothing_is_written_and_the_dirty_flag_stays_set() {
    let wallet = wallet_with_funding_notes(TIP, 1);
    let mut client = LightClient::new_for_test(wallet).await;
    client.wallet().write().await.save_required = false;
    let plan = scheduled_plan(&client).await;

    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("the plan commits");

    assert!(
        !client.wallet_path().exists(),
        "no save task owns the file, so nothing is written"
    );
    assert!(
        client.wallet().read().await.save_required,
        "the consumer that saves the bytes itself sees the dirty flag"
    );
    assert!(migration_of(&client).await.phase == MigrationPhase::Prepared);
}

#[tokio::test]
async fn a_failed_write_rolls_back_commit_migration() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .orchard_note(RESIDUAL_NOTE)
        .tip(TIP)
        .build();
    let chain_type = wallet.chain_type();
    let mut client = LightClient::new_for_test(wallet).await;
    client.flush().await.expect("the baseline file writes");
    client.wallet().write().await.save_required = false;
    client.save_task().await;
    let plan = scheduled_plan(&client).await;

    let blocker = block_the_wallet_file(&client);
    let error = client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect_err("the wallet file cannot be written");
    eprintln!("commit_migration returned: {error:?}");

    assert!(
        client.wallet().read().await.migration.is_none(),
        "the failed write rolls the commit back in memory"
    );
    assert!(
        client
            .wallet()
            .read()
            .await
            .reserved_output_ids()
            .is_empty(),
        "nothing stays reserved"
    );
    std::fs::remove_dir(&blocker).expect("the blocker is removable");
    assert!(
        on_disk(&client, chain_type).await.migration.is_none(),
        "the failed write left no migration on disk"
    );

    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("the commit is repeatable once the file is writable");
    assert!(on_disk(&client, chain_type).await.migration.is_some());
    let _ = client.shutdown_save_task().await;
}

#[tokio::test]
async fn a_failed_write_rolls_back_commit_schedule() {
    let wallet = wallet_with_funding_notes(TIP, 2);
    let chain_type = wallet.chain_type();
    let mut client = committed_client(wallet).await;
    client.flush().await.expect("the baseline file writes");
    client.wallet().write().await.save_required = false;
    client.save_task().await;
    let before = migration_of(&client).await;
    assert_eq!(before.phase, MigrationPhase::Prepared);
    let proposed = client.propose_schedule(1).await.expect("proposes");

    let blocker = block_the_wallet_file(&client);
    let error = client
        .commit_schedule(&proposed)
        .await
        .expect_err("the wallet file cannot be written");
    eprintln!("commit_schedule returned: {error:?}");
    std::fs::remove_dir(&blocker).expect("the blocker is removable");

    assert_eq!(
        migration_of(&client).await,
        before,
        "the failed write rolls the schedule back in memory"
    );
    assert_eq!(
        on_disk(&client, chain_type).await.migration,
        Some(before),
        "the file still holds the prepared migration"
    );

    client
        .commit_schedule(&proposed)
        .await
        .expect("the commit is repeatable once the file is writable");
    assert_eq!(
        on_disk(&client, chain_type)
            .await
            .migration
            .map(|s| s.phase),
        Some(MigrationPhase::Scheduled)
    );
    let _ = client.shutdown_save_task().await;
}

#[tokio::test]
async fn a_failed_write_rolls_back_release_transfer() {
    let (mut client, chain_type) = client_with_a_scheduled_transfer(TIP, 4).await;
    client.save_task().await;
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let before = migration_of(&client).await;

    let blocker = block_the_wallet_file(&client);
    let error = client
        .release_transfer(TransferId(0))
        .await
        .expect_err("the wallet file cannot be written");
    eprintln!("release_transfer returned: {error:?}");
    std::fs::remove_dir(&blocker).expect("the blocker is removable");

    assert_eq!(migration_of(&client).await, before);
    assert_eq!(
        client.wallet().read().await.reserved_output_ids().len(),
        1,
        "the note stays reserved"
    );
    assert_eq!(
        on_disk(&client, chain_type).await.migration,
        Some(before),
        "the file still holds the pending transfer"
    );

    client
        .release_transfer(TransferId(0))
        .await
        .expect("the release is repeatable once the file is writable");
    assert_eq!(
        on_disk(&client, chain_type)
            .await
            .migration
            .expect("stands")
            .transfers[0]
            .state,
        TransferState::Released
    );
    let _ = client.shutdown_save_task().await;
}

#[tokio::test]
async fn a_failed_write_rolls_back_cancel_migration() {
    let (mut client, chain_type) = client_with_a_scheduled_transfer(TIP, 4).await;
    client.save_task().await;
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let before = migration_of(&client).await;

    let blocker = block_the_wallet_file(&client);
    let error = client
        .cancel_migration()
        .await
        .expect_err("the wallet file cannot be written");
    eprintln!("cancel_migration returned: {error:?}");
    std::fs::remove_dir(&blocker).expect("the blocker is removable");

    assert_eq!(
        migration_of(&client).await,
        before,
        "the failed write keeps the migration in memory"
    );
    assert_eq!(
        on_disk(&client, chain_type).await.migration,
        Some(before),
        "the file still holds the migration"
    );

    client
        .cancel_migration()
        .await
        .expect("the cancel is repeatable once the file is writable");
    assert!(on_disk(&client, chain_type).await.migration.is_none());
    let _ = client.shutdown_save_task().await;
}

#[tokio::test]
async fn a_failed_write_leaves_reconciliation_dirty_for_the_next_save() {
    let (mut client, chain_type) = client_with_a_scheduled_transfer(TIP, 0).await;
    client.save_task().await;

    let blocker = block_the_wallet_file(&client);
    let error = client
        .reconcile_migration()
        .await
        .expect_err("the wallet file cannot be written");
    eprintln!("reconcile_migration returned: {error:?}");

    assert!(
        client.wallet().read().await.save_required,
        "the unwritten reconciliation stays dirty"
    );
    assert_eq!(
        on_disk(&client, chain_type)
            .await
            .migration
            .expect("stands")
            .transfers[0]
            .bucket_index,
        Some(0),
        "the file still holds the missed window"
    );
    std::fs::remove_dir(&blocker).expect("the blocker is removable");
    let _ = client.shutdown_save_task().await;
}

#[tokio::test]
async fn a_schedule_change_that_fails_before_the_write_rolls_back_in_memory() {
    let (mut client, _) = client_with_a_scheduled_transfer(TIP, 4).await;
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let before = migration_of(&client).await;
    let proposed = client.propose_schedule(2).await.expect("proposes");

    client.wallet().write().await.sync_state = pepper_sync::wallet::SyncState::new();

    let error = client
        .commit_schedule(&proposed)
        .await
        .expect_err("a wallet with no chain height cannot bind its transfers");
    eprintln!("commit_schedule returned: {error:?}");

    assert_eq!(
        migration_of(&client).await,
        before,
        "the failed schedule change leaves the committed schedule in memory"
    );
}

#[tokio::test]
async fn a_relaunch_reads_the_schedule_back_through_the_new_api() {
    let wallet = wallet_with_funding_notes(TIP, 2);
    let chain_type = wallet.chain_type();
    let mut client = LightClient::new_for_test(wallet).await;
    client.save_task().await;
    let plan = scheduled_plan(&client).await;
    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .expect("commits");
    let proposed = client.propose_schedule(1).await.expect("proposes");
    client.commit_schedule(&proposed).await.expect("commits");
    let in_memory = client.migration_status().await.expect("status reads");
    client
        .shutdown_save_task()
        .await
        .expect("the save task stops");

    let reloaded = on_disk(&client, chain_type).await;
    drop(client);
    let relaunched = LightClient::new_for_test(reloaded).await;
    let status = relaunched.migration_status().await.expect("status reads");
    assert_eq!(status.phase, Some(MigrationPhase::Scheduled));
    let placement = |status: &crate::lightclient::migrate::MigrationStatus| {
        status
            .transfers
            .iter()
            .map(|transfer| {
                (
                    transfer.id,
                    transfer.denomination,
                    transfer.window,
                    transfer.boundary,
                    transfer.progress,
                    transfer.missed_windows,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(placement(&status), placement(&in_memory));
    assert_eq!(status.due_now, in_memory.due_now);
    assert_eq!(
        relaunched.wallet().read().await.reserved_output_ids().len(),
        2,
        "the reservation is derived from the reloaded schedule"
    );
}
