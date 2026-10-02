//! The two route resolvers of the mixnet-covered surfaces (ADR 0011,
//! amendment 2026-09-11).
//!
//! The mixnet-only surfaces, price-fetch, the liveness probe, and the
//! migration transmission client, take the session's conduit while Mixnet
//! Mode is [`Ready`](crate::mixnet::Indicator::Ready) and refuse in every
//! other state. Transmission reads a second input, the session's
//! [`TransmitPolicy`]: under [`TransmitPolicy::Mixnet`] it routes exactly
//! as the mixnet-only surfaces do, and under [`TransmitPolicy::Clearnet`]
//! it takes clearnet at once, whatever the transport's state. The policy
//! is the consumer's per-session choice and never reaches the mixnet-only
//! resolver, so a price lookup cannot leak to clearnet through a send
//! setting.
#![forbid(unsafe_code)]

use crate::mixnet::Indicator;

/// The session's transmission policy: the consumer's per-session choice of
/// where a transaction travels. Independent of the transport's state, so
/// it can flip while the client runs and while a transport bootstraps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransmitPolicy {
    /// Transmit through the mixnet, refusing while the transport is not
    /// ready. The default: absence of a choice is not consent to clearnet.
    Mixnet,
    /// Transmit over clearnet through the configured sync indexer, whatever
    /// the transport's state.
    Clearnet,
}

/// The resolved network route for a transmission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MixnetRoute {
    /// Route over clearnet. Reached only under [`TransmitPolicy::Clearnet`].
    Clearnet,
    /// Route through the session's conduit.
    Mixnet(zingo_netutils::conduit::MixnetConduit),
}

/// A mixnet-covered surface was attempted while the mixnet was unavailable.
/// Fail-closed: the surface refuses rather than falling back to clearnet,
/// and the refusal names the actual state so the user learns the right
/// remedy: waiting out a bootstrap and restarting a dead proxy are
/// different actions.
#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum MixnetNotReady {
    /// No mixnet transport is established: the initial state, the state
    /// after a failed enable, and the state after a disable.
    #[error(
        "the Nym mixnet is not enabled; this operation requires it and refuses rather than \
         use clearnet without consent. Enable Mixnet Mode to proceed"
    )]
    Unattached,
    /// The mixnet is enabled but not yet reachable. Readiness is coming.
    #[error("the Nym mixnet is bootstrapping; this operation requires it to be ready")]
    Bootstrapping,
    /// The proxy died after being spawned. Only re-enabling recovers.
    #[error(
        "the Nym mixnet proxy died; this operation refuses rather than fall back to \
         clearnet. Re-enable Mixnet Mode to restart the proxy"
    )]
    Died,
}

/// Resolve the route of a mixnet-only surface: the session's conduit while
/// Mixnet Mode is `Ready`, a refusal naming the state otherwise. `Ready`
/// before a conduit exists refuses as bootstrapping.
// The conduit arrives rather than being minted from an address, because the
// session's rotation supersedes one conduit and every surface must be
// holding that one for the supersession to reach it (ADR 0048).
pub fn resolve_mixnet_only_route(
    mode: Indicator,
    conduit: Option<zingo_netutils::conduit::MixnetConduit>,
) -> Result<zingo_netutils::conduit::MixnetConduit, MixnetNotReady> {
    match mode {
        // A disable leaves no transport behind, so it refuses as the
        // ground state does.
        Indicator::Unattached | Indicator::SwitchedOff => Err(MixnetNotReady::Unattached),
        // Stale-proven routes exactly as earned Ready: the difference is
        // evidentiary, resolved by the promotion and demotion loop, never
        // by refusing the surface.
        Indicator::Ready | Indicator::PreviouslyProvenThisEpoch => {
            conduit.ok_or(MixnetNotReady::Bootstrapping)
        }
        Indicator::Bootstrapping => Err(MixnetNotReady::Bootstrapping),
        Indicator::Died => Err(MixnetNotReady::Died),
    }
}

/// Resolve the route of a transmission under the session's policy:
/// [`TransmitPolicy::Clearnet`] yields clearnet in every transport state,
/// and [`TransmitPolicy::Mixnet`] yields the conduit or the refusal the
/// mixnet-only resolver would.
pub fn resolve_send_route(
    policy: TransmitPolicy,
    mode: Indicator,
    conduit: Option<zingo_netutils::conduit::MixnetConduit>,
) -> Result<MixnetRoute, MixnetNotReady> {
    match policy {
        TransmitPolicy::Clearnet => Ok(MixnetRoute::Clearnet),
        TransmitPolicy::Mixnet => resolve_mixnet_only_route(mode, conduit).map(MixnetRoute::Mixnet),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conduit() -> zingo_netutils::conduit::MixnetConduit {
        zingo_netutils::conduit::MixnetConduit::over(
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

    /// The clearnet row of the matrix: under the clearnet policy a send
    /// routes clearnet in every transport state, conduit or not.
    #[test]
    fn the_clearnet_policy_transmits_over_clearnet_in_every_state() {
        for mode in Indicator::ALL {
            assert_eq!(
                resolve_send_route(TransmitPolicy::Clearnet, mode, None),
                Ok(MixnetRoute::Clearnet),
                "{mode} must not block a clearnet send"
            );
            assert_eq!(
                resolve_send_route(TransmitPolicy::Clearnet, mode, Some(conduit())),
                Ok(MixnetRoute::Clearnet),
                "a ready conduit must not override the clearnet policy while {mode}"
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
}
