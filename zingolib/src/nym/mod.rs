//! Nym mixnet IP-obfuscation transport for the Transmission and price-fetch
//! surfaces, seam B of `docs/adr/0011-nym-mixnet-transmission.md`.
//!
//! This module holds the mixnet control and policy logic: the [`MixnetMode`]
//! tri-state, the fail-closed [`route`] resolver shared by every mixnet-only
//! surface, the escalating fan-out [`broadcast`] over an injected per-arm
//! runner and random-number generator, the curated Broadcast Indexer list, and
//! the [`supervisor`] that owns the spawned `nym-proxy` child. The fan-out
//! orchestrates the shared per-submission resilience policy across rounds, and
//! because its arm runner and RNG are injected, the round, escalation, and cap
//! logic runs in CI without a reachable mixnet or real time.
#![forbid(unsafe_code)]

pub mod broadcast;
pub mod broadcast_indexers;
mod mode;
pub mod probe;
pub mod route;
pub mod supervisor;

pub use mode::{IP_CORRELATION_DISCLAIMER, MixnetMode};
pub use route::{MixnetNotReady, MixnetRoute, resolve_route};
pub use supervisor::{MixnetProxy, MixnetProxyError};

/// The taxonomy stage of a SOCKS5 transmit failure
/// (`docs/agents/net-diag-design.md`): a pure, typed match over the error's
/// variants — no substring inspection anywhere. This is the one classifier
/// for [`zingo_netutils::Socks5TransmitError`], shared by the fan-out, the
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

#[cfg(test)]
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
