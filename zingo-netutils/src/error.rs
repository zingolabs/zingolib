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
    /// Failed to build the Nym mixnet client.
    #[error("failed to build Nym client: {0}")]
    Build(Box<nym_sdk::Error>),

    /// Failed to connect to the Nym mixnet.
    #[error("failed to connect to Nym mixnet: {0}")]
    Connect(Box<nym_sdk::Error>),

    /// Failed to query the Nym API for service providers.
    #[error("Nym API query failed: {0}")]
    DiscoveryApi(String),

    /// No public exit gateway could be discovered.
    #[error("no public Nym exit gateway found")]
    NoProvider,

    /// End-to-end connectivity check through the SOCKS5 tunnel failed.
    #[error("connectivity check failed: {0}")]
    ConnectivityCheck(String),

    /// A single provider connect attempt exceeded its per-attempt timeout.
    #[error("provider connect attempt timed out after {0}s")]
    AttemptTimeout(u64),

    /// Every raced connect attempt failed; the message accounts for each
    /// attempted provider and its failure.
    #[error("no provider connected: {0}")]
    AttemptsExhausted(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_from_conversion() {
        // Verify the From impl exists at compile time.
        let _: fn(tonic::transport::Error) -> GetClientError = GetClientError::from;
    }
}
