use super::StandingClient;
use zl_netutils::conduit::ConduitState;

/// How many recheck intervals a held guard is observed across before
/// the wait is called a wait rather than a scheduling accident.
const DRAIN_OBSERVATION_POLLS: u32 = 3;

fn client() -> StandingClient {
    let proxy = crate::mixnet::MixnetProxy::ready_for_slot_tests(
        "127.0.0.1:1080".parse().expect("the test address parses"),
        vec![crate::mixnet::ExitNodeId::from("exit-handed-off")],
    );
    StandingClient::new(proxy, None, true)
}

/// HYPOTHESIS: a client mints one conduit and hands the same one to
/// every reader, so superseding it reaches work the session already
/// routed. Falsified if two reads yield conduits with separate counts.
#[tokio::test]
async fn a_client_hands_every_reader_the_same_conduit() {
    let client = client();
    let first = client.conduit().expect("a ready client mints its conduit");
    let second = client.conduit().expect("the conduit is minted once");
    let held = first.dial();
    assert_eq!(
        second.in_flight(),
        1,
        "a second reader must see the first reader's use"
    );
    drop(held);
    assert_eq!(second.in_flight(), 0);
}

/// HYPOTHESIS: retirement holds the transport open for work already
/// dialed and stops the moment that work ends, so ADR 0048's hand-off
/// cuts no send. Falsified if the transport stops with a guard
/// outstanding.
#[tokio::test]
async fn a_retiring_client_waits_for_its_outstanding_work() {
    let client = client();
    let conduit = client.conduit().expect("a ready client mints its conduit");
    let held = conduit.dial();

    let mut retiring = tokio::spawn(client.retire());
    assert!(
        tokio::time::timeout(
            zl_netutils::time::CONDUIT_DRAIN_POLL * DRAIN_OBSERVATION_POLLS,
            &mut retiring,
        )
        .await
        .is_err(),
        "a hand-off must not stop the transport under work in flight"
    );
    assert_eq!(
        conduit.state(),
        ConduitState::Superseded,
        "the outgoing conduit takes no new work while it drains"
    );

    drop(held);
    tokio::time::timeout(zl_netutils::time::CONDUIT_DRAIN_BUDGET, retiring)
        .await
        .expect("a drained conduit lets its transport stop")
        .expect("the retirement runs to completion");
    assert_eq!(conduit.state(), ConduitState::Retired);
}

/// HYPOTHESIS: a guard that outlives the drain budget does not hold the
/// transport open forever, because a leaked guard would leave two
/// clients alive for the rest of the session. Falsified if retirement
/// never returns.
#[tokio::test(start_paused = true)]
async fn a_leaked_guard_does_not_strand_the_transport() {
    let client = client();
    let conduit = client.conduit().expect("a ready client mints its conduit");
    let _leaked = conduit.dial();
    client.retire().await;
    assert_eq!(
        conduit.state(),
        ConduitState::Superseded,
        "the budget expired rather than the work finishing"
    );
}
