#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! The shared network-failure taxonomy (`docs/agents/net-diag-design.md`).
//!
//! A network failure reported as prose can be read but never dispatched on.
//! When a Zingo operation failed somewhere between the local mixnet proxy
//! and a remote indexer, the wallet used to report one flattened sentence,
//! and every consumer — above all the mobile Connection Doctor — was
//! reduced to substring-matching that sentence to guess which layer broke.
//! Each layer that folded its cause into a string destroyed the evidence
//! the next layer needed. This crate ends the guessing: every covered
//! operation — the price fetch, the broadcast fan-out, the attach
//! validation, and the sync-path connectivity probe — reports its failures
//! as data. A [`NetOpFailure`] names which [`NetOpStage`] failed, against
//! what target, with the full cause chain carried as one text per layer — a
//! vector, never a concatenated string. One taxonomy serves them all, so a
//! consumer that learns to read one operation's failures can read every
//! operation's.
//!
//! The crate is used in two roles. A *producer* — code that owns an error
//! type — decides the stage and captures the cause chain with
//! [`NetOpFailure::from_error`] (or walks a chain directly with
//! [`chain_texts`]). This crate deliberately holds no classifiers of its
//! own: classification belongs with the crates that own the error types
//! (`zingo-price` classifies `reqwest::Error`; zingolib classifies
//! `Socks5TransmitError`), and this crate holds only the taxonomy and the
//! generic chain inspector. A *consumer* matches on the typed fields and
//! chooses its own presentation; the `Display` rendering exists for humans
//! and logs only. The crate is std-only with zero dependencies, a hard
//! requirement: it is what lets one crate serve two lockfile-isolated cargo
//! workspaces (the parent workspace and the standalone `zingo-netutils`
//! workspace) without resolver coupling.
//!
//! ```
//! use zingo_net_diag::{NetOpFailure, NetOpStage};
//!
//! // The producer side: at the seam that owns the error, name the stage
//! // and capture the whole source() chain, one text per layer.
//! let refused = std::io::Error::new(
//!     std::io::ErrorKind::ConnectionRefused,
//!     "connection refused",
//! );
//! let failure = NetOpFailure::from_error(
//!     NetOpStage::LocalProxyConnect,
//!     "127.0.0.1:1080",
//!     &refused,
//! );
//!
//! // The consumer side: dispatch on fields, never on rendered prose.
//! match &failure.stage {
//!     NetOpStage::LocalProxyConnect => {
//!         // The local SOCKS endpoint is down: repair the proxy before
//!         // any remote target is worth probing.
//!     }
//!     NetOpStage::TimedOut { after_ms } => {
//!         println!("gave up after {after_ms}ms");
//!     }
//!     _ => {}
//! }
//! assert_eq!(failure.cause_chain, ["connection refused"]);
//! assert_eq!(
//!     failure.to_string(),
//!     "failed at local-proxy-connect to 127.0.0.1:1080: connection refused",
//! );
//! ```
//!
//! For a production producer in a Nym-touching context, see
//! `zingolib::nym::socks5_transmit_stage`: a pure typed match that
//! classifies every `Socks5TransmitError` variant into its stage with no
//! substring inspection. For the consumer-visible payoff, see
//! `LightClient::mixnet_death_detail`, which answers *why* the mixnet
//! transport died with one of these records instead of a sentence.

use std::fmt;

/// Where, along a covered network operation, the failure occurred.
///
/// Kept deliberately small: two adjacent stages with a documented boundary
/// beat five precise ones nobody can produce. The [`fmt::Display`] rendering
/// is kebab-case (`remote-tls`, `timed-out(25000ms)`) and is part of the
/// stability contract described on [`NetOpFailure`]. Behind the `serde`
/// feature the wire discriminant is minted from the same kebab tokens by
/// `rename_all`, and zingolib's `nym::driver::wire_contract` pins every
/// variant's token against its `Display` rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum NetOpStage {
    /// Refused before any network touch: the mixnet route resolved to
    /// off, bootstrapping, or died, or a policy check refused the target.
    RouteResolution,
    /// The direct (untunneled) connection to the remote target could not be
    /// established. Where the transport reports its whole connect phase as
    /// one failure (DNS, TCP, and the secure channel undistinguished), that
    /// failure lands here; the staged sync-path probe separates the phases.
    /// This is the direct-path sibling of [`Self::LocalProxyConnect`], added
    /// by the design's sync-path addendum.
    RemoteConnect,
    /// The local SOCKS5 endpoint could not be reached.
    LocalProxyConnect,
    /// SOCKS5 negotiation with the local proxy failed, including a tunnel
    /// the proxy's exit could not establish to the destination.
    SocksHandshake,
    /// The transport was established and the data path then broke.
    TunnelTransport,
    /// TLS with the remote target failed.
    RemoteTls,
    /// The remote target answered with an HTTP-level failure (a status, a
    /// gRPC verdict, or an application rejection).
    RemoteHttp,
    /// The target's response body was undecodable or structurally short.
    PayloadDecode,
    /// The operation exceeded its client-side bound.
    TimedOut {
        /// The client-side bound that was exceeded, in milliseconds.
        after_ms: u64,
    },
}

impl fmt::Display for NetOpStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetOpStage::RouteResolution => write!(f, "route-resolution"),
            NetOpStage::RemoteConnect => write!(f, "remote-connect"),
            NetOpStage::LocalProxyConnect => write!(f, "local-proxy-connect"),
            NetOpStage::SocksHandshake => write!(f, "socks-handshake"),
            NetOpStage::TunnelTransport => write!(f, "tunnel-transport"),
            NetOpStage::RemoteTls => write!(f, "remote-tls"),
            NetOpStage::RemoteHttp => write!(f, "remote-http"),
            NetOpStage::PayloadDecode => write!(f, "payload-decode"),
            NetOpStage::TimedOut { after_ms } => write!(f, "timed-out({after_ms}ms)"),
        }
    }
}

/// The reusable failure record for one attempt against one target.
///
/// The cause chain is carried as a vector — one `Display` text per layer,
/// outermost first — never concatenated into a single string, so a consumer
/// (the mobile FFI's fielded probe legs, a report renderer) receives the
/// layers as data and decides its own presentation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NetOpFailure {
    /// The stage along the operation at which the failure occurred.
    pub stage: NetOpStage,
    /// The remote target, as a host or URI string. For stages before the
    /// tunnel, the local SOCKS endpoint.
    pub target: String,
    /// The full cause chain, one text per layer, outermost first.
    pub cause_chain: Vec<String>,
}

impl NetOpFailure {
    /// The failure record for `error`, with its whole `source()` chain
    /// captured layer by layer.
    pub fn from_error(
        stage: NetOpStage,
        target: impl Into<String>,
        error: &(dyn std::error::Error + 'static),
    ) -> Self {
        NetOpFailure {
            stage,
            target: target.into(),
            cause_chain: chain_texts(error),
        }
    }

    /// A single-layer failure record, for conditions that carry no error
    /// value (a probe that answered nothing, a bound that elapsed).
    pub fn message(stage: NetOpStage, target: impl Into<String>, text: impl Into<String>) -> Self {
        NetOpFailure {
            stage,
            target: target.into(),
            cause_chain: vec![text.into()],
        }
    }
}

/// Renders `failed at {stage} to {target}: {chain}`, the chain's layers
/// joined with `: ` outermost first.
///
/// This rendering is a stability contract: logs and the mobile error
/// messages carry it, and it must not change shape without a changelog
/// entry. Consumers must never parse it to make decisions — it is for
/// humans and logs; machine dispatch reads the typed fields.
impl fmt::Display for NetOpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed at {} to {}: {}",
            self.stage,
            self.target,
            self.cause_chain.join(": ")
        )
    }
}

impl std::error::Error for NetOpFailure {}

/// Walks a cause chain and returns each layer's `Display` text, outermost
/// first. Pure.
///
/// This is the one sanctioned way to capture a chain. Each error type's own
/// `Display` prints its own layer only; the chain belongs in
/// [`std::error::Error::source`], never concatenated into a message, because
/// a consumer that walks `source()` (the mobile FFI does) would otherwise
/// render the same text once per layer.
pub fn chain_texts(error: &(dyn std::error::Error + 'static)) -> Vec<String> {
    let mut texts = vec![error.to_string()];
    let mut cursor = error.source();
    while let Some(cause) = cursor {
        texts.push(cause.to_string());
        cursor = cause.source();
    }
    texts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_rendering_is_the_pinned_kebab_case_contract() {
        let table = [
            (NetOpStage::RouteResolution, "route-resolution"),
            (NetOpStage::RemoteConnect, "remote-connect"),
            (NetOpStage::LocalProxyConnect, "local-proxy-connect"),
            (NetOpStage::SocksHandshake, "socks-handshake"),
            (NetOpStage::TunnelTransport, "tunnel-transport"),
            (NetOpStage::RemoteTls, "remote-tls"),
            (NetOpStage::RemoteHttp, "remote-http"),
            (NetOpStage::PayloadDecode, "payload-decode"),
            (
                NetOpStage::TimedOut { after_ms: 25000 },
                "timed-out(25000ms)",
            ),
        ];
        for (stage, rendered) in table {
            assert_eq!(stage.to_string(), rendered);
        }
    }

    #[test]
    fn failure_rendering_is_the_pinned_shape() {
        let failure = NetOpFailure {
            stage: NetOpStage::RemoteTls,
            target: "zec.rocks:443".to_string(),
            cause_chain: vec!["transport error".to_string(), "handshake eof".to_string()],
        };
        assert_eq!(
            failure.to_string(),
            "failed at remote-tls to zec.rocks:443: transport error: handshake eof"
        );
    }

    /// A three-layer fabricated chain captures one text per layer, outermost
    /// first, with no layer's text repeated — the property the mobile
    /// `source()` walk depends on.
    #[test]
    fn from_error_captures_every_layer_once() {
        #[derive(Debug)]
        struct Layer {
            text: &'static str,
            below: Option<Box<Layer>>,
        }
        impl fmt::Display for Layer {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.text)
            }
        }
        impl std::error::Error for Layer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.below
                    .as_deref()
                    .map(|layer| layer as &(dyn std::error::Error + 'static))
            }
        }

        let chain = Layer {
            text: "request failed",
            below: Some(Box::new(Layer {
                text: "connect failed",
                below: Some(Box::new(Layer {
                    text: "connection refused",
                    below: None,
                })),
            })),
        };

        let failure = NetOpFailure::from_error(NetOpStage::RemoteConnect, "zec.rocks:443", &chain);
        assert_eq!(
            failure.cause_chain,
            vec!["request failed", "connect failed", "connection refused"]
        );
        let distinct: std::collections::HashSet<_> = failure.cause_chain.iter().collect();
        assert_eq!(
            distinct.len(),
            failure.cause_chain.len(),
            "no layer text repeats"
        );
    }
}
