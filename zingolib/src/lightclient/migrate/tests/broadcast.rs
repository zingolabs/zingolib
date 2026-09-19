use std::sync::atomic::Ordering;
use std::time::Duration;

use pepper_sync::wallet::{NoteInterface as _, OrchardNote, OutputInterface as _};
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zip32::AccountId;

use super::fixtures::{
    NOTE_VALUE, TIP, assigned_transfer, committed_client, current_bucket_of, migration_error,
    mixnet_receipt, scheduled_state, signed_transfer, wallet_with_funding_notes,
    wallet_with_migration_note, wallet_with_notes, window_end_of,
};
use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, MigrationError};
use crate::lightclient::migrate::{
    BatchReport, MigrationProgress, TransferBroadcastResult, TransferOutcome,
};
use crate::mocks::transmission::MockBroadcastClient;
use crate::wallet::LightWallet;
use crate::wallet::migration::{
    BroadcastClient, BroadcastReceipt, BroadcastRoute, MigrationParams, TransferBroadcastError,
    TransferId, TransferState,
};
use pepper_sync::wallet::SyncMode;

const NO_SPACING: Duration = Duration::ZERO;

fn wallet_with_a_signed_open_window_transfer(tip: u32, txid: TxId) -> LightWallet {
    let (mut wallet, bound_note) = wallet_with_migration_note(tip);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let window_end = u32::from(window_end_of(current_bucket, &params));
    let transfer = signed_transfer(
        0,
        bound_note,
        current_bucket,
        txid,
        window_end,
        Some(vec![0xAB; 64]),
    );
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    wallet
}

fn transfer_of(wallet: &LightWallet, index: usize) -> crate::wallet::migration::TransferRecord {
    wallet
        .migration
        .as_ref()
        .expect("the migration exists")
        .transfers[index]
        .clone()
}

#[tokio::test]
async fn broadcast_due_transfers_refuses_without_a_migration() {
    let (wallet, _) = wallet_with_migration_note(TIP);
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    let result = client.broadcast_due_transfers(NO_SPACING).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::NoMigration
    ));
}

#[tokio::test]
async fn broadcast_due_transfers_refuses_before_the_schedule_is_committed() {
    let wallet = wallet_with_funding_notes(TIP, 1);
    let mut client = committed_client(wallet).await;
    let result = client.broadcast_due_transfers(NO_SPACING).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::NotScheduled
    ));
    let result = client.broadcast_missed_now(NO_SPACING).await;
    assert!(matches!(
        migration_error(result),
        MigrationError::NotScheduled
    ));
}

#[tokio::test]
async fn broadcast_due_transfers_refuses_offline_once_scheduled() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    wallet.migration = Some(scheduled_state(
        params,
        vec![assigned_transfer(0, bound_note, current_bucket)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    let result = client.broadcast_due_transfers(NO_SPACING).await;
    assert!(
        matches!(result, Err(LightClientError::Offline)),
        "no endpoint resolves without an indexer, got {result:?}"
    );
}

#[tokio::test]
async fn broadcast_due_transfers_reports_nothing_when_no_window_is_open() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    wallet.migration = Some(scheduled_state(
        params,
        vec![assigned_transfer(0, bound_note, current_bucket + 1)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    client.migration_transmission_uri = Some("http://127.0.0.1:1".parse().expect("static uri"));

    let report = client
        .broadcast_due_transfers(NO_SPACING)
        .await
        .expect("nothing due is not an error");
    assert_eq!(report, BatchReport::default());
    assert_eq!(transfer_of(&*client.wallet().read().await, 0).attempts, 0);
}

#[tokio::test]
async fn broadcast_due_transfers_attempts_the_open_window_and_slides_without_a_witness() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let known_height = wallet
        .sync_state
        .last_known_chain_height()
        .expect("synced synthetic wallet");
    wallet.migration = Some(scheduled_state(
        params,
        vec![assigned_transfer(0, bound_note, current_bucket)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    let mock = MockBroadcastClient::default();

    let report = client
        .broadcast_due_transfers_with(&mock, NO_SPACING)
        .await
        .expect("the batch runs");

    assert_eq!(
        report.outcomes,
        vec![TransferOutcome {
            transfer: TransferId(0),
            denomination: NOTE_VALUE,
            result: TransferBroadcastResult::Slid,
        }]
    );
    assert!(report.halted.is_none());
    assert!(
        mock.submissions.lock().unwrap().is_empty(),
        "a slid transfer reaches nothing"
    );
    let wallet = client.wallet().read().await;
    let transfer = transfer_of(&wallet, 0);
    assert_eq!(
        transfer.state,
        TransferState::Assigned,
        "a slide writes nothing"
    );
    assert_eq!(transfer.attempts, 0, "a slide records no attempt");
    assert_eq!(
        wallet.sync_state.last_known_chain_height(),
        Some(known_height),
        "the broadcast never synchronizes"
    );
    assert_eq!(client.sync_mode(), SyncMode::NotRunning);
}

#[tokio::test]
async fn broadcast_due_transfers_sends_a_signed_transfer_of_the_open_window() {
    const OPEN_TIP: u32 = 300;
    let own_txid = TxId::from_bytes([7; 32]);
    let mut client = LightClient::new_for_test(wallet_with_a_signed_open_window_transfer(
        OPEN_TIP, own_txid,
    ))
    .await;
    let mock = MockBroadcastClient::default();

    let report = client
        .broadcast_due_transfers_with(&mock, NO_SPACING)
        .await
        .expect("the batch runs");

    assert_eq!(
        report.outcomes,
        vec![TransferOutcome {
            transfer: TransferId(0),
            denomination: NOTE_VALUE,
            result: TransferBroadcastResult::Sent(mixnet_receipt(own_txid)),
        }]
    );
    assert_eq!(report.halted, None);
    assert_eq!(report.sent_txids(), vec![own_txid]);
    {
        let submissions = mock.submissions.lock().unwrap();
        assert_eq!(submissions.len(), 1, "the endpoint received the transfer");
        assert_eq!(
            submissions[0].0,
            vec![0xAB; 64],
            "the signed bytes were submitted"
        );
    }
    let transfer = transfer_of(&*client.wallet().read().await, 0);
    assert_eq!(transfer.state, TransferState::Broadcast);
    assert_eq!(transfer.attempts, 1, "the attempt is recorded");
    assert_eq!(
        *client.migration_progress().borrow(),
        MigrationProgress::Idle,
        "the progress channel returns to idle"
    );
}

#[tokio::test]
async fn broadcast_due_transfers_attempts_a_transfer_whose_target_is_still_ahead() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let mut transfer = assigned_transfer(0, bound_note, current_bucket);
    transfer.target_height = Some(BlockHeight::from_u32(TIP + 60));
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    let mock = MockBroadcastClient::default();

    let report = client
        .broadcast_due_transfers_with(&mock, NO_SPACING)
        .await
        .expect("the batch runs");

    assert!(
        matches!(
            report.outcomes[..],
            [TransferOutcome {
                result: TransferBroadcastResult::Slid,
                ..
            }]
        ),
        "the scheduled broadcast height never gates the open window: {:?}",
        report.outcomes
    );
}

#[tokio::test]
async fn a_refused_submission_is_reported_failed_and_the_transfer_stays_signed() {
    const OPEN_TIP: u32 = 300;
    let own_txid = TxId::from_bytes([7; 32]);
    let mut client = LightClient::new_for_test(wallet_with_a_signed_open_window_transfer(
        OPEN_TIP, own_txid,
    ))
    .await;
    let mock = MockBroadcastClient::default();
    mock.fail.store(true, Ordering::Relaxed);

    let report = client
        .broadcast_due_transfers_with(&mock, NO_SPACING)
        .await
        .expect("the batch runs");

    let halted = report
        .halted
        .as_deref()
        .expect("a refused submission halts the batch");
    assert!(
        halted.contains("mock transport failure"),
        "the halt carries the submission error, got {halted:?}"
    );
    assert_eq!(
        report.outcomes,
        vec![TransferOutcome {
            transfer: TransferId(0),
            denomination: NOTE_VALUE,
            result: TransferBroadcastResult::Failed {
                error: halted.to_string(),
            },
        }]
    );
    assert!(report.sent_txids().is_empty());
    {
        let transfer = transfer_of(&*client.wallet().read().await, 0);
        assert_eq!(transfer.state, TransferState::Signed);
        assert_eq!(transfer.attempts, 1, "the refusal counts as an attempt");
        assert_eq!(transfer.txid, Some(own_txid));
    }

    mock.fail.store(false, Ordering::Relaxed);
    let retry = client
        .broadcast_due_transfers_with(&mock, NO_SPACING)
        .await
        .expect("the batch runs");
    assert_eq!(
        retry.outcomes,
        vec![TransferOutcome {
            transfer: TransferId(0),
            denomination: NOTE_VALUE,
            result: TransferBroadcastResult::Sent(mixnet_receipt(own_txid)),
        }],
        "the next batch resubmits the same transfer"
    );
    assert!(retry.halted.is_none());
    assert_eq!(
        transfer_of(&*client.wallet().read().await, 0).state,
        TransferState::Broadcast
    );
}

#[tokio::test]
async fn a_refusal_halts_the_batch_after_the_accepted_transfers() {
    const OPEN_TIP: u32 = 300;
    let (mut wallet, notes) = wallet_with_notes(OPEN_TIP, 3);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let window_end = u32::from(window_end_of(current_bucket, &params));
    let txids = [
        TxId::from_bytes([1; 32]),
        TxId::from_bytes([2; 32]),
        TxId::from_bytes([3; 32]),
    ];
    let transfers = txids
        .iter()
        .enumerate()
        .map(|(index, txid)| {
            signed_transfer(
                index as u32,
                notes[index],
                current_bucket,
                *txid,
                window_end,
                Some(vec![index as u8; 64]),
            )
        })
        .collect();
    wallet.migration = Some(scheduled_state(params, transfers));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    let mock = MockBroadcastClient::default();
    mock.fail_from.store(1, Ordering::Relaxed);

    let report = client
        .broadcast_due_transfers_with(&mock, NO_SPACING)
        .await
        .expect("the batch runs");

    let halted = report
        .halted
        .as_deref()
        .expect("the refusal halts the batch");
    assert_eq!(
        report.outcomes,
        vec![
            TransferOutcome {
                transfer: TransferId(0),
                denomination: NOTE_VALUE,
                result: TransferBroadcastResult::Sent(mixnet_receipt(txids[0])),
            },
            TransferOutcome {
                transfer: TransferId(1),
                denomination: NOTE_VALUE,
                result: TransferBroadcastResult::Failed {
                    error: halted.to_string(),
                },
            },
        ],
        "the accepted transfer is Sent, the refused one Failed, the rest has no outcome"
    );
    assert_eq!(mock.submissions.lock().unwrap().len(), 1);
    {
        let wallet = client.wallet().read().await;
        assert_eq!(transfer_of(&wallet, 0).state, TransferState::Broadcast);
        assert_eq!(transfer_of(&wallet, 1).state, TransferState::Signed);
        assert_eq!(transfer_of(&wallet, 1).attempts, 1);
        assert_eq!(transfer_of(&wallet, 2).state, TransferState::Signed);
        assert_eq!(
            transfer_of(&wallet, 2).attempts,
            0,
            "the transfer after the halt is not attempted"
        );
    }

    mock.fail_from.store(usize::MAX, Ordering::Relaxed);
    let retry = client
        .broadcast_due_transfers_with(&mock, NO_SPACING)
        .await
        .expect("the batch runs");
    assert_eq!(
        retry.sent_txids(),
        vec![txids[1], txids[2]],
        "only the transfers not yet accepted are submitted"
    );
    assert_eq!(
        mock.submissions.lock().unwrap().len(),
        3,
        "each transfer is accepted exactly once"
    );
}

#[tokio::test]
async fn a_failed_transfer_reports_the_whole_cause_chain() {
    const OPEN_TIP: u32 = 300;
    let (mut wallet, bound_note) = wallet_with_migration_note(OPEN_TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let window_end = u32::from(window_end_of(current_bucket, &params));
    let own_txid = TxId::from_bytes([7; 32]);
    let transfer = signed_transfer(0, bound_note, current_bucket, own_txid, window_end, None);
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    let mock = MockBroadcastClient::default();

    let report = client
        .broadcast_due_transfers_with(&mock, NO_SPACING)
        .await
        .expect("the batch runs");

    let halted = report.halted.expect("the batch halted on the failure");
    assert!(
        halted.contains(&own_txid.to_string()),
        "the report carries the whole cause chain, got {halted:?}"
    );
    let [
        TransferOutcome {
            result: TransferBroadcastResult::Failed { error },
            ..
        },
    ] = &report.outcomes[..]
    else {
        panic!(
            "the one transfer is reported failed, got {:?}",
            report.outcomes
        );
    };
    assert_eq!(*error, halted);
}

struct AlreadyKnownClient;

impl BroadcastClient for AlreadyKnownClient {
    async fn submit(
        &self,
        _raw_tx: Vec<u8>,
        _expiry_height: BlockHeight,
    ) -> Result<BroadcastReceipt, TransferBroadcastError> {
        Err(TransferBroadcastError::AlreadyKnown {
            route: BroadcastRoute::Clearnet {
                endpoint: "known.example".to_string(),
            },
        })
    }
}

#[tokio::test]
async fn an_already_known_transaction_counts_as_sent() {
    const OPEN_TIP: u32 = 300;
    let own_txid = TxId::from_bytes([7; 32]);
    let mut client = LightClient::new_for_test(wallet_with_a_signed_open_window_transfer(
        OPEN_TIP, own_txid,
    ))
    .await;

    let report = client
        .broadcast_due_transfers_with(&AlreadyKnownClient, NO_SPACING)
        .await
        .expect("the batch runs");

    assert_eq!(
        report.outcomes,
        vec![TransferOutcome {
            transfer: TransferId(0),
            denomination: NOTE_VALUE,
            result: TransferBroadcastResult::Sent(BroadcastReceipt {
                txid: own_txid,
                route: BroadcastRoute::Clearnet {
                    endpoint: "known.example".to_string(),
                },
            }),
        }]
    );
    assert!(report.halted.is_none());
    assert_eq!(
        transfer_of(&*client.wallet().read().await, 0).state,
        TransferState::Broadcast
    );
    let history = client.indexer_history_handle().load();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].outcome, Ok(()));
}

#[tokio::test]
async fn a_failed_submission_is_recorded_in_the_indexer_history() {
    const OPEN_TIP: u32 = 300;
    let mut client = LightClient::new_for_test(wallet_with_a_signed_open_window_transfer(
        OPEN_TIP,
        TxId::from_bytes([7; 32]),
    ))
    .await;
    let mock = MockBroadcastClient::default();
    mock.fail.store(true, Ordering::Relaxed);

    let report = client
        .broadcast_due_transfers_with(&mock, NO_SPACING)
        .await
        .expect("the batch runs");
    assert!(report.halted.is_some(), "premise: the submission failed");

    let history = client.indexer_history_handle().load();
    assert_eq!(
        history.len(),
        1,
        "the failed submission leaves one attempt in the history, got {history:?}"
    );
    assert!(history[0].outcome.is_err());
}

#[test]
fn transfer_route_evidence_records_the_typed_category() {
    use crate::lightclient::indexer_history::{FailureKind, IndexerHistoryHandle};

    let history = IndexerHistoryHandle::default();
    crate::lightclient::migrate::record_transfer_route(
        &history,
        &BroadcastRoute::Clearnet {
            endpoint: "indexer.example".to_string(),
        },
        std::time::Instant::now(),
        Err(FailureKind::Rejected),
    );
    let recorded = history.load();
    assert_eq!(recorded.len(), 1, "one attempt is recorded");
    assert_eq!(recorded[0].outcome, Err(FailureKind::Rejected));
}

#[tokio::test]
async fn a_missed_transfer_does_not_block_the_open_window() {
    const LATE_TIP: u32 = 700;
    let (mut wallet, notes) = wallet_with_notes(LATE_TIP, 2);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let window_end = u32::from(window_end_of(current_bucket, &params));
    let missed = assigned_transfer(0, notes[0], current_bucket - 2);
    let on_time_txid = TxId::from_bytes([9; 32]);
    let on_time = signed_transfer(
        1,
        notes[1],
        current_bucket,
        on_time_txid,
        window_end,
        Some(vec![0xCD; 64]),
    );
    wallet.migration = Some(scheduled_state(params, vec![missed, on_time]));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let mock = MockBroadcastClient::default();

    let report = client
        .broadcast_due_transfers_with(&mock, NO_SPACING)
        .await
        .expect("the batch runs");

    assert_eq!(report.sent_txids(), vec![on_time_txid]);
    assert_eq!(mock.submissions.lock().unwrap().len(), 1);
    let wallet = client.wallet().read().await;
    let rescheduled = transfer_of(&wallet, 0);
    assert_eq!(rescheduled.state, TransferState::Assigned);
    assert!(
        rescheduled
            .bucket_index
            .is_some_and(|bucket| bucket > current_bucket),
        "the missed transfer waits in a later window, not the open one"
    );
}

#[tokio::test]
async fn broadcast_missed_now_moves_only_missed_transfers_into_the_current_window() {
    let (mut wallet, notes) = wallet_with_notes(TIP, 3);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let missed = assigned_transfer(0, notes[0], 0);
    let ahead = assigned_transfer(1, notes[1], current_bucket + 2);
    let also_ahead = assigned_transfer(2, notes[2], current_bucket + 3);
    wallet.migration = Some(scheduled_state(params, vec![missed, ahead, also_ahead]));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    client.migration_transmission_uri = Some("http://127.0.0.1:1".parse().expect("static uri"));

    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    {
        let wallet = client.wallet().read().await;
        let rescheduled = transfer_of(&wallet, 0);
        assert_eq!(rescheduled.missed_windows, 1, "premise: one window missed");
        assert_ne!(rescheduled.bucket_index, Some(current_bucket));
        assert_eq!(transfer_of(&wallet, 1).missed_windows, 0);
    }

    let report = client
        .broadcast_missed_now(NO_SPACING)
        .await
        .expect("send now runs");

    assert_eq!(
        report.outcomes.len(),
        1,
        "only the missed transfer is attempted: {report:?}"
    );
    assert_eq!(report.outcomes[0].transfer, TransferId(0));
    assert_eq!(report.outcomes[0].result, TransferBroadcastResult::Slid);
    let wallet = client.wallet().read().await;
    let moved = transfer_of(&wallet, 0);
    assert_eq!(
        moved.bucket_index,
        Some(current_bucket),
        "the missed transfer was moved into the current window"
    );
    assert_eq!(
        moved.target_height, None,
        "it fires the moment the window is open"
    );
    assert!(
        moved
            .anchor_bucket
            .is_some_and(|anchor| anchor < current_bucket),
        "a fresh anchor below the current window was drawn"
    );
    assert_eq!(
        moved.missed_windows, 1,
        "the missed count is history, not reset"
    );
    assert_eq!(
        transfer_of(&wallet, 1).bucket_index,
        Some(current_bucket + 2),
        "a transfer that missed nothing stays where it is"
    );
    assert_eq!(
        transfer_of(&wallet, 2).bucket_index,
        Some(current_bucket + 3)
    );
}

#[tokio::test]
async fn broadcast_missed_now_with_nothing_missed_is_an_ordinary_batch() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    wallet.migration = Some(scheduled_state(
        params,
        vec![assigned_transfer(0, bound_note, current_bucket + 1)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    client.migration_transmission_uri = Some("http://127.0.0.1:1".parse().expect("static uri"));

    let report = client
        .broadcast_missed_now(NO_SPACING)
        .await
        .expect("send now runs");
    assert_eq!(report, BatchReport::default());
    assert_eq!(
        transfer_of(&*client.wallet().read().await, 0).bucket_index,
        Some(current_bucket + 1)
    );
}

#[tokio::test]
async fn an_unreachable_endpoint_reports_the_transfer_failed() {
    const OPEN_TIP: u32 = 300;
    let own_txid = TxId::from_bytes([7; 32]);
    let mut client = LightClient::new_for_test(wallet_with_a_signed_open_window_transfer(
        OPEN_TIP, own_txid,
    ))
    .await;
    client.consent_to_clearnet_for_tests().await;
    client.migration_transmission_uri = Some("http://127.0.0.1:1".parse().expect("static uri"));

    let report = client
        .broadcast_due_transfers(NO_SPACING)
        .await
        .expect("an unreachable endpoint is a per-transfer failure, not a command error");

    assert!(report.halted.is_some(), "the batch halted: {report:?}");
    assert!(matches!(
        report.outcomes[..],
        [TransferOutcome {
            transfer: TransferId(0),
            result: TransferBroadcastResult::Failed { .. },
            ..
        }]
    ));
    let transfer = transfer_of(&*client.wallet().read().await, 0);
    assert_eq!(transfer.state, TransferState::Signed);
    assert_eq!(transfer.attempts, 1);
}

mod transmission_client_selection {
    use super::super::fixtures::{TIP, wallet_with_migration_note};
    use crate::lightclient::LightClient;
    use crate::lightclient::error::LightClientError;

    #[tokio::test]
    async fn a_session_with_no_endpoint_refuses_offline() {
        let (wallet, _) = wallet_with_migration_note(TIP);
        let mut client = LightClient::new_for_test(wallet).await;
        client.consent_to_clearnet_for_tests().await;
        let result = client.migration_transmission_client();
        assert!(
            matches!(result, Err(LightClientError::Offline)),
            "no indexer and no migration endpoint resolve nothing"
        );
    }

    #[tokio::test]
    async fn a_configured_migration_endpoint_resolves_a_client() {
        let (wallet, _) = wallet_with_migration_note(TIP);
        let mut client = LightClient::new_for_test(wallet).await;
        client.consent_to_clearnet_for_tests().await;
        client.migration_transmission_uri =
            Some("https://migration.example:443".parse().expect("static uri"));
        assert!(
            client.migration_transmission_client().is_ok(),
            "a configured migration endpoint is the transmission target"
        );
    }

    #[tokio::test]
    async fn the_sync_indexer_carries_transfers_when_nothing_else_is_configured() {
        let (wallet, _) = wallet_with_migration_note(TIP);
        let mut client = LightClient::new_for_test(wallet).await;
        client.consent_to_clearnet_for_tests().await;
        client
            .set_indexer_uri_lazy("https://indexer.example:443".parse().expect("static uri"))
            .expect("a lazy indexer needs no connection");
        assert!(
            client.migration_transmission_client().is_ok(),
            "the synchronization endpoint carries transfers over clearnet"
        );
    }
}

#[tokio::test]
async fn every_sent_transfer_carries_the_receipt_of_its_wire() {
    const OPEN_TIP: u32 = 300;
    let (mut wallet, notes) = wallet_with_notes(OPEN_TIP, 2);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let window_end = u32::from(window_end_of(current_bucket, &params));
    let transfers = (0..2)
        .map(|index| {
            signed_transfer(
                index,
                notes[index as usize],
                current_bucket,
                TxId::from_bytes([index as u8 + 1; 32]),
                window_end,
                Some(vec![index as u8; 64]),
            )
        })
        .collect();
    wallet.migration = Some(scheduled_state(params, transfers));
    let mut client = LightClient::new_for_test(wallet).await;
    let mixnet = MockBroadcastClient::default();
    let clearnet = MockBroadcastClient::clearnet();

    let first = client
        .broadcast_due_transfers_with(&mixnet, NO_SPACING)
        .await
        .expect("the batch runs");
    let routes_of = |report: &BatchReport| -> Vec<BroadcastRoute> {
        report
            .outcomes
            .iter()
            .filter_map(|outcome| match &outcome.result {
                TransferBroadcastResult::Sent(receipt) => Some(receipt.route.clone()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(first.sent_txids().len(), 2, "{first:?}");
    assert!(
        routes_of(&first).iter().all(BroadcastRoute::is_mixnet),
        "a mixnet session's receipts all name the mixnet: {first:?}"
    );

    let mut leaked = client.wallet().write().await;
    for transfer in leaked
        .migration
        .as_mut()
        .expect("stands")
        .transfers
        .iter_mut()
    {
        transfer.state = TransferState::Signed;
    }
    drop(leaked);
    let second = client
        .broadcast_due_transfers_with(&clearnet, NO_SPACING)
        .await
        .expect("the batch runs");

    assert_eq!(second.sent_txids().len(), 2, "{second:?}");
    let routes = routes_of(&second);
    assert!(
        routes.iter().any(|route| !route.is_mixnet()),
        "a clearnet submission is visible in the report as the leak it is: {routes:?}"
    );
    assert!(
        routes.iter().all(|route| matches!(route, BroadcastRoute::Clearnet { endpoint } if endpoint == "mock.clearnet.indexer")),
        "each receipt names the endpoint the mock answered with: {routes:?}"
    );
    assert_eq!(
        clearnet.submissions.lock().unwrap().len(),
        second.sent_txids().len(),
        "every sent transfer reached the wire exactly once"
    );
}

mod no_sync_data_preserves_migration_state {
    use pepper_sync::wallet::SyncState;

    use super::*;
    use crate::wallet::error::WalletError;

    async fn broadcast_error_must_preserve_the_state(
        empty_the_sync_state: impl FnOnce(&mut LightWallet),
    ) {
        let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
        let params = MigrationParams::provisional(wallet.chain_type());
        wallet.migration = Some(scheduled_state(
            params,
            vec![assigned_transfer(0, bound_note, 0)],
        ));

        empty_the_sync_state(&mut wallet);
        assert!(wallet.sync_state.last_known_chain_height().is_none());
        assert!(
            wallet.migration.is_some(),
            "emptying the sync data keeps the migration"
        );

        let mut client = LightClient::new_for_test(wallet).await;

        client.consent_to_clearnet_for_tests().await;
        let mock = MockBroadcastClient::default();
        let result = client.broadcast_due_transfers_with(&mock, NO_SPACING).await;
        assert!(
            matches!(
                result,
                Err(LightClientError::WalletError(WalletError::NoSyncData))
            ),
            "the broadcast fails with NoSyncData, got {result:?}"
        );

        assert!(
            client.wallet().read().await.migration.is_some(),
            "an error before the restore must not destroy the migration state"
        );
    }

    #[tokio::test]
    async fn via_clear_all() {
        broadcast_error_must_preserve_the_state(LightWallet::clear_all).await;
    }

    #[tokio::test]
    async fn via_empty_sync_state() {
        broadcast_error_must_preserve_the_state(|wallet| {
            wallet.sync_state = SyncState::new();
        })
        .await;
    }
}

#[tokio::test]
async fn a_boundary_without_its_own_checkpoint_anchors_below_it() {
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

    assert!(
        transfer_of(&wallet, 0).anchor_witness.is_some(),
        "the boundary anchors at the checkpoint below it"
    );
}

mod per_transfer_conditions_skip {
    use super::*;
    use crate::wallet::migration::transfers::SkipReason;
    use crate::wallet::migration::{BoundNote, BoundaryWitness, BuildResult};

    fn assigned_transfer_with_witness(
        bound_note: BoundNote,
        bucket: u64,
        anchor_bucket: u64,
    ) -> crate::wallet::migration::TransferRecord {
        let mut transfer = assigned_transfer(0, bound_note, bucket);
        transfer.anchor_bucket = Some(anchor_bucket);
        transfer.anchor_witness = Some(BoundaryWitness {
            anchor: [0; 32],
            position: 0,
            auth_path: Vec::new(),
        });
        transfer
    }

    #[test]
    fn a_spent_funding_note_skips() {
        let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
        let params = MigrationParams::provisional(wallet.chain_type());
        let bucket = current_bucket_of(&wallet, &params);
        super::super::fixtures::mark_note_spent_by(
            &mut wallet,
            &bound_note,
            TxId::from_bytes([9; 32]),
        );

        let mut transfer = assigned_transfer_with_witness(bound_note, bucket, bucket - 1);
        let result = wallet
            .build_transfer(AccountId::ZERO, &mut transfer, &params)
            .expect("a spent funding note is a skip, not an error");
        assert!(matches!(
            result,
            BuildResult::Skip(SkipReason::BoundNoteSpent { bound })
                if bound == bound_note.output_id
        ));
    }

    #[test]
    fn a_mismatched_nullifier_skips() {
        let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
        let params = MigrationParams::provisional(wallet.chain_type());
        let bucket = current_bucket_of(&wallet, &params);
        let diverged = BoundNote {
            nullifier: [0xAA; 32],
            ..bound_note
        };

        let mut transfer = assigned_transfer_with_witness(diverged, bucket, bucket - 1);
        let result = wallet
            .build_transfer(AccountId::ZERO, &mut transfer, &params)
            .expect("a diverged funding note is a skip, not an error");
        assert!(matches!(
            result,
            BuildResult::Skip(SkipReason::BoundNoteMismatch { bound })
                if bound == bound_note.output_id
        ));
    }

    #[test]
    fn a_pre_activation_anchor_skips() {
        let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
        let params = MigrationParams::provisional(wallet.chain_type());

        let mut transfer = assigned_transfer_with_witness(bound_note, 1, 0);
        let result = wallet
            .build_transfer(AccountId::ZERO, &mut transfer, &params)
            .expect("a pre-activation anchor is a skip, not an error");
        assert!(matches!(
            result,
            BuildResult::Skip(SkipReason::BoundaryBeforeActivation { .. })
        ));
    }

    #[test]
    fn an_undrawn_anchor_skips_until_the_capture_pass_draws_it() {
        let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
        let params = MigrationParams::provisional(wallet.chain_type());
        let bucket = current_bucket_of(&wallet, &params);

        let mut transfer = assigned_transfer_with_witness(bound_note, bucket, bucket - 1);
        transfer.anchor_bucket = None;
        let result = wallet
            .build_transfer(AccountId::ZERO, &mut transfer, &params)
            .expect("a missing anchor is a skip, not an error");
        assert!(matches!(
            result,
            BuildResult::Skip(SkipReason::AnchorNotDrawn)
        ));

        transfer.anchor_witness = None;
        wallet.migration = Some(scheduled_state(params, vec![transfer]));
        wallet
            .refresh_transfer_witnesses()
            .expect("the capture pass draws a missing anchor");
        let drawn = transfer_of(&wallet, 0)
            .anchor_bucket
            .expect("the capture pass drew an anchor");
        assert!(drawn < bucket, "the drawn anchor is below the open window");
    }
}

#[tokio::test]
async fn a_signed_transfer_past_its_expiry_is_not_broadcast_late() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let expired_txid = TxId::from_bytes([7; 32]);
    let transfer = signed_transfer(
        0,
        bound_note,
        current_bucket,
        expired_txid,
        TIP - 10,
        Some(vec![0xAB; 64]),
    );
    super::fixtures::insert_calculated_transaction(&mut wallet, expired_txid, TIP - 100);
    wallet.migration = Some(scheduled_state(params, vec![transfer]));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    client.migration_transmission_uri = Some("http://127.0.0.1:1".parse().expect("static uri"));

    let report = client
        .broadcast_due_transfers(NO_SPACING)
        .await
        .expect("the command runs");

    assert_eq!(
        report,
        BatchReport::default(),
        "the command's own reconciliation discards the expired signature before anything is due"
    );
    let wallet = client.wallet().read().await;
    let transfer = transfer_of(&wallet, 0);
    assert_eq!(transfer.state, TransferState::Assigned);
    assert_eq!(transfer.txid, None, "the signature was discarded");
    assert_eq!(transfer.previous_txids, vec![expired_txid]);
    assert!(
        transfer
            .bucket_index
            .is_some_and(|bucket| bucket > current_bucket),
        "the transfer waits in a coming window"
    );
    assert_eq!(
        transfer.attempts, 0,
        "the expired bytes never reached the wire"
    );
    assert!(
        matches!(
            super::fixtures::transaction_status(&wallet, &expired_txid),
            Some(zingo_status::confirmation_status::ConfirmationStatus::Failed(_))
        ),
        "the expired transaction is failed in the wallet"
    );
}

#[tokio::test]
async fn broadcast_missed_now_discards_a_submitted_signature_behind_the_current_window() {
    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let submitted_txid = TxId::from_bytes([7; 32]);
    let behind = super::fixtures::broadcast_transfer(
        0,
        bound_note,
        current_bucket - 1,
        submitted_txid,
        super::fixtures::FAR_EXPIRY,
    );
    super::fixtures::insert_calculated_transaction(&mut wallet, submitted_txid, TIP - 100);
    wallet.migration = Some(scheduled_state(params, vec![behind]));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    client.migration_transmission_uri = Some("http://127.0.0.1:1".parse().expect("static uri"));
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    assert_eq!(
        transfer_of(&*client.wallet().read().await, 0).state,
        TransferState::Broadcast,
        "premise: reconciliation keeps a submitted transfer for one extra window"
    );

    let report = client
        .broadcast_missed_now(NO_SPACING)
        .await
        .expect("send now runs");

    assert_eq!(
        report.outcomes,
        vec![TransferOutcome {
            transfer: TransferId(0),
            denomination: NOTE_VALUE,
            result: TransferBroadcastResult::Slid,
        }],
        "the re-signed transfer is attempted in the current window"
    );
    let wallet = client.wallet().read().await;
    let moved = transfer_of(&wallet, 0);
    assert_eq!(
        moved.state,
        TransferState::Assigned,
        "re-signed from scratch"
    );
    assert_eq!(moved.txid, None);
    assert_eq!(moved.expiry_height, None);
    assert!(moved.signed_blob.is_none());
    assert_eq!(
        moved.previous_txids,
        vec![submitted_txid],
        "the old txid is archived"
    );
    assert_eq!(moved.bucket_index, Some(current_bucket));
    assert_eq!(
        moved.target_height, None,
        "it fires the moment the window is open"
    );
    assert!(
        moved
            .anchor_bucket
            .is_some_and(|anchor| anchor < current_bucket),
        "a fresh anchor below the current window was drawn"
    );
    assert!(
        matches!(
            super::fixtures::transaction_status(&wallet, &submitted_txid),
            Some(zingo_status::confirmation_status::ConfirmationStatus::Failed(_))
        ),
        "the discarded transaction is failed in the wallet"
    );
}

#[tokio::test]
async fn broadcast_missed_now_skips_a_transfer_with_no_legal_anchor_in_the_current_window() {
    use zingo_common_components::protocol::ActivationHeights;

    const LATE_ACTIVATION: u32 = 300;
    let heights = ActivationHeights::builder()
        .set_overwinter(Some(1))
        .set_sapling(Some(1))
        .set_blossom(Some(1))
        .set_heartwood(Some(1))
        .set_canopy(Some(1))
        .set_nu5(Some(1))
        .set_nu6(Some(1))
        .set_nu6_1(Some(1))
        .set_nu6_2(Some(1))
        .set_nu6_3(Some(LATE_ACTIVATION))
        .set_nu7(None)
        .build();
    let mut wallet =
        crate::testutils::synthetic_wallet::SyntheticWalletBuilder::new(super::fixtures::SEED)
            .orchard_note(NOTE_VALUE)
            .activation_heights(heights)
            .tip(TIP)
            .build();
    let bound_note = super::fixtures::bound_note_of(&wallet, NOTE_VALUE);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let activation_bucket = crate::wallet::migration::bucket_index(
        BlockHeight::from_u32(LATE_ACTIVATION),
        params.bucket_modulus,
    );
    assert!(
        activation_bucket + 1 >= current_bucket,
        "premise: no anchor bucket above the activation sits below the current window"
    );
    wallet.migration = Some(scheduled_state(
        params,
        vec![assigned_transfer(0, bound_note, 0)],
    ));
    let mut client = LightClient::new_for_test(wallet).await;
    client.consent_to_clearnet_for_tests().await;
    client.migration_transmission_uri = Some("http://127.0.0.1:1".parse().expect("static uri"));
    client
        .reconcile_migration()
        .await
        .expect("reconciliation runs");
    let rescheduled = transfer_of(&*client.wallet().read().await, 0);
    assert_eq!(
        rescheduled.missed_windows, 1,
        "premise: the transfer missed a window"
    );
    let waiting_in = rescheduled.bucket_index.expect("placed");
    assert!(waiting_in > current_bucket);

    let report = client
        .broadcast_missed_now(NO_SPACING)
        .await
        .expect("send now runs and skips what it cannot anchor");

    assert_eq!(
        report,
        BatchReport::default(),
        "nothing could be moved into the current window"
    );
    let untouched = transfer_of(&*client.wallet().read().await, 0);
    assert_eq!(
        untouched.bucket_index,
        Some(waiting_in),
        "the transfer keeps its legal window"
    );
    assert_eq!(untouched.state, TransferState::Assigned);
    assert_eq!(untouched.attempts, 0);
}
