//! A complete [`Indexer`] abstraction for communicating with Zcash chain
//! indexers (`zingod`).
//!
//! # Organizing principle
//!
//! The [`Indexer`] trait is the sole interface a Zcash wallet or tool needs
//! to query, sync, and transmit against a chain indexer. It is
//! implementation-agnostic: production code uses the provided [`GrpcIndexer`]
//! (gRPC over tonic), while tests can supply a mock implementor with no
//! network dependency.
//!
//! All proto types come from
//! [`lightwallet-protocol`](https://crates.io/crates/lightwallet-protocol)
//! and are re-exported via `pub use lightwallet_protocol` so consumers do
//! not need an additional dependency.
//!
//! # Feature gates
//!
//! All features are **off by default**.
//!
//! | Feature | What it enables |
//! |---|---|
//! | `globally-public-transparent` | `TransparentIndexer` sub-trait for t-address balance, transaction history, and UTXO queries. Pulls in `tokio-stream`. |
//! | `ping-very-insecure` | `Indexer::ping` method. Name mirrors the server-side `--ping-very-insecure` CLI flag. Testing only. |
//!

use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::time::Duration;

use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder, MaybeHttpsStream};
use hyper_util::client::legacy::connect::HttpConnector;
use tonic::Request;
use tonic::transport::{Channel, Endpoint};
use tower::ServiceExt;

use lightwallet_protocol::{
    BlockId, BlockRange, ChainSpec, CompactBlock, CompactTx, CompactTxStreamerClient, Empty,
    GetMempoolTxRequest, GetSubtreeRootsArg, LightdInfo, RawTransaction, StandardFee, SubtreeRoot,
    TxFilter,
};

#[cfg(feature = "ping-very-insecure")]
use lightwallet_protocol::{Duration as ProtoDuration, PingResponse};

pub mod crypto;
pub mod default_indexer;
pub mod diag;
pub mod dns;
pub mod error;
pub mod network_access;

pub use crypto::ensure_default_crypto_provider;

/// The stdout line prefix the spawnable `nym-proxy` binary uses to announce
/// its local SOCKS5 address (ADR 0011, consumption model A). Defined here so
/// the binary that emits it and the wallet supervisor that parses it share one
/// definition. The full line is `SOCKS5_ADDR=127.0.0.1:<port>`.
pub const SOCKS5_ADDR_LINE_PREFIX: &str = "SOCKS5_ADDR=";

/// The stdout line prefix the spawnable `nym-proxy` binary uses for live
/// bootstrap progress, before the address announcement. Defined here so the
/// binary that emits it and the wallet supervisor that parses it share one
/// definition. The full line is e.g.
/// `NYM_STATUS=attempt 4/10: 2 in flight, 2 failed`.
pub const NYM_STATUS_LINE_PREFIX: &str = "NYM_STATUS=";

/// The stdout line prefix announcing the bound Exit Node identity, emitted
/// before the address line so the ready snapshot carries it.
pub const NYM_EXIT_LINE_PREFIX: &str = "NYM_EXIT=";

pub mod request;
mod rpc;
// The temporal parameters of every crate that can see this one, including
// the values owned by tests (`time::test`). See the module's registry.
pub mod time;
pub mod wire;
pub use error::*;
pub use lightwallet_protocol;
pub use network_access::NetworkAccess;
pub use tonic::{Status, Streaming, TimeoutExpired};

#[cfg(feature = "globally-public-transparent")]
mod globally_public;
#[cfg(feature = "globally-public-transparent")]
pub use globally_public::TransparentIndexer;

// Deliberately ungated: the pure core of the nym proxy lifecycle, whose
// unit tests run in the default build without the nym-sdk stack.
mod mixnet_connect;

// Deliberately ungated and public: the pure racing planner shared by the
// mixnet bootstrap here and zingolib's send escalation (ADR 0011).
pub mod arm_race;

// Deliberately ungated: Exit Node identity and health are data, so gating
// them would make the featureless build harder to reason about (ADR 0046).
pub mod exit;

pub mod conduit;

pub mod provider;

#[cfg(feature = "nym")]
mod nym_proxy;
#[cfg(feature = "nym")]
pub use nym_proxy::{BootstrapEvent, NymProxy};

#[cfg(feature = "socks5-fetch")]
pub mod socks5_fetch;

#[cfg(feature = "socks5-transmit")]
mod socks5_transmit;
#[cfg(feature = "socks5-transmit")]
pub use socks5_transmit::{ProxyDialFailure, Socks5Indexer, Socks5TransmitError, TunnelFailure};

#[cfg(feature = "socks5-transmit")]
pub mod sentinel;

/// The committed localhost certificate and key.
#[cfg(any(test, feature = "testutils"))]
pub mod test_tls {
    /// The self-signed `localhost` certificate.
    pub const LOCALHOST_CERT_PEM: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-data/localhost.pem"
    ));
    /// Its private key.
    pub const LOCALHOST_KEY_PEM: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-data/localhost.key"
    ));
}

/// The TCP half of a connection to an indexer, built here rather than left to
/// tonic so that [`crate::time::INDEXER_CONNECT_TIMEOUT`] covers the TLS
/// handshake as well.
///
/// tonic applies `Endpoint::connect_timeout` in one of two places. Through
/// `connect` and `connect_lazy` it reaches only the TCP connector, so a path
/// that accepts the connection and then goes silent stalls the handshake with
/// no bound at all. Through `connect_with_connector` it wraps the whole
/// connector, TLS included. This one is configured the way tonic's own would be
/// for these endpoints.
fn indexer_connector<R>(resolver: R) -> HttpConnector<R> {
    let mut http = HttpConnector::new_with_resolver(resolver);
    http.enforce_http(false);
    http.set_nodelay(true);
    http
}

fn mozilla_roots() -> rustls::RootCertStore {
    rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned())
}

fn client_tls_config(roots: rustls::RootCertStore) -> rustls::ClientConfig {
    ensure_default_crypto_provider();
    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    // Resumption MUST be set to disabled to stop us from resending
    // tracking tokens to server when switching from wifi->cellular backhaul
    config.resumption = rustls::client::Resumption::disabled();
    config
}

type IndexerHttps = HttpsConnector<HttpConnector>;
type IndexerStream = <IndexerHttps as tower::Service<http::Uri>>::Response;
type IndexerDialError = <IndexerHttps as tower::Service<http::Uri>>::Error;

const ALPN_H2: &[u8] = b"h2";

#[derive(Debug, thiserror::Error)]
#[error("HTTP/2 was not negotiated.")]
struct H2NotNegotiated;

fn require_h2(stream: IndexerStream) -> Result<IndexerStream, IndexerDialError> {
    let refused = match &stream {
        MaybeHttpsStream::Https(tls) => tls.inner().get_ref().1.alpn_protocol() != Some(ALPN_H2),
        MaybeHttpsStream::Http(_) => false,
    };
    if refused {
        return Err(H2NotNegotiated.into());
    }
    Ok(stream)
}

fn indexer_endpoint(
    uri: http::Uri,
    roots: rustls::RootCertStore,
) -> Result<
    (
        Endpoint,
        impl tower::Service<http::Uri, Response = IndexerStream, Error = IndexerDialError, Future: Send>
        + Send
        + 'static,
    ),
    GetClientError,
> {
    if !matches!(uri.scheme_str(), Some("http" | "https")) {
        return Err(GetClientError::InvalidScheme);
    }
    let authority = uri.authority().ok_or(GetClientError::InvalidAuthority)?;
    let endpoint = Endpoint::from_shared(format!("http://{authority}"))?
        .origin(uri.clone())
        .connect_timeout(crate::time::INDEXER_CONNECT_TIMEOUT);
    let https = HttpsConnectorBuilder::new()
        .with_tls_config(client_tls_config(roots))
        .https_or_http() // plaintext only for `http://` URIs, never a fallback
        .enable_http2()
        .wrap_connector(indexer_connector(dns::hyper_resolver()));
    let dial = tower::service_fn(move |_tonic_dial: http::Uri| {
        let dialed = https.clone().oneshot(uri.clone());
        async move { dialed.await.and_then(require_h2) }
    });
    Ok((endpoint, dial))
}

/// Trait for communicating with a Zcash chain indexer.
///
/// Implementors provide access to a server speaking the lightwallet gRPC protocol.
/// Callers can depend on the following guarantees:
///
/// - Each method opens a fresh connection (or reuses a pooled one) — no
///   persistent session state is assumed between calls.
pub trait Indexer {
    type TreeState;

    /// Return server metadata (chain name, block height, version, etc.).
    ///
    /// The returned [`LightdInfo`] includes the chain name, current block height,
    /// server version, and consensus branch ID. Callers should not cache this
    /// value across sync boundaries as the block height is a point-in-time snapshot.
    fn get_lightd_info(
        &mut self,
        timeout: Duration,
    ) -> impl Future<Output = Result<LightdInfo, tonic::Status>> + Send;

    /// Return the backend's standard fee report.
    ///
    /// The [`StandardFee`] carries the ZIP 317 marginal fee, zatoshis per
    /// logical action, the backend's `getstandardfee` RPC reports, and the
    /// fee-estimator version (0 is the static ZIP 317 marginal fee).
    /// Backends predating the RPC answer `Unimplemented`.
    fn get_standard_fee(
        &mut self,
        timeout: Duration,
    ) -> impl Future<Output = Result<StandardFee, tonic::Status>> + Send;

    /// Return the height and hash of the chain tip.
    ///
    /// The returned [`BlockId`] identifies the most recent block the server
    /// is aware of. The hash may be omitted by some implementations.
    fn get_latest_block(
        &mut self,
        timeout: Duration,
    ) -> impl Future<Output = Result<BlockId, tonic::Status>> + Send;

    /// Submit a raw transaction to the network.
    ///
    /// On success, returns the transaction ID as a hex string.
    /// On rejection by the network, returns a [`tonic::Status`]
    /// containing the rejection reason. Callers should be prepared for
    /// transient failures and may retry.
    fn send_transaction(
        &mut self,
        tx: RawTransaction,
        timeout: Duration,
    ) -> impl Future<Output = Result<String, tonic::Status>> + Send;

    /// Fetch the note commitment tree state for the given block.
    ///
    /// Returns Sapling and Orchard commitment tree frontiers as of the
    /// end of the specified block. The block can be identified by height,
    /// hash, or both via [`BlockId`]. Requesting an unmined block is an error.
    fn get_tree_state(
        &mut self,
        block_id: BlockId,
        timeout: Duration,
    ) -> impl Future<Output = Result<Self::TreeState, tonic::Status>> + Send;

    /// Return the compact block at the given height.
    ///
    /// The returned [`CompactBlock`] contains compact transaction data
    /// sufficient for trial decryption and nullifier detection.
    fn get_block(
        &mut self,
        block_id: BlockId,
        timeout: Duration,
    ) -> impl Future<Output = Result<CompactBlock, tonic::Status>> + Send;

    /// Return the compact block at the given height, containing only nullifiers.
    ///
    /// The returned [`CompactBlock`] omits output data, retaining only
    /// spend nullifiers. Callers should migrate to [`get_block`](Indexer::get_block).
    #[deprecated(note = "use get_block instead")]
    fn get_block_nullifiers(
        &mut self,
        block_id: BlockId,
        timeout: Duration,
    ) -> impl Future<Output = Result<CompactBlock, tonic::Status>> + Send;

    /// Return a stream of consecutive compact blocks for the given range.
    ///
    /// Both endpoints of the range are inclusive. If `start <= end`, blocks
    /// are yielded in ascending height order; if `start > end`, blocks are
    /// yielded in descending height order. See the test
    /// `tests::get_block_range_supports_descending_order` for a live
    /// verification of descending order against a public indexer.
    ///
    /// Callers must consume or drop the stream before the connection is reused.
    fn get_block_range(
        &mut self,
        range: BlockRange,
        timeout: Duration,
    ) -> impl Future<Output = Result<tonic::Streaming<CompactBlock>, tonic::Status>> + Send;

    /// Return a stream of consecutive compact blocks (nullifiers only) for the given range.
    ///
    /// Same streaming guarantees as [`get_block_range`](Indexer::get_block_range)
    /// but each block contains only nullifiers.
    /// Callers should migrate to [`get_block_range`](Indexer::get_block_range).
    #[deprecated(note = "use get_block_range instead")]
    fn get_block_range_nullifiers(
        &mut self,
        range: BlockRange,
        timeout: Duration,
    ) -> impl Future<Output = Result<tonic::Streaming<CompactBlock>, tonic::Status>> + Send;

    /// Return the full serialized transaction matching the given filter.
    ///
    /// The filter identifies a transaction by its txid hash. The returned
    /// [`RawTransaction`] contains the complete serialized bytes and the
    /// block height at which it was mined (0 if in the mempool).
    fn get_transaction(
        &mut self,
        filter: TxFilter,
        timeout: Duration,
    ) -> impl Future<Output = Result<RawTransaction, tonic::Status>> + Send;

    /// Return a stream of compact transactions currently in the mempool.
    ///
    /// The request may include txid suffixes to exclude from the results,
    /// allowing the caller to avoid re-fetching known transactions.
    /// Results may be seconds out of date.
    fn get_mempool_tx(
        &mut self,
        request: GetMempoolTxRequest,
        timeout: Duration,
    ) -> impl Future<Output = Result<tonic::Streaming<CompactTx>, tonic::Status>> + Send;

    /// Return a stream of raw mempool transactions.
    ///
    /// The stream remains open while there are mempool transactions and
    /// closes when a new block is mined. It is a subscription with no
    /// natural end time, so it carries no call deadline: the server ends it,
    /// and [`crate::time::INDEXER_CONNECT_TIMEOUT`] bounds the connection
    /// it is opened on.
    fn get_mempool_stream(
        &mut self,
    ) -> impl Future<Output = Result<tonic::Streaming<RawTransaction>, tonic::Status>> + Send;

    /// Return the note commitment tree state at the chain tip.
    ///
    /// Equivalent to calling [`get_tree_state`](Indexer::get_tree_state) with
    /// the current tip height, but avoids the need to query the tip first.
    fn get_latest_tree_state(
        &mut self,
        timeout: Duration,
    ) -> impl Future<Output = Result<Self::TreeState, tonic::Status>> + Send;

    /// Return a stream of subtree roots for the given shielded protocol.
    ///
    /// Yields roots in ascending index order starting from `start_index`.
    /// Pass `max_entries = 0` to request all available roots.
    fn get_subtree_roots(
        &mut self,
        arg: GetSubtreeRootsArg,
        timeout: Duration,
    ) -> impl Future<Output = Result<tonic::Streaming<SubtreeRoot>, tonic::Status>> + Send;

    /// Simulate server latency for testing.
    ///
    /// The server will delay for the requested duration before responding.
    /// Returns the number of concurrent Ping RPCs at entry and exit.
    /// Requires the server to be started with `--ping-very-insecure`.
    /// Do not enable in production.
    #[cfg(feature = "ping-very-insecure")]
    fn ping(
        &mut self,
        duration: ProtoDuration,
        timeout: Duration,
    ) -> impl Future<Output = Result<PingResponse, tonic::Status>> + Send;
}

pub struct GrpcIndexer<V = wire::Generated> {
    nakednet_client: CompactTxStreamerClient<Channel>,
    vocabulary: PhantomData<fn() -> V>,
}

impl<V> Clone for GrpcIndexer<V> {
    fn clone(&self) -> Self {
        Self {
            nakednet_client: self.nakednet_client.clone(),
            vocabulary: PhantomData,
        }
    }
}

impl<V> fmt::Debug for GrpcIndexer<V> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GrpcIndexer")
            .field("nakednet_client", &self.nakednet_client)
            .field("vocabulary", &self.vocabulary)
            .finish()
    }
}

impl<V> GrpcIndexer<V> {
    #[must_use]
    pub fn with_vocabulary<W>(self) -> GrpcIndexer<W> {
        GrpcIndexer {
            nakednet_client: self.nakednet_client,
            vocabulary: PhantomData,
        }
    }
}

impl GrpcIndexer {
    pub async fn new(uri: http::Uri) -> Result<Self, GetClientError> {
        let (endpoint, dial) = indexer_endpoint(uri, mozilla_roots())?;
        let channel = endpoint.connect_with_connector(dial).await?;
        Ok(Self::over(channel))
    }

    /// As [`Self::new`], but the underlying channel connects on first use
    /// instead of eagerly. Useful when the indexer may not be reachable at
    /// construction time, including offline tests that never issue an RPC.
    pub fn new_lazy(uri: http::Uri) -> Result<Self, GetClientError> {
        let (endpoint, dial) = indexer_endpoint(uri, mozilla_roots())?;
        let channel = endpoint.connect_with_connector_lazy(dial);
        Ok(Self::over(channel))
    }

    fn over(channel: Channel) -> Self {
        Self {
            nakednet_client: CompactTxStreamerClient::new(channel),
            vocabulary: PhantomData,
        }
    }

    /// Returns the "surface net" gRPC client where the IP address is not obfuscated.
    pub async fn get_nakednet_client(&self) -> CompactTxStreamerClient<Channel> {
        self.nakednet_client.clone()
    }
}

/// A lightwalletd `SendResponse` rejection: the server heard the submission
/// and said no, reported with its complete data (the numeric code and the
/// message), so the caller decides what to make of it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("code {code}: {message}")]
pub struct SendRejection {
    /// The server's nonzero `error_code`.
    pub code: i32,
    /// The server's `error_message`, verbatim.
    pub message: String,
}

/// Interpret a lightwalletd `SendResponse`: `error_code` 0 means the
/// transaction was accepted and `error_message` carries the txid (sometimes
/// quote-wrapped, which is stripped). Any other code is a rejection carrying
/// both fields. The single definition shared by the nakednet
/// [`GrpcIndexer::send_transaction`] and the SOCKS5 transmit path.
pub(crate) fn parse_send_response(
    error_code: i32,
    error_message: String,
) -> Result<String, SendRejection> {
    if error_code == 0 {
        let mut transaction_id = error_message;
        // The length guard keeps a lone `"` from satisfying both quote
        // checks with the same byte and panicking the slice below.
        if transaction_id.starts_with('\"')
            && transaction_id.ends_with('\"')
            && transaction_id.len() >= 2
        {
            transaction_id = transaction_id[1..transaction_id.len() - 1].to_string();
        }
        Ok(transaction_id)
    } else {
        Err(SendRejection {
            code: error_code,
            message: error_message,
        })
    }
}

#[cfg(test)]
mod send_response_parsing {
    use super::*;

    #[test]
    fn acceptance_unquotes_the_txid() {
        assert_eq!(
            parse_send_response(0, "\"deadbeef\"".to_string()),
            Ok("deadbeef".to_string())
        );
    }

    #[test]
    fn acceptance_passes_a_bare_txid_through() {
        assert_eq!(
            parse_send_response(0, "deadbeef".to_string()),
            Ok("deadbeef".to_string())
        );
    }

    #[test]
    fn rejection_carries_the_code_and_the_server_message() {
        assert_eq!(
            parse_send_response(-25, "failed to validate".to_string()),
            Err(SendRejection {
                code: -25,
                message: "failed to validate".to_string()
            })
        );
    }

    /// A one-character `"` message satisfies both `starts_with('"')` and
    /// `ends_with('"')` (they see the same byte), so unquoting must not
    /// slice `[1..0]`. Pins the review's finding 4.
    #[test]
    fn a_lone_quote_acceptance_does_not_panic() {
        assert_eq!(
            parse_send_response(0, "\"".to_string()),
            Ok("\"".to_string())
        );
    }
}

impl<V: wire::Vocabulary> Indexer for GrpcIndexer<V> {
    type TreeState = V::TreeState;

    async fn get_lightd_info(&mut self, timeout: Duration) -> Result<LightdInfo, tonic::Status> {
        rpc::call(Empty {}, timeout, |request| {
            self.nakednet_client.get_lightd_info(request)
        })
        .await
    }

    async fn get_standard_fee(&mut self, timeout: Duration) -> Result<StandardFee, tonic::Status> {
        rpc::call(Empty {}, timeout, |request| {
            self.nakednet_client.get_standard_fee(request)
        })
        .await
    }

    async fn get_latest_block(&mut self, timeout: Duration) -> Result<BlockId, tonic::Status> {
        rpc::call(ChainSpec {}, timeout, |request| {
            self.nakednet_client.get_latest_block(request)
        })
        .await
    }

    async fn send_transaction(
        &mut self,
        tx: RawTransaction,
        timeout: Duration,
    ) -> Result<String, tonic::Status> {
        let sendresponse = rpc::call(tx, timeout, |request| {
            self.nakednet_client.send_transaction(request)
        })
        .await?;
        // The nakednet path keeps its historical error text: the bare server
        // message, without the code prefix `SendRejection` renders.
        parse_send_response(sendresponse.error_code, sendresponse.error_message)
            .map_err(|rejection| tonic::Status::new(tonic::Code::Unknown, rejection.message))
    }

    async fn get_tree_state(
        &mut self,
        block_id: BlockId,
        timeout: Duration,
    ) -> Result<V::TreeState, tonic::Status> {
        rpc::call(block_id, timeout, |request| {
            self.nakednet_client.get_tree_state(request)
        })
        .await
        .map(V::tree_state)
    }

    async fn get_block(
        &mut self,
        block_id: BlockId,
        timeout: Duration,
    ) -> Result<CompactBlock, tonic::Status> {
        rpc::call(block_id, timeout, |request| {
            self.nakednet_client.get_block(request)
        })
        .await
    }

    #[allow(deprecated)]
    async fn get_block_nullifiers(
        &mut self,
        block_id: BlockId,
        timeout: Duration,
    ) -> Result<CompactBlock, tonic::Status> {
        rpc::call(block_id, timeout, |request| {
            self.nakednet_client.get_block_nullifiers(request)
        })
        .await
    }

    async fn get_block_range(
        &mut self,
        range: BlockRange,
        timeout: Duration,
    ) -> Result<tonic::Streaming<CompactBlock>, tonic::Status> {
        rpc::call(range, timeout, |request| {
            self.nakednet_client.get_block_range(request)
        })
        .await
    }

    #[allow(deprecated)]
    async fn get_block_range_nullifiers(
        &mut self,
        range: BlockRange,
        timeout: Duration,
    ) -> Result<tonic::Streaming<CompactBlock>, tonic::Status> {
        rpc::call(range, timeout, |request| {
            self.nakednet_client.get_block_range_nullifiers(request)
        })
        .await
    }

    async fn get_transaction(
        &mut self,
        filter: TxFilter,
        timeout: Duration,
    ) -> Result<RawTransaction, tonic::Status> {
        rpc::call(filter, timeout, |request| {
            self.nakednet_client.get_transaction(request)
        })
        .await
    }

    async fn get_mempool_tx(
        &mut self,
        request: GetMempoolTxRequest,
        timeout: Duration,
    ) -> Result<tonic::Streaming<CompactTx>, tonic::Status> {
        rpc::call(request, timeout, |request| {
            self.nakednet_client.get_mempool_tx(request)
        })
        .await
    }

    async fn get_mempool_stream(
        &mut self,
    ) -> Result<tonic::Streaming<RawTransaction>, tonic::Status> {
        let request = Request::new(Empty {});
        rpc::reply(self.nakednet_client.get_mempool_stream(request)).await
    }

    async fn get_latest_tree_state(
        &mut self,
        timeout: Duration,
    ) -> Result<V::TreeState, tonic::Status> {
        rpc::call(Empty {}, timeout, |request| {
            self.nakednet_client.get_latest_tree_state(request)
        })
        .await
        .map(V::tree_state)
    }

    async fn get_subtree_roots(
        &mut self,
        arg: GetSubtreeRootsArg,
        timeout: Duration,
    ) -> Result<tonic::Streaming<SubtreeRoot>, tonic::Status> {
        rpc::call(arg, timeout, |request| {
            self.nakednet_client.get_subtree_roots(request)
        })
        .await
    }

    #[cfg(feature = "ping-very-insecure")]
    async fn ping(
        &mut self,
        duration: ProtoDuration,
        timeout: Duration,
    ) -> Result<PingResponse, tonic::Status> {
        rpc::call(duration, timeout, |request| {
            self.nakednet_client.ping(request)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    //! Unit and integration-style tests for `zl-netutils`.
    //!
    //! These tests focus on:
    //! - TLS test asset sanity (`test-data/localhost.pem` + `.key`)
    //! - Rustls plumbing (adding a local cert to a root store)
    //! - Connector correctness (scheme validation, HTTP/2 expectations)
    //! - URI rewrite behavior (no panics; returns structured errors)
    //!
    //! Notes:
    //! - Some tests spin up an in-process TLS server and use aggressive timeouts to
    //!   avoid hangs under nextest.
    //! - We explicitly install a rustls crypto provider to avoid
    //!   provider-selection panics in test binaries.

    use crate::default_indexer;
    use http::{Request, Response};
    use hyper::{
        body::{Bytes, Incoming},
        service::service_fn,
    };
    use hyper_util::rt::TokioIo;
    use tokio::{net::TcpListener, sync::oneshot, time::timeout};
    use tokio_rustls::{TlsAcceptor, rustls};

    use super::*;

    use tokio_rustls::rustls::RootCertStore;

    use crate::time::test::LOCAL_TLS_TEST_BOUND;

    const LOCALHOST_CERT_COUNT: usize = 1;

    fn with_localhost_cert(roots: RootCertStore) -> RootCertStore {
        let mut pem = test_tls::LOCALHOST_CERT_PEM;
        rustls_pemfile::certs(&mut pem).fold(roots, |mut roots, cert| {
            let cert = cert.expect("the embedded localhost.pem is well-formed PEM");
            roots
                .add(cert)
                .expect("the embedded localhost.pem holds an X.509 certificate");
            roots
        })
    }

    struct UnderivedVocabulary;

    impl wire::Vocabulary for UnderivedVocabulary {
        type TreeState = u64;

        fn tree_state(reply: lightwallet_protocol::TreeState) -> Self::TreeState {
            reply.height
        }
    }

    fn require_clone_and_debug<T: Clone + fmt::Debug>() {}

    #[test]
    fn an_indexer_is_clone_and_debug_for_a_vocabulary_that_is_neither() {
        require_clone_and_debug::<GrpcIndexer<UnderivedVocabulary>>();
    }

    #[test]
    fn with_localhost_cert_adds_the_embedded_certificate() {
        let roots = with_localhost_cert(RootCertStore::empty());

        assert_eq!(roots.len(), LOCALHOST_CERT_COUNT);
    }

    /// Ensures the committed localhost test certificate exists and is parseable as X.509.
    ///
    /// This catches:
    /// - missing file / wrong working directory assumptions
    /// - invalid PEM encoding
    /// - accidentally committing the wrong artifact (e.g., key instead of cert)
    #[test]
    fn localhost_cert_file_exists_and_is_parseable() {
        const CERT_PATH: &str = "test-data/localhost.pem";

        let pem = std::fs::read(CERT_PATH).expect("missing test-data/localhost.pem");

        let mut cursor = std::io::BufReader::new(pem.as_slice());
        let certs = rustls_pemfile::certs(&mut cursor)
            .filter_map(Result::ok)
            .collect::<Vec<_>>();

        assert!(!certs.is_empty(), "no certs found in {CERT_PATH}");

        for cert in certs {
            let der = cert.as_ref();
            let parsed = x509_parser::parse_x509_certificate(der);
            assert!(
                parsed.is_ok(),
                "failed to parse a cert from {CERT_PATH} as X.509"
            );
        }
    }

    /// Guards against committing a CA certificate as the TLS server certificate.
    ///
    /// Rustls rejects certificates with CA constraints when used as an end-entity
    /// server certificate (e.g. `CaUsedAsEndEntity`), even if the cert is in the
    /// root store. This test ensures the committed localhost cert has `CA:FALSE`.
    #[test]
    fn localhost_cert_is_end_entity_not_ca() {
        let pem =
            std::fs::read("test-data/localhost.pem").expect("missing test-data/localhost.pem");
        let mut cursor = std::io::BufReader::new(pem.as_slice());

        let certs = rustls_pemfile::certs(&mut cursor)
            .filter_map(Result::ok)
            .collect::<Vec<_>>();

        assert!(!certs.is_empty(), "no certs found in localhost.pem");

        let der = certs[0].as_ref();
        let parsed = x509_parser::parse_x509_certificate(der).expect("failed to parse X.509");
        let x509 = parsed.1;

        let constraints = x509
            .basic_constraints()
            .expect("missing basic constraints extension");

        assert!(
            !constraints.unwrap().value.ca,
            "localhost.pem must be CA:FALSE"
        );
    }

    /// Loads a rustls `ServerConfig` for a local TLS server using the committed
    /// test certificate and private key.
    ///
    /// The cert/key pair is *test-only* and is stored under `test-data/`.
    /// This is used to verify that the client-side root-store injection
    /// (`with_localhost_cert`) actually enables successful TLS handshakes.
    fn load_test_server_config() -> std::sync::Arc<rustls::ServerConfig> {
        // Tests build rustls configs directly, outside client_tls_config's
        // install path.
        crate::ensure_default_crypto_provider();
        let cert_pem =
            std::fs::read("test-data/localhost.pem").expect("missing test-data/localhost.pem");
        let key_pem =
            std::fs::read("test-data/localhost.key").expect("missing test-data/localhost.key");

        let mut cert_cursor = std::io::BufReader::new(cert_pem.as_slice());
        let mut key_cursor = std::io::BufReader::new(key_pem.as_slice());

        let certs = rustls_pemfile::certs(&mut cert_cursor)
            .filter_map(Result::ok)
            .collect::<Vec<_>>();

        let key = rustls_pemfile::private_key(&mut key_cursor)
            .expect("failed to read private key")
            .expect("no private key found");

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("bad cert or key");

        std::sync::Arc::new(config)
    }
    /// Smoke test: adding the committed localhost cert to a rustls root store enables
    /// a client to complete a TLS handshake and perform an HTTP request.
    ///
    /// Implementation notes:
    /// - Uses a local TLS server with the committed cert/key.
    /// - Uses strict timeouts to prevent hangs under nextest.
    /// - Explicitly drains the request body and disables keep-alive so that
    ///   `serve_connection` terminates deterministically.
    /// - Installs the rustls crypto provider to avoid provider
    ///   selection panics in test binaries.
    #[tokio::test]
    async fn with_localhost_cert_enables_tls_handshake() {
        use http_body_util::Full;
        use hyper::service::service_fn;
        use hyper_util::rt::TokioIo;
        use tokio::net::TcpListener;
        use tokio_rustls::TlsAcceptor;
        use tokio_rustls::rustls;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
        let addr = listener.local_addr().expect("local_addr failed");

        let tls_config = load_test_server_config();
        let acceptor = TlsAcceptor::from(tls_config);

        let ready = oneshot::channel::<()>();
        let ready_tx = ready.0;
        let ready_rx = ready.1;

        let server_task = tokio::spawn(async move {
            let _ = ready_tx.send(());

            let accept_res = timeout(Duration::from_secs(3), listener.accept()).await;
            let (socket, _) = accept_res
                .expect("server accept timed out")
                .expect("accept failed");

            let tls_stream = timeout(Duration::from_secs(3), acceptor.accept(socket))
                .await
                .expect("tls accept timed out")
                .expect("tls accept failed");

            let io = TokioIo::new(tls_stream);

            let svc = service_fn(|mut req: http::Request<hyper::body::Incoming>| async move {
                use http_body_util::BodyExt;

                while let Some(frame) = req.body_mut().frame().await {
                    if frame.is_err() {
                        break;
                    }
                }

                let mut resp = http::Response::new(Full::new(Bytes::from_static(b"ok")));
                resp.headers_mut().insert(
                    http::header::CONNECTION,
                    http::HeaderValue::from_static("close"),
                );
                Ok::<_, hyper::Error>(resp)
            });

            timeout(
                Duration::from_secs(3),
                hyper::server::conn::http1::Builder::new()
                    .keep_alive(false)
                    .serve_connection(io, svc),
            )
            .await
            .expect("serve_connection timed out")
            .expect("serve_connection failed");
        });

        timeout(Duration::from_secs(1), ready_rx)
            .await
            .expect("server ready signal timed out")
            .expect("server dropped before ready");

        // Build client root store and add the test cert.
        let roots = with_localhost_cert(rustls::RootCertStore::empty());

        let client_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();

        // This MUST allow http1 since the server uses hyper http1 builder.
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(client_config)
            .https_only()
            .enable_http1()
            .build();

        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(https);

        let uri: http::Uri = format!("https://127.0.0.1:{}/", addr.port())
            .parse()
            .expect("bad uri");

        let req = http::Request::builder()
            .method("GET")
            .uri(uri)
            .body(Full::<Bytes>::new(Bytes::new()))
            .expect("request build failed");

        let res = timeout(Duration::from_secs(3), client.request(req))
            .await
            .expect("client request timed out")
            .expect("TLS handshake or request failed");

        assert!(res.status().is_success());

        timeout(Duration::from_secs(3), server_task)
            .await
            .expect("server task timed out")
            .expect("server task failed");
    }

    /// Validates that the connector rejects non-HTTP(S) URIs.
    ///
    /// This test is intended to fail until production code checks for
    /// `http` and `https` schemes only, rejecting everything else (e.g. `ftp`).
    #[tokio::test]
    async fn rejects_non_http_schemes() {
        let uri: http::Uri = "ftp://example.com:1234".parse().unwrap();
        let res = GrpcIndexer::new(uri).await;

        assert!(
            res.is_err(),
            "expected GrpcIndexer::new() to reject non-http(s) schemes, but got Ok"
        );
    }

    /// Demonstrates the HTTPS downgrade hazard: the underlying client can successfully
    /// talk to an HTTP/1.1-only TLS server if the HTTPS branch does not enforce HTTP/2.
    ///
    /// This is intentionally written as a “should be HTTP/2” test so it fails until
    /// the HTTPS client is constructed with `http2_only(true)`.
    #[tokio::test]
    async fn https_connector_must_not_downgrade_to_http1() {
        use http_body_util::Full;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
        let addr = listener.local_addr().expect("local_addr failed");

        let tls_config = load_test_server_config();
        let acceptor = TlsAcceptor::from(tls_config);

        let server_task = tokio::spawn(async move {
            let accept_res = timeout(Duration::from_secs(3), listener.accept()).await;
            let (socket, _) = accept_res
                .expect("server accept timed out")
                .expect("accept failed");

            let tls_stream = acceptor.accept(socket).await.expect("tls accept failed");
            let io = TokioIo::new(tls_stream);

            let svc = service_fn(|_req: Request<Incoming>| async move {
                Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from_static(b"ok"))))
            });

            // This may error with VersionH2 if the client sends an h2 preface, or it may
            // simply never be reached if ALPN fails earlier. Either is fine for this test.
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, svc)
                .await;
        });

        let base = format!("https://127.0.0.1:{}", addr.port());
        let uri = base.parse::<http::Uri>().expect("bad base uri");

        let roots = with_localhost_cert(mozilla_roots());
        let (endpoint, dial) = indexer_endpoint(uri, roots)
            .expect("an https URI with a host and a port has a valid scheme and authority");
        let connect_res = endpoint.connect_with_connector(dial).await;

        // A gRPC (HTTP/2) client must not succeed against an HTTP/1.1-only TLS server.
        let Err(refusal) = connect_res else {
            panic!("expected connect to fail (no downgrade to HTTP/1.1), but it succeeded");
        };
        assert!(
            diag::chain_texts(&refusal)
                .iter()
                .any(|text| text == &H2NotNegotiated.to_string()),
            "expected the ALPN refusal in the chain, got {refusal:?}"
        );

        server_task.abort();
    }

    const DIALS_PER_CONNECTOR: usize = 2;
    const POST_HANDSHAKE_BYTES: &[u8] = b"t";

    async fn localhost_h2_listener() -> (TcpListener, TlsAcceptor, http::Uri) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the loopback interface always has a free ephemeral port");
        let port = listener
            .local_addr()
            .expect("a bound listener has a local address")
            .port();
        let uri = format!("https://127.0.0.1:{port}")
            .parse::<http::Uri>()
            .expect("an https scheme, a loopback address, and a port form a URI");

        let mut server_config = std::sync::Arc::unwrap_or_clone(load_test_server_config());
        server_config.alpn_protocols = vec![ALPN_H2.to_vec()];
        let acceptor = TlsAcceptor::from(std::sync::Arc::new(server_config));

        (listener, acceptor, uri)
    }

    async fn handshake_kinds_over_redials<D>(
        mut dial: D,
        uri: http::Uri,
        listener: TcpListener,
        acceptor: TlsAcceptor,
    ) -> Vec<rustls::HandshakeKind>
    where
        D: tower::Service<http::Uri, Response = IndexerStream>,
        D::Error: std::fmt::Debug,
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let server_task = tokio::spawn(async move {
            let mut kinds = Vec::with_capacity(DIALS_PER_CONNECTOR);
            for _ in 0..DIALS_PER_CONNECTOR {
                let (socket, _) = listener
                    .accept()
                    .await
                    .expect("the client dials this listener once per iteration");
                let mut tls_stream = acceptor
                    .accept(socket)
                    .await
                    .expect("the client trusts the localhost certificate and offers h2");
                kinds.push(
                    tls_stream
                        .get_ref()
                        .1
                        .handshake_kind()
                        .expect("an accepted TLS stream has completed its handshake"),
                );
                tls_stream
                    .write_all(POST_HANDSHAKE_BYTES)
                    .await
                    .expect("the client holds the stream open until it reads these bytes");
                let mut until_client_closes = Vec::new();
                let _ = tls_stream.read_to_end(&mut until_client_closes).await;
            }
            kinds
        });

        let redials = async {
            for _ in 0..DIALS_PER_CONNECTOR {
                let stream = dial
                    .ready()
                    .await
                    .expect("the connector is always ready")
                    .call(uri.clone())
                    .await
                    .expect("the listener serves the localhost certificate over h2");
                let mut stream = TokioIo::new(stream);
                let mut received = [0u8; POST_HANDSHAKE_BYTES.len()];
                stream
                    .read_exact(&mut received)
                    .await
                    .expect("the server writes these bytes after every handshake");
            }
        };
        timeout(LOCAL_TLS_TEST_BOUND, redials)
            .await
            .expect("loopback dials settle within the local TLS bound");

        timeout(LOCAL_TLS_TEST_BOUND, server_task)
            .await
            .expect("the server ends once the last client stream closes")
            .expect("the server task does not panic when every handshake succeeds")
    }

    #[tokio::test]
    async fn a_redial_through_the_indexer_connector_never_resumes() {
        let (listener, acceptor, uri) = localhost_h2_listener().await;

        let roots = with_localhost_cert(mozilla_roots());
        let (_endpoint, dial) = indexer_endpoint(uri.clone(), roots)
            .expect("an https URI with a host and a port has a valid scheme and authority");

        let kinds = handshake_kinds_over_redials(dial, uri, listener, acceptor).await;

        assert_eq!(kinds.len(), DIALS_PER_CONNECTOR);
        assert!(
            !kinds.contains(&rustls::HandshakeKind::Resumed),
            "the indexer connector resumed a TLS session: {kinds:?}"
        );
    }

    #[tokio::test]
    async fn a_redial_resumes_when_resumption_stays_enabled() {
        let (listener, acceptor, uri) = localhost_h2_listener().await;

        let roots = with_localhost_cert(mozilla_roots());
        let mut resuming_config = client_tls_config(roots);
        resuming_config.resumption = rustls::client::Resumption::default();
        let dial = HttpsConnectorBuilder::new()
            .with_tls_config(resuming_config)
            .https_or_http()
            .enable_http2()
            .wrap_connector(indexer_connector(dns::hyper_resolver()));

        let kinds = handshake_kinds_over_redials(dial, uri, listener, acceptor).await;

        assert_eq!(
            kinds.last(),
            Some(&rustls::HandshakeKind::Resumed),
            "the control connector did not resume, so the test above proves nothing: {kinds:?}"
        );
    }

    /// A path that goes silent while the channel is connecting must fail the
    /// call, not hold it. This listener accepts the TCP connection and never
    /// answers, which is what a dropped network looks like to a client that is
    /// reconnecting: the TLS handshake waits for a reply that is not coming.
    ///
    /// Before `INDEXER_CONNECT_TIMEOUT` the call below never returned, whatever
    /// deadline it carried, because a request's deadline only starts once the
    /// channel has a connection to send it on.
    #[tokio::test]
    async fn a_silent_indexer_fails_the_call_instead_of_hanging() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
        let addr = listener.local_addr().expect("local_addr failed");

        // Held open and never written to.
        let silent = tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                let (socket, _) = listener.accept().await.expect("accept failed");
                held.push(socket);
            }
        });

        let uri: http::Uri = format!("https://127.0.0.1:{}", addr.port())
            .parse()
            .expect("uri");
        let mut indexer = GrpcIndexer::new_lazy(uri).expect("lazy indexer");

        let outcome = timeout(
            crate::time::INDEXER_CONNECT_TIMEOUT + Duration::from_secs(10),
            indexer.get_latest_block(Duration::from_secs(1)),
        )
        .await;
        silent.abort();

        assert!(
            matches!(outcome, Ok(Err(_))),
            "expected the call to fail within the connect bound, got {outcome:?}"
        );
    }

    #[tokio::test]
    async fn connects_to_public_mainnet_indexer_and_gets_info() {
        let endpoint = default_indexer::MAINNET_DEFAULT_INDEXER.to_string();

        let uri: http::Uri = endpoint.parse().expect("bad mainnet indexer URI");

        let response = GrpcIndexer::new(uri)
            .await
            .expect("URI to be valid.")
            .get_lightd_info(LOCAL_TLS_TEST_BOUND)
            .await
            .expect("to get info");
        assert!(
            !response.chain_name.is_empty(),
            "chain_name should not be empty"
        );
        assert!(
            response.block_height > 0,
            "block_height should be > 0, got {}",
            response.block_height
        );

        let chain = response.chain_name.to_ascii_lowercase();
        assert!(
            chain.contains("main"),
            "expected a mainnet server, got chain_name={:?}",
            response.chain_name
        );
    }

    /// The proto spec says:
    ///   "If range.start <= range.end, blocks are returned increasing height order;
    ///    otherwise blocks are returned in decreasing height order."
    ///
    /// Our doc for `get_block_range` currently claims ascending-only.
    /// This test requests a descending range (start > end) and asserts
    /// the server returns blocks in decreasing height order.
    #[tokio::test]
    async fn get_block_range_supports_descending_order() {
        use tokio_stream::StreamExt;

        let uri: http::Uri = default_indexer::MAINNET_DEFAULT_INDEXER
            .parse()
            .expect("MAINNET_DEFAULT_INDEXER is a literal that parses");
        let mut indexer = GrpcIndexer::new(uri).await.expect("valid URI");

        let tip = indexer
            .get_latest_block(LOCAL_TLS_TEST_BOUND)
            .await
            .expect("get_latest_block");
        let start_height = tip.height;
        let end_height = start_height.saturating_sub(4);

        // start > end → proto says descending order
        let range = BlockRange {
            start: Some(BlockId {
                height: start_height,
                hash: vec![],
            }),
            end: Some(BlockId {
                height: end_height,
                hash: vec![],
            }),
            pool_types: vec![],
        };

        let mut stream = indexer
            .get_block_range(range, LOCAL_TLS_TEST_BOUND)
            .await
            .expect("get_block_range");

        let mut heights = Vec::new();
        while let Some(block) = stream.next().await {
            let block = block.expect("stream item");
            heights.push(block.height);
        }

        assert!(
            !heights.is_empty(),
            "expected at least one block in the descending range",
        );

        // The proto guarantees descending order when start > end.
        // If this assertion fails, the server does not support descending ranges.
        for window in heights.windows(2) {
            assert!(
                window[0] > window[1],
                "expected descending order, but got heights: {heights:?}",
            );
        }
    }
}
