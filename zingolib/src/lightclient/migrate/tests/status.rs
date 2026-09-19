use std::collections::BTreeSet;
use std::time::Duration;

use pepper_sync::wallet::{OrchardNote, OutputId, WalletTransaction};
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::memo::Memo;
use zingo_status::confirmation_status::ConfirmationStatus;
use zip32::AccountId;

use super::fixtures::{
    FAR_EXPIRY, FUNDING_NOTE, NOTE_VALUE, SEED, TIP, UNPREPARED_NOTE, assigned_transfer,
    bound_note_of, broadcast_transfer, committed_client, confirmed_transfer, current_bucket_of,
    migration_of, phase_of as phase_of_client, preparing_state, record_confirmed_transfer_spend,
    scheduled_client, scheduled_state, signed_transfer, wallet_with_funding_notes,
    wallet_with_migration_note, wallet_with_notes, window_end_of,
};
use crate::lightclient::LightClient;
use crate::lightclient::migrate::{MigrationProgress, TransferBroadcastResult, TransferProgress};
use crate::mocks::orchard_note::OrchardCryptoNoteBuilder;
use crate::mocks::transmission::MockBroadcastClient;
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
use crate::wallet::migration::{MigrationParams, MigrationPhase, TransferId, schedule};

#[tokio::test]
async fn migration_status_without_a_migration_is_empty() {
    let (wallet, _) = wallet_with_migration_note(TIP);
    let client = LightClient::new_for_test(wallet).await;
    let status = client.migration_status().await.expect("status reads");
    assert_eq!(status.phase, None);
    assert!(status.transfers.is_empty());
    assert_eq!(status.transfers_total, 0);
    assert_eq!(status.due_now, None);
    assert!(status.upcoming_windows.is_empty());
    let windows = status
        .windows
        .expect("the calendar exists without a migration");
    assert_eq!(windows.len(), 1);
    assert!(windows[0].is_current);
}

#[tokio::test]
async fn migration_status_reports_each_transfers_progress_window_and_missed_windows() {
    let (mut wallet, notes) = wallet_with_notes(TIP, 4);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let modulus = params.bucket_modulus;
    let missed = assigned_transfer(0, notes[0], current_bucket - 1);
    let mut ahead = assigned_transfer(1, notes[1], current_bucket + 2);
    ahead.target_height = Some(schedule::boundary_of(current_bucket + 2, modulus) + 7);
    let in_flight = broadcast_transfer(
        2,
        notes[2],
        current_bucket,
        TxId::from_bytes([2; 32]),
        FAR_EXPIRY,
    );
    let mut confirmed = confirmed_transfer(notes[3], TxId::from_bytes([3; 32]), 300);
    confirmed.id = TransferId(3);
    record_confirmed_transfer_spend(&mut wallet, &notes[3], TxId::from_bytes([3; 32]), 300);
    wallet.migration = Some(scheduled_state(
        params,
        vec![missed, ahead, in_flight, confirmed],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    let status = client.migration_status().await.expect("status reads");

    assert_eq!(status.phase, Some(MigrationPhase::Scheduled));
    assert_eq!(status.transfers.len(), 4);
    let rescheduled = &status.transfers[0];
    assert_eq!(rescheduled.id, TransferId(0));
    assert_eq!(rescheduled.denomination, NOTE_VALUE);
    assert_eq!(rescheduled.progress, TransferProgress::Pending);
    assert_eq!(rescheduled.missed_windows, 1);
    let window = rescheduled
        .window
        .expect("a rescheduled transfer has a window");
    assert!(window > current_bucket);
    assert_eq!(
        rescheduled.boundary,
        Some(schedule::boundary_of(window, modulus))
    );
    assert!(rescheduled.scheduled_broadcast_unix_time.is_some());

    let ahead = &status.transfers[1];
    assert_eq!(ahead.progress, TransferProgress::Pending);
    assert_eq!(ahead.window, Some(current_bucket + 2));
    assert_eq!(ahead.missed_windows, 0);

    assert_eq!(status.transfers[2].progress, TransferProgress::Broadcast);
    assert_eq!(status.transfers[2].window, Some(current_bucket));
    assert_eq!(status.transfers[3].progress, TransferProgress::Confirmed);

    assert_eq!(status.transfers_total, 4);
    assert_eq!(status.transfers_confirmed, 1);
    assert_eq!(status.value_total, 4 * NOTE_VALUE);
    assert_eq!(status.value_migrated, NOTE_VALUE);
}

#[tokio::test]
async fn migration_status_counts_only_transfers_classified_confirmed() {
    const CONFIRMED_AT: u32 = 200;
    let (mut wallet, notes) = wallet_with_notes(TIP, 2);
    let params = MigrationParams::provisional(wallet.chain_type());
    let held = TxId::from_bytes([1; 32]);
    let reorged = TxId::from_bytes([2; 32]);
    record_confirmed_transfer_spend(&mut wallet, &notes[0], held, CONFIRMED_AT);
    let mut second = confirmed_transfer(notes[1], reorged, CONFIRMED_AT);
    second.id = TransferId(1);
    wallet.migration = Some(scheduled_state(
        params,
        vec![confirmed_transfer(notes[0], held, CONFIRMED_AT), second],
    ));
    let client = LightClient::new_for_test(wallet).await;

    let status = client.migration_status().await.expect("status reads");

    assert_eq!(status.transfers[0].progress, TransferProgress::Confirmed);
    assert_eq!(
        status.transfers[1].progress,
        TransferProgress::Broadcast,
        "a confirmed record whose block is gone is not confirmed"
    );
    assert_eq!(status.transfers_confirmed, 1);
    assert_eq!(status.value_migrated, NOTE_VALUE);
    assert_eq!(status.transfers_total, 2);
    assert_eq!(status.value_total, 2 * NOTE_VALUE);
}

#[tokio::test]
async fn value_migrated_ignores_ironwood_funds_from_other_sources() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(NOTE_VALUE)
        .ironwood_note(77_777)
        .tip(TIP)
        .build();
    let bound_note = bound_note_of(&wallet, NOTE_VALUE);
    let mut wallet = wallet;
    let params = MigrationParams::provisional(wallet.chain_type());
    let transfer_txid = TxId::from_bytes([0xE1; 32]);
    record_confirmed_transfer_spend(&mut wallet, &bound_note, transfer_txid, 300);
    wallet.migration = Some(scheduled_state(
        params,
        vec![confirmed_transfer(bound_note, transfer_txid, 300)],
    ));
    let client = LightClient::new_for_test(wallet).await;

    let status = client.migration_status().await.expect("status reads");
    assert_eq!(
        status.value_migrated, NOTE_VALUE,
        "only confirmed transfer denominations count, never the whole Ironwood balance"
    );
}

#[tokio::test]
async fn due_now_names_exactly_what_broadcast_due_transfers_attempts() {
    const OPEN_TIP: u32 = 300;
    let (mut wallet, notes) = wallet_with_notes(OPEN_TIP, 4);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let window_end = u32::from(window_end_of(current_bucket, &params));
    let signed = signed_transfer(
        0,
        notes[0],
        current_bucket,
        TxId::from_bytes([7; 32]),
        window_end,
        Some(vec![0xAB; 64]),
    );
    let mut target_ahead = assigned_transfer(1, notes[1], current_bucket);
    target_ahead.target_height = Some(BlockHeight::from_u32(OPEN_TIP + 50));
    let future = assigned_transfer(2, notes[2], current_bucket + 1);
    let in_flight = broadcast_transfer(
        3,
        notes[3],
        current_bucket,
        TxId::from_bytes([3; 32]),
        FAR_EXPIRY,
    );
    wallet.migration = Some(scheduled_state(
        params.clone(),
        vec![signed, target_ahead, future, in_flight],
    ));
    let mut client = LightClient::new_for_test(wallet).await;

    let batch = client
        .migration_status()
        .await
        .expect("status reads")
        .due_now
        .expect("the open window owes a batch");
    assert_eq!(
        batch.boundary,
        schedule::boundary_of(current_bucket, params.bucket_modulus)
    );
    assert_eq!(batch.denominations, vec![NOTE_VALUE, NOTE_VALUE]);
    let advertised: BTreeSet<TransferId> = batch.transfer_ids.iter().copied().collect();

    let mock = MockBroadcastClient::default();
    let report = client
        .broadcast_due_transfers_with(&mock, Duration::ZERO)
        .await
        .expect("the batch runs");
    let attempted: BTreeSet<TransferId> = report
        .outcomes
        .iter()
        .map(|outcome| outcome.transfer)
        .collect();

    assert_eq!(
        advertised, attempted,
        "due_now is the set a broadcast attempts"
    );
    assert_eq!(
        advertised,
        BTreeSet::from([TransferId(0), TransferId(1)]),
        "the signed and the assigned transfer of the open window, nothing in flight or ahead"
    );
    assert!(matches!(
        report.outcomes[0].result,
        TransferBroadcastResult::Sent(_)
    ));
    assert_eq!(report.outcomes[1].result, TransferBroadcastResult::Slid);
    assert_eq!(
        client
            .migration_status()
            .await
            .expect("status reads")
            .due_now
            .expect("the slid transfer is still due")
            .transfer_ids,
        vec![TransferId(1)],
        "after the batch the sent transfer is in flight and only the slid one is still due"
    );
}

#[tokio::test]
async fn due_now_is_none_off_phase_and_once_every_transfer_settled() {
    let wallet = wallet_with_funding_notes(TIP, 1);
    let client = committed_client(wallet).await;
    assert_eq!(
        client
            .migration_status()
            .await
            .expect("status reads")
            .due_now,
        None,
        "before the schedule nothing is due"
    );

    let wallet = super::fixtures::wallet_with_one_confirmed_transfer(TIP, None);
    let client = LightClient::new_for_test(wallet).await;
    assert_eq!(
        client
            .migration_status()
            .await
            .expect("status reads")
            .due_now,
        None,
        "a fully confirmed schedule offers no batch"
    );
}

#[tokio::test]
async fn the_first_window_is_due_the_moment_the_schedule_is_committed() {
    let wallet = wallet_with_funding_notes(TIP, 2);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let client = scheduled_client(wallet, 1).await;

    let status = client.migration_status().await.expect("status reads");
    let batch = status
        .due_now
        .expect("the first window opens in the current bucket");
    assert_eq!(batch.transfer_ids, vec![TransferId(0)]);
    assert_eq!(
        batch.boundary,
        schedule::boundary_of(current_bucket, params.bucket_modulus)
    );
    assert_eq!(status.transfers[0].window, Some(current_bucket));
}

#[tokio::test]
async fn upcoming_windows_are_strictly_future() {
    let (mut wallet, notes) = wallet_with_notes(TIP, 4);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let modulus = params.bucket_modulus;
    wallet.migration = Some(scheduled_state(
        params,
        vec![
            assigned_transfer(0, notes[0], current_bucket),
            assigned_transfer(1, notes[1], current_bucket + 1),
            assigned_transfer(2, notes[2], current_bucket + 3),
            assigned_transfer(3, notes[3], current_bucket + 3),
        ],
    ));
    let client = LightClient::new_for_test(wallet).await;

    let status = client.migration_status().await.expect("status reads");

    let windows: Vec<u64> = status
        .upcoming_windows
        .iter()
        .map(|window| window.bucket_index)
        .collect();
    assert_eq!(
        windows,
        vec![current_bucket + 1, current_bucket + 3],
        "the open window is due_now, not upcoming"
    );
    assert_eq!(
        status.upcoming_windows[1].transfer_ids,
        vec![TransferId(2), TransferId(3)]
    );
    assert_eq!(
        status.upcoming_windows[0].boundary,
        schedule::boundary_of(current_bucket + 1, modulus)
    );
    assert!(
        status.upcoming_windows[0].window_opens_unix_time
            <= status.upcoming_windows[0].latest_target_unix_time
    );
    assert_eq!(
        status
            .due_now
            .expect("the open window owes a batch")
            .transfer_ids,
        vec![TransferId(0)]
    );
    let timeline = status.windows.expect("the calendar exists");
    let listed: Vec<u64> = timeline.iter().map(|window| window.bucket_index).collect();
    assert_eq!(
        listed,
        vec![current_bucket, current_bucket + 1, current_bucket + 3]
    );
    assert!(timeline[0].is_current);
}

#[tokio::test]
async fn a_missed_transfer_is_not_advertised_as_due() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    wallet.migration = Some(scheduled_state(
        params,
        vec![assigned_transfer(0, bound_note, 0)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");

    let status = client.migration_status().await.expect("status reads");
    assert_eq!(status.due_now, None);
    assert_eq!(status.transfers[0].missed_windows, 1);
    assert_eq!(status.transfers[0].progress, TransferProgress::Pending);
    assert_eq!(
        status.upcoming_windows.len(),
        1,
        "the rescheduled window is upcoming"
    );
    assert_eq!(
        *client.migration_progress().borrow(),
        MigrationProgress::Idle
    );
}

#[tokio::test]
async fn committed_phase_status_projects_the_plan() {
    const TRANSFERS_OF_TEN_ZEC: [u64; 9] = [
        500_000_000,
        200_000_000,
        200_000_000,
        50_000_000,
        20_000_000,
        20_000_000,
        5_000_000,
        2_000_000,
        2_000_000,
    ];
    const PREPARATION_FEE: u64 = 11 * 5_000;
    const RESIDUAL: u64 = 765_000;
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(UNPREPARED_NOTE)
        .tip(TIP)
        .build();
    let client = committed_client(wallet).await;
    assert_eq!(phase_of_client(&client).await, MigrationPhase::Committed);

    let status = client.migration_status().await.expect("status reads");

    assert_eq!(status.phase, Some(MigrationPhase::Committed));
    assert!(status.transfers.is_empty(), "nothing is bound yet");
    assert_eq!(
        status.transfers_total, 9,
        "ten ZEC decompose greedily down the 1-2-5 ladder into nine denominations"
    );
    assert_eq!(status.value_total, 999_000_000);
    assert_eq!(status.value_total, TRANSFERS_OF_TEN_ZEC.iter().sum::<u64>());
    assert_eq!(
        status.value_total
            + 9 * crate::wallet::migration::preparation::CANONICAL_TRANSFER_FEE
            + PREPARATION_FEE
            + RESIDUAL,
        UNPREPARED_NOTE,
        "the denominations, nine transfer fees, one eleven-action preparation fee and the residual account for the note"
    );
    assert_eq!(status.transfers_confirmed, 0);
    assert_eq!(status.value_migrated, 0);
    assert_eq!(status.orchard_confirmed_spendable, UNPREPARED_NOTE);
    assert_eq!(status.due_now, None);
}

#[tokio::test]
async fn prepared_phase_status_projects_the_plan() {
    let wallet = wallet_with_funding_notes(TIP, 3);
    let client = committed_client(wallet).await;

    let status = client.migration_status().await.expect("status reads");
    assert_eq!(status.phase, Some(MigrationPhase::Prepared));
    assert_eq!(status.transfers_total, 3);
    assert_eq!(status.value_total, 3 * NOTE_VALUE);
    assert_eq!(status.due_now, None);
    assert!(migration_of(&client).await.transfers.is_empty());
}

#[tokio::test]
async fn mid_round_status_projects_over_the_pending_outputs() {
    const PENDING_OUTPUTS: [u64; 2] = [600_000_000, 600_000_000];
    const TRANSFERS_OF_TWELVE_ZEC: [u64; 8] = [
        1_000_000_000,
        100_000_000,
        50_000_000,
        20_000_000,
        20_000_000,
        5_000_000,
        2_000_000,
        2_000_000,
    ];
    const PREPARATION_FEE: u64 = 11 * 5_000;
    const RESIDUAL: u64 = 785_000;
    let mut wallet = SyntheticWalletBuilder::new(SEED).tip(TIP).build();
    let round_txid = TxId::from_bytes([7; 32]);
    let outputs = PENDING_OUTPUTS
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let crypto_note = OrchardCryptoNoteBuilder::default()
                .value(orchard::value::NoteValue::from_raw(*value))
                .note_version(orchard::NoteVersion::V2)
                .build();
            OrchardNote::new_for_test(
                OutputId::new(round_txid, u32::try_from(index).expect("two outputs")),
                AccountId::ZERO,
                zip32::Scope::External,
                crypto_note,
                Memo::Empty,
                None,
            )
        })
        .collect();
    wallet.wallet_transactions.insert(
        round_txid,
        WalletTransaction::new_for_test_with_orchard_notes(
            round_txid,
            ConfirmationStatus::Transmitted(BlockHeight::from_u32(TIP + 1)),
            outputs,
            vec![],
        ),
    );
    let params = MigrationParams::provisional(wallet.chain_type());
    wallet.migration = Some(preparing_state(
        params,
        MigrationPhase::Preparing {
            round: 0,
            pending_txids: vec![round_txid],
        },
    ));
    let client = LightClient::new_for_test(wallet).await;

    let status = client.migration_status().await.expect("status reads");

    assert_eq!(
        status.transfers_total, 8,
        "the two pending six-ZEC outputs merge into one twelve-ZEC decomposition of eight denominations"
    );
    assert_eq!(
        status.value_total, 1_199_000_000,
        "a round in flight counts as its pending outputs"
    );
    assert_eq!(
        status.value_total,
        TRANSFERS_OF_TWELVE_ZEC.iter().sum::<u64>()
    );
    assert_eq!(
        status.value_total
            + 8 * crate::wallet::migration::preparation::CANONICAL_TRANSFER_FEE
            + PREPARATION_FEE
            + RESIDUAL,
        PENDING_OUTPUTS.iter().sum::<u64>(),
        "the denominations, eight transfer fees, one eleven-action preparation fee and the residual account for the pending value"
    );
    assert!(status.transfers.is_empty());
    assert_eq!(status.transfers_confirmed, 0);
    assert_eq!(status.value_migrated, 0);
    assert_eq!(
        status.orchard_confirmed_spendable, 0,
        "unconfirmed outputs are not spendable"
    );
}

#[tokio::test]
async fn status_reads_the_orchard_pool_balance_specifically() {
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(FUNDING_NOTE)
        .ironwood_note(123_456)
        .tip(TIP)
        .build();
    let client = committed_client(wallet).await;
    let status = client.migration_status().await.expect("status reads");
    assert_eq!(
        status.orchard_confirmed_spendable, FUNDING_NOTE,
        "the Orchard figure excludes Ironwood funds"
    );
}

#[tokio::test]
async fn migration_status_without_a_migration_reports_the_orchard_balance_of_account_zero() {
    const IRONWOOD: u64 = 77_777;
    let wallet = SyntheticWalletBuilder::new(SEED)
        .orchard_note(NOTE_VALUE)
        .orchard_note(UNPREPARED_NOTE)
        .ironwood_note(IRONWOOD)
        .tip(TIP)
        .build();
    let spent = bound_note_of(&wallet, UNPREPARED_NOTE);
    let mut wallet = wallet;
    super::fixtures::mark_note_spent_by(&mut wallet, &spent, TxId::from_bytes([9; 32]));
    let client = LightClient::new_for_test(wallet).await;

    let status = client.migration_status().await.expect("status reads");

    assert_eq!(status.phase, None);
    assert_eq!(
        status.orchard_confirmed_spendable, NOTE_VALUE,
        "the Orchard figure is account zero's unspent pre-Ironwood balance, without the Ironwood note or the spent note"
    );
}
