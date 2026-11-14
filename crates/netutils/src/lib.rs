//! A complete [`Indexer`] abstraction for communicating with Zcash chain
//! indexers (`zainod`).
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
pub mod default_indexer;
pub mod diag;
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

// The temporal parameters of every crate that can see this one, including
// the values owned by tests (`time::test`). See the module's registry.
pub mod time;
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
fn indexer_connector() -> hyper_util::client::legacy::connect::HttpConnector {
    let mut http = hyper_util::client::legacy::connect::HttpConnector::new();
    http.enforce_http(false);
    http.set_nodelay(true);
    http
}

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

#[derive(Debug, Clone)]
pub struct GrpcIndexer {
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

        let endpoint = Endpoint::from_shared(uri.to_string())?
            .connect_timeout(crate::time::INDEXER_CONNECT_TIMEOUT);
        let endpoint = if scheme == "https" {
            endpoint.tls_config(client_tls_config())?
        } else {
            endpoint
        };
        let channel = endpoint.connect_with_connector(indexer_connector()).await?;
        let clear_net_client = CompactTxStreamerClient::new(channel);

        Ok(Self { clear_net_client })
    }

    /// As [`Self::new`], but the underlying channel connects on first use
    /// instead of eagerly. Useful when the indexer may not be reachable at
    /// construction time, including offline tests that never issue an RPC.
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

        let endpoint = Endpoint::from_shared(uri.to_string())?
            .connect_timeout(crate::time::INDEXER_CONNECT_TIMEOUT);
        let endpoint = if scheme == "https" {
            endpoint.tls_config(client_tls_config())?
        } else {
            endpoint
        };
        let channel = endpoint.connect_with_connector_lazy(indexer_connector());
        let clear_net_client = CompactTxStreamerClient::new(channel);

        Ok(Self { clear_net_client })
    }

    /// Returns the "surface net" gRPC client where the IP address is not obfuscated.
    pub async fn get_clear_net_client(&self) -> CompactTxStreamerClient<Channel> {
        self.clear_net_client.clone()
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
mod send_response_parsing;

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
        // The nakednet path keeps its historical error text: the bare server
        // message, without the code prefix `SendRejection` renders.
        parse_send_response(sendresponse.error_code, sendresponse.error_message)
            .map_err(|rejection| tonic::Status::new(tonic::Code::Unknown, rejection.message))
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
    ) -> Result<tonic::Streaming<RawTransaction>, tonic::Status> {
        Ok(self
            .clear_net_client
            .get_mempool_stream(Request::new(Empty {}))
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
mod tests;
