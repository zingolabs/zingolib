//! Error types for the [`Indexer`](super::Indexer) and
//! `TransparentIndexer` traits.
//!
/// Callers can depend on:
/// - `InvalidScheme` and `InvalidAuthority` are deterministic — retrying
///   with the same URI will always fail.
/// - `Transport` wraps a [`tonic::transport::Error`] and may be transient
///   (e.g. DNS resolution, TCP connect timeout). Retrying may succeed.
///
/// ```
/// use zingo_netutils::GetClientError;
///
/// let e = GetClientError::InvalidScheme;
/// assert_eq!(e.to_string(), "bad uri: invalid scheme");
///
/// let e = GetClientError::InvalidAuthority;
/// assert_eq!(e.to_string(), "bad uri: invalid authority");
///
/// // Transport variant accepts From<tonic::transport::Error>
/// let _: fn(tonic::transport::Error) -> GetClientError = GetClientError::from;
/// ```
#[derive(Debug, thiserror::Error)]
pub enum GetClientError {
    #[error("bad uri: invalid scheme")]
    InvalidScheme,

    #[error("bad uri: invalid authority")]
    InvalidAuthority,

    #[error(transparent)]
    Transport(#[from] tonic::transport::Error),
}

/// Error from [`NymProxy`](crate::NymProxy) lifecycle operations. Gated on
/// the `nym` feature, whose dependencies resolve only in this crate's own
/// lockfile (ADR 0011).
#[cfg(feature = "nym")]
#[derive(Debug, thiserror::Error)]
pub enum NymProxyError {
    /// Failed to build the Nym mixnet client. The cause rides in
    /// `source()`, not the message, so a chain walk
    /// ([`zingo_net_diag::chain_texts`]) sees one text per layer.
    #[error("failed to build Nym client")]
    Build(#[source] Box<nym_sdk::Error>),

    /// Failed to connect to the Nym mixnet. The cause rides in `source()`,
    /// as on [`Self::Build`].
    #[error("failed to connect to Nym mixnet")]
    Connect(#[source] Box<nym_sdk::Error>),

    /// Failed to query the Nym API for Exit Nodes.
    #[error("Nym API query failed: {0}")]
    DiscoveryApi(String),

    /// No public Exit Node could be discovered.
    #[error("no public Nym exit gateway found")]
    NoExitNode,

    /// End-to-end connectivity check through the SOCKS5 tunnel failed.
    #[error("connectivity check failed: {0}")]
    ConnectivityCheck(String),

    /// A single Exit Node connect attempt exceeded its per-attempt timeout.
    #[error("exit node connect attempt timed out after {0}s")]
    AttemptTimeout(u64),

    /// A bound Exit Node carried no round trip within the Sentinel's budget.
    #[error("the bound exit node carried nothing within {0}ms")]
    CarriesNothing(u64),

    /// Every raced connect attempt failed. Each attempt is a typed
    /// [`zingo_net_diag::NetOpFailure`] — the stage, the shortened Exit Node
    /// name as the target, and the cause chain as a vector — so a consumer
    /// dispatches on fields (every Exit Node timed out, versus one refused
    /// and the rest were never launched) instead of parsing prose. The
    /// joined-prose rendering below is `Display` only, never the storage
    /// form (`docs/agents/net-diag-design.md`, issue #2562).
    #[error(
        "no exit node connected after contacting {attempts} exit nodes: {}",
        join_failures(failures)
    )]
    AttemptsExhausted {
        /// The number of distinct Exit Nodes contacted.
        attempts: usize,
        /// Every attempt's failure, in completion order.
        failures: Vec<zingo_net_diag::NetOpFailure>,
    },
}

/// The `Display` joining of the typed attempt records, semicolon-separated
/// in completion order. Rendering only; consumers dispatch on the records.
#[cfg(feature = "nym")]
fn join_failures(failures: &[zingo_net_diag::NetOpFailure]) -> String {
    failures
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_from_conversion() {
        // Verify the From impl exists at compile time.
        let _: fn(tonic::transport::Error) -> GetClientError = GetClientError::from;
    }

    /// The prose account of an exhausted race is a rendering of the typed
    /// records, pinned here. The records are the storage form; a consumer
    /// dispatches on their fields, never on this sentence.
    #[cfg(feature = "nym")]
    #[test]
    fn attempts_exhausted_renders_each_typed_attempt() {
        use zingo_net_diag::{NetOpFailure, NetOpStage};

        let error = NymProxyError::AttemptsExhausted {
            attempts: 2,
            failures: vec![
                NetOpFailure::message(NetOpStage::RemoteConnect, "Emq7Gc3PLdp…", "gateway refused"),
                NetOpFailure::message(
                    NetOpStage::TimedOut { after_ms: 20_000 },
                    "9f2kQvR8sWx…",
                    "exit node connect attempt timed out after 20s",
                ),
            ],
        };
        assert_eq!(
            error.to_string(),
            "no exit node connected after contacting 2 exit nodes: \
             failed at remote-connect to Emq7Gc3PLdp…: gateway refused; \
             failed at timed-out(20000ms) to 9f2kQvR8sWx…: \
             exit node connect attempt timed out after 20s"
        );
    }
}
