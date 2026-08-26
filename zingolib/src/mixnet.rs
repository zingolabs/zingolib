//! Nym mixnet IP-obfuscation transport for the Transmission and price-fetch
//! surfaces, seam B of `docs/adr/0011-nym-mixnet-transmission.md`.
//!
//! This module holds the mixnet control and policy logic: the five-state
//! [`Indicator`], the fail-closed [`route`] resolver shared by every mixnet-only
//! surface, the escalating [`destination_rotation`] over an injected per-arm
//! runner and random-number generator, the curated Destination list, and
//! the [`supervisor`] that owns the spawned `nym-proxy` child. The escalation
//! orchestrates the shared per-submission resilience policy across rounds, and
//! because its arm runner and RNG are injected, the round, escalation, and cap
//! logic runs in CI without a reachable mixnet or real time.
#![forbid(unsafe_code)]
#![cfg(feature = "nym")]

pub mod acquire;

/// Exit identity, defined below the seam (ADR 0046).
pub use zingo_netutils::exit::{BlankExitNodeId, ExitNodeId};

/// The party a failure stage charges, unattributed when the stage cannot
/// say which side failed.
pub(crate) fn charge_phase(
    stage: &zingo_net_diag::NetOpStage,
) -> crate::destination::health::FailurePhase {
    use crate::destination::health::FailurePhase;
    use zingo_net_diag::NetOpStage;
    match stage {
        NetOpStage::RouteResolution
        | NetOpStage::LocalProxyConnect
        | NetOpStage::SocksHandshake
        | NetOpStage::TunnelTransport => FailurePhase::Tunnel,
        NetOpStage::RemoteTls | NetOpStage::RemoteHttp | NetOpStage::PayloadDecode => {
            FailurePhase::Destination
        }
        _ => FailurePhase::Unattributed,
    }
}

pub mod destination_rotation;

pub mod driver;

mod mode;

pub mod probe;

pub(crate) mod quartet;

pub mod provision;

pub mod route;

pub mod speed;

pub mod supervisor;

pub mod sweep;

pub use acquire::TransportError;

pub use driver::{MixnetStartPolicy, MixnetStatus, ProvisionStrategy};

pub(crate) use driver::{StatusPublisher, status_publisher};

pub use mode::Indicator;

pub(crate) use mode::MixnetSlot;

pub(crate) use mode::StandingClient;

pub use route::{MixnetNotReady, MixnetRoute, resolve_route};

/// Conduit, defined below the seam (ADR 0046).
pub use zingo_netutils::conduit::MixnetConduit;

pub use supervisor::{DeathReport, MixnetProxy, MixnetProxyError};

/// The temporal calibration a consumer of the mixnet transport reads from
/// the wallet, so a user interface paces itself from the same source of
/// truth as the gates (`zingo_netutils::time`) instead of pinning copies
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
        attach_readiness_budget: zingo_netutils::time::ATTACH_READINESS_BUDGET,
        mixnet_round_trip_bound: zingo_netutils::time::MIXNET_ROUND_TRIP_BOUND,
        client_rotation_min: zingo_netutils::time::CLIENT_ROTATION_MIN,
        client_rotation_max: zingo_netutils::time::CLIENT_ROTATION_MAX,
    }
}

/// The taxonomy stage of a SOCKS5 transmit failure
/// (`docs/agents/net-diag-design.md`): a pure, typed match over the error's
/// variants — no substring inspection anywhere. This is the one classifier
/// for [`zingo_netutils::Socks5TransmitError`], shared by the escalation, the
/// mixnet probe leg, and the attach readiness gate.
pub(crate) fn socks5_transmit_stage(
    error: &zingo_netutils::Socks5TransmitError,
) -> zingo_net_diag::NetOpStage {
    use zingo_net_diag::NetOpStage;
    use zingo_netutils::Socks5TransmitError;
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

/// The [`zingo_net_diag::NetOpFailure`] record for one SOCKS5 transmit
/// failure against `target`: stage from [`socks5_transmit_stage`], cause
/// chain captured layer by layer from the error's `source()` walk.
pub(crate) fn socks5_transmit_failure(
    error: &zingo_netutils::Socks5TransmitError,
    target: impl Into<String>,
) -> zingo_net_diag::NetOpFailure {
    zingo_net_diag::NetOpFailure::from_error(socks5_transmit_stage(error), target, error)
}

#[cfg(all(test, feature = "nym"))]
mod classifier_tests {
    use super::socks5_transmit_stage;
    use zingo_net_diag::NetOpStage;
    use zingo_netutils::{ProxyDialFailure, Socks5TransmitError, TunnelFailure};

    /// Every variant classifies by type alone; fabricated values cover the
    /// whole table.
    #[test]
    fn every_variant_classifies_by_type_alone() {
        let io = || std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        let table: Vec<(Socks5TransmitError, NetOpStage)> = vec![
            (
                Socks5TransmitError::ProxyUnreachable {
                    proxy: "127.0.0.1:1080".into(),
                    elapsed: std::time::Duration::from_secs(1),
                    source: ProxyDialFailure::Io(io()),
                },
                NetOpStage::LocalProxyConnect,
            ),
            (
                Socks5TransmitError::TunnelRefused {
                    destination: "zec.rocks:443".into(),
                    elapsed: std::time::Duration::from_secs(1),
                    source: TunnelFailure::TimedOut,
                },
                NetOpStage::SocksHandshake,
            ),
            (
                Socks5TransmitError::TunnelTransport {
                    destination: "zec.rocks:443".into(),
                    detail: "tls handshake eof".into(),
                    source: None,
                },
                NetOpStage::TunnelTransport,
            ),
            (
                Socks5TransmitError::Rpc {
                    destination: "zec.rocks:443".into(),
                    status: zingo_netutils::Status::unavailable("overloaded"),
                },
                NetOpStage::RemoteHttp,
            ),
            (
                Socks5TransmitError::TimedOut {
                    destination: "zec.rocks:443".into(),
                    after: std::time::Duration::from_secs(25),
                },
                NetOpStage::TimedOut { after_ms: 25_000 },
            ),
            (
                Socks5TransmitError::InsecureScheme {
                    indexer: "http://zec.rocks:9067".into(),
                },
                NetOpStage::RouteResolution,
            ),
        ];
        for (error, expected) in table {
            assert_eq!(socks5_transmit_stage(&error), expected, "error: {error}");
        }
    }
}
