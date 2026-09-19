use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pepper_sync::wallet::SyncMode;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

use super::fixtures::{
    current_bucket_of, scheduled_state, signed_transfer, wallet_with_notes, window_end_of,
};
use crate::data::PollReport;
use crate::lightclient::LightClient;
use crate::lightclient::migrate::{BatchPhase, MigrationProgress, TransferBroadcastResult};
use crate::mocks::transmission::MockBroadcastClient;
use crate::wallet::LightWallet;
use crate::wallet::migration::{
    BroadcastClient, BroadcastReceipt, BroadcastRoute, MigrationParams, TransferBroadcastError,
};

const OPEN_TIP: u32 = 300;

fn wallet_with_signed_open_window_transfers(count: usize) -> LightWallet {
    let (mut wallet, notes) = wallet_with_notes(OPEN_TIP, count);
    let params = MigrationParams::provisional(wallet.chain_type());
    let current_bucket = current_bucket_of(&wallet, &params);
    let window_end = u32::from(window_end_of(current_bucket, &params));
    let transfers = (0..count)
        .map(|index| {
            signed_transfer(
                index as u32,
                notes[index],
                current_bucket,
                TxId::from_bytes([index as u8 + 1; 32]),
                window_end,
                Some(vec![index as u8; 64]),
            )
        })
        .collect();
    wallet.migration = Some(scheduled_state(params, transfers));
    wallet
}

fn receipt() -> BroadcastReceipt {
    BroadcastReceipt {
        txid: crate::mocks::default_txid(),
        route: BroadcastRoute::Clearnet {
            endpoint: "observer.example".to_string(),
        },
    }
}

struct ClockReadingClient {
    submitted_at: Mutex<Vec<tokio::time::Instant>>,
    phases_seen: Mutex<Vec<BatchPhase>>,
    progress: tokio::sync::watch::Receiver<MigrationProgress>,
}

impl BroadcastClient for ClockReadingClient {
    async fn submit(
        &self,
        _raw_tx: Vec<u8>,
        _expiry_height: BlockHeight,
    ) -> Result<BroadcastReceipt, TransferBroadcastError> {
        self.submitted_at
            .lock()
            .expect("clock mutex")
            .push(tokio::time::Instant::now());
        if let MigrationProgress::Sending(status) = &*self.progress.borrow() {
            self.phases_seen
                .lock()
                .expect("phase mutex")
                .push(status.phase);
        }
        Ok(receipt())
    }
}

#[tokio::test(start_paused = true)]
async fn a_batch_waits_out_the_spacing_between_accepted_transfers() {
    const SPACING: Duration = Duration::from_secs(30);
    let mut client = LightClient::new_for_test(wallet_with_signed_open_window_transfers(3)).await;
    let clock = ClockReadingClient {
        submitted_at: Mutex::new(Vec::new()),
        phases_seen: Mutex::new(Vec::new()),
        progress: client.migration_progress(),
    };
    let started = tokio::time::Instant::now();

    let report = client
        .broadcast_due_transfers_with(&clock, SPACING)
        .await
        .expect("the batch runs");
    let finished = tokio::time::Instant::now();

    assert_eq!(report.sent_txids().len(), 3, "{report:?}");
    let submitted_at = clock.submitted_at.into_inner().expect("clock mutex");
    assert_eq!(submitted_at.len(), 3);
    for pair in submitted_at.windows(2) {
        assert!(
            pair[1].duration_since(pair[0]) >= SPACING,
            "consecutive submissions are at least the spacing apart: {:?}",
            pair[1].duration_since(pair[0])
        );
    }
    assert!(
        finished.duration_since(started) >= 2 * SPACING,
        "two gaps separate three transfers"
    );
    assert!(
        finished.duration_since(started) < 3 * SPACING,
        "no spacing follows the last transfer"
    );
    assert_eq!(
        clock.phases_seen.into_inner().expect("phase mutex"),
        vec![BatchPhase::Sending; 3],
        "every submission happens in the sending phase, after the spacing"
    );
}

#[tokio::test(start_paused = true)]
async fn zero_spacing_sends_the_batch_without_waiting() {
    let mut client = LightClient::new_for_test(wallet_with_signed_open_window_transfers(2)).await;
    let clock = ClockReadingClient {
        submitted_at: Mutex::new(Vec::new()),
        phases_seen: Mutex::new(Vec::new()),
        progress: client.migration_progress(),
    };
    let started = tokio::time::Instant::now();

    let report = client
        .broadcast_due_transfers_with(&clock, Duration::ZERO)
        .await
        .expect("the batch runs");

    assert_eq!(report.sent_txids().len(), 2);
    assert_eq!(
        tokio::time::Instant::now().duration_since(started),
        Duration::ZERO,
        "the paused clock never advanced"
    );
}

struct SyncModeReadingClient {
    sync_mode: Arc<AtomicU8>,
    modes_seen: Mutex<Vec<SyncMode>>,
}

impl BroadcastClient for SyncModeReadingClient {
    async fn submit(
        &self,
        _raw_tx: Vec<u8>,
        _expiry_height: BlockHeight,
    ) -> Result<BroadcastReceipt, TransferBroadcastError> {
        let mode = SyncMode::from_atomic_u8(self.sync_mode.clone()).expect("a valid sync mode");
        self.modes_seen.lock().expect("mode mutex").push(mode);
        Ok(receipt())
    }
}

#[tokio::test]
async fn a_batch_holds_the_sync_pause_across_every_transfer_and_resumes_after() {
    let mut client = LightClient::new_for_test(wallet_with_signed_open_window_transfers(3)).await;
    client
        .sync_mode
        .store(SyncMode::Running as u8, Ordering::Release);
    let observer = SyncModeReadingClient {
        sync_mode: client.sync_mode.clone(),
        modes_seen: Mutex::new(Vec::new()),
    };

    let report = client
        .broadcast_due_transfers_with(&observer, Duration::from_millis(5))
        .await
        .expect("the batch runs");

    assert_eq!(report.sent_txids().len(), 3, "{report:?}");
    assert_eq!(
        observer.modes_seen.into_inner().expect("mode mutex"),
        vec![SyncMode::Paused; 3],
        "the engine stays paused through every submission and the spacing between them"
    );
    assert_eq!(
        client.sync_mode(),
        SyncMode::Running,
        "the batch resumes the engine it paused"
    );
}

#[tokio::test]
async fn a_successful_window_broadcast_never_synchronizes() {
    let mut client = LightClient::new_for_test(wallet_with_signed_open_window_transfers(2)).await;
    let known_height = client
        .wallet()
        .read()
        .await
        .sync_state
        .last_known_chain_height();
    let scanned = client
        .wallet()
        .read()
        .await
        .sync_state
        .fully_scanned_height();
    let mock = MockBroadcastClient::default();

    let report = client
        .broadcast_due_transfers_with(&mock, Duration::ZERO)
        .await
        .expect("the batch runs");

    assert_eq!(
        report.sent_txids().len(),
        2,
        "premise: both transfers were accepted"
    );
    assert!(
        matches!(client.poll_sync(), PollReport::NoHandle),
        "no sync task was launched"
    );
    assert!(
        client.latest_sync_status().is_none(),
        "the sync engine never published"
    );
    let wallet = client.wallet().read().await;
    assert_eq!(wallet.sync_state.last_known_chain_height(), known_height);
    assert_eq!(wallet.sync_state.fully_scanned_height(), scanned);
    assert_eq!(client.sync_mode(), SyncMode::NotRunning);
}

#[tokio::test]
async fn a_mixnet_session_yields_only_mixnet_receipts() {
    let mut client = LightClient::new_for_test(wallet_with_signed_open_window_transfers(3)).await;
    let mixnet = MockBroadcastClient::default();

    let report = client
        .broadcast_due_transfers_with(&mixnet, Duration::ZERO)
        .await
        .expect("the batch runs");

    let receipts: Vec<&BroadcastReceipt> = report
        .outcomes
        .iter()
        .filter_map(|outcome| match &outcome.result {
            TransferBroadcastResult::Sent(receipt) => Some(receipt),
            _ => None,
        })
        .collect();
    assert_eq!(receipts.len(), 3, "every transfer was sent: {report:?}");
    assert!(
        receipts.iter().all(|receipt| receipt.route.is_mixnet()),
        "every receipt names the mixnet wire: {receipts:?}"
    );
    assert_eq!(mixnet.submissions.lock().unwrap().len(), 3);
}
