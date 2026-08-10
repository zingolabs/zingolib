//! The fail-closed route resolver shared by every mixnet-only surface.
//!
//! Send and price-fetch both obey one policy (ADR 0011): when Mixnet Mode is
//! [`Ready`](crate::nym::MixnetMode::Ready) they route through the local
//! SOCKS5 proxy. When it is
//! [`SwitchedOff`](crate::nym::MixnetMode::SwitchedOff), reachable only by
//! the user's deliberate toggle-off, they route over clearnet as informed
//! consent. While [`Unattached`](crate::nym::MixnetMode::Unattached),
//! [`Bootstrapping`](crate::nym::MixnetMode::Bootstrapping), or after
//! [`Died`](crate::nym::MixnetMode::Died) they refuse rather than leak to
//! clearnet. This module names that decision once so both surfaces share it
//! instead of each re-deriving the mode semantics.
#![forbid(unsafe_code)]

use crate::nym::MixnetMode;

/// The session slot's tunnel, whose one exit is Shared across every
/// Correspondent contact the slot's surfaces make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotTunnel {
    socks5_addr: String,
}

impl SlotTunnel {
    /// The tunnel's local SOCKS5 address, for one more Correspondent
    /// contact.
    pub fn addr(&self) -> &str {
        &self.socks5_addr
    }
}

/// The resolved network route for a mixnet-only surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MixnetRoute {
    /// Route over clearnet. Reached only when Mixnet Mode is
    /// [`SwitchedOff`](MixnetMode::SwitchedOff), i.e. the user deliberately
    /// toggled it off.
    Clearnet,
    /// Route through the session slot's tunnel.
    Mixnet(SlotTunnel),
}

impl MixnetRoute {
    /// The SOCKS5 proxy address to fetch through, or `None` for clearnet.
    /// Shapes the route for a proxy-aware client such as the price fetch.
    pub fn socks5_proxy(&self) -> Option<&str> {
        match self {
            MixnetRoute::Clearnet => None,
            MixnetRoute::Mixnet(tunnel) => Some(tunnel.addr()),
        }
    }
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
         clearnet without consent. Run `nym on` to enable the mixnet, or `nym off` \
         to choose clearnet"
    )]
    Unattached,
    /// The mixnet is enabled but not yet reachable. Readiness is coming.
    #[error("the Nym mixnet is bootstrapping; this operation requires it to be ready")]
    Bootstrapping,
    /// The proxy died after being spawned. Only re-enabling recovers.
    #[error(
        "the Nym mixnet proxy died; this operation refuses rather than fall back to \
         clearnet. Run `nym on` to restart the proxy"
    )]
    Died,
}

/// Resolve the fail-closed route for the given Mixnet Mode and the proxy's
/// SOCKS5 address. `Ready` yields the mixnet route, `SwitchedOff` yields
/// clearnet, and `Unattached`, `Bootstrapping`, `Died`, or `Ready` with no
/// address yet all refuse. Crucially, only the deliberate `SwitchedOff`
/// yields clearnet: a never-enabled session and a `Died` proxy both refuse
/// rather than leaking the send to clearnet without consent.
pub fn resolve_route(
    mode: MixnetMode,
    socks5_addr: Option<String>,
) -> Result<MixnetRoute, MixnetNotReady> {
    match mode {
        MixnetMode::Unattached => Err(MixnetNotReady::Unattached),
        MixnetMode::SwitchedOff => Ok(MixnetRoute::Clearnet),
        MixnetMode::Ready => socks5_addr
            .map(|socks5_addr| MixnetRoute::Mixnet(SlotTunnel { socks5_addr }))
            .ok_or(MixnetNotReady::Bootstrapping),
        MixnetMode::Bootstrapping => Err(MixnetNotReady::Bootstrapping),
        MixnetMode::Died => Err(MixnetNotReady::Died),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switched_off_routes_clearnet() {
        assert_eq!(
            resolve_route(MixnetMode::SwitchedOff, None),
            Ok(MixnetRoute::Clearnet)
        );
    }

    #[test]
    fn unattached_refuses_because_absence_is_not_consent() {
        // The 2026-07-28 invariant: a session that never enabled the mixnet
        // (or whose enable failed) has not consented to clearnet, so it
        // refuses exactly as bootstrapping and died do.
        assert_eq!(
            resolve_route(MixnetMode::Unattached, None),
            Err(MixnetNotReady::Unattached)
        );
        assert_eq!(
            resolve_route(MixnetMode::Unattached, Some("127.0.0.1:9050".to_string())),
            Err(MixnetNotReady::Unattached),
            "a stray address must not conjure a route without a transport"
        );
    }

    #[test]
    fn ready_routes_through_the_proxy() {
        let route = resolve_route(MixnetMode::Ready, Some("127.0.0.1:9050".to_string()));
        assert_eq!(
            route,
            Ok(MixnetRoute::Mixnet(SlotTunnel {
                socks5_addr: "127.0.0.1:9050".to_string()
            }))
        );
        assert_eq!(route.unwrap().socks5_proxy(), Some("127.0.0.1:9050"));
    }

    #[test]
    fn bootstrapping_refuses_rather_than_leak() {
        assert_eq!(
            resolve_route(MixnetMode::Bootstrapping, None),
            Err(MixnetNotReady::Bootstrapping)
        );
    }

    #[test]
    fn a_died_proxy_refuses_and_never_leaks_to_clearnet() {
        // The critical invariant: an unexpected proxy death is NOT consent to
        // clearnet. Even with a stale address still on hand, Died refuses.
        assert_eq!(
            resolve_route(MixnetMode::Died, None),
            Err(MixnetNotReady::Died)
        );
        assert_eq!(
            resolve_route(MixnetMode::Died, Some("127.0.0.1:9050".to_string())),
            Err(MixnetNotReady::Died),
            "a stale address must not resurrect a dead proxy into a route"
        );
    }

    /// HYPOTHESIS: the refusal names the actual state, so a user with a dead
    /// proxy is told to run `nym on` rather than that the mixnet is
    /// bootstrapping. Falsified if the Died refusal renders the
    /// bootstrapping message.
    #[test]
    fn the_refusal_message_names_the_actual_state() {
        let bootstrapping = MixnetNotReady::Bootstrapping.to_string();
        assert!(bootstrapping.contains("bootstrapping"), "{bootstrapping}");

        let died = MixnetNotReady::Died.to_string();
        assert!(died.contains("died"), "{died}");
        assert!(died.contains("nym on"), "{died}");
        assert!(!died.contains("bootstrapping"), "{died}");

        let unattached = MixnetNotReady::Unattached.to_string();
        assert!(unattached.contains("not enabled"), "{unattached}");
        assert!(unattached.contains("nym on"), "{unattached}");
        assert!(!unattached.contains("bootstrapping"), "{unattached}");
        assert!(!unattached.contains("died"), "{unattached}");
    }

    #[test]
    fn ready_without_an_address_refuses() {
        assert_eq!(
            resolve_route(MixnetMode::Ready, None),
            Err(MixnetNotReady::Bootstrapping)
        );
    }

    #[test]
    fn clearnet_has_no_proxy() {
        assert_eq!(MixnetRoute::Clearnet.socks5_proxy(), None);
    }
}
