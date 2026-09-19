use zcash_primitives::transaction::TxId;
use zingo_status::confirmation_status::ConfirmationStatus;

use super::fixtures::{
    FAR_EXPIRY, TIP, assigned_transfer, broadcast_transfer, confirmed_transfer, current_bucket_of,
    insert_calculated_transaction, migration_error, migration_of, record_confirmed_transfer_spend,
    scheduled_state, signed_transfer, sorted, transaction_status, wallet_with_migration_note,
    wallet_with_notes,
};
use crate::lightclient::LightClient;
use crate::lightclient::error::MigrationError;
use crate::lightclient::migrate::TransferProgress;
use crate::wallet::migration::{MigrationParams, MigrationPhase, TransferId, TransferState};

#[tokio::test]
async fn release_transfer_frees_the_funding_note() {
    let (mut wallet, notes) = wallet_with_notes(TIP, 2);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    wallet.migration = Some(scheduled_state(
        params,
        vec![
            assigned_transfer(0, notes[0], current_bucket + 1),
            assigned_transfer(1, notes[1], current_bucket + 1),
        ],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    assert_eq!(
        sorted(client.wallet().read().await.reserved_output_ids()),
        sorted(vec![notes[0].output_id, notes[1].output_id]),
        "both funding notes are reserved"
    );

    client
        .release_transfer(TransferId(0))
        .await
        .expect("a pending transfer releases");

    let wallet = client.wallet().read().await;
    assert_eq!(
        wallet.reserved_output_ids(),
        vec![notes[1].output_id],
        "the released transfer's note is free"
    );
    let state = wallet.migration.as_ref().expect("the migration stands");
    assert_eq!(state.transfers[0].state, TransferState::Released);
    assert_eq!(state.transfers[1].state, TransferState::Assigned);
    assert_eq!(state.phase, MigrationPhase::Scheduled);
    drop(wallet);
    let status = client.migration_status().await.expect("status reads");
    assert_eq!(status.transfers[0].progress, TransferProgress::Released);
}

#[tokio::test]
async fn release_transfer_refuses_a_confirmed_transfer() {
    const CONFIRMED_AT: u32 = 300;
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let own_txid = TxId::from_bytes([7; 32]);
    record_confirmed_transfer_spend(&mut wallet, &bound_note, own_txid, CONFIRMED_AT);
    wallet.migration = Some(scheduled_state(
        params,
        vec![confirmed_transfer(bound_note, own_txid, CONFIRMED_AT)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let before = migration_of(&client).await;

    let result = client.release_transfer(TransferId(0)).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::TransferNotPending(0)
    ));
    assert_eq!(
        migration_of(&client).await,
        before,
        "the refusal writes nothing"
    );
}

#[tokio::test]
async fn release_transfer_refuses_an_unknown_transfer() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    wallet.migration = Some(scheduled_state(
        params,
        vec![assigned_transfer(0, bound_note, 5)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    let result = client.release_transfer(TransferId(3)).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::TransferNotPending(3)
    ));
}

#[tokio::test]
async fn release_transfer_refuses_without_a_migration() {
    let (wallet, _) = wallet_with_migration_note(TIP);
    let mut client = LightClient::new_for_test(wallet).await;
    let result = client.release_transfer(TransferId(0)).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::NoMigration
    ));
}

#[tokio::test]
async fn release_transfer_fails_the_transaction_of_a_signed_transfer() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let txid = TxId::from_bytes([7; 32]);
    insert_calculated_transaction(&mut wallet, txid, TIP + 1);
    wallet.migration = Some(scheduled_state(
        params,
        vec![signed_transfer(
            0,
            bound_note,
            current_bucket,
            txid,
            FAR_EXPIRY,
            None,
        )],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    client
        .release_transfer(TransferId(0))
        .await
        .expect("a signed transfer releases");

    let wallet = client.wallet().read().await;
    let transfer = &wallet
        .migration
        .as_ref()
        .expect("the migration stands")
        .transfers[0];
    assert_eq!(transfer.state, TransferState::Released);
    assert_eq!(transfer.txid, None, "the signature is discarded");
    assert_eq!(transfer.previous_txids, vec![txid]);
    assert!(
        matches!(
            transaction_status(&wallet, &txid),
            Some(ConfirmationStatus::Failed(_))
        ),
        "the wallet transaction is failed so the note is free"
    );
    assert!(wallet.reserved_output_ids().is_empty());
}

#[tokio::test]
async fn release_transfer_keeps_a_broadcast_transaction_on_the_wire() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let txid = TxId::from_bytes([7; 32]);
    insert_calculated_transaction(&mut wallet, txid, TIP + 1);
    wallet.migration = Some(scheduled_state(
        params,
        vec![broadcast_transfer(
            0,
            bound_note,
            current_bucket,
            txid,
            FAR_EXPIRY,
        )],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    client
        .release_transfer(TransferId(0))
        .await
        .expect("a broadcast transfer releases");

    let wallet = client.wallet().read().await;
    let transfer = &wallet.migration.as_ref().expect("stands").transfers[0];
    assert_eq!(transfer.state, TransferState::Released);
    assert_eq!(
        transfer.txid,
        Some(txid),
        "the transaction stays on the wire"
    );
    assert!(matches!(
        transaction_status(&wallet, &txid),
        Some(ConfirmationStatus::Calculated(_))
    ));
    assert_eq!(
        wallet.reserved_output_ids(),
        vec![bound_note.output_id],
        "the note stays reserved until the transaction resolves"
    );
}

#[tokio::test]
async fn cancel_migration_refuses_without_a_migration() {
    let (wallet, _) = wallet_with_migration_note(TIP);
    let mut client = LightClient::new_for_test(wallet).await;
    let result = client.cancel_migration().await;
    assert!(matches!(
        migration_error(result),
        MigrationError::NoMigration
    ));
}

#[tokio::test]
async fn cancel_migration_frees_every_note() {
    let (mut wallet, notes) = wallet_with_notes(TIP, 2);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    wallet.migration = Some(scheduled_state(
        params,
        vec![
            assigned_transfer(0, notes[0], current_bucket + 1),
            assigned_transfer(1, notes[1], current_bucket + 2),
        ],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    assert_eq!(client.wallet().read().await.reserved_output_ids().len(), 2);

    client.cancel_migration().await.expect("cancels");

    let wallet = client.wallet().read().await;
    assert!(wallet.migration.is_none(), "the migration is gone");
    assert!(wallet.reserved_output_ids().is_empty());
    let summaries = wallet.note_summaries::<pepper_sync::wallet::OrchardNote>(false);
    assert!(summaries.iter().all(|note| !note.reserved));
}

#[tokio::test]
async fn cancel_migration_releases_every_transfer_and_keeps_the_wire_transactions() {
    const CONFIRMED_AT: u32 = 300;
    let (mut wallet, notes) = wallet_with_notes(TIP, 3);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let signed_txid = TxId::from_bytes([1; 32]);
    let broadcast_txid = TxId::from_bytes([2; 32]);
    let confirmed_txid = TxId::from_bytes([3; 32]);
    insert_calculated_transaction(&mut wallet, signed_txid, TIP + 1);
    insert_calculated_transaction(&mut wallet, broadcast_txid, TIP + 1);
    record_confirmed_transfer_spend(&mut wallet, &notes[2], confirmed_txid, CONFIRMED_AT);
    let mut confirmed = confirmed_transfer(notes[2], confirmed_txid, CONFIRMED_AT);
    confirmed.id = TransferId(2);
    wallet.migration = Some(scheduled_state(
        params,
        vec![
            signed_transfer(0, notes[0], current_bucket, signed_txid, FAR_EXPIRY, None),
            broadcast_transfer(1, notes[1], current_bucket, broadcast_txid, FAR_EXPIRY),
            confirmed,
        ],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    client.cancel_migration().await.expect("cancels");

    let wallet = client.wallet().read().await;
    let state = wallet
        .migration
        .as_ref()
        .expect("a transaction on the wire keeps the record");
    assert_eq!(state.transfers[0].state, TransferState::Released);
    assert_eq!(state.transfers[1].state, TransferState::Released);
    assert!(matches!(
        transaction_status(&wallet, &signed_txid),
        Some(ConfirmationStatus::Failed(_))
    ));
    assert!(
        matches!(
            transaction_status(&wallet, &broadcast_txid),
            Some(ConfirmationStatus::Calculated(_))
        ),
        "a broadcast transaction stays on the wire"
    );
    assert_eq!(
        wallet.reserved_output_ids(),
        vec![notes[1].output_id],
        "only the note on the wire stays reserved"
    );
    assert!(
        matches!(
            transaction_status(&wallet, &confirmed_txid),
            Some(ConfirmationStatus::Confirmed(_))
        ),
        "a confirmed transfer stands"
    );
}

#[tokio::test]
async fn cancel_migration_before_the_schedule_frees_every_note() {
    let wallet = super::fixtures::wallet_with_funding_notes(TIP, 2);
    let mut client = super::fixtures::committed_client(wallet).await;
    assert_eq!(client.wallet().read().await.reserved_output_ids().len(), 2);

    client.cancel_migration().await.expect("cancels");

    let wallet = client.wallet().read().await;
    assert!(wallet.migration.is_none());
    assert!(wallet.reserved_output_ids().is_empty());
}

#[tokio::test]
async fn release_transfer_refuses_a_transfer_reconciliation_just_invalidated() {
    const MINED_AT: u32 = 350;
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    record_confirmed_transfer_spend(
        &mut wallet,
        &bound_note,
        TxId::from_bytes([0xF0; 32]),
        MINED_AT,
    );
    wallet.migration = Some(scheduled_state(
        params,
        vec![assigned_transfer(0, bound_note, current_bucket + 1)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    let result = client.release_transfer(TransferId(0)).await;

    assert!(
        matches!(
            migration_error(result),
            MigrationError::TransferNotPending(0)
        ),
        "reconciliation ran first and found the note spent outside the migration"
    );
    assert_eq!(
        migration_of(&client).await.transfers[0].state,
        TransferState::Invalidated
    );
}

#[tokio::test]
async fn cancel_migration_confirms_a_mined_transfer_before_releasing_the_rest() {
    const MINED_AT: u32 = 350;
    let (mut wallet, notes) = wallet_with_notes(TIP, 2);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let own_txid = TxId::from_bytes([7; 32]);
    record_confirmed_transfer_spend(&mut wallet, &notes[0], own_txid, MINED_AT);
    wallet.migration = Some(scheduled_state(
        params,
        vec![
            broadcast_transfer(0, notes[0], current_bucket, own_txid, FAR_EXPIRY),
            assigned_transfer(1, notes[1], current_bucket + 1),
        ],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    client.cancel_migration().await.expect("cancels");

    let wallet = client.wallet().read().await;
    assert!(
        wallet.migration.is_none(),
        "the mined transfer was confirmed first, so nothing is on the wire and the record goes"
    );
    assert!(matches!(
        transaction_status(&wallet, &own_txid),
        Some(ConfirmationStatus::Confirmed(_))
    ));
    assert!(wallet.reserved_output_ids().is_empty());
}
