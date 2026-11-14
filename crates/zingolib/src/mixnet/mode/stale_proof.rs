use super::{Indicator, MixnetSlot, StandingClient};

/// HYPOTHESIS: a Standing Client born on stale proof reports
/// PreviouslyProvenThisEpoch until a round trip of its own confirms
/// the exit, and Ready after — falsified if stale proof masquerades
/// as earned Ready, the trust hazard of a trusting birth.
#[tokio::test]
async fn stale_proof_is_typed_until_a_round_trip_confirms_it() {
    let proxy = crate::mixnet::MixnetProxy::ready_for_slot_tests(
        "127.0.0.1:1080".parse().expect("the test address parses"),
        vec![crate::mixnet::ExitNodeId::from("exit-stale-proven")],
    );
    let slot = MixnetSlot::Attached(StandingClient::new(proxy, None, false));

    assert_eq!(
        slot.mode(),
        Indicator::PreviouslyProvenThisEpoch,
        "stale proof must not masquerade as earned Ready"
    );

    if let MixnetSlot::Attached(client) = &slot {
        assert!(
            client.note_round_trip(),
            "the first confirmed round trip is the promoting one"
        );
    }
    assert_eq!(
        slot.mode(),
        Indicator::Ready,
        "a confirmed round trip promotes stale proof to earned"
    );
}

/// HYPOTHESIS: a Standing Client whose birth answered the Sentinel
/// reports earned Ready from its first instant — falsified if a probed
/// birth is typed stale.
#[tokio::test]
async fn a_probed_birth_is_ready_from_its_first_instant() {
    let proxy = crate::mixnet::MixnetProxy::ready_for_slot_tests(
        "127.0.0.1:1080".parse().expect("the test address parses"),
        vec![crate::mixnet::ExitNodeId::from("exit-earned")],
    );
    let slot = MixnetSlot::Attached(StandingClient::new(proxy, None, true));
    assert_eq!(slot.mode(), Indicator::Ready);
}
