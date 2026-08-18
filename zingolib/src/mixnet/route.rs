//! The fail-closed route resolver shared by every mixnet-only surface.
//!
//! Send and price-fetch both obey one policy (ADR 0011): when Mixnet Mode is
//! [`Ready`](crate::mixnet::Indicator::Ready) they route through the local
//! SOCKS5 proxy. When it is
//! [`SwitchedOff`](crate::mixnet::Indicator::SwitchedOff), reachable only by
//! the user's deliberate toggle-off, they route over clearnet as informed
//! consent. While [`Unattached`](crate::mixnet::Indicator::Unattached),
//! [`Bootstrapping`](crate::mixnet::Indicator::Bootstrapping), or after
//! [`Died`](crate::mixnet::Indicator::Died) they refuse rather than leak to
//! clearnet. This module names that decision once so both surfaces share it
//! instead of each re-deriving the mode semantics.
#![forbid(unsafe_code)]

use crate::mixnet::Indicator;

/// The session slot's tunnel, whose one exit is Shared across every
/// request the slot's surfaces send to a Correspondent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotTunnel {
    socks5_addr: std::net::SocketAddr,
}

impl SlotTunnel {
    /// The tunnel's local SOCKS5 address, for one more request to a
    /// Correspondent.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.socks5_addr
    }

    /// Yields the tunnel's local SOCKS5 address for the one owned dial.
    pub fn into_addr(self) -> std::net::SocketAddr {
        self.socks5_addr
    }

    /// Wraps the published proxy address of a ready transport.
    fn over(socks5_addr: std::net::SocketAddr) -> Self {
        Self { socks5_addr }
    }
}

/// The resolved network route for a mixnet-only surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MixnetRoute {
    /// Route over clearnet. Reached only when Mixnet Mode is
    /// [`SwitchedOff`](Indicator::SwitchedOff), i.e. the user deliberately
    /// toggled it off.
    Clearnet,
    /// Route through the Standing Client's tunnel.
    Mixnet(SlotTunnel),
}

/// A mixnet-only surface was attempted while the mixnet was unavailable
/// without the user's consent to clearnet. Fail-closed: the surface refuses
/// rather than falling back to clearnet, and the refusal names the actual
/// state so the user learns the right remedy: waiting out a bootstrap and
/// restarting a dead proxy are different actions.
#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum MixnetNotReady {
    /// No mixnet transport is established and the user has not consented to
    /// clearnet: the initial state, and the state after a failed enable.
    /// Absence is not consent, so the surface refuses.
    #[error(
        "the Nym mixnet is not enabled; this operation refuses rather than use \
         clearnet without consent. Enable Mixnet Mode to use the mixnet, or \
         switch it off to choose clearnet"
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

/// Resolve the fail-closed route for the given Mixnet Mode and SOCKS5
/// address — `Ready` yields the mixnet route, only the deliberate
/// `SwitchedOff` yields clearnet, and `Unattached`, `Bootstrapping`,
/// `Died`, or `Ready` before an address is published all refuse rather
/// than leak a send to clearnet without consent.
pub fn resolve_route(
    mode: Indicator,
    socks5_addr: Option<std::net::SocketAddr>,
) -> Result<MixnetRoute, MixnetNotReady> {
    match mode {
        Indicator::Unattached => Err(MixnetNotReady::Unattached),
        Indicator::SwitchedOff => Ok(MixnetRoute::Clearnet),
        // Stale-proven routes exactly as earned Ready: the difference is
        // evidentiary, resolved by the promotion and demotion loop, never
        // by refusing the surface.
        Indicator::Ready | Indicator::PreviouslyProvenThisEpoch => socks5_addr
            .map(SlotTunnel::over)
            .map(MixnetRoute::Mixnet)
            .ok_or(MixnetNotReady::Bootstrapping),
        Indicator::Bootstrapping => Err(MixnetNotReady::Bootstrapping),
        Indicator::Died => Err(MixnetNotReady::Died),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switched_off_routes_clearnet() {
        assert_eq!(
            resolve_route(Indicator::SwitchedOff, None),
            Ok(MixnetRoute::Clearnet)
        );
    }

    #[test]
    fn unattached_refuses_because_absence_is_not_consent() {
        // The 2026-07-28 invariant: a session that never enabled the mixnet
        // (or whose enable failed) has not consented to clearnet, so it
        // refuses exactly as bootstrapping and died do.
        assert_eq!(
            resolve_route(Indicator::Unattached, None),
            Err(MixnetNotReady::Unattached)
        );
        assert_eq!(
            resolve_route(
                Indicator::Unattached,
                Some("127.0.0.1:9050".parse().expect("the test address parses"))
            ),
            Err(MixnetNotReady::Unattached),
            "a stray address must not conjure a route without a transport"
        );
    }

    #[test]
    fn ready_routes_through_the_proxy() {
        let route = resolve_route(
            Indicator::Ready,
            Some("127.0.0.1:9050".parse().expect("the test address parses")),
        );
        match route.unwrap() {
            MixnetRoute::Mixnet(tunnel) => assert_eq!(
                tunnel.addr(),
                "127.0.0.1:9050"
                    .parse::<std::net::SocketAddr>()
                    .expect("the test address parses")
            ),
            MixnetRoute::Clearnet => panic!("ready must route through the proxy"),
        }
    }

    #[test]
    fn bootstrapping_refuses_rather_than_leak() {
        assert_eq!(
            resolve_route(Indicator::Bootstrapping, None),
            Err(MixnetNotReady::Bootstrapping)
        );
    }

    #[test]
    fn a_died_proxy_refuses_and_never_leaks_to_clearnet() {
        // The critical invariant: an unexpected proxy death is NOT consent to
        // clearnet. Even with a stale address still on hand, Died refuses.
        assert_eq!(
            resolve_route(Indicator::Died, None),
            Err(MixnetNotReady::Died)
        );
        assert_eq!(
            resolve_route(
                Indicator::Died,
                Some("127.0.0.1:9050".parse().expect("the test address parses"))
            ),
            Err(MixnetNotReady::Died),
            "a stale address must not resurrect a dead proxy into a route"
        );
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

    #[test]
    fn ready_without_an_address_refuses() {
        assert_eq!(
            resolve_route(Indicator::Ready, None),
            Err(MixnetNotReady::Bootstrapping)
        );
    }
}
