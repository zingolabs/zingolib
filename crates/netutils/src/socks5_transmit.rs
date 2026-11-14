//! Wallet-side SOCKS5-dialing transmission (ADR 0011, consumption model A).
//!
//! Routes a raw transaction to an indexer through a local SOCKS5 proxy (the
//! `nym-proxy` child process the wallet spawns) and returns the
//! server-reported txid. This path is deliberately light: it needs only a
//! SOCKS5 client and the tonic machinery already present, no nym-sdk, so it
//! resolves and builds in the main workspace's lockfile. See
//! `zingo-adrs zingolib/0011`.
//!
//! Failures are typed by the connection phase that produced them (proxy-dial,
//! tunnel establishment, post-tunnel transport, the RPC's own status, server
//! rejection), and each variant carries that phase's complete
//! data (source errors, elapsed times, the whole `tonic::Status`, the
//! rejection code and message), so a failed send distinguishes "the proxy
//! child is dead" from "the mixnet exit refused this destination" from "the
//! indexer itself said no". The caller decides what to do with a failure.
//! [`Socks5TransmitError::is_failover_candidate`] offers the escalation's
//! reading without discarding anything. [`Socks5Indexer::get_lightd_info`]
//! mirrors the nakednet probe through the same tunnel, pairing the two
//! routes for diagnosis.
#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http::Uri;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

use crate::SendRejection;
use crate::crypto::ensure_default_crypto_provider;
use lightwallet_protocol::{
    BlockId, ChainSpec, CompactTxStreamerClient, Empty, LightdInfo, RawTransaction, TxFilter,
};

/// Why a SOCKS5-tunneled operation did not complete, typed by the connection
/// phase that failed and carrying that phase's complete underlying data
/// (sources, elapsed times, codes, and messages), so the caller decides what
/// to make of a failure. One reading, whether another Destination is
/// worth trying, is offered as [`Self::is_failover_candidate`]. Nothing is
/// flattened away to support it.
#[derive(Debug, thiserror::Error)]
pub enum Socks5TransmitError {
    /// TCP to the local SOCKS5 proxy itself failed: the nym-proxy child is
    /// dead, not yet listening, or the address is stale.
    #[error(
        "the local SOCKS5 proxy at {proxy} is unreachable ({source} after {elapsed:.1?}) — \
         is the nym-proxy child running?"
    )]
    ProxyUnreachable {
        /// The proxy address that refused the dial.
        proxy: String,
        /// How long the dial phase ran before failing.
        elapsed: Duration,
        /// The dial failure itself.
        #[source]
        source: ProxyDialFailure,
    },
    /// The proxy accepted the dial but the SOCKS5 tunnel to the destination
    /// could not be established: the mixnet exit refused, could not reach, or
    /// timed out on the destination, including an Exit Node whose exit policy
    /// blocks the destination host or port.
    #[error("the mixnet exit could not reach {destination} ({source} after {elapsed:.1?})")]
    TunnelRefused {
        /// The destination `host:port` the tunnel was asked for.
        destination: String,
        /// How long the tunnel phase ran before failing.
        elapsed: Duration,
        /// The SOCKS5 failure itself.
        #[source]
        source: TunnelFailure,
    },
    /// The tunnel was established but the channel over it (TLS, HTTP/2)
    /// failed, or the endpoint could not be prepared for dialing at all.
    #[error("transport to {destination} failed ({detail})")]
    TunnelTransport {
        /// The destination `host:port` the tunnel carried.
        destination: String,
        /// The full rendered error chain, so string consumers lose nothing.
        detail: String,
        /// The transport failure, when one structured error produced this
        /// (`None` for pre-dial refusals such as a host-less URI).
        #[source]
        source: Option<tonic::transport::Error>,
    },
    /// The tunnel and channel were established but the RPC ended in a status
    /// rather than a response. The status is carried whole (code, message,
    /// and any transport source chain), and
    /// [`Self::is_failover_candidate`] reads its code as either a transport
    /// failure worth another Destination or a server verdict that is not.
    #[error(
        "rpc to {destination} ended in status {code:?}: {message}",
        code = .status.code(),
        message = .status.message()
    )]
    Rpc {
        /// The destination `host:port` the RPC targeted.
        destination: String,
        /// The complete status the RPC ended with.
        #[source]
        status: tonic::Status,
    },
    /// The tunnel and channel were established but the RPC did not complete
    /// within the client-side bound. Distinct from [`Self::Rpc`]: no status
    /// arrived — the client gave up waiting, which at mixnet latencies means
    /// "slow", not "the server answered". The bound rides along so the
    /// taxonomy's timed-out stage can carry it (issue #2564).
    #[error("the rpc to {destination} did not complete within {after:.1?}")]
    TimedOut {
        /// The destination `host:port` the RPC targeted.
        destination: String,
        /// The client-side bound that elapsed.
        after: Duration,
    },
    /// The indexer heard the submission and rejected it on its merits: a
    /// lightwalletd `SendResponse` with a nonzero error code, carried with
    /// both its fields. Never a failover candidate, since another
    /// Destination would hear the same transaction and say the same.
    #[error("indexer rejected the transaction: {0}")]
    Rejected(#[from] SendRejection),
    /// The indexer URI is not https. Mixnet transmission is TLS-only so the
    /// exit gateway cannot read or tamper with the traffic. A plaintext
    /// indexer is refused rather than dialed.
    #[error("refusing to transmit to a non-https indexer: {indexer}")]
    InsecureScheme {
        /// The offending non-https URI.
        indexer: String,
    },
}

/// How the dial to the local SOCKS5 proxy failed.
#[derive(Debug, thiserror::Error)]
pub enum ProxyDialFailure {
    /// The dial did not complete within the phase timeout.
    #[error("timed out")]
    TimedOut,
    /// The dial completed with an I/O failure (refused, unreachable, ...).
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// How the SOCKS5 tunnel establishment failed.
#[derive(Debug, thiserror::Error)]
pub enum TunnelFailure {
    /// The tunnel did not complete within the phase timeout.
    #[error("timed out")]
    TimedOut,
    /// The proxy's SOCKS5 reply, carried whole.
    #[error(transparent)]
    Socks(#[from] tokio_socks::Error),
}

impl Socks5TransmitError {
    /// The failover policy's reading of this failure: whether submitting to
    /// another Destination could plausibly succeed. A server verdict on
    /// the transaction ([`Self::Rejected`], or an [`Self::Rpc`] status whose
    /// code is a verdict) is final, because every other Destination would
    /// answer the same, while every phase or transport failure is worth
    /// another arm.
    /// This is one interpretation of the complete data above. The caller
    /// decides what to do with it.
    pub fn is_failover_candidate(&self) -> bool {
        match self {
            Socks5TransmitError::Rejected(_) => false,
            Socks5TransmitError::Rpc { status, .. } => {
                status_disposition(status.code()) == StatusDisposition::Transport
            }
            Socks5TransmitError::ProxyUnreachable { .. }
            | Socks5TransmitError::TunnelRefused { .. }
            | Socks5TransmitError::TunnelTransport { .. }
            | Socks5TransmitError::TimedOut { .. }
            | Socks5TransmitError::InsecureScheme { .. } => true,
        }
    }
}

/// Separates one link of a rendered cause chain from the next in a transmit
/// detail, which keeps every link on the one line.
const TRANSMIT_CHAIN_SEPARATOR: &str = ": ";

/// Renders `error` with its complete `source()` chain, which the top-level
/// `Display` of transport errors (tonic's "transport error") otherwise hides.
fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    crate::diag::chain_texts(error).join(TRANSMIT_CHAIN_SEPARATOR)
}

/// How a post-tunnel RPC status reads for the failover policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusDisposition {
    /// The RPC ended without a server verdict (the tunnel, channel, or
    /// deadline gave out), so another Destination is worth trying.
    Transport,
    /// The server judged the request and said no. Another Destination
    /// would hear the same request and say the same.
    Verdict,
}

/// Classify a status code into the failover policy's two readings. The
/// asymmetry is deliberate: a verdict misread as transport merely costs a
/// redundant arm (a duplicate submission already counts as success), while
/// transport misread as a verdict suppresses exactly the failover the
/// escalation exists for. Codes that are not clearly server verdicts
/// therefore read as transport, including `Unknown`, which tonic uses for
/// mid-RPC connection failures (server-side rejections arrive as a
/// `SendResponse` error code over this path, not as a status).
fn status_disposition(code: tonic::Code) -> StatusDisposition {
    match code {
        tonic::Code::InvalidArgument
        | tonic::Code::FailedPrecondition
        | tonic::Code::OutOfRange
        | tonic::Code::PermissionDenied
        | tonic::Code::Unauthenticated
        | tonic::Code::Unimplemented
        | tonic::Code::NotFound
        | tonic::Code::AlreadyExists => StatusDisposition::Verdict,
        _ => StatusDisposition::Transport,
    }
}

/// Bound `rpc` by `after`, typing an elapse as
/// [`Socks5TransmitError::TimedOut`] against `destination`. The one
/// client-side RPC bound on this path — the channel deliberately carries
/// none (see [`Socks5Indexer::dial`]), so this timer is never raced for the
/// classification.
async fn bounded_rpc<T>(
    destination: String,
    after: Duration,
    rpc: impl std::future::Future<Output = Result<tonic::Response<T>, tonic::Status>>,
) -> Result<T, Socks5TransmitError> {
    match tokio::time::timeout(after, rpc).await {
        Err(_elapsed) => Err(Socks5TransmitError::TimedOut { destination, after }),
        Ok(Ok(response)) => Ok(response.into_inner()),
        Ok(Err(status)) => Err(Socks5TransmitError::Rpc {
            destination,
            status,
        }),
    }
}

/// Webpki roots, plus the committed localhost certificate in test builds.
fn tunnel_tls_config() -> ClientTlsConfig {
    let config = ClientTlsConfig::new().with_webpki_roots();
    #[cfg(any(test, feature = "testutils"))]
    let config = config.ca_certificate(tonic::transport::Certificate::from_pem(
        crate::test_tls::LOCALHOST_CERT_PEM,
    ));
    config
}

/// One indexer reached through the local SOCKS5 proxy, every operation
/// opening its own https tunnel under one round-trip bound.
pub struct Socks5Indexer {
    socks5_addr: SocketAddr,
    indexer: Uri,
    timeout: Duration,
}

impl Socks5Indexer {
    /// Groups the local SOCKS5 proxy address, the indexer URI, and the
    /// round-trip bound that every tunneled operation shares.
    pub fn new(socks5_addr: SocketAddr, indexer: Uri, timeout: Duration) -> Self {
        Self {
            socks5_addr,
            indexer,
            timeout,
        }
    }

    /// Submits `raw_tx` at `height` to the indexer through the proxy,
    /// returning the server-reported txid on acceptance.
    pub async fn send_transaction(
        &self,
        raw_tx: &[u8],
        height: u64,
    ) -> Result<String, Socks5TransmitError> {
        let response = self
            .round_trip(
                RawTransaction {
                    data: raw_tx.to_vec(),
                    height,
                },
                |mut client, request| async move { client.send_transaction(request).await },
            )
            .await?;

        // lightwalletd convention: error_code 0 means accepted, and error_message
        // carries the txid (sometimes quote-wrapped). One shared interpretation
        // with GrpcIndexer's own send_transaction handling.
        Ok(crate::parse_send_response(
            response.error_code,
            response.error_message,
        )?)
    }

    /// Fetches the indexer's chain tip through the proxy, the lightest
    /// liveness probe an indexer answers.
    pub async fn get_latest_block(&self) -> Result<BlockId, Socks5TransmitError> {
        self.round_trip(ChainSpec {}, |mut client, request| async move {
            client.get_latest_block(request).await
        })
        .await
    }

    /// Fetches the indexer's `GetLightdInfo` through the proxy, the probe
    /// that names the chain a candidate serves.
    pub async fn get_lightd_info(&self) -> Result<LightdInfo, Socks5TransmitError> {
        self.round_trip(Empty {}, |mut client, request| async move {
            client.get_lightd_info(request).await
        })
        .await
    }

    /// Reports whether the indexer knows the transaction identified by
    /// `txid_hash`, reading a transport failure or an error status as
    /// not-yet-delivered.
    pub async fn transaction_known(&self, txid_hash: &[u8]) -> bool {
        self.round_trip(
            TxFilter {
                block: None,
                index: 0,
                hash: txid_hash.to_vec(),
            },
            |mut client, request| async move { client.get_transaction(request).await },
        )
        .await
        .is_ok()
    }

    /// The `host:port` a tunnel to the indexer targets (https default 443).
    fn destination(&self) -> String {
        format!(
            "{}:{}",
            self.indexer.host().unwrap_or_default(),
            self.indexer.port_u16().unwrap_or(443)
        )
    }

    /// Runs `rpc` against a freshly dialed client with `message` stamped by
    /// the round-trip bound, the one pipeline every public operation shares.
    async fn round_trip<Req, Resp, Fut>(
        &self,
        message: Req,
        rpc: impl FnOnce(CompactTxStreamerClient<Channel>, tonic::Request<Req>) -> Fut,
    ) -> Result<Resp, Socks5TransmitError>
    where
        Fut: std::future::Future<Output = Result<tonic::Response<Resp>, tonic::Status>>,
    {
        let client = self.dial().await?;
        let mut request = tonic::Request::new(message);
        request.set_timeout(self.timeout);
        // The client-side RPC bound: an elapse is typed as its own variant, so
        // a slow round trip is never misread as the server answering.
        bounded_rpc(self.destination(), self.timeout, rpc(client, request)).await
    }

    /// Builds a gRPC client to the indexer through a fresh SOCKS5 tunnel,
    /// with TLS layered on top so the exit gateway cannot read or tamper
    /// with the traffic.
    async fn dial(&self) -> Result<CompactTxStreamerClient<Channel>, Socks5TransmitError> {
        ensure_default_crypto_provider();

        // Every dial opens its own SOCKS5 tunnel. The proxy dial and the
        // tunnel establishment each run under the round-trip bound and record
        // a phase-typed error out of band: tonic collapses connector errors
        // into an opaque "transport error", so the connector deposits the
        // typed failure in a slot this function reads back in preference to
        // tonic's rendering.
        // The connector closure below must be `'static` (tonic stores it in
        // the `Channel`), so what it uses is bound to an owned local first;
        // any capture spelled through `&self` would tie it to this borrow.
        let timeout = self.timeout;

        // Mixnet transmission is https-only: the connection must be TLS end to end
        // so the mixnet exit gateway, which terminates the SOCKS5 tunnel, cannot
        // read or tamper with the traffic. A plaintext (http) indexer is refused
        // rather than dialed.
        if self.indexer.scheme_str() != Some("https") {
            return Err(Socks5TransmitError::InsecureScheme {
                indexer: self.indexer.to_string(),
            });
        }
        let host = self
            .indexer
            .host()
            .ok_or_else(|| Socks5TransmitError::TunnelTransport {
                destination: self.indexer.to_string(),
                detail: "indexer uri has no host".to_string(),
                source: None,
            })?
            .to_string();
        let port = self.indexer.port_u16().unwrap_or(443);
        let destination = self.destination();
        // The one dial-string rendering: every socket address has exactly one
        // dial form, and it is derived here alone, never by a caller.
        let socks5_addr = self.socks5_addr.to_string();

        let endpoint = Endpoint::from_shared(self.indexer.to_string())
            .map_err(|e| Socks5TransmitError::TunnelTransport {
                destination: destination.clone(),
                detail: error_chain(&e),
                source: Some(e),
            })?
            .tcp_nodelay(true)
            // `connect_timeout` bounds the channel establishment — critically
            // the TLS handshake tonic runs on top of the SOCKS5 tunnel, which
            // the connector's own per-phase timeouts do not cover. Without this
            // a Destination that completes the tunnel but stalls the handshake
            // (observed: a lightwalletd on a non-standard port the mixnet exit
            // mishandles) hangs for minutes instead of failing over. The RPC
            // itself is deliberately NOT bounded here: tonic's channel timeout
            // would surface as an opaque status racing the callers' own typed
            // bound, so `round_trip` wraps every RPC in
            // `tokio::time::timeout` and classifies the elapse as
            // [`Socks5TransmitError::TimedOut`] (issue #2564).
            .connect_timeout(timeout)
            .tls_config(tunnel_tls_config())
            .map_err(|e| Socks5TransmitError::TunnelTransport {
                destination: destination.clone(),
                detail: error_chain(&e),
                source: Some(e),
            })?;

        let phase_error: Arc<Mutex<Option<Socks5TransmitError>>> = Arc::default();
        let connector_phase = phase_error.clone();
        let connector_destination = destination.clone();
        let connector = tower::service_fn(move |_uri: Uri| {
            let socks5_addr = socks5_addr.clone();
            let host = host.clone();
            let phase = connector_phase.clone();
            let destination = connector_destination.clone();
            async move {
                let deposit = |error: Socks5TransmitError| {
                    let io = std::io::Error::other(error.to_string());
                    *phase.lock().expect("socks5 phase mutex poisoned") = Some(error);
                    io
                };

                let started = Instant::now();
                let socket =
                    match tokio::time::timeout(timeout, TcpStream::connect(socks5_addr.as_str()))
                        .await
                    {
                        Err(_elapsed) => Err(ProxyDialFailure::TimedOut),
                        Ok(dial) => dial.map_err(ProxyDialFailure::from),
                    }
                    .map_err(|source| {
                        deposit(Socks5TransmitError::ProxyUnreachable {
                            proxy: socks5_addr.clone(),
                            elapsed: started.elapsed(),
                            source,
                        })
                    })?;

                let tunnel_started = Instant::now();
                let stream = match tokio::time::timeout(
                    timeout,
                    tokio_socks::tcp::Socks5Stream::connect_with_socket(
                        socket,
                        (host.as_str(), port),
                    ),
                )
                .await
                {
                    Err(_elapsed) => Err(TunnelFailure::TimedOut),
                    Ok(tunnel) => tunnel.map_err(TunnelFailure::from),
                }
                .map_err(|source| {
                    deposit(Socks5TransmitError::TunnelRefused {
                        destination: destination.clone(),
                        elapsed: tunnel_started.elapsed(),
                        source,
                    })
                })?;

                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        });

        let channel = endpoint
            .connect_with_connector(connector)
            .await
            .map_err(|e| {
                phase_error
                    .lock()
                    .expect("socks5 phase mutex poisoned")
                    .take()
                    .unwrap_or_else(|| Socks5TransmitError::TunnelTransport {
                        destination: destination.clone(),
                        detail: error_chain(&e),
                        source: Some(e),
                    })
            })?;

        Ok(CompactTxStreamerClient::new(channel))
    }
}

#[cfg(test)]
mod tests;
