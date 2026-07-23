//! Wallet-side SOCKS5-dialing transmission (ADR 0011, consumption model A).
//!
//! Routes a raw transaction to an indexer through a local SOCKS5 proxy — the
//! `nym-proxy` child process the wallet spawns — and returns the
//! server-reported txid. This path is deliberately light: it needs only a
//! SOCKS5 client and the tonic machinery already present, no nym-sdk, so it
//! resolves and builds in the main workspace's lockfile. See
//! `docs/adr/0011-nym-mixnet-transmission.md`.
//!
//! Failures are typed by the connection phase that produced them —
//! proxy-dial, tunnel establishment, post-tunnel transport, server rejection
//! — with the phase's elapsed time, so a failed send distinguishes "the
//! proxy child is dead" from "the mixnet exit refused this destination"
//! from "the indexer itself misbehaved". [`get_lightd_info_via_socks5`]
//! mirrors the clearnet probe through the same tunnel, pairing the two
//! routes for diagnosis.
#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use http::Uri;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

use crate::crypto::ensure_default_crypto_provider;
use lightwallet_protocol::{CompactTxStreamerClient, Empty, LightdInfo, RawTransaction, TxFilter};

/// Why a SOCKS5-tunneled operation did not complete, typed by the connection
/// phase that failed. Every variant but [`Self::Rejected`] is a candidate for
/// failover to another Broadcast Indexer.
#[derive(Debug, thiserror::Error)]
pub enum Socks5TransmitError {
    /// TCP to the local SOCKS5 proxy itself failed: the nym-proxy child is
    /// dead, not yet listening, or the address is stale.
    #[error(
        "the local SOCKS5 proxy at {proxy} is unreachable ({detail}) — is the nym-proxy child running?"
    )]
    ProxyUnreachable {
        /// The proxy address that refused the dial.
        proxy: String,
        /// The underlying failure and how long the phase took.
        detail: String,
    },
    /// The proxy accepted the dial but the SOCKS5 tunnel to the destination
    /// could not be established: the mixnet exit refused, could not reach, or
    /// timed out on the destination — including a provider whose exit policy
    /// blocks the destination host or port.
    #[error("the mixnet exit could not reach {destination} ({detail})")]
    TunnelRefused {
        /// The destination `host:port` the tunnel was asked for.
        destination: String,
        /// The SOCKS5 reply or timeout and how long the phase took.
        detail: String,
    },
    /// The tunnel was established but the transport over it (TLS, HTTP/2,
    /// the RPC itself) failed.
    #[error("transport to {destination} failed after the tunnel was established ({detail})")]
    TunnelTransport {
        /// The destination `host:port` the tunnel carried.
        destination: String,
        /// The full error chain of the transport failure.
        detail: String,
    },
    /// The indexer was reached but rejected the operation on its merits.
    #[error("indexer rejected the transaction: {0}")]
    Rejected(String),
    /// The indexer URI is not https. Mixnet transmission is TLS-only so the
    /// exit gateway cannot read or tamper with the traffic; a plaintext
    /// indexer is refused rather than dialed.
    #[error("refusing to transmit to a non-https indexer: {indexer}")]
    InsecureScheme {
        /// The offending non-https URI.
        indexer: String,
    },
}

/// Renders `error` with its complete `source()` chain, which the top-level
/// `Display` of transport errors (tonic's "transport error") otherwise hides.
fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        rendered.push_str(": ");
        rendered.push_str(&cause.to_string());
        source = cause.source();
    }
    rendered
}

/// Submits `raw_tx` to `indexer` through the local SOCKS5 proxy at
/// `socks5_addr` (for example `"127.0.0.1:43210"`), returning the
/// server-reported txid on acceptance. `height` fills the `RawTransaction`
/// height field.
///
/// A phase-typed connection failure (see [`Socks5TransmitError`]) lets the
/// caller fail over to a different indexer; a server-side rejection yields
/// [`Socks5TransmitError::Rejected`].
pub async fn send_transaction_via_socks5(
    socks5_addr: &str,
    indexer: &Uri,
    raw_tx: &[u8],
    height: u64,
    timeout: Duration,
) -> Result<String, Socks5TransmitError> {
    let mut client = connect_via_socks5(socks5_addr, indexer, timeout).await?;
    let mut request = tonic::Request::new(RawTransaction {
        data: raw_tx.to_vec(),
        height,
    });
    request.set_timeout(timeout);

    let response = client
        .send_transaction(request)
        .await
        .map_err(|status| Socks5TransmitError::Rejected(format!("{status:?}")))?
        .into_inner();

    // lightwalletd convention: error_code 0 means accepted, and error_message
    // carries the txid (sometimes quote-wrapped). One shared interpretation
    // with GrpcIndexer's own send_transaction handling.
    crate::parse_send_response(response.error_code, response.error_message)
        .map_err(Socks5TransmitError::Rejected)
}

/// Fetches the indexer's `GetLightdInfo` through the local SOCKS5 proxy —
/// the mixnet leg of a paired clearnet/mixnet probe. The same phase-typed
/// failures as the send path, so a probe diagnoses exactly what a send
/// would hit.
pub async fn get_lightd_info_via_socks5(
    socks5_addr: &str,
    indexer: &Uri,
    timeout: Duration,
) -> Result<LightdInfo, Socks5TransmitError> {
    let mut client = connect_via_socks5(socks5_addr, indexer, timeout).await?;
    let mut request = tonic::Request::new(Empty {});
    request.set_timeout(timeout);
    client
        .get_lightd_info(request)
        .await
        .map(tonic::Response::into_inner)
        .map_err(|status| Socks5TransmitError::Rejected(format!("{status:?}")))
}

/// Build a gRPC client to `indexer` dialed through the local SOCKS5 proxy at
/// `socks5_addr`. Shared by the send, delivery-check, and probe paths so the
/// dialing plumbing lives in one place. Each RPC opens its own SOCKS5 tunnel
/// with TLS layered on top; the indexer must be https (a plaintext scheme is
/// refused) so the exit gateway cannot read or tamper with the traffic.
///
/// The proxy dial and the tunnel establishment each run under `timeout` and
/// record a phase-typed error out of band: tonic collapses connector errors
/// into an opaque "transport error", so the connector deposits the typed
/// failure in a slot this function reads back in preference to tonic's
/// rendering.
async fn connect_via_socks5(
    socks5_addr: &str,
    indexer: &Uri,
    timeout: Duration,
) -> Result<CompactTxStreamerClient<Channel>, Socks5TransmitError> {
    ensure_default_crypto_provider();

    // Mixnet transmission is https-only: the connection must be TLS end to end
    // so the mixnet exit gateway, which terminates the SOCKS5 tunnel, cannot
    // read or tamper with the traffic. A plaintext (http) indexer is refused
    // rather than dialed.
    if indexer.scheme_str() != Some("https") {
        return Err(Socks5TransmitError::InsecureScheme {
            indexer: indexer.to_string(),
        });
    }
    let host = indexer
        .host()
        .ok_or_else(|| Socks5TransmitError::TunnelTransport {
            destination: indexer.to_string(),
            detail: "indexer uri has no host".to_string(),
        })?
        .to_string();
    let port = indexer.port_u16().unwrap_or(443);
    let destination = format!("{host}:{port}");
    let socks5_addr = socks5_addr.to_string();

    let endpoint = Endpoint::from_shared(indexer.to_string())
        .map_err(|e| Socks5TransmitError::TunnelTransport {
            destination: destination.clone(),
            detail: e.to_string(),
        })?
        .tcp_nodelay(true)
        // `timeout` bounds each RPC; `connect_timeout` bounds the channel
        // establishment — critically the TLS handshake tonic runs on top of
        // the SOCKS5 tunnel, which the connector's own per-phase timeouts do
        // not cover. Without this a witness that completes the tunnel but
        // stalls the handshake (observed: a lightwalletd on a non-standard
        // port the mixnet exit mishandles) hangs for minutes instead of
        // failing over.
        .timeout(timeout)
        .connect_timeout(timeout)
        .tls_config(ClientTlsConfig::new().with_webpki_roots())
        .map_err(|e| Socks5TransmitError::TunnelTransport {
            destination: destination.clone(),
            detail: e.to_string(),
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
            let socket = tokio::time::timeout(timeout, TcpStream::connect(socks5_addr.as_str()))
                .await
                .map_err(|_| "timed out".to_string())
                .and_then(|dial| dial.map_err(|e| e.to_string()))
                .map_err(|detail| {
                    deposit(Socks5TransmitError::ProxyUnreachable {
                        proxy: socks5_addr.clone(),
                        detail: format!("{detail} after {:.1?}", started.elapsed()),
                    })
                })?;

            let tunnel_started = Instant::now();
            let stream = tokio::time::timeout(
                timeout,
                tokio_socks::tcp::Socks5Stream::connect_with_socket(socket, (host.as_str(), port)),
            )
            .await
            .map_err(|_| "timed out".to_string())
            .and_then(|tunnel| tunnel.map_err(|e| e.to_string()))
            .map_err(|detail| {
                deposit(Socks5TransmitError::TunnelRefused {
                    destination: destination.clone(),
                    detail: format!("{detail} after {:.1?}", tunnel_started.elapsed()),
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
                })
        })?;

    Ok(CompactTxStreamerClient::new(channel))
}

/// Whether the indexer, reached through the SOCKS5 proxy, knows the
/// transaction identified by `txid_hash` — the SOCKS5 mirror of the clearnet
/// `get_transaction` delivery check the resilient transmit policy runs after
/// its retries. A transport failure or an error status both read as "not
/// known", so the result is a plain bool the caller treats as not-yet-delivered.
pub async fn transaction_known_via_socks5(
    socks5_addr: &str,
    indexer: &Uri,
    txid_hash: &[u8],
    timeout: Duration,
) -> bool {
    let Ok(mut client) = connect_via_socks5(socks5_addr, indexer, timeout).await else {
        return false;
    };
    let mut request = tonic::Request::new(TxFilter {
        block: None,
        index: 0,
        hash: txid_hash.to_vec(),
    });
    request.set_timeout(timeout);
    client.get_transaction(request).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn an_indexer() -> Uri {
        "https://indexer.example:443".parse().expect("static uri")
    }

    /// HYPOTHESIS: a plaintext (http) indexer is refused before any dial, so
    /// mixnet traffic is never sent unencrypted to the exit gateway. Falsified
    /// if an http URI reaches the connector.
    #[tokio::test]
    async fn a_non_https_indexer_is_refused() {
        let http = "http://indexer.example:9067".parse().expect("static uri");
        let err =
            send_transaction_via_socks5("127.0.0.1:1", &http, b"tx", 1, Duration::from_secs(5))
                .await
                .expect_err("http must be refused");
        assert!(
            matches!(err, Socks5TransmitError::InsecureScheme { .. }),
            "expected InsecureScheme, got: {err}"
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
        let addr = listener.local_addr().expect("local addr").to_string();
        drop(listener);

        let err =
            send_transaction_via_socks5(&addr, &an_indexer(), b"tx", 1, Duration::from_secs(5))
                .await
                .expect_err("no proxy is listening");
        assert!(
            matches!(err, Socks5TransmitError::ProxyUnreachable { .. }),
            "expected ProxyUnreachable, got: {err}"
        );
    }

    /// HYPOTHESIS: a proxy that accepts the dial but breaks the SOCKS5
    /// handshake is reported as the tunnel phase — the "exit could not reach
    /// the destination" signature. Falsified if it reads as a proxy or
    /// transport failure.
    #[tokio::test]
    async fn a_broken_tunnel_reports_the_tunnel_phase() {
        // A fake proxy that accepts the connection and immediately closes it,
        // so the SOCKS5 greeting fails.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("local addr").to_string();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                drop(socket);
            }
        });

        let err =
            send_transaction_via_socks5(&addr, &an_indexer(), b"tx", 1, Duration::from_secs(5))
                .await
                .expect_err("the handshake dies");
        assert!(
            matches!(err, Socks5TransmitError::TunnelRefused { .. }),
            "expected TunnelRefused, got: {err}"
        );
    }
}
