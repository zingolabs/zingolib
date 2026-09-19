use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zingo_status::confirmation_status::ConfirmationStatus;

use super::fixtures::{
    FAR_EXPIRY, NOTE_VALUE, TIP, assessment_of, assigned_transfer, broadcast_transfer,
    current_bucket_of, insert_calculated_transaction, migration_of, phase_of,
    record_confirmed_transfer_spend, scheduled_state, set_tip, signed_transfer, transaction_status,
    wallet_with_migration_note, wallet_with_notes, window_end_of,
};
use crate::lightclient::LightClient;
use crate::lightclient::migrate::TransferProgress;
use crate::wallet::LightWallet;
use crate::wallet::migration::{
    BoundNote, MigrationParams, MigrationPhase, RecommendedAction, TransferClass, TransferId,
    TransferRecord, TransferState,
};

const OWN_TXID: TxId = TxId::from_bytes([7; 32]);

fn transfer_of(wallet: &LightWallet, index: usize) -> TransferRecord {
    wallet
        .migration
        .as_ref()
        .expect("the migration exists")
        .transfers[index]
        .clone()
}

fn released_on_the_wire(note: BoundNote, bucket: u64, expiry: u32) -> TransferRecord {
    let mut transfer = broadcast_transfer(0, note, bucket, OWN_TXID, expiry);
    transfer
        .mark_released()
        .expect("a broadcast transfer releases");
    assert!(
        transfer.is_on_the_wire(),
        "premise: the transaction is on the wire"
    );
    transfer
}

fn wallet_with_a_released_transfer_on_the_wire(
    tip: u32,
    bucket: u64,
    expiry: u32,
) -> (LightWallet, BoundNote) {
    let (mut wallet, bound_note) = wallet_with_migration_note(tip);
    let params = MigrationParams::provisional(wallet.chain_type());
    insert_calculated_transaction(&mut wallet, OWN_TXID, tip);
    wallet.migration = Some(scheduled_state(
        params,
        vec![released_on_the_wire(bound_note, bucket, expiry)],
    ));
    (wallet, bound_note)
}

#[tokio::test]
async fn release_transfer_keeps_the_txid_of_a_submitted_signed_transfer() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    insert_calculated_transaction(&mut wallet, OWN_TXID, TIP + 1);
    let mut submitted = signed_transfer(0, bound_note, current_bucket, OWN_TXID, FAR_EXPIRY, None);
    submitted.record_attempt();
    assert!(
        submitted.is_on_the_wire(),
        "premise: a submitted signature is on the wire"
    );
    wallet.migration = Some(scheduled_state(params, vec![submitted]));
    let mut client = LightClient::new_for_test(wallet).await;

    client
        .release_transfer(TransferId(0))
        .await
        .expect("a submitted transfer releases");

    let wallet = client.wallet().read().await;
    let transfer = transfer_of(&wallet, 0);
    assert_eq!(transfer.state, TransferState::Released);
    assert_eq!(
        transfer.txid,
        Some(OWN_TXID),
        "a signature that may have reached the network is kept"
    );
    assert!(transfer.previous_txids.is_empty());
    assert!(matches!(
        transaction_status(&wallet, &OWN_TXID),
        Some(ConfirmationStatus::Calculated(_))
    ));
    assert_eq!(
        wallet.reserved_output_ids(),
        vec![bound_note.output_id],
        "the note stays reserved while the transaction is on the wire"
    );
}

#[tokio::test]
async fn a_released_transfer_on_the_wire_stays_on_track_and_is_not_settled() {
    const WINDOW: u64 = 1;
    let (wallet, bound_note) = wallet_with_a_released_transfer_on_the_wire(TIP, WINDOW, FAR_EXPIRY);
    let mut client = LightClient::new_for_test(wallet).await;

    let report = client
        .reconcile_migration()
        .await
        .expect("reconciliation runs")
        .expect("a migration exists");

    assert_eq!(report.assessments[0].class, TransferClass::OnTrack);
    assert!(report.actions.is_empty(), "{:?}", report.actions);
    let wallet = client.wallet().read().await;
    let transfer = transfer_of(&wallet, 0);
    assert_eq!(transfer.state, TransferState::Released);
    assert_eq!(transfer.txid, Some(OWN_TXID));
    assert_eq!(wallet.reserved_output_ids(), vec![bound_note.output_id]);
    let state = wallet.migration.as_ref().expect("the migration stands");
    assert_eq!(
        state.phase,
        MigrationPhase::Scheduled,
        "a transaction on the wire keeps the migration from completing"
    );
}

#[tokio::test]
async fn a_released_transfer_whose_transaction_mines_counts_as_confirmed() {
    const MINED_AT: u32 = 350;
    let (mut wallet, bound_note) = wallet_with_a_released_transfer_on_the_wire(TIP, 1, FAR_EXPIRY);
    record_confirmed_transfer_spend(&mut wallet, &bound_note, OWN_TXID, MINED_AT);
    let mut client = LightClient::new_for_test(wallet).await;

    let before = assessment_of(&client).await;
    assert_eq!(before.assessments[0].class, TransferClass::Confirmed);
    assert!(
        before
            .actions
            .contains(&RecommendedAction::PromoteConfirmed {
                transfer: TransferId(0),
                height: BlockHeight::from_u32(MINED_AT),
            })
    );

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    let state = migration_of(&client).await;
    assert_eq!(
        state.transfers[0].state,
        TransferState::Confirmed {
            height: BlockHeight::from_u32(MINED_AT)
        }
    );
    assert_eq!(
        state.phase,
        MigrationPhase::Complete { residual: 0 },
        "the mined transaction settles the released transfer and completes the migration"
    );
    let status = client.migration_status().await.expect("status reads");
    assert_eq!(status.transfers[0].progress, TransferProgress::Confirmed);
    assert_eq!(status.value_migrated, NOTE_VALUE);
}

#[tokio::test]
async fn a_released_transfer_whose_note_a_foreign_transaction_spent_abandons_the_wire() {
    const MINED_AT: u32 = 350;
    let foreign = TxId::from_bytes([0xF0; 32]);
    let (mut wallet, bound_note) = wallet_with_a_released_transfer_on_the_wire(TIP, 1, FAR_EXPIRY);
    record_confirmed_transfer_spend(&mut wallet, &bound_note, foreign, MINED_AT);
    let mut client = LightClient::new_for_test(wallet).await;

    let before = assessment_of(&client).await;
    assert_eq!(before.assessments[0].class, TransferClass::OnTrack);
    assert!(before.actions.contains(&RecommendedAction::AbandonWire {
        transfer: TransferId(0)
    }));

    let settled = client
        .reconcile_migration()
        .await
        .expect("reconciliation runs")
        .expect("a migration exists");

    assert_eq!(
        settled.assessments[0].class,
        TransferClass::Released,
        "with the wire forgotten the transfer reads as released"
    );
    let wallet = client.wallet().read().await;
    let transfer = transfer_of(&wallet, 0);
    assert_eq!(
        transfer.state,
        TransferState::Released,
        "a released transfer is never invalidated"
    );
    assert_eq!(transfer.txid, None, "the wire is forgotten");
    assert_eq!(transfer.previous_txids, vec![OWN_TXID]);
    assert_eq!(transfer.expiry_height, None);
    assert!(
        matches!(
            transaction_status(&wallet, &OWN_TXID),
            Some(ConfirmationStatus::Failed(_))
        ),
        "the abandoned transaction is failed"
    );
    assert!(wallet.reserved_output_ids().is_empty());
}

#[tokio::test]
async fn a_released_transfer_past_its_expiry_abandons_the_wire() {
    let (wallet, _) = wallet_with_a_released_transfer_on_the_wire(TIP, 1, TIP);
    let mut client = LightClient::new_for_test(wallet).await;

    assert!(
        assessment_of(&client)
            .await
            .actions
            .contains(&RecommendedAction::AbandonWire {
                transfer: TransferId(0)
            })
    );

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    let wallet = client.wallet().read().await;
    let transfer = transfer_of(&wallet, 0);
    assert_eq!(transfer.state, TransferState::Released);
    assert_eq!(transfer.txid, None);
    assert_eq!(transfer.previous_txids, vec![OWN_TXID]);
    assert!(matches!(
        transaction_status(&wallet, &OWN_TXID),
        Some(ConfirmationStatus::Failed(_))
    ));
    assert!(wallet.reserved_output_ids().is_empty());
}

#[tokio::test]
async fn a_released_transfer_unmined_one_window_past_its_window_abandons_the_wire() {
    const WINDOW: u64 = 1;
    let (mut wallet, bound_note) =
        wallet_with_a_released_transfer_on_the_wire(TIP, WINDOW, FAR_EXPIRY);
    let params = MigrationParams::provisional(wallet.chain_type());
    let window_end = u32::from(window_end_of(WINDOW, &params));
    let modulus = params.bucket_modulus;

    set_tip(&mut wallet, window_end + modulus - 1);
    let mut client = LightClient::new_for_test(wallet).await;
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    {
        let wallet = client.wallet().read().await;
        assert_eq!(
            transfer_of(&wallet, 0).txid,
            Some(OWN_TXID),
            "one block short of the extra window the transaction may still mine"
        );
        assert_eq!(wallet.reserved_output_ids(), vec![bound_note.output_id]);
    }

    set_tip(&mut *client.wallet().write().await, window_end + modulus);
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    let wallet = client.wallet().read().await;
    let transfer = transfer_of(&wallet, 0);
    assert_eq!(transfer.txid, None, "the extra window passed unmined");
    assert_eq!(transfer.previous_txids, vec![OWN_TXID]);
    assert_eq!(transfer.state, TransferState::Released);
    assert!(matches!(
        transaction_status(&wallet, &OWN_TXID),
        Some(ConfirmationStatus::Failed(_))
    ));
    assert!(wallet.reserved_output_ids().is_empty());
}

#[tokio::test]
async fn reconciliation_completes_in_the_pass_that_abandons_the_last_wire() {
    let (wallet, _) = wallet_with_a_released_transfer_on_the_wire(TIP, 1, TIP);
    let mut client = LightClient::new_for_test(wallet).await;

    let report = client
        .reconcile_migration()
        .await
        .expect("reconciliation runs")
        .expect("a migration exists");

    assert_eq!(report.assessments[0].class, TransferClass::Released);
    assert!(
        report.actions.contains(&RecommendedAction::MarkComplete {
            residual: NOTE_VALUE
        }),
        "the returned report is the one whose actions were applied last: {:?}",
        report.actions
    );
    assert_eq!(
        phase_of(&client).await,
        MigrationPhase::Complete {
            residual: NOTE_VALUE
        },
        "the abandon and the completion happen in one reconciliation call, the freed note as residual"
    );
}

#[tokio::test]
async fn reconciliation_completes_in_the_pass_that_confirms_the_last_transfer() {
    const MINED_AT: u32 = 350;
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    record_confirmed_transfer_spend(&mut wallet, &bound_note, OWN_TXID, MINED_AT);
    wallet.migration = Some(scheduled_state(
        params,
        vec![broadcast_transfer(
            0,
            bound_note,
            current_bucket,
            OWN_TXID,
            FAR_EXPIRY,
        )],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    let report = client
        .reconcile_migration()
        .await
        .expect("reconciliation runs")
        .expect("a migration exists");

    assert!(
        report
            .actions
            .contains(&RecommendedAction::MarkComplete { residual: 0 }),
        "{:?}",
        report.actions
    );
    let state = migration_of(&client).await;
    assert_eq!(
        state.transfers[0].state,
        TransferState::Confirmed {
            height: BlockHeight::from_u32(MINED_AT)
        }
    );
    assert_eq!(state.phase, MigrationPhase::Complete { residual: 0 });
}

#[tokio::test]
async fn cancel_migration_keeps_the_record_until_the_wire_confirms_then_completes() {
    const MINED_AT: u32 = 350;
    let (mut wallet, notes) = wallet_with_notes(TIP, 2);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    insert_calculated_transaction(&mut wallet, OWN_TXID, TIP + 1);
    wallet.migration = Some(scheduled_state(
        params,
        vec![
            broadcast_transfer(0, notes[0], current_bucket, OWN_TXID, FAR_EXPIRY),
            assigned_transfer(1, notes[1], current_bucket + 1),
        ],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    client.cancel_migration().await.expect("cancels");

    {
        let wallet = client.wallet().read().await;
        let state = wallet
            .migration
            .as_ref()
            .expect("a transaction on the wire keeps the record");
        assert_eq!(state.phase, MigrationPhase::Scheduled);
        assert_eq!(state.transfers[0].state, TransferState::Released);
        assert_eq!(state.transfers[0].txid, Some(OWN_TXID));
        assert_eq!(state.transfers[1].state, TransferState::Released);
        assert_eq!(state.transfers[1].txid, None);
        assert_eq!(wallet.reserved_output_ids(), vec![notes[0].output_id]);
    }

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    assert_eq!(
        phase_of(&client).await,
        MigrationPhase::Scheduled,
        "the record waits for the wire to resolve"
    );

    record_confirmed_transfer_spend(
        &mut *client.wallet().write().await,
        &notes[0],
        OWN_TXID,
        MINED_AT,
    );
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    let state = migration_of(&client).await;
    assert_eq!(
        state.transfers[0].state,
        TransferState::Confirmed {
            height: BlockHeight::from_u32(MINED_AT)
        },
        "a released transfer whose transaction mines anyway counts as confirmed"
    );
    assert_eq!(
        state.phase,
        MigrationPhase::Complete {
            residual: NOTE_VALUE
        },
        "the freed note is the residual"
    );
    let status = client.migration_status().await.expect("status reads");
    assert_eq!(status.value_migrated, NOTE_VALUE);
    assert_eq!(status.transfers_confirmed, 1);
    assert!(
        client
            .wallet()
            .read()
            .await
            .reserved_output_ids()
            .is_empty()
    );
}

#[tokio::test]
async fn cancel_migration_keeps_the_record_until_the_wire_is_abandoned_then_completes() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let window_end = u32::from(window_end_of(current_bucket, &params));
    let modulus = params.bucket_modulus;
    insert_calculated_transaction(&mut wallet, OWN_TXID, TIP + 1);
    wallet.migration = Some(scheduled_state(
        params,
        vec![broadcast_transfer(
            0,
            bound_note,
            current_bucket,
            OWN_TXID,
            FAR_EXPIRY,
        )],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    client.cancel_migration().await.expect("cancels");
    assert_eq!(phase_of(&client).await, MigrationPhase::Scheduled);

    set_tip(&mut *client.wallet().write().await, window_end + modulus);
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    let wallet = client.wallet().read().await;
    let state = wallet.migration.as_ref().expect("the record completes");
    assert_eq!(state.transfers[0].state, TransferState::Released);
    assert_eq!(state.transfers[0].txid, None);
    assert_eq!(state.transfers[0].previous_txids, vec![OWN_TXID]);
    assert_eq!(
        state.phase,
        MigrationPhase::Complete {
            residual: NOTE_VALUE
        }
    );
    assert!(matches!(
        transaction_status(&wallet, &OWN_TXID),
        Some(ConfirmationStatus::Failed(_))
    ));
    assert!(wallet.reserved_output_ids().is_empty());
}
