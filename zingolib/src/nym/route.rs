//! The fail-closed route resolver shared by every mixnet-only surface.
//!
//! Send and price-fetch both obey one policy (ADR 0011): when Mixnet Mode is
//! [`Ready`](crate::nym::MixnetMode::Ready) they route through the local
//! SOCKS5 proxy; when it is [`Off`](crate::nym::MixnetMode::Off) — reachable
//! only by the user's deliberate toggle-off — they route over clearnet as
//! informed consent; and while
//! [`Bootstrapping`](crate::nym::MixnetMode::Bootstrapping) they refuse rather
//! than leak to clearnet. This module names that decision once so both
//! surfaces share it instead of each re-deriving the tri-state.
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

/// A mixnet-only surface was attempted while the mixnet was not yet reachable.
/// Fail-closed: the surface refuses rather than falling back to clearnet.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("the Nym mixnet is bootstrapping; this operation requires it to be ready")]
pub struct MixnetNotReady;

/// Resolve the fail-closed route for the given Mixnet Mode and the proxy's
/// SOCKS5 address. `Ready` yields the mixnet route, `Off` yields clearnet, and
/// `Bootstrapping` (or `Ready` with no address yet) refuses.
pub fn resolve_route(
    mode: MixnetMode,
    socks5_addr: Option<String>,
) -> Result<MixnetRoute, MixnetNotReady> {
    match mode {
        MixnetMode::Off => Ok(MixnetRoute::Clearnet),
        MixnetMode::Ready => socks5_addr.map(MixnetRoute::Mixnet).ok_or(MixnetNotReady),
        MixnetMode::Bootstrapping => Err(MixnetNotReady),
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
            Err(MixnetNotReady)
        );
    }

    #[test]
    fn ready_without_an_address_refuses() {
        assert_eq!(resolve_route(MixnetMode::Ready, None), Err(MixnetNotReady));
    }

    #[test]
    fn clearnet_has_no_proxy() {
        assert_eq!(MixnetRoute::Clearnet.socks5_proxy(), None);
    }
}
