//! The two route resolvers of the mixnet-covered surfaces (ADR 0011,
//! amendment 2026-09-11).
//!
//! The mixnet-only surfaces, price-fetch, the liveness probe, and the
//! migration transmission client, take the session's conduit while Mixnet
//! Mode is [`Ready`](crate::mixnet::Indicator::Ready) and refuse in every
//! other state. Transmission reads a second input, the session's
//! [`TransmitPolicy`]: under [`TransmitPolicy::Mixnet`] it routes exactly
//! as the mixnet-only surfaces do, and under [`TransmitPolicy::Nakednet`]
//! it takes nakednet at once, whatever the transport's state. The policy
//! is the consumer's per-session choice and never reaches the mixnet-only
//! resolver, so a price lookup cannot leak to nakednet through a send
//! setting.
#![forbid(unsafe_code)]

use crate::mixnet::Indicator;

/// The session's transmission policy: the consumer's per-session choice of
/// where a transaction travels. Independent of the transport's state, so
/// it can flip while the client runs and while a transport bootstraps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransmitPolicy {
    /// Transmit through the mixnet, refusing while the transport is not
    /// ready. The default: absence of a choice is not consent to nakednet.
    Mixnet,
    /// Transmit over nakednet through the configured sync indexer, whatever
    /// the transport's state.
    Nakednet,
}

/// The resolved network route for a transmission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MixnetRoute {
    /// Route over nakednet. Reached only under [`TransmitPolicy::Nakednet`].
    Nakednet,
    /// Route through the session's conduit.
    Mixnet(zl_netutils::conduit::MixnetConduit),
}

/// A mixnet-covered surface was attempted while the mixnet was unavailable.
/// Fail-closed: the surface refuses rather than falling back to nakednet,
/// and the refusal names the actual state so the user learns the right
/// remedy: waiting out a bootstrap and restarting a dead proxy are
/// different actions.
#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum MixnetNotReady {
    /// No mixnet transport is established: the initial state, the state
    /// after a failed enable, and the state after a disable.
    #[error(
        "the Nym mixnet is not enabled; this operation requires it and refuses rather than \
         use nakednet without consent. Enable Mixnet Mode to proceed"
    )]
    Unattached,
    /// The mixnet is enabled but not yet reachable. Readiness is coming.
    #[error("the Nym mixnet is bootstrapping; this operation requires it to be ready")]
    Bootstrapping,
    /// The proxy died after being spawned. Only re-enabling recovers.
    #[error(
        "the Nym mixnet proxy died; this operation refuses rather than fall back to \
         nakednet. Re-enable Mixnet Mode to restart the proxy"
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
    conduit: Option<zl_netutils::conduit::MixnetConduit>,
) -> Result<zl_netutils::conduit::MixnetConduit, MixnetNotReady> {
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
/// [`TransmitPolicy::Nakednet`] yields nakednet in every transport state,
/// and [`TransmitPolicy::Mixnet`] yields the conduit or the refusal the
/// mixnet-only resolver would.
pub fn resolve_send_route(
    policy: TransmitPolicy,
    mode: Indicator,
    conduit: Option<zl_netutils::conduit::MixnetConduit>,
) -> Result<MixnetRoute, MixnetNotReady> {
    match policy {
        TransmitPolicy::Nakednet => Ok(MixnetRoute::Nakednet),
        TransmitPolicy::Mixnet => resolve_mixnet_only_route(mode, conduit).map(MixnetRoute::Mixnet),
    }
}

#[cfg(test)]
mod tests;
