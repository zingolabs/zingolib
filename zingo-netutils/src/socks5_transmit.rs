//! Wallet-side SOCKS5-dialing transmission (ADR 0011, consumption model A).
//!
//! Routes a raw transaction to an indexer through a local SOCKS5 proxy — the
//! `nym-proxy` child process the wallet spawns — and returns the
//! server-reported txid. This path is deliberately light: it needs only a
//! SOCKS5 client and the tonic machinery already present, no nym-sdk, so it
//! resolves and builds in the main workspace's lockfile. See
//! `docs/adr/0011-nym-mixnet-transmission.md`.
#![forbid(unsafe_code)]

use std::time::Duration;

use http::Uri;
use hyper_util::rt::TokioIo;
use tonic::transport::{ClientTlsConfig, Endpoint};

use crate::crypto::ensure_default_crypto_provider;
use lightwallet_protocol::{CompactTxStreamerClient, RawTransaction};

/// Why a SOCKS5-tunneled transmission did not yield a txid.
#[derive(Debug, thiserror::Error)]
pub enum Socks5TransmitError {
    /// The indexer could not be reached through the proxy — a proxy-dial,
    /// tunnel, or connect failure. A candidate for failover to another
    /// Broadcast Indexer.
    #[error("indexer unreachable through the SOCKS5 proxy: {0}")]
    Unreachable(String),
    /// The indexer was reached but rejected the transaction on its merits.
    #[error("indexer rejected the transaction: {0}")]
    Rejected(String),
}

/// Submits `raw_tx` to `indexer` through the local SOCKS5 proxy at
/// `socks5_addr` (for example `"127.0.0.1:43210"`), returning the
/// server-reported txid on acceptance. `height` fills the `RawTransaction`
/// height field.
///
/// A connection or tunnel failure yields [`Socks5TransmitError::Unreachable`]
/// so the caller can fail over to a different indexer; a server-side
/// rejection yields [`Socks5TransmitError::Rejected`].
pub async fn send_transaction_via_socks5(
    socks5_addr: &str,
    indexer: &Uri,
    raw_tx: &[u8],
    height: u64,
    timeout: Duration,
) -> Result<String, Socks5TransmitError> {
    ensure_default_crypto_provider();

    let is_https = indexer.scheme_str() == Some("https");
    let host = indexer
        .host()
        .ok_or_else(|| Socks5TransmitError::Unreachable("indexer uri has no host".to_string()))?
        .to_string();
    let port = indexer.port_u16().unwrap_or(if is_https { 443 } else { 9067 });
    let socks5_addr = socks5_addr.to_string();

    let mut endpoint = Endpoint::from_shared(indexer.to_string())
        .map_err(|e| Socks5TransmitError::Unreachable(e.to_string()))?
        .tcp_nodelay(true)
        .timeout(timeout);
    if is_https {
        endpoint = endpoint
            .tls_config(ClientTlsConfig::new().with_webpki_roots())
            .map_err(|e| Socks5TransmitError::Unreachable(e.to_string()))?;
    }

    // tonic dials through the SOCKS5 proxy: for each connection the connector
    // opens a SOCKS5 tunnel to the indexer's host:port and hands tonic the
    // tunneled stream. TLS, if the indexer is https, is layered on top by the
    // endpoint's tls_config.
    let connector = tower::service_fn(move |_uri: Uri| {
        let socks5_addr = socks5_addr.clone();
        let host = host.clone();
        async move {
            let stream = tokio_socks::tcp::Socks5Stream::connect(
                socks5_addr.as_str(),
                (host.as_str(), port),
            )
            .await
            .map_err(std::io::Error::other)?;
            Ok::<_, std::io::Error>(TokioIo::new(stream))
        }
    });

    let channel = endpoint
        .connect_with_connector(connector)
        .await
        .map_err(|e| Socks5TransmitError::Unreachable(e.to_string()))?;

    let mut client = CompactTxStreamerClient::new(channel);
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
    // carries the txid (sometimes quote-wrapped). Mirror GrpcIndexer's own
    // send_transaction handling.
    if response.error_code == 0 {
        let mut txid = response.error_message;
        if txid.starts_with('"') && txid.ends_with('"') && txid.len() >= 2 {
            txid = txid[1..txid.len() - 1].to_string();
        }
        Ok(txid)
    } else {
        Err(Socks5TransmitError::Rejected(response.error_message))
    }
}
