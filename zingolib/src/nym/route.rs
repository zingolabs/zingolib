//! The fail-closed route resolver shared by every mixnet-only surface.
//!
//! Send and price-fetch both obey one policy (ADR 0011): when Mixnet Mode is
//! [`Ready`](crate::nym::MixnetMode::Ready) they route through the local
//! SOCKS5 proxy; when it is [`Off`](crate::nym::MixnetMode::Off) — reachable
//! only by the user's deliberate toggle-off — they route over clearnet as
//! informed consent; and while
//! [`Bootstrapping`](crate::nym::MixnetMode::Bootstrapping) or after
//! [`Died`](crate::nym::MixnetMode::Died) they refuse rather than leak to
//! clearnet. This module names that decision once so both surfaces share it
//! instead of each re-deriving the mode semantics.
#![forbid(unsafe_code)]

use crate::nym::MixnetMode;

/// The resolved network route for a mixnet-only surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MixnetRoute {
    /// Route over clearnet. Reached only when Mixnet Mode is
    /// [`Off`](MixnetMode::Off), i.e. the user deliberately toggled it off.
    Clearnet,
    /// Route through the local SOCKS5 proxy at this address.
    Mixnet(String),
}

impl MixnetRoute {
    /// The SOCKS5 proxy address to fetch through, or `None` for clearnet.
    /// Shapes the route for a proxy-aware client such as the price fetch.
    pub fn socks5_proxy(&self) -> Option<&str> {
        match self {
            MixnetRoute::Clearnet => None,
            MixnetRoute::Mixnet(addr) => Some(addr),
        }
    }
}

/// A mixnet-only surface was attempted while the mixnet was unavailable
/// without the user's consent to clearnet. Fail-closed: the surface refuses
/// rather than falling back to clearnet, and the refusal names the actual
/// state so the user learns the right remedy — waiting out a bootstrap and
/// restarting a dead proxy are different actions.
#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum MixnetNotReady {
    /// The mixnet is enabled but not yet reachable; readiness is coming.
    #[error("the Nym mixnet is bootstrapping; this operation requires it to be ready")]
    Bootstrapping,
    /// The proxy died after being spawned; only re-enabling recovers.
    #[error(
        "the Nym mixnet proxy died; this operation refuses rather than fall back to \
         clearnet — run `nym on` to restart the proxy"
    )]
    Died,
}

/// Resolve the fail-closed route for the given Mixnet Mode and the proxy's
/// SOCKS5 address. `Ready` yields the mixnet route, `Off` yields clearnet, and
/// `Bootstrapping`, `Died`, or `Ready` with no address yet all refuse.
/// Crucially, only the deliberate `Off` yields clearnet: a `Died` proxy
/// refuses rather than leaking the send to clearnet without consent.
pub fn resolve_route(
    mode: MixnetMode,
    socks5_addr: Option<String>,
) -> Result<MixnetRoute, MixnetNotReady> {
    match mode {
        MixnetMode::Off => Ok(MixnetRoute::Clearnet),
        MixnetMode::Ready => socks5_addr
            .map(MixnetRoute::Mixnet)
            .ok_or(MixnetNotReady::Bootstrapping),
        MixnetMode::Bootstrapping => Err(MixnetNotReady::Bootstrapping),
        MixnetMode::Died => Err(MixnetNotReady::Died),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_routes_clearnet() {
        assert_eq!(
            resolve_route(MixnetMode::Off, None),
            Ok(MixnetRoute::Clearnet)
        );
    }

    #[test]
    fn ready_routes_through_the_proxy() {
        let route = resolve_route(MixnetMode::Ready, Some("127.0.0.1:9050".to_string()));
        assert_eq!(route, Ok(MixnetRoute::Mixnet("127.0.0.1:9050".to_string())));
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
