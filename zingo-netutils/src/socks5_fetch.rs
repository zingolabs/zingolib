#![forbid(unsafe_code)]

use std::time::Duration;

use zingo_net_diag::{NetOpFailure, NetOpStage, chain_texts};

use crate::conduit::ConduitDial;
use crate::crypto::ensure_default_crypto_provider;

/// One conduit-guarded HTTP request that did not return a body.
#[derive(Debug, thiserror::Error)]
#[error("fetch failed at {} to {}", failure.stage, failure.target)]
pub struct Socks5FetchError {
    failure: NetOpFailure,
    #[source]
    source: reqwest::Error,
}

impl Socks5FetchError {
    /// The typed record of which stage failed, against what target.
    pub fn net_op_failure(&self) -> NetOpFailure {
        self.failure.clone()
    }
}

/// The scheme a SOCKS5 proxy URL carries so the destination host resolves at
/// the exit rather than locally.
const SOCKS5_REMOTE_DNS_SCHEME: &str = "socks5h";

#[derive(Clone, Copy, Debug, Default)]
struct RequestSignals {
    is_timeout: bool,
    is_connect: bool,
    is_status: bool,
    is_decode_or_body: bool,
}

impl RequestSignals {
    fn of(error: &reqwest::Error) -> Self {
        RequestSignals {
            is_timeout: error.is_timeout(),
            is_connect: error.is_connect(),
            is_status: error.is_status(),
            is_decode_or_body: error.is_decode() || error.is_body(),
        }
    }
}

fn classify_stage(
    signals: RequestSignals,
    chain: &[String],
    socks5_proxy: Option<&str>,
    bound: Duration,
) -> NetOpStage {
    let chain_mentions = |needle: &str| {
        chain
            .iter()
            .any(|layer| layer.to_ascii_lowercase().contains(needle))
    };
    if signals.is_timeout {
        return NetOpStage::TimedOut {
            after_ms: bound.as_millis().try_into().unwrap_or(u64::MAX),
        };
    }
    if signals.is_connect && socks5_proxy.is_some_and(&chain_mentions) {
        return NetOpStage::LocalProxyConnect;
    }
    if chain_mentions("socks") {
        return NetOpStage::SocksHandshake;
    }
    if chain_mentions("tls") || chain_mentions("certificate") || chain_mentions("handshake") {
        if chain_mentions("handshake eof") {
            return NetOpStage::TunnelTransport;
        }
        return NetOpStage::RemoteTls;
    }
    if signals.is_status {
        return NetOpStage::RemoteHttp;
    }
    if signals.is_decode_or_body {
        return NetOpStage::PayloadDecode;
    }
    if signals.is_connect {
        return NetOpStage::RemoteConnect;
    }
    NetOpStage::TunnelTransport
}

fn classify_request(
    error: &reqwest::Error,
    socks5_proxy: Option<&str>,
    url: &str,
    bound: Duration,
) -> NetOpFailure {
    let chain = chain_texts(error);
    let stage = classify_stage(RequestSignals::of(error), &chain, socks5_proxy, bound);
    let target = match (&stage, socks5_proxy) {
        (NetOpStage::LocalProxyConnect | NetOpStage::SocksHandshake, Some(addr)) => {
            addr.to_string()
        }
        _ => url.to_string(),
    };
    NetOpFailure {
        stage,
        target,
        cause_chain: chain,
    }
}

async fn fetch_over(
    socks5_proxy: Option<&str>,
    url: &str,
    request_timeout: Duration,
    connect_timeout: Duration,
) -> Result<String, Socks5FetchError> {
    ensure_default_crypto_provider();
    let typed = |error: reqwest::Error| Socks5FetchError {
        failure: classify_request(&error, socks5_proxy, url, request_timeout),
        source: error,
    };
    let mut builder = reqwest::Client::builder()
        .timeout(request_timeout)
        .connect_timeout(connect_timeout);
    if let Some(addr) = socks5_proxy {
        builder = builder.proxy(
            reqwest::Proxy::all(format!("{SOCKS5_REMOTE_DNS_SCHEME}://{addr}")).map_err(typed)?,
        );
    }
    builder
        .build()
        .map_err(typed)?
        .get(url)
        .send()
        .await
        .map_err(typed)?
        .text()
        .await
        .map_err(typed)
}

impl ConduitDial {
    /// The body at `url`, fetched through this use of the conduit.
    pub async fn fetch_text(
        &self,
        url: &str,
        request_timeout: Duration,
        connect_timeout: Duration,
    ) -> Result<String, Socks5FetchError> {
        let proxy = self.socks5().to_string();
        fetch_over(Some(&proxy), url, request_timeout, connect_timeout).await
    }
}

/// The body at `url`, fetched without a conduit, for tests alone.
#[cfg(feature = "testutils")]
pub async fn fetch_text_untunneled(
    url: &str,
    request_timeout: Duration,
    connect_timeout: Duration,
) -> Result<String, Socks5FetchError> {
    fetch_over(None, url, request_timeout, connect_timeout).await
}
