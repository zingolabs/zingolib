//! A complete [`Indexer`] abstraction for communicating with Zcash chain
//! indexers (`zainod`).
//!
//! # Organizing principle
//!
//! The [`Indexer`] trait is the sole interface a Zcash wallet or tool needs
//! to query, sync, and broadcast against a chain indexer. It is
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

use std::future::Future;
use std::time::Duration;

use tonic::Request;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

use lightwallet_protocol::{
    BlockId, BlockRange, ChainSpec, CompactBlock, CompactTx, CompactTxStreamerClient, Empty,
    GetMempoolTxRequest, GetSubtreeRootsArg, LightdInfo, RawTransaction, SubtreeRoot, TreeState,
    TxFilter,
};

#[cfg(feature = "ping-very-insecure")]
use lightwallet_protocol::{Duration as ProtoDuration, PingResponse};

pub mod crypto;
pub mod error;

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
pub use error::*;
pub use lightwallet_protocol;
pub use tonic::{Status, Streaming};

#[cfg(feature = "globally-public-transparent")]
mod globally_public;
#[cfg(feature = "globally-public-transparent")]
pub use globally_public::TransparentIndexer;

// Deliberately ungated: the pure core of the nym proxy lifecycle, whose
// unit tests run in the default build without the nym-sdk stack.
mod mixnet_connect;

// Deliberately ungated and public: the pure racing planner shared by the
// mixnet bootstrap here and zingolib's send fan-out (ADR 0011).
pub mod arm_race;

#[cfg(feature = "nym")]
mod nym_proxy;
#[cfg(feature = "nym")]
pub use nym_proxy::NymProxy;

#[cfg(feature = "socks5-transmit")]
mod socks5_transmit;
#[cfg(feature = "socks5-transmit")]
pub use socks5_transmit::{
    Socks5TransmitError, get_lightd_info_via_socks5, send_transaction_via_socks5,
    transaction_known_via_socks5,
};

fn client_tls_config() -> ClientTlsConfig {
    // The config built here is consumed by rustls at connect time; make
    // sure a process-level CryptoProvider exists before that happens.
    ensure_default_crypto_provider();
    // Allow self-signed certs in tests
    #[cfg(test)]
    {
        ClientTlsConfig::new()
            .ca_certificate(tonic::transport::Certificate::from_pem(
                std::fs::read("test-data/localhost.pem").expect("test file"),
            ))
            .with_webpki_roots()
    }
    #[cfg(not(test))]
    ClientTlsConfig::new().with_webpki_roots()
}

/// Trait for communicating with a Zcash chain indexer.
///
/// Implementors provide access to a server speaking the lightwallet gRPC protocol.
/// Callers can depend on the following guarantees:
///
/// - Each method opens a fresh connection (or reuses a pooled one) — no
///   persistent session state is assumed between calls.
pub trait Indexer {
    /// Return server metadata (chain name, block height, version, etc.).
    ///
    /// The returned [`LightdInfo`] includes the chain name, current block height,
    /// server version, and consensus branch ID. Callers should not cache this
    /// value across sync boundaries as the block height is a point-in-time snapshot.
    fn get_lightd_info(
        &mut self,
        timeout: Duration,
    ) -> impl Future<Output = Result<LightdInfo, tonic::Status>> + Send;

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
    ) -> impl Future<Output = Result<TreeState, tonic::Status>> + Send;

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
    /// closes when a new block is mined.
    fn get_mempool_stream(
        &mut self,
        timeout: Duration,
    ) -> impl Future<Output = Result<tonic::Streaming<RawTransaction>, tonic::Status>> + Send;

    /// Return the note commitment tree state at the chain tip.
    ///
    /// Equivalent to calling [`get_tree_state`](Indexer::get_tree_state) with
    /// the current tip height, but avoids the need to query the tip first.
    fn get_latest_tree_state(
        &mut self,
        timeout: Duration,
    ) -> impl Future<Output = Result<TreeState, tonic::Status>> + Send;

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

/// gRPC-backed [`Indexer`] that connects to a Zcash chain indexer (server).
#[derive(Debug, Clone)]
pub struct GrpcIndexer {
    uri: http::Uri,
    clear_net_client: CompactTxStreamerClient<Channel>,
}

impl GrpcIndexer {
    pub async fn new(uri: http::Uri) -> Result<Self, GetClientError> {
        let scheme = uri
            .scheme_str()
            .ok_or(GetClientError::InvalidScheme)?
            .to_string();
        if scheme != "http" && scheme != "https" {
            return Err(GetClientError::InvalidScheme);
        }
        let _authority = uri
            .authority()
            .ok_or(GetClientError::InvalidAuthority)?
            .clone();

        let endpoint = Endpoint::from_shared(uri.to_string())?.tcp_nodelay(true);
        let endpoint = if scheme == "https" {
            endpoint.tls_config(client_tls_config())?
        } else {
            endpoint
        };
        let channel = endpoint.connect().await?;
        let clear_net_client = CompactTxStreamerClient::new(channel);

        Ok(Self {
            uri,
            clear_net_client,
        })
    }

    /// As [`Self::new`], but the underlying channel connects on first use
    /// instead of eagerly. Useful when the indexer may not be reachable at
    /// construction time — including offline tests that never issue an RPC.
    pub fn new_lazy(uri: http::Uri) -> Result<Self, GetClientError> {
        let scheme = uri
            .scheme_str()
            .ok_or(GetClientError::InvalidScheme)?
            .to_string();
        if scheme != "http" && scheme != "https" {
            return Err(GetClientError::InvalidScheme);
        }
        let _authority = uri
            .authority()
            .ok_or(GetClientError::InvalidAuthority)?
            .clone();

        let endpoint = Endpoint::from_shared(uri.to_string())?.tcp_nodelay(true);
        let endpoint = if scheme == "https" {
            endpoint.tls_config(client_tls_config())?
        } else {
            endpoint
        };
        let channel = endpoint.connect_lazy();
        let clear_net_client = CompactTxStreamerClient::new(channel);

        Ok(Self {
            uri,
            clear_net_client,
        })
    }

    /// Returns URI the gRPC client(s) are connected to.
    pub fn uri(&self) -> &http::Uri {
        &self.uri
    }

    /// Returns the "surface net" gRPC client where the IP address is not obfuscated.
    pub async fn get_clear_net_client(&self) -> CompactTxStreamerClient<Channel> {
        self.clear_net_client.clone()
    }
}

/// Interpret a lightwalletd `SendResponse`: `error_code` 0 means the
/// transaction was accepted and `error_message` carries the txid (sometimes
/// quote-wrapped, which is stripped); any other code is a rejection whose
/// message is returned as the error. The single definition shared by the
/// clearnet [`GrpcIndexer::send_transaction`] and the SOCKS5 transmit path.
pub(crate) fn parse_send_response(
    error_code: i32,
    error_message: String,
) -> Result<String, String> {
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
        Err(error_message)
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
    fn rejection_carries_the_server_message() {
        assert_eq!(
            parse_send_response(-25, "failed to validate".to_string()),
            Err("failed to validate".to_string())
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

impl Indexer for GrpcIndexer {
    async fn get_lightd_info(&mut self, timeout: Duration) -> Result<LightdInfo, tonic::Status> {
        let mut request = Request::new(Empty {});
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_lightd_info(request)
            .await?
            .into_inner())
    }

    async fn get_latest_block(&mut self, timeout: Duration) -> Result<BlockId, tonic::Status> {
        let mut request = Request::new(ChainSpec {});
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_latest_block(request)
            .await?
            .into_inner())
    }

    async fn send_transaction(
        &mut self,
        tx: RawTransaction,
        timeout: Duration,
    ) -> Result<String, tonic::Status> {
        let mut request = Request::new(tx);
        request.set_timeout(timeout);
        let sendresponse = self
            .clear_net_client
            .send_transaction(request)
            .await?
            .into_inner();
        parse_send_response(sendresponse.error_code, sendresponse.error_message)
            .map_err(|message| tonic::Status::new(tonic::Code::Unknown, message))
    }

    async fn get_tree_state(
        &mut self,
        block_id: BlockId,
        timeout: Duration,
    ) -> Result<TreeState, tonic::Status> {
        let mut request = Request::new(block_id);
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_tree_state(request)
            .await?
            .into_inner())
    }

    async fn get_block(
        &mut self,
        block_id: BlockId,
        timeout: Duration,
    ) -> Result<CompactBlock, tonic::Status> {
        let mut request = Request::new(block_id);
        request.set_timeout(timeout);
        Ok(self.clear_net_client.get_block(request).await?.into_inner())
    }

    #[allow(deprecated)]
    async fn get_block_nullifiers(
        &mut self,
        block_id: BlockId,
        timeout: Duration,
    ) -> Result<CompactBlock, tonic::Status> {
        let mut request = Request::new(block_id);
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_block_nullifiers(request)
            .await?
            .into_inner())
    }

    async fn get_block_range(
        &mut self,
        range: BlockRange,
        timeout: Duration,
    ) -> Result<tonic::Streaming<CompactBlock>, tonic::Status> {
        let mut request = Request::new(range);
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_block_range(request)
            .await?
            .into_inner())
    }

    #[allow(deprecated)]
    async fn get_block_range_nullifiers(
        &mut self,
        range: BlockRange,
        timeout: Duration,
    ) -> Result<tonic::Streaming<CompactBlock>, tonic::Status> {
        let mut request = Request::new(range);
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_block_range_nullifiers(request)
            .await?
            .into_inner())
    }

    async fn get_transaction(
        &mut self,
        filter: TxFilter,
        timeout: Duration,
    ) -> Result<RawTransaction, tonic::Status> {
        let mut request = Request::new(filter);
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_transaction(request)
            .await?
            .into_inner())
    }

    async fn get_mempool_tx(
        &mut self,
        request: GetMempoolTxRequest,
        timeout: Duration,
    ) -> Result<tonic::Streaming<CompactTx>, tonic::Status> {
        let mut request = Request::new(request);
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_mempool_tx(request)
            .await?
            .into_inner())
    }

    async fn get_mempool_stream(
        &mut self,
        timeout: Duration,
    ) -> Result<tonic::Streaming<RawTransaction>, tonic::Status> {
        let mut request = Request::new(Empty {});
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_mempool_stream(request)
            .await?
            .into_inner())
    }

    async fn get_latest_tree_state(
        &mut self,
        timeout: Duration,
    ) -> Result<TreeState, tonic::Status> {
        let mut request = Request::new(Empty {});
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_latest_tree_state(request)
            .await?
            .into_inner())
    }

    async fn get_subtree_roots(
        &mut self,
        arg: GetSubtreeRootsArg,
        timeout: Duration,
    ) -> Result<tonic::Streaming<SubtreeRoot>, tonic::Status> {
        let mut request = Request::new(arg);
        request.set_timeout(timeout);
        Ok(self
            .clear_net_client
            .get_subtree_roots(request)
            .await?
            .into_inner())
    }

    #[cfg(feature = "ping-very-insecure")]
    async fn ping(
        &mut self,
        duration: ProtoDuration,
        timeout: Duration,
    ) -> Result<PingResponse, tonic::Status> {
        let mut request = Request::new(duration);
        request.set_timeout(timeout);
        Ok(self.clear_net_client.ping(request).await?.into_inner())
    }
}

#[cfg(test)]
mod tests {
    //! Unit and integration-style tests for `zingo-netutils`.
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

    use std::time::Duration;

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

    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

    fn add_test_cert_to_roots(roots: &mut RootCertStore) {
        use tonic::transport::CertificateDer;
        eprintln!("Adding test cert to roots");

        const TEST_PEMFILE_PATH: &str = "test-data/localhost.pem";

        let Ok(fd) = std::fs::File::open(TEST_PEMFILE_PATH) else {
            eprintln!("Test TLS cert not found at {TEST_PEMFILE_PATH}, skipping");
            return;
        };

        let mut buf = std::io::BufReader::new(fd);
        let certs_bytes: Vec<tonic::transport::CertificateDer> = rustls_pemfile::certs(&mut buf)
            .filter_map(Result::ok)
            .collect();

        let certs: Vec<CertificateDer<'_>> = certs_bytes.into_iter().collect();
        roots.add_parsable_certificates(certs);
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
    /// (`add_test_cert_to_roots`) actually enables successful TLS handshakes.
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
    async fn add_test_cert_to_roots_enables_tls_handshake() {
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
        let mut roots = rustls::RootCertStore::empty();
        add_test_cert_to_roots(&mut roots);

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

        let endpoint = tonic::transport::Endpoint::from_shared(uri.to_string())
            .expect("endpoint")
            .tcp_nodelay(true);

        let connect_res = endpoint
            .tls_config(client_tls_config())
            .expect("tls_config failed")
            .connect()
            .await;

        // A gRPC (HTTP/2) client must not succeed against an HTTP/1.1-only TLS server.
        assert!(
            connect_res.is_err(),
            "expected connect to fail (no downgrade to HTTP/1.1), but it succeeded"
        );

        server_task.abort();
    }

    #[tokio::test]
    async fn connects_to_public_mainnet_indexer_and_gets_info() {
        let endpoint = "https://zec.rocks:443".to_string();

        let uri: http::Uri = endpoint.parse().expect("bad mainnet indexer URI");

        let response = GrpcIndexer::new(uri)
            .await
            .expect("URI to be valid.")
            .get_lightd_info(DEFAULT_TIMEOUT)
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

        let uri: http::Uri = "https://zec.rocks:443".parse().unwrap();
        let mut indexer = GrpcIndexer::new(uri).await.expect("valid URI");

        let tip = indexer
            .get_latest_block(DEFAULT_TIMEOUT)
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
            .get_block_range(range, DEFAULT_TIMEOUT)
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
