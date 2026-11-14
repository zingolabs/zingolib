use super::*;

fn conduit() -> zl_netutils::conduit::MixnetConduit {
    zl_netutils::conduit::MixnetConduit::over(
        "127.0.0.1:9050".parse().expect("the test address parses"),
    )
}

/// Every state that refuses a mixnet-only surface, with the refusal it
/// names. The states absent here route.
const REFUSING: [(Indicator, MixnetNotReady); 4] = [
    (Indicator::Unattached, MixnetNotReady::Unattached),
    (Indicator::SwitchedOff, MixnetNotReady::Unattached),
    (Indicator::Bootstrapping, MixnetNotReady::Bootstrapping),
    (Indicator::Died, MixnetNotReady::Died),
];

/// Every state that routes a mixnet-only surface through the conduit.
const ROUTING: [Indicator; 2] = [Indicator::Ready, Indicator::PreviouslyProvenThisEpoch];

/// The two tables partition [`Indicator::ALL`], so a new state cannot
/// ship without a row in the matrix.
#[test]
fn the_matrix_covers_every_indicator_exactly_once() {
    let mut rows: Vec<Indicator> = REFUSING
        .iter()
        .map(|(mode, _)| *mode)
        .chain(ROUTING)
        .collect();
    rows.sort_by_key(|mode| mode.as_str());
    let mut all = Indicator::ALL.to_vec();
    all.sort_by_key(|mode| mode.as_str());
    assert_eq!(rows, all);
}

#[test]
fn a_mixnet_only_surface_refuses_every_state_but_ready() {
    for (mode, refusal) in REFUSING {
        assert_eq!(
            resolve_mixnet_only_route(mode, None),
            Err(refusal),
            "{mode} must refuse"
        );
        assert_eq!(
            resolve_mixnet_only_route(mode, Some(conduit())),
            Err(refusal),
            "a stray conduit must not conjure a route while {mode}"
        );
    }
}

#[test]
fn a_mixnet_only_surface_rides_the_conduit_when_ready() {
    for mode in ROUTING {
        let routed = resolve_mixnet_only_route(mode, Some(conduit()))
            .unwrap_or_else(|refusal| panic!("{mode} must route, refused {refusal}"));
        assert_eq!(
            routed.dial().socks5(),
            "127.0.0.1:9050"
                .parse::<std::net::SocketAddr>()
                .expect("the test address parses")
        );
    }
}

#[test]
fn ready_without_a_conduit_refuses_as_bootstrapping() {
    for mode in ROUTING {
        assert_eq!(
            resolve_mixnet_only_route(mode, None),
            Err(MixnetNotReady::Bootstrapping)
        );
        assert_eq!(
            resolve_send_route(TransmitPolicy::Mixnet, mode, None),
            Err(MixnetNotReady::Bootstrapping)
        );
    }
}

/// HYPOTHESIS: the resolver hands out the session's own conduit rather
/// than one of its own making, so work it routes counts against the
/// conduit a rotation supersedes. Falsified if the route carries a
/// conduit whose uses the caller cannot see.
#[test]
fn the_route_carries_the_session_s_own_conduit() {
    let session = conduit();
    let routed = resolve_mixnet_only_route(Indicator::Ready, Some(session.clone())).unwrap();
    let held = routed.dial();
    assert_eq!(
        session.in_flight(),
        1,
        "a use of the routed conduit must count against the session's"
    );
    drop(held);
    assert_eq!(session.in_flight(), 0);
}

/// The nakednet row of the matrix: under the nakednet policy a send
/// routes nakednet in every transport state, conduit or not.
#[test]
fn the_nakednet_policy_transmits_over_nakednet_in_every_state() {
    for mode in Indicator::ALL {
        assert_eq!(
            resolve_send_route(TransmitPolicy::Nakednet, mode, None),
            Ok(MixnetRoute::Nakednet),
            "{mode} must not block a nakednet send"
        );
        assert_eq!(
            resolve_send_route(TransmitPolicy::Nakednet, mode, Some(conduit())),
            Ok(MixnetRoute::Nakednet),
            "a ready conduit must not override the nakednet policy while {mode}"
        );
    }
}

/// The mixnet row of the matrix: under the mixnet policy a send refuses
/// exactly where a mixnet-only surface refuses, and never leaks.
#[test]
fn the_mixnet_policy_refuses_exactly_as_a_mixnet_only_surface() {
    for (mode, refusal) in REFUSING {
        assert_eq!(
            resolve_send_route(TransmitPolicy::Mixnet, mode, None),
            Err(refusal),
            "{mode} must refuse a mixnet send"
        );
        assert_eq!(
            resolve_send_route(TransmitPolicy::Mixnet, mode, Some(conduit())),
            Err(refusal),
            "a stray conduit must not conjure a mixnet send while {mode}"
        );
    }
}

#[test]
fn the_mixnet_policy_rides_the_conduit_when_ready() {
    for mode in ROUTING {
        let session = conduit();
        match resolve_send_route(TransmitPolicy::Mixnet, mode, Some(session.clone())) {
            Ok(MixnetRoute::Mixnet(routed)) => {
                let _held = routed.dial();
                assert_eq!(
                    session.in_flight(),
                    1,
                    "the send must ride the session's conduit"
                );
            }
            other => panic!("{mode} must route through the proxy, got {other:?}"),
        }
    }
}

/// HYPOTHESIS: the refusal names the actual state, so a user with a dead
/// proxy is not told that the mixnet is bootstrapping. Falsified if the
/// Died refusal renders the bootstrapping message.
#[test]
fn the_refusal_message_names_the_actual_state() {
    let bootstrapping = MixnetNotReady::Bootstrapping.to_string();
    assert!(bootstrapping.contains("bootstrapping"), "{bootstrapping}");

    let died = MixnetNotReady::Died.to_string();
    assert!(died.contains("died"), "{died}");
    assert!(!died.contains("bootstrapping"), "{died}");

    let unattached = MixnetNotReady::Unattached.to_string();
    assert!(unattached.contains("not enabled"), "{unattached}");
    assert!(!unattached.contains("bootstrapping"), "{unattached}");
    assert!(!unattached.contains("died"), "{unattached}");
}

/// HYPOTHESIS: a refusal names the condition and never a frontend's
/// remedy, because only a frontend knows whether the user has a command
/// line or a toggle. Falsified if any refusal spells a command.
#[test]
fn no_refusal_speaks_a_frontend_s_vocabulary() {
    for refusal in [
        MixnetNotReady::Unattached,
        MixnetNotReady::Bootstrapping,
        MixnetNotReady::Died,
    ] {
        let rendered = refusal.to_string();
        for command in ["network on", "network off", "network status"] {
            assert!(
                !rendered.contains(command),
                "the {refusal:?} refusal spells the command `{command}`, \
                     which a frontend without a command line cannot offer: \
                     {rendered}"
            );
        }
    }
}
