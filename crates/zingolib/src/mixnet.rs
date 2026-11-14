//! Nym mixnet IP-obfuscation transport for the Transmission and price-fetch
//! surfaces, seam B of `zingo-adrs zingolib/0011`.
//!
//! This module holds the mixnet control and policy logic: the five-state
//! [`Indicator`], the fail-closed [`route`] resolver shared by every mixnet-only
//! surface, and the [`supervisor`] that owns the spawned `nym-proxy` child.
#![forbid(unsafe_code)]
#![cfg(feature = "nym")]

pub mod acquire;

/// Exit identity, defined below the seam (ADR 0046).
pub use zl_netutils::exit::{BlankExitNodeId, ExitNodeId};

pub(crate) use crate::lightclient::indexer_history::fault_domain;

pub mod driver;

mod mode;

pub mod probe;

pub(crate) mod roles;

pub mod provision;

pub mod route;

pub mod speed;

pub(crate) mod pools;

pub mod supervisor;

pub use acquire::TransportError;

#[cfg(test)]
pub(crate) fn test_indexer() -> http::Uri {
    "https://indexer.test:443"
        .parse()
        .expect("the literal is a well-formed https URI")
}

pub use driver::{
    MixnetStartPolicy, MixnetStatus, ProvisionStrategy, SessionStartError, SessionStarted,
};

pub(crate) use driver::{StatusPublisher, status_publisher};

pub use mode::Indicator;

pub(crate) use mode::MixnetSlot;

pub(crate) use mode::StandingClient;

pub use route::{
    MixnetNotReady, MixnetRoute, TransmitPolicy, resolve_mixnet_only_route, resolve_send_route,
};

/// Conduit, defined below the seam (ADR 0046).
pub use zl_netutils::conduit::MixnetConduit;

pub use supervisor::{DeathReport, MixnetProxy, MixnetProxyError};

/// The temporal calibration a consumer of the mixnet transport reads from
/// the wallet, so a user interface paces itself from the same source of
/// truth as the gates (`zl_netutils::time`) instead of pinning copies
/// across the FFI, where no compiler catches drift (issue #2564). Field
/// names match the constants they carry.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixnetTiming {
    /// The attach readiness gate's total worst-case budget: how long
    /// "Connecting to mixnet…" may legitimately take before a `died`
    /// verdict can land. A consumer's connect patience derives from this.
    pub attach_readiness_budget: std::time::Duration,
    /// Bound on one data round trip through the tunnel, the per-attempt
    /// patience behind the budget.
    pub mixnet_round_trip_bound: std::time::Duration,
    /// The shortest a session holds one client before rotating it, for a
    /// platform whose policy rotates at all (ADR 0048).
    pub client_rotation_min: std::time::Duration,
    /// The longest, so no exit observes more than this much of a session.
    pub client_rotation_max: std::time::Duration,
}

/// The current [`MixnetTiming`], read from the one place the constants
/// live. Pure and infallible, so the FFI layer can surface it as a plain
/// record.
pub fn mixnet_timing() -> MixnetTiming {
    MixnetTiming {
        attach_readiness_budget: zl_netutils::time::ATTACH_READINESS_BUDGET,
        mixnet_round_trip_bound: zl_netutils::time::MIXNET_ROUND_TRIP_BOUND,
        client_rotation_min: zl_netutils::time::CLIENT_ROTATION_MIN,
        client_rotation_max: zl_netutils::time::CLIENT_ROTATION_MAX,
    }
}

/// The taxonomy stage of a SOCKS5 transmit failure
/// (`docs/agents/net-diag-design.md`): a pure, typed match over the error's
/// variants — no substring inspection anywhere. This is the one classifier
/// for [`zl_netutils::Socks5TransmitError`], shared by the send, the
/// mixnet probe leg, and the attach readiness gate.
pub(crate) fn socks5_transmit_stage(
    error: &zl_netutils::Socks5TransmitError,
) -> zl_netutils::diag::NetOpStage {
    use zl_netutils::Socks5TransmitError;
    use zl_netutils::diag::NetOpStage;
    match error {
        Socks5TransmitError::ProxyUnreachable { .. } => NetOpStage::LocalProxyConnect,
        Socks5TransmitError::TunnelRefused { .. } => NetOpStage::SocksHandshake,
        Socks5TransmitError::TunnelTransport { .. } => NetOpStage::TunnelTransport,
        // A gRPC status and an application rejection are both the remote
        // target answering with a verdict rather than a transport break.
        Socks5TransmitError::Rpc { .. } | Socks5TransmitError::Rejected(_) => {
            NetOpStage::RemoteHttp
        }
        // The client's own bound elapsed: no answer arrived, so this is
        // never a remote verdict. The bound rides into the stage so a
        // consumer can distinguish "slow" from "absent" (issue #2564).
        Socks5TransmitError::TimedOut { after, .. } => NetOpStage::TimedOut {
            after_ms: after.as_millis() as u64,
        },
        Socks5TransmitError::InsecureScheme { .. } => NetOpStage::RouteResolution,
    }
}

/// The [`zl_netutils::diag::NetOpFailure`] record for one SOCKS5 transmit
/// failure against `target`: stage from [`socks5_transmit_stage`], cause
/// chain captured layer by layer from the error's `source()` walk.
pub(crate) fn socks5_transmit_failure(
    error: &zl_netutils::Socks5TransmitError,
    target: impl Into<String>,
) -> zl_netutils::diag::NetOpFailure {
    zl_netutils::diag::NetOpFailure::from_error(socks5_transmit_stage(error), target, error)
}

#[cfg(all(test, feature = "nym"))]
mod classifier_tests;
