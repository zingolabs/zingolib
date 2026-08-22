//! Wallet-side SOCKS5-dialing transmission (ADR 0011, consumption model A).
//!
//! Routes a raw transaction to an indexer through a local SOCKS5 proxy (the
//! `nym-proxy` child process the wallet spawns) and returns the
//! server-reported txid. This path is deliberately light: it needs only a
//! SOCKS5 client and the tonic machinery already present, no nym-sdk, so it
//! resolves and builds in the main workspace's lockfile. See
//! `docs/adr/0011-nym-mixnet-transmission.md`.
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
//! mirrors the clearnet probe through the same tunnel, pairing the two
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
/// to make of a failure. One reading, whether another Correspondent is
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
    /// failure worth another Correspondent or a server verdict that is not.
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
    /// Correspondent would hear the same transaction and say the same.
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
    /// another Correspondent could plausibly succeed. A server verdict on
    /// the transaction ([`Self::Rejected`], or an [`Self::Rpc`] status whose
    /// code is a verdict) is final, because every other Correspondent would
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
    zingo_net_diag::chain_texts(error).join(TRANSMIT_CHAIN_SEPARATOR)
}

/// How a post-tunnel RPC status reads for the failover policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusDisposition {
    /// The RPC ended without a server verdict (the tunnel, channel, or
    /// deadline gave out), so another Correspondent is worth trying.
    Transport,
    /// The server judged the request and said no. Another Correspondent
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
            // a Correspondent that completes the tunnel but stalls the handshake
            // (observed: a lightwalletd on a non-standard port the mixnet exit
            // mishandles) hangs for minutes instead of failing over. The RPC
            // itself is deliberately NOT bounded here: tonic's channel timeout
            // would surface as an opaque status racing the callers' own typed
            // bound, so `round_trip` wraps every RPC in
            // `tokio::time::timeout` and classifies the elapse as
            // [`Socks5TransmitError::TimedOut`] (issue #2564).
            .connect_timeout(timeout)
            .tls_config(ClientTlsConfig::new().with_webpki_roots())
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
mod tests {
    use super::*;

    use crate::time::test::MOCK_OP_BOUND;

    fn an_indexer() -> Uri {
        "https://indexer.example:443".parse().expect("static uri")
    }

    /// One submission to [`an_indexer`] through a proxy at `addr`, the shared
    /// subject of the phase-classification tests.
    async fn a_send_through(addr: SocketAddr) -> Result<String, Socks5TransmitError> {
        Socks5Indexer::new(addr, an_indexer(), MOCK_OP_BOUND)
            .send_transaction(b"tx", 1)
            .await
    }

    #[derive(Debug, thiserror::Error)]
    #[error("the inner layer gave out")]
    struct InnerLayer;

    #[derive(Debug, thiserror::Error)]
    #[error("the outer layer gave out")]
    struct OuterLayer(#[source] InnerLayer);

    /// HYPOTHESIS: this module renders a two-link cause chain exactly as the
    /// one sanctioned chain walk does, so the rendering carries no private
    /// copy of the walk. Falsified if the two renderings differ by a single
    /// byte.
    #[test]
    fn the_chain_rendering_matches_the_sanctioned_walk() {
        let error = OuterLayer(InnerLayer);

        assert_eq!(
            error_chain(&error),
            zingo_net_diag::chain_texts(&error).join(": ")
        );
    }

    /// HYPOTHESIS: an RPC that never answers lands the typed timeout with the
    /// exact bound and destination, proven on paused time so no wall clock
    /// passes. Falsified if the elapse surfaces as any other variant or the
    /// record loses the bound. (The full tunnel-and-TLS path cannot stall in
    /// a unit test — the connector pins webpki roots by the https-only rule —
    /// so the bounding seam itself is the unit under test; the Android field
    /// run of issue #2564 is the end-to-end evidence.)
    #[tokio::test(start_paused = true)]
    async fn a_stalled_rpc_lands_the_typed_timeout() {
        let outcome = bounded_rpc::<()>(
            "indexer.example:443".to_string(),
            MOCK_OP_BOUND,
            std::future::pending(),
        )
        .await;
        match outcome {
            Err(Socks5TransmitError::TimedOut { destination, after }) => {
                assert_eq!(destination, "indexer.example:443");
                assert_eq!(after, MOCK_OP_BOUND);
            }
            other => panic!("expected the typed timeout, got {other:?}"),
        }
    }

    /// HYPOTHESIS: an elapsed client bound is typed as its own variant, reads
    /// as a failover candidate (a slow round trip is worth another
    /// Correspondent, never a verdict), and renders the bound it carries.
    /// Falsified if the
    /// variant is misread as final or loses the bound.
    #[test]
    fn a_timed_out_rpc_is_a_failover_candidate_and_names_its_bound() {
        let err = Socks5TransmitError::TimedOut {
            destination: "indexer.example:443".to_string(),
            after: MOCK_OP_BOUND,
        };
        assert!(err.is_failover_candidate());
        let rendered = err.to_string();
        assert!(
            rendered.contains("did not complete within"),
            "rendering must say the client gave up waiting: {rendered}"
        );
    }

    fn an_rpc_error(code: tonic::Code) -> Socks5TransmitError {
        Socks5TransmitError::Rpc {
            destination: "indexer.example:443".to_string(),
            status: tonic::Status::new(code, "boom"),
        }
    }

    /// HYPOTHESIS: an RPC status whose code is transport-shaped (the tunnel,
    /// channel, or deadline gave out without a server verdict) is a failover
    /// candidate. Falsified if any such code reads as final, which would
    /// suppress exactly the failover the escalation exists for
    /// (the PR #2470 review's finding M2).
    #[test]
    fn transport_shaped_statuses_are_failover_candidates() {
        for code in [
            tonic::Code::Unknown,
            tonic::Code::Unavailable,
            tonic::Code::DeadlineExceeded,
            tonic::Code::Cancelled,
            tonic::Code::ResourceExhausted,
            tonic::Code::Aborted,
            tonic::Code::Internal,
        ] {
            assert!(
                an_rpc_error(code).is_failover_candidate(),
                "{code:?} must be worth another Correspondent"
            );
        }
    }

    /// HYPOTHESIS: an RPC status whose code is a server verdict is final, since
    /// another Correspondent would hear the same request and say the same.
    /// Falsified if a verdict code triggers pointless failover arms.
    #[test]
    fn verdict_statuses_are_not_failover_candidates() {
        for code in [
            tonic::Code::InvalidArgument,
            tonic::Code::FailedPrecondition,
            tonic::Code::OutOfRange,
            tonic::Code::PermissionDenied,
            tonic::Code::Unauthenticated,
            tonic::Code::Unimplemented,
            tonic::Code::NotFound,
            tonic::Code::AlreadyExists,
        ] {
            assert!(
                !an_rpc_error(code).is_failover_candidate(),
                "{code:?} is the server's answer; failover cannot change it"
            );
        }
    }

    /// HYPOTHESIS: a `SendResponse` rejection is never a failover candidate,
    /// and the error carries the server's complete data (code and message)
    /// as its typed source. Falsified if either field is flattened away.
    #[test]
    fn a_send_rejection_is_final_and_carries_its_data() {
        let error = Socks5TransmitError::from(crate::SendRejection {
            code: -25,
            message: "failed to validate".to_string(),
        });

        assert!(!error.is_failover_candidate());
        let Socks5TransmitError::Rejected(rejection) = &error else {
            panic!("expected Rejected, got: {error}");
        };
        assert_eq!(rejection.code, -25);
        assert_eq!(rejection.message, "failed to validate");
        assert_eq!(
            error.to_string(),
            "indexer rejected the transaction: code -25: failed to validate"
        );
    }

    /// HYPOTHESIS: an RPC failure keeps the whole status reachable through
    /// `source()`, and its display names the code and message. Falsified if
    /// the status is reduced to prose.
    #[test]
    fn an_rpc_failure_reports_the_status_whole() {
        let error = an_rpc_error(tonic::Code::DeadlineExceeded);

        assert_eq!(
            error.to_string(),
            "rpc to indexer.example:443 ended in status DeadlineExceeded: boom"
        );
        let source = std::error::Error::source(&error).expect("the status is the source");
        let status = source
            .downcast_ref::<tonic::Status>()
            .expect("the source is the status itself");
        assert_eq!(status.code(), tonic::Code::DeadlineExceeded);
        assert_eq!(status.message(), "boom");
    }

    /// Every phase failure (proxy, tunnel, transport, scheme) stays a
    /// failover candidate, the contract the escalation relies on.
    #[test]
    fn phase_failures_are_failover_candidates() {
        let phases = [
            Socks5TransmitError::ProxyUnreachable {
                proxy: "127.0.0.1:1".to_string(),
                elapsed: Duration::from_millis(3),
                source: ProxyDialFailure::TimedOut,
            },
            Socks5TransmitError::TunnelRefused {
                destination: "indexer.example:443".to_string(),
                elapsed: Duration::from_millis(3),
                source: TunnelFailure::TimedOut,
            },
            Socks5TransmitError::TunnelTransport {
                destination: "indexer.example:443".to_string(),
                detail: "tls handshake failed".to_string(),
                source: None,
            },
            Socks5TransmitError::InsecureScheme {
                indexer: "http://indexer.example:9067".to_string(),
            },
        ];
        for error in phases {
            assert!(error.is_failover_candidate(), "not a candidate: {error}");
        }
    }

    /// HYPOTHESIS: a plaintext (http) indexer is refused before any dial, so
    /// mixnet traffic is never sent unencrypted to the exit gateway. Falsified
    /// if an http URI reaches the connector.
    #[tokio::test]
    async fn a_non_https_indexer_is_refused() {
        let http = "http://indexer.example:9067".parse().expect("static uri");
        let refused_port = "127.0.0.1:1".parse().expect("the static address parses");
        let err = Socks5Indexer::new(refused_port, http, MOCK_OP_BOUND)
            .send_transaction(b"tx", 1)
            .await
            .expect_err("http must be refused");
        assert!(
            matches!(err, Socks5TransmitError::InsecureScheme { .. }),
            "expected InsecureScheme, got: {err}"
        );
    }

    /// HYPOTHESIS: the seam accepts the typed socket address a caller holds,
    /// so no caller renders the address and the one dial-string rendering
    /// lives inside the connector. Falsified if the call demands text.
    #[tokio::test]
    async fn the_seam_accepts_the_typed_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr: std::net::SocketAddr = listener.local_addr().expect("local addr");
        drop(listener);
        let err = a_send_through(addr)
            .await
            .expect_err("no proxy is listening");
        assert!(
            matches!(err, Socks5TransmitError::ProxyUnreachable { .. }),
            "expected ProxyUnreachable, got: {err}"
        );
    }

    /// HYPOTHESIS: a dead local proxy is reported as the proxy phase, not an
    /// opaque transport error. Falsified if the error is any other variant.
    #[tokio::test]
    async fn a_dead_proxy_reports_the_proxy_phase() {
        // Bind then drop a listener so the port is known-refused.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);

        let err = a_send_through(addr)
            .await
            .expect_err("no proxy is listening");
        assert!(
            matches!(err, Socks5TransmitError::ProxyUnreachable { .. }),
            "expected ProxyUnreachable, got: {err}"
        );
    }

    /// HYPOTHESIS: a proxy that accepts the dial but breaks the SOCKS5
    /// handshake is reported as the tunnel phase, the "exit could not reach
    /// the destination" signature. Falsified if it reads as a proxy or
    /// transport failure.
    #[tokio::test]
    async fn a_broken_tunnel_reports_the_tunnel_phase() {
        // A fake proxy that accepts the connection and immediately closes it,
        // so the SOCKS5 greeting fails.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                drop(socket);
            }
        });

        let err = a_send_through(addr).await.expect_err("the handshake dies");
        assert!(
            matches!(err, Socks5TransmitError::TunnelRefused { .. }),
            "expected TunnelRefused, got: {err}"
        );
    }
}
