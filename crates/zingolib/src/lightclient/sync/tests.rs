use futures::FutureExt as _;
use pepper_sync::wallet::SyncMode;
use zcash_protocol::consensus::H0;

use super::atomic;
use crate::data::PollReport;
use crate::lightclient::{LightClient, SyncResult};
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

/// An offline client whose sync-mode atomic the tests drive directly.
/// No engine runs, so every observed transition is the guard's own.
async fn offline_client(mode: SyncMode) -> LightClient {
    let wallet =
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
    let client = LightClient::new_for_test(wallet).await;
    client
        .sync_mode
        .store(mode as u8, atomic::Ordering::Release);
    client
}

#[tokio::test]
async fn running_pauses_and_drop_resumes() {
    let client = offline_client(SyncMode::Running).await;
    let guard = client.pause_sync_scoped().expect("a running engine pauses");
    assert_eq!(client.sync_mode(), SyncMode::Paused);
    drop(guard);
    assert_eq!(client.sync_mode(), SyncMode::Running);
}

#[tokio::test]
async fn not_running_is_a_no_op_guard() {
    let client = offline_client(SyncMode::NotRunning).await;
    let guard = client.pause_sync_scoped().expect("no engine needs a pause");
    assert_eq!(client.sync_mode(), SyncMode::NotRunning);
    drop(guard);
    assert_eq!(client.sync_mode(), SyncMode::NotRunning);
}

#[tokio::test]
async fn paused_guard_does_not_steal_the_resume() {
    let client = offline_client(SyncMode::Paused).await;
    let guard = client
        .pause_sync_scoped()
        .expect("a paused engine needs no pause");
    drop(guard);
    assert_eq!(
        client.sync_mode(),
        SyncMode::Paused,
        "whoever paused the engine owns its resume"
    );
}

#[tokio::test]
async fn shutdown_cannot_be_paused() {
    let client = offline_client(SyncMode::Shutdown).await;
    assert!(
        client.pause_sync_scoped().is_err(),
        "a shutting-down engine still scans its final batch"
    );
}

#[tokio::test]
async fn stop_while_held_wins_over_the_resume() {
    let client = offline_client(SyncMode::Running).await;
    let guard = client.pause_sync_scoped().expect("a running engine pauses");
    client.stop_sync().expect("a paused engine can be stopped");
    drop(guard);
    assert_eq!(
        client.sync_mode(),
        SyncMode::Shutdown,
        "the drop must not overwrite a shutdown request"
    );
}

#[tokio::test]
async fn wait_for_sync_resolves_at_once_without_an_engine() {
    let client = offline_client(SyncMode::NotRunning).await;
    assert!(
        client.wait_for_sync().now_or_never().is_some(),
        "no engine holds the status channel open"
    );
}

#[tokio::test]
async fn the_wait_and_the_subscription_end_with_the_engine() {
    let mut client = offline_client(SyncMode::Running).await;
    let (engine, status) = tokio::sync::watch::channel(None);
    client.sync_progress = status;
    let mut subscriber = client.subscribe_sync_status();

    engine.send_replace(None);
    assert!(
        client.wait_for_sync().now_or_never().is_none(),
        "a running engine holds the wait open"
    );
    assert!(
        subscriber.changed().await.is_ok(),
        "the push reaches the subscriber"
    );

    drop(engine);
    assert!(
        client.wait_for_sync().now_or_never().is_some(),
        "the engine returning releases the wait"
    );
    assert!(
        subscriber.changed().await.is_err(),
        "the subscription closes with the session"
    );
}

/// HYPOTHESIS: going offline asks the engine to shut down and waits
/// for it to return, so the engine ends its session between batches.
/// Falsified if the engine is cancelled before it acts on the request.
#[tokio::test]
async fn going_offline_waits_for_the_engine_to_shut_down() {
    let mut client = offline_client(SyncMode::Running).await;
    let sync_mode = client.sync_mode.clone();
    let (shut_down, engine_shut_down) = tokio::sync::oneshot::channel();
    client.sync_handle = Some(tokio::spawn(async move {
        while SyncMode::from_atomic_u8(&sync_mode)? != SyncMode::Shutdown {
            tokio::task::yield_now().await;
        }
        let _receiver_outlives_the_engine = shut_down.send(());
        Ok(session_without_scanning())
    }));

    client.go_offline().await;

    assert!(
        engine_shut_down.await.is_ok(),
        "the engine saw the shutdown request and returned"
    );
    assert_eq!(
        client.sync_mode(),
        SyncMode::NotRunning,
        "the next session can launch"
    );
    assert!(
        matches!(client.poll_sync(), PollReport::NoHandle),
        "going offline collected the session"
    );
}

/// The result of a session that was stopped before it scanned a block.
fn session_without_scanning() -> SyncResult {
    const NOTHING_SCANNED: u32 = 0;
    SyncResult {
        sync_start_height: H0,
        sync_end_height: H0,
        blocks_scanned: NOTHING_SCANNED,
        sapling_outputs_scanned: NOTHING_SCANNED,
        orchard_outputs_scanned: NOTHING_SCANNED,
        ironwood_outputs_scanned: NOTHING_SCANNED,
        total_outputs_scanned: 0,
        total_outputs: 0,
        nullifier_retrieval_pending: false,
    }
}
