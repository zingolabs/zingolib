//! The ProofAcquisition contract: the arbiter's evidence promotes or
//! convicts the Standing Client's exit, the conviction runs the
//! two-layer failover, and the expiry watchdog fires a new
//! ProofAcquisition the moment the proof stops being epoch-fresh.

use super::{adjudicate_standing_proof, standing_proof_watchdog};
use crate::mixnet::pools::exit_pool::Reservation;
use crate::mixnet::{Indicator, MixnetSlot, StandingClient};

fn slot_with(born_probed: bool, exit: &str) -> std::sync::Arc<std::sync::Mutex<MixnetSlot>> {
    let proxy = crate::mixnet::MixnetProxy::ready_for_slot_tests(
        "127.0.0.1:9".parse().expect("the test address parses"),
        vec![crate::mixnet::ExitNodeId::from(exit)],
    );
    std::sync::Arc::new(std::sync::Mutex::new(MixnetSlot::Attached(
        StandingClient::new(
            proxy,
            Some(Reservation::dangling_for_test(exit)),
            born_probed,
        ),
    )))
}

/// HYPOTHESIS: an answered arbiter probe promotes stale proof to
/// earned Ready and refreshes the exit's EpochProven observation.
/// Falsified if an answer convicts, demotes, or writes nothing.
#[tokio::test]
async fn an_answer_promotes_and_refreshes_the_observation() {
    let slot = slot_with(false, "exit-answers");
    let pools = crate::mixnet::pools::Pools::new();
    let status = crate::mixnet::status_publisher();

    adjudicate_standing_proof(
        slot.clone(),
        pools.clone(),
        std::sync::Arc::clone(&status),
        zl_netutils::sentinel::evidence_of(Some(64), 900),
    )
    .await;

    assert_eq!(slot.lock().unwrap().mode(), Indicator::Ready);
    assert!(
        pools.exits.lock().unwrap().epoch_proven(
            &crate::mixnet::ExitNodeId::from("exit-answers"),
            std::time::Instant::now()
        ),
        "the answer refreshes the exit's EpochProven observation"
    );
    assert_eq!(status.borrow().mode, Indicator::Ready);
}

/// HYPOTHESIS: a silent arbiter probe convicts the exit, and with no
/// acquirer to rebirth from the failover exhausts into a latched Died,
/// published to subscribers, with the conviction remembered. Falsified
/// if silence leaves readiness standing or the conviction unwritten.
#[tokio::test]
async fn silence_convicts_and_a_rebirthless_failover_latches_died() {
    let slot = slot_with(true, "exit-silent");
    let pools = crate::mixnet::pools::Pools::new();
    let status = crate::mixnet::status_publisher();

    adjudicate_standing_proof(
        slot.clone(),
        pools.clone(),
        std::sync::Arc::clone(&status),
        zl_netutils::sentinel::ExitEvidence::Silent,
    )
    .await;

    assert_eq!(slot.lock().unwrap().mode(), Indicator::Died);
    assert!(
        pools.exits.lock().unwrap().epoch_failed(
            &crate::mixnet::ExitNodeId::from("exit-silent"),
            std::time::Instant::now()
        ),
        "the conviction reaches the NodeHealthIndex"
    );
    assert_eq!(status.borrow().mode, Indicator::Died);
}

/// HYPOTHESIS: when the proof deadline lapses, the expiry watchdog
/// fires a new ProofAcquisition at that earliest feasible moment: the
/// arbiter probe runs against the tunnel (refused here, so silent),
/// the exit is convicted, and the rebirthless failover latches Died —
/// all without any operation touching the client. Falsified if expiry
/// passes unnoticed.
#[tokio::test(start_paused = true)]
async fn expiry_fires_a_proof_acquisition_unprompted() {
    let slot = slot_with(true, "exit-expired");
    if let MixnetSlot::Attached(client) = &*slot.lock().unwrap() {
        client.set_proof_deadline(std::time::Instant::now());
    }
    let pools = crate::mixnet::pools::Pools::new();
    let status = crate::mixnet::status_publisher();
    let mut watcher = status.subscribe();

    let watchdog = tokio::spawn(standing_proof_watchdog(
        slot.clone(),
        pools.clone(),
        std::sync::Arc::clone(&status),
    ));
    watcher
        .wait_for(|published| published.mode == Indicator::Died)
        .await
        .expect("the watchdog publishes the exhausted failover");
    watchdog.abort();

    assert!(
        pools.exits.lock().unwrap().epoch_failed(
            &crate::mixnet::ExitNodeId::from("exit-expired"),
            std::time::Instant::now()
        ),
        "the unprompted ProofAcquisition convicted the lapsed exit"
    );
}
