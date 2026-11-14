use std::sync::atomic;

use pepper_sync::wallet::SyncMode;
use zcash_protocol::PoolType;
use zcash_protocol::value::Zatoshis;

use crate::data::receivers::{Receiver, transaction_request_from_receivers};
use crate::lightclient::LightClient;
use crate::testutils::synthetic_wallet::{self, SyntheticWalletBuilder};
use crate::wallet::LightWallet;

/// A synthetic-wallet client whose sync-mode atomic reads `Running`,
/// simulating a consumer whose background sync engine is between
/// batches. No engine task exists, so every mode transition observed
/// (or leaked) is the code under test's own.
async fn client_with_running_engine(wallet: LightWallet) -> LightClient {
    let client = LightClient::new_for_test(wallet).await;
    client
        .sync_mode
        .store(SyncMode::Running as u8, atomic::Ordering::Release);
    client
}

/// The pause a proposal takes exists for the proposal it stores. A
/// proposing call that fails stores nothing, so it must restore the
/// engine on its way out. The imperative discipline pauses first and
/// error-returns past every resume, leaving a consumer's background
/// sync silently suspended with nothing pending.
#[tokio::test]
async fn failed_proposal_leaves_no_leaked_pause() {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(10_000)
        .build();
    let mut client = client_with_running_engine(wallet).await;
    let request = transaction_request_from_receivers(vec![Receiver::new(
        synthetic_wallet::external_zcash_address(PoolType::ORCHARD),
        Zatoshis::const_from_u64(50_000),
        None,
    )])
    .unwrap();

    let result = client.propose_send(request, zip32::AccountId::ZERO).await;

    assert!(result.is_err(), "a 50_000 send from 10_000 must fail");
    assert_eq!(
        client.sync_mode(),
        SyncMode::Running,
        "a failed proposal must leave the engine as it found it"
    );
}

/// A shield proposal must not come into existence while the engine is
/// running: the proposal reads spendable coins the engine mutates.
/// `propose_send` pauses for exactly this reason. The shield path
/// never did.
#[tokio::test]
async fn shield_proposal_is_never_created_under_a_running_engine() {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .transparent_coin(100_000)
        .build();
    let mut client = client_with_running_engine(wallet).await;

    let result = client.propose_shield(zip32::AccountId::ZERO).await;

    assert!(
        result.is_err() || client.sync_mode() != SyncMode::Running,
        "a shield proposal was created while the engine ran"
    );
}

/// `quick_shield` goes further than proposing: it builds and stores
/// signed transactions, so its first wallet read must already run
/// under a pause. Simulate the engine mid-batch by holding the
/// wallet write lock: the call blocks on that lock, so if the mode
/// still reads `Running` while the call is in flight, the build began
/// without pausing the engine first.
#[tokio::test]
async fn quick_shield_never_builds_under_a_running_engine() {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .transparent_coin(100_000)
        .build();
    let mut client = client_with_running_engine(wallet).await;
    let sync_mode = client.sync_mode.clone();
    let wallet_handle = client.wallet().clone();
    let engine_holds_wallet = wallet_handle.write().await;

    let call = tokio::spawn(async move {
        let _offline_transmission_failure = client.quick_shield(zip32::AccountId::ZERO).await;
    });
    // The current-thread runtime polls the spawned call until it
    // blocks on the held wallet lock.
    tokio::task::yield_now().await;

    assert_ne!(
        SyncMode::from_atomic_u8(&sync_mode).unwrap(),
        SyncMode::Running,
        "quick_shield began its wallet reads while the engine ran"
    );

    drop(engine_holds_wallet);
    call.await.unwrap();
}

/// The send-all proposal is `propose_send_all`'s first wallet read
/// and must already run under a pause. Simulate the engine mid-batch
/// by holding the wallet write lock: the proposal blocks on that lock,
/// so if the engine's mode still reads `Running` while the call is in
/// flight, the call began its reads without pausing the engine first.
#[tokio::test]
async fn send_all_sizing_runs_under_pause() {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(100_000)
        .build();
    let mut client = client_with_running_engine(wallet).await;
    let sync_mode = client.sync_mode.clone();
    let wallet_handle = client.wallet().clone();
    let engine_holds_wallet = wallet_handle.write().await;

    let address = synthetic_wallet::external_zcash_address(PoolType::ORCHARD);
    let call = tokio::spawn(async move {
        client
            .propose_send_all(address, None, zip32::AccountId::ZERO)
            .await
    });
    // The current-thread runtime polls the spawned call until it
    // blocks on the held wallet lock.
    tokio::task::yield_now().await;

    assert_ne!(
        SyncMode::from_atomic_u8(&sync_mode).unwrap(),
        SyncMode::Running,
        "the send-all sizing began while the engine ran"
    );

    drop(engine_holds_wallet);
    call.await.unwrap().unwrap();
}

/// A request against a running-engine client that a proposal succeeds
/// for, shared by the stored-pause protocol tests below.
async fn client_with_stored_proposal() -> LightClient {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(100_000)
        .build();
    let mut client = client_with_running_engine(wallet).await;
    let request = transaction_request_from_receivers(vec![Receiver::new(
        synthetic_wallet::external_zcash_address(PoolType::ORCHARD),
        Zatoshis::const_from_u64(10_000),
        None,
    )])
    .unwrap();
    client
        .propose_send(request, zip32::AccountId::ZERO)
        .await
        .unwrap();
    client
}

/// A successful proposal holds its pause for as long as the
/// proposal is stored: the engine stays paused, so the state the
/// proposal selected against cannot shift before the send builds it.
#[tokio::test]
async fn proposing_holds_the_pause_for_the_stored_proposal() {
    let client = client_with_stored_proposal().await;

    assert_eq!(
        client.sync_mode(),
        SyncMode::Paused,
        "a stored proposal must hold the engine paused"
    );
}

/// Clearing a stored proposal (the decline path of the two-phase
/// send) restores the engine to the mode it held before proposing.
/// Previously the pause outlived the declined proposal until some
/// later send opted into resuming.
#[tokio::test]
async fn clearing_the_proposal_restores_the_engine() {
    let mut client = client_with_stored_proposal().await;

    client.clear_proposal().await;

    assert_eq!(
        client.sync_mode(),
        SyncMode::Running,
        "declining a proposal must restore the engine"
    );
}

/// An Indexerless send attempt fails before consuming the stored
/// proposal (ADR 0006), so the proposal and the pause guarding
/// it survive for retry once an Indexer is configured.
#[tokio::test]
async fn offline_send_failure_keeps_the_proposal_guarded() {
    let mut client = client_with_stored_proposal().await;

    let result = client.send_stored_proposal(true).await;

    assert!(result.is_err(), "an Indexerless send must fail");
    assert_eq!(
        client.sync_mode(),
        SyncMode::Paused,
        "a send that consumed nothing must release nothing"
    );
}
