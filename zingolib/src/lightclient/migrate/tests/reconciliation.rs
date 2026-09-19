use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zingo_status::confirmation_status::ConfirmationStatus;

use super::fixtures::{
    FAR_EXPIRY, FUNDING_NOTE, NOTE_VALUE, TIP, assessment_of, assigned_transfer,
    broadcast_transfer, confirmed_transfer, current_bucket_of, insert_calculated_transaction,
    mark_note_spent_by, migration_of, phase_of, record_confirmed_transfer_spend, scheduled_state,
    set_tip, set_tip_with_unscanned_gap, signed_transfer, transaction_status,
    wallet_with_migration_note, wallet_with_one_confirmed_transfer, window_end_of,
};
use crate::lightclient::LightClient;
use crate::lightclient::migrate::TransferProgress;
use crate::wallet::LightWallet;
use crate::wallet::migration::{
    MigrationParams, MigrationPhase, RecommendedAction, TransferClass, TransferRecord,
    TransferState,
};

fn transfer_of(wallet: &LightWallet, index: usize) -> TransferRecord {
    wallet
        .migration
        .as_ref()
        .expect("the migration exists")
        .transfers[index]
        .clone()
}

fn assert_placed_in_a_later_window(transfer: &TransferRecord, current_bucket: u64, modulus: u32) {
    let bucket = transfer
        .bucket_index
        .expect("a rescheduled transfer has a window");
    assert!(
        bucket > current_bucket,
        "window {bucket} is strictly after the current bucket {current_bucket}"
    );
    let anchor = transfer
        .anchor_bucket
        .expect("a rescheduled transfer has a fresh anchor");
    assert!(
        anchor < bucket,
        "the anchor {anchor} sits below the window {bucket}"
    );
    let target = transfer
        .target_height
        .expect("a rescheduled transfer has a scheduled broadcast height");
    let boundary = crate::wallet::migration::schedule::boundary_of(bucket, modulus);
    let close = crate::wallet::migration::schedule::boundary_of(bucket + 1, modulus);
    assert!(
        boundary <= target && target < close,
        "the scheduled broadcast height {target} lies inside the window"
    );
    assert!(
        transfer.anchor_witness.is_none(),
        "a fresh anchor has no stale witness"
    );
}

#[tokio::test]
async fn reconciliation_reschedules_a_transfer_whose_window_closed() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let modulus = params.bucket_modulus;
    wallet.migration = Some(scheduled_state(
        params,
        vec![assigned_transfer(0, bound_note, current_bucket - 1)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    let before = assessment_of(&client).await;
    assert_eq!(before.assessments[0].class, TransferClass::Missed);
    assert!(before.actions.contains(&RecommendedAction::Reschedule {
        transfer: crate::wallet::migration::TransferId(0)
    }));

    let settled = client
        .reconcile_migration()
        .await
        .expect("reconciliation runs")
        .expect("a migration exists");

    assert_eq!(
        settled.assessments[0].class,
        TransferClass::OnTrack,
        "the rescheduled transfer is on track in its new window"
    );
    assert!(settled.actions.is_empty(), "{:?}", settled.actions);
    let transfer = transfer_of(&*client.wallet().read().await, 0);
    assert_eq!(transfer.state, TransferState::Assigned);
    assert_eq!(transfer.missed_windows, 1);
    assert_placed_in_a_later_window(&transfer, current_bucket, modulus);
    assert_eq!(transfer.attempts, 0);
}

#[tokio::test]
async fn reconciliation_counts_every_missed_window() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let modulus = params.bucket_modulus;
    wallet.migration = Some(scheduled_state(
        params.clone(),
        vec![assigned_transfer(0, bound_note, 0)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let first = transfer_of(&*client.wallet().read().await, 0);
    assert_eq!(first.missed_windows, 1);
    let first_window = first.bucket_index.expect("placed");

    let later_tip = u32::from(window_end_of(first_window, &params)) + 10;
    set_tip(&mut *client.wallet().write().await, later_tip);
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    let second = transfer_of(&*client.wallet().read().await, 0);
    assert_eq!(second.missed_windows, 2, "each missed window is counted");
    assert_placed_in_a_later_window(
        &second,
        crate::wallet::migration::bucket_index(BlockHeight::from_u32(later_tip), modulus),
        modulus,
    );
}

#[tokio::test]
async fn reconciliation_discards_an_unattempted_signature_past_its_window() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let modulus = params.bucket_modulus;
    let txid = TxId::from_bytes([7; 32]);
    let transfer = signed_transfer(
        0,
        bound_note,
        current_bucket - 1,
        txid,
        FAR_EXPIRY,
        Some(vec![0xAB; 64]),
    );
    assert_eq!(transfer.attempts, 0, "premise: never submitted");
    insert_calculated_transaction(&mut wallet, txid, TIP - 100);
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    let mut client = LightClient::new_for_test(wallet).await;

    assert_eq!(
        assessment_of(&client).await.assessments[0].class,
        TransferClass::Missed
    );

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let wallet = client.wallet().read().await;
    let transfer = transfer_of(&wallet, 0);
    assert_eq!(transfer.state, TransferState::Assigned, "re-signed later");
    assert_eq!(transfer.txid, None);
    assert_eq!(transfer.expiry_height, None);
    assert!(transfer.signed_blob.is_none());
    assert_eq!(
        transfer.previous_txids,
        vec![txid],
        "the discarded txid stays the transfer's own"
    );
    assert_eq!(transfer.missed_windows, 1);
    assert_placed_in_a_later_window(&transfer, current_bucket, modulus);
    assert!(
        matches!(
            transaction_status(&wallet, &txid),
            Some(ConfirmationStatus::Failed(_))
        ),
        "the wallet transaction is failed"
    );
}

#[tokio::test]
async fn reconciliation_keeps_a_submitted_transfer_for_one_extra_window() {
    const WINDOW: u64 = 1;
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let window_end = u32::from(window_end_of(WINDOW, &params));
    let modulus = params.bucket_modulus;
    let evidence_below_the_extra_window = window_end + modulus - 1;
    assert!(evidence_below_the_extra_window > window_end);
    set_tip(&mut wallet, evidence_below_the_extra_window);
    let txid = TxId::from_bytes([7; 32]);
    let transfer = broadcast_transfer(0, bound_note, WINDOW, txid, FAR_EXPIRY);
    insert_calculated_transaction(&mut wallet, txid, window_end - 10);
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    let mut client = LightClient::new_for_test(wallet).await;

    let report = client
        .reconcile_migration()
        .await
        .expect("reconciliation runs")
        .expect("a migration exists");

    assert_eq!(
        report.assessments[0].class,
        TransferClass::OnTrack,
        "a submitted transaction may still mine one window after its own closed"
    );
    assert!(report.actions.is_empty(), "{:?}", report.actions);
    let wallet = client.wallet().read().await;
    let transfer = transfer_of(&wallet, 0);
    assert_eq!(transfer.state, TransferState::Broadcast);
    assert_eq!(transfer.txid, Some(txid));
    assert_eq!(transfer.bucket_index, Some(WINDOW));
    assert!(matches!(
        transaction_status(&wallet, &txid),
        Some(ConfirmationStatus::Calculated(_))
    ));
}

#[tokio::test]
async fn reconciliation_discards_a_submitted_transfer_once_the_extra_window_passed() {
    const WINDOW: u64 = 1;
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let window_end = u32::from(window_end_of(WINDOW, &params));
    let modulus = params.bucket_modulus;
    let evidence_past_the_extra_window = window_end + modulus;
    set_tip(&mut wallet, evidence_past_the_extra_window);
    let current_bucket = current_bucket_of(&wallet, &params);
    let txid = TxId::from_bytes([7; 32]);
    let transfer = broadcast_transfer(0, bound_note, WINDOW, txid, FAR_EXPIRY);
    insert_calculated_transaction(&mut wallet, txid, window_end - 10);
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    let mut client = LightClient::new_for_test(wallet).await;

    assert_eq!(
        assessment_of(&client).await.assessments[0].class,
        TransferClass::Missed
    );

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let wallet = client.wallet().read().await;
    let transfer = transfer_of(&wallet, 0);
    assert_eq!(transfer.state, TransferState::Assigned);
    assert_eq!(transfer.previous_txids, vec![txid]);
    assert_eq!(transfer.txid, None);
    assert_eq!(transfer.missed_windows, 1);
    assert_placed_in_a_later_window(&transfer, current_bucket, modulus);
    assert!(matches!(
        transaction_status(&wallet, &txid),
        Some(ConfirmationStatus::Failed(_))
    ));
}

#[tokio::test]
async fn reconciliation_discards_an_expired_signature() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let modulus = params.bucket_modulus;
    let txid = TxId::from_bytes([7; 32]);
    let transfer = broadcast_transfer(0, bound_note, current_bucket, txid, TIP - 1);
    insert_calculated_transaction(&mut wallet, txid, TIP - 100);
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    let mut client = LightClient::new_for_test(wallet).await;

    assert_eq!(
        assessment_of(&client).await.assessments[0].class,
        TransferClass::Expired
    );

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let wallet = client.wallet().read().await;
    let transfer = transfer_of(&wallet, 0);
    assert_eq!(transfer.state, TransferState::Assigned);
    assert_eq!(transfer.previous_txids, vec![txid]);
    assert_placed_in_a_later_window(&transfer, current_bucket, modulus);
    assert!(matches!(
        transaction_status(&wallet, &txid),
        Some(ConfirmationStatus::Failed(_))
    ));
}

#[tokio::test]
async fn reconciliation_keeps_a_submitted_transfer_whose_expiry_lies_in_the_unscanned_gap() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let txid = TxId::from_bytes([7; 32]);
    let transfer = broadcast_transfer(0, bound_note, 1, txid, 400);
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    set_tip_with_unscanned_gap(&mut wallet, TIP, 600);
    let mut client = LightClient::new_for_test(wallet).await;

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    let transfer = transfer_of(&*client.wallet().read().await, 0);
    assert_eq!(
        transfer.txid,
        Some(txid),
        "condemning against the unscanned tip would invite a false invalidation"
    );
    assert_eq!(transfer.state, TransferState::Broadcast);
}

#[tokio::test]
async fn reconciliation_confirms_a_transfer_spent_by_a_discarded_txid() {
    const MINED_AT: u32 = 350;
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let old_txid = TxId::from_bytes([7; 32]);
    let mut transfer = assigned_transfer(0, bound_note, current_bucket + 1);
    transfer.previous_txids.push(old_txid);
    transfer.missed_windows = 1;
    record_confirmed_transfer_spend(&mut wallet, &bound_note, old_txid, MINED_AT);
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    let mut client = LightClient::new_for_test(wallet).await;

    assert_eq!(
        assessment_of(&client).await.assessments[0].class,
        TransferClass::Confirmed
    );

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let transfer = transfer_of(&*client.wallet().read().await, 0);
    assert_eq!(
        transfer.state,
        TransferState::Confirmed {
            height: BlockHeight::from_u32(MINED_AT)
        },
        "a late confirmation of a discarded signature is the transfer's own"
    );
}

#[tokio::test]
async fn reconciliation_invalidates_a_transfer_whose_note_a_foreign_transaction_spent() {
    const MINED_AT: u32 = 350;
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let foreign = TxId::from_bytes([0xF0; 32]);
    record_confirmed_transfer_spend(&mut wallet, &bound_note, foreign, MINED_AT);
    wallet.migration = Some(scheduled_state(
        params,
        vec![assigned_transfer(0, bound_note, current_bucket + 1)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    assert_eq!(
        assessment_of(&client).await.assessments[0].class,
        TransferClass::Invalidated
    );

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    assert_eq!(
        transfer_of(&*client.wallet().read().await, 0).state,
        TransferState::Invalidated
    );
    let status = client.migration_status().await.expect("status reads");
    assert_eq!(status.transfers[0].progress, TransferProgress::Invalid);
}

#[tokio::test]
async fn reconciliation_invalidates_a_confirmed_transfer_whose_note_a_foreign_transaction_holds() {
    const CONFIRMED_AT: u32 = 200;
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let own_txid = TxId::from_bytes([7; 32]);
    let foreign = TxId::from_bytes([0xF0; 32]);
    record_confirmed_transfer_spend(&mut wallet, &bound_note, foreign, CONFIRMED_AT);
    wallet.migration = Some(scheduled_state(
        params,
        vec![confirmed_transfer(bound_note, own_txid, CONFIRMED_AT)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    assert_eq!(
        transfer_of(&*client.wallet().read().await, 0).state,
        TransferState::Invalidated
    );
}

#[tokio::test]
async fn reconciliation_demotes_a_reorged_confirmed_transfer_without_completing() {
    const CONFIRMED_AT: u32 = 200;
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let own_txid = TxId::from_bytes([7; 32]);
    wallet.migration = Some(scheduled_state(
        params,
        vec![confirmed_transfer(bound_note, own_txid, CONFIRMED_AT)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    let before = assessment_of(&client).await;
    assert_eq!(before.assessments[0].class, TransferClass::Reorged);
    assert!(
        !before
            .actions
            .iter()
            .any(|action| matches!(action, RecommendedAction::MarkComplete { .. })),
        "a reorged transfer is not settled: {:?}",
        before.actions
    );

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let state = migration_of(&client).await;
    assert_eq!(state.phase, MigrationPhase::Scheduled);
    assert_eq!(state.transfers[0].state, TransferState::Broadcast);
    assert_eq!(state.transfers[0].txid, Some(own_txid));
    let status = client.migration_status().await.expect("status reads");
    assert_eq!(status.transfers_confirmed, 0);
    assert_eq!(status.value_migrated, 0);
    assert_eq!(status.transfers[0].progress, TransferProgress::Broadcast);
}

#[tokio::test]
async fn reconciliation_reopens_a_completed_migration_holding_a_reorged_transfer() {
    const CONFIRMED_AT: u32 = 200;
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let mut state = scheduled_state(
        params,
        vec![confirmed_transfer(
            bound_note,
            TxId::from_bytes([7; 32]),
            CONFIRMED_AT,
        )],
    );
    state.phase = MigrationPhase::Complete { residual: 0 };
    wallet.migration = Some(state);
    let mut client = LightClient::new_for_test(wallet).await;

    let before = assessment_of(&client).await;
    assert_eq!(before.assessments[0].class, TransferClass::Reorged);
    assert!(before.actions.contains(&RecommendedAction::Reopen));

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let state = migration_of(&client).await;
    assert_eq!(state.phase, MigrationPhase::Scheduled);
    assert_eq!(state.transfers[0].state, TransferState::Broadcast);
}

#[tokio::test]
async fn reconciliation_completes_only_with_spend_evidence_at_the_tip() {
    let mut wallet = wallet_with_one_confirmed_transfer(TIP, None);
    set_tip_with_unscanned_gap(&mut wallet, TIP, 600);
    let mut client = LightClient::new_for_test(wallet).await;

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    assert_eq!(
        phase_of(&client).await,
        MigrationPhase::Scheduled,
        "evidence short of the tip cannot conclude the migration"
    );

    set_tip(&mut *client.wallet().write().await, 600);
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    assert_eq!(
        phase_of(&client).await,
        MigrationPhase::Complete { residual: 0 }
    );
}

#[tokio::test]
async fn reconciliation_completes_with_a_fundable_orchard_balance_left() {
    let wallet = wallet_with_one_confirmed_transfer(TIP, Some(FUNDING_NOTE));
    let mut client = LightClient::new_for_test(wallet).await;

    let report = client
        .reconcile_migration()
        .await
        .expect("reconciliation runs")
        .expect("a migration exists");

    assert!(report.actions.contains(&RecommendedAction::MarkComplete {
        residual: FUNDING_NOTE
    }));
    assert_eq!(
        phase_of(&client).await,
        MigrationPhase::Complete {
            residual: FUNDING_NOTE
        },
        "a leftover that could fund a transfer never stalls completion"
    );
    let status = client.migration_status().await.expect("status reads");
    assert_eq!(status.transfers_confirmed, 1);
    assert_eq!(status.value_migrated, NOTE_VALUE);
    assert_eq!(status.orchard_confirmed_spendable, FUNDING_NOTE);
    assert!(
        client
            .wallet()
            .read()
            .await
            .reserved_output_ids()
            .is_empty(),
        "a completed migration reserves nothing"
    );
}

#[tokio::test]
async fn a_rescan_does_not_complete_the_migration_without_evidence() {
    const RECEIPT: u64 = 700_000;
    let mut twin = wallet_with_one_confirmed_transfer(TIP, Some(RECEIPT));
    let transactions = std::mem::take(&mut twin.wallet_transactions);
    let sync_state = std::mem::replace(&mut twin.sync_state, pepper_sync::wallet::SyncState::new());

    let mut wallet = wallet_with_one_confirmed_transfer(TIP, Some(RECEIPT));
    wallet.clear_all();
    let mut client = LightClient::new_for_test(wallet).await;

    client
        .reconcile_migration()
        .await
        .expect("reconciliation during a rescan must not error");
    assert_eq!(
        phase_of(&client).await,
        MigrationPhase::Scheduled,
        "a rescan holds no evidence to conclude against"
    );

    {
        let mut wallet = client.wallet().write().await;
        wallet.wallet_transactions = transactions;
        wallet.sync_state = sync_state;
    }
    client
        .reconcile_migration()
        .await
        .expect("reconciliation after the rescan runs");
    assert_eq!(
        phase_of(&client).await,
        MigrationPhase::Complete { residual: RECEIPT },
        "the residual is derived from the restored evidence"
    );
}

#[tokio::test]
async fn reconciliation_on_a_wallet_without_a_migration_does_nothing() {
    let (wallet, _) = wallet_with_migration_note(TIP);
    let mut client = LightClient::new_for_test(wallet).await;
    client.wallet().write().await.save_required = false;
    let report = client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    assert!(report.is_none());
    assert!(!client.wallet().read().await.save_required);
}

#[tokio::test]
async fn reconciliation_leaves_an_open_window_transfer_on_track() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let transfer = assigned_transfer(0, bound_note, current_bucket);
    wallet.migration = Some(scheduled_state(params, vec![transfer.clone()]));
    let mut client = LightClient::new_for_test(wallet).await;

    let before = assessment_of(&client).await;
    assert_eq!(before.assessments[0].class, TransferClass::OnTrack);
    assert!(before.actions.is_empty());

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let after = transfer_of(&*client.wallet().read().await, 0);
    assert_eq!(after.bucket_index, transfer.bucket_index);
    assert_eq!(after.missed_windows, 0);
}

#[tokio::test]
async fn reconciliation_ignores_a_pending_spend_by_the_transfers_own_transaction() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let txid = TxId::from_bytes([7; 32]);
    let transfer = broadcast_transfer(0, bound_note, current_bucket, txid, FAR_EXPIRY);
    mark_note_spent_by(&mut wallet, &bound_note, txid);
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    let mut client = LightClient::new_for_test(wallet).await;

    assert_eq!(
        assessment_of(&client).await.assessments[0].class,
        TransferClass::OnTrack
    );

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    assert_eq!(
        transfer_of(&*client.wallet().read().await, 0).state,
        TransferState::Broadcast,
        "an unconfirmed own spend is still in flight"
    );
}

#[tokio::test]
async fn the_capture_pass_replaces_a_cached_witness_whose_anchor_is_not_the_boundary_root() {
    use pepper_sync::wallet::{NoteInterface as _, OrchardNote, OutputInterface as _};
    use shardtree::store::{Checkpoint, ShardStore as _};

    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let anchor_bucket = current_bucket - 1;
    let boundary =
        crate::wallet::migration::schedule::boundary_of(anchor_bucket, params.bucket_modulus);
    let position = wallet
        .wallet_transactions
        .values()
        .flat_map(OrchardNote::transaction_outputs)
        .find(|note| note.value() == NOTE_VALUE)
        .and_then(|note| note.position())
        .expect("the fabricated note is scanned into the tree");
    wallet
        .shard_trees
        .orchard
        .store_mut()
        .add_checkpoint(boundary - 60, Checkpoint::at_position(position))
        .expect("infallible on the memory store");
    let mut transfer = assigned_transfer(0, bound_note, current_bucket);
    transfer.anchor_bucket = Some(anchor_bucket);
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    wallet
        .refresh_transfer_witnesses()
        .expect("the capture pass reads the tree only");
    let captured = transfer_of(&wallet, 0)
        .anchor_witness
        .expect("the boundary anchors at the checkpoint below it");
    wallet.save_required = false;

    wallet
        .refresh_transfer_witnesses()
        .expect("the capture pass reads the tree only");
    assert_eq!(
        transfer_of(&wallet, 0).anchor_witness.as_ref(),
        Some(&captured),
        "a witness whose anchor is the boundary root is kept"
    );
    assert!(!wallet.save_required, "an unchanged witness writes nothing");

    let stale = crate::wallet::migration::BoundaryWitness {
        anchor: [0xFF; 32],
        position: captured.position,
        auth_path: vec![[0xEE; 32]; 3],
    };
    wallet.migration.as_mut().expect("stands").transfers[0].anchor_witness = Some(stale.clone());
    wallet
        .refresh_transfer_witnesses()
        .expect("the capture pass reads the tree only");

    let refreshed = transfer_of(&wallet, 0)
        .anchor_witness
        .expect("the witness is recaptured in the same pass");
    assert_ne!(refreshed, stale, "the stale witness is gone");
    assert_eq!(
        refreshed, captured,
        "the recaptured witness carries the wallet's current root at the boundary"
    );
    assert!(wallet.save_required, "the refreshed witness is dirty");
}
