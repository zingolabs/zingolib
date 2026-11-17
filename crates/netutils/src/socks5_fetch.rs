#![forbid(unsafe_code)]

use std::time::Duration;

use crate::diag::{NetOpFailure, NetOpStage, chain_texts};

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
    if signals.is_connect && socks5_proxy.is_some_and(chain_mentions) {
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

fn classify_error(
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

/// The parameters of an HTTP GET request.
struct GetRequest<'a> {
    socks5_proxy: Option<&'a str>,
    url: &'a str,
    request_timeout: Duration,
    connect_timeout: Duration,
}

impl GetRequest<'_> {
    fn error(&self, source: reqwest::Error) -> Socks5FetchError {
        Socks5FetchError {
            failure: classify_error(&source, self.socks5_proxy, self.url, self.request_timeout),
            source,
        }
    }

    async fn send(&self) -> Result<reqwest::Response, Socks5FetchError> {
        ensure_default_crypto_provider();
        let mut builder = reqwest::Client::builder()
            .timeout(self.request_timeout)
            .connect_timeout(self.connect_timeout);
        if let Some(addr) = self.socks5_proxy {
            let proxy = reqwest::Proxy::all(format!("{SOCKS5_REMOTE_DNS_SCHEME}://{addr}"))
                .map_err(|source| self.error(source))?;
            builder = builder.proxy(proxy);
        }
        builder
            .build()
            .map_err(|source| self.error(source))?
            .get(self.url)
            .send()
            .await
            .map_err(|source| self.error(source))
    }

    async fn text(&self) -> Result<String, Socks5FetchError> {
        self.send()
            .await?
            .text()
            .await
            .map_err(|source| self.error(source))
    }

    async fn response(&self, body_limit: usize) -> Result<HttpResponse, HttpResponseError> {
        let mut response = self.send().await?;
        let status = response.status().as_u16();
        let declared_over_limit = response.content_length().is_some_and(|length| {
            !usize::try_from(length).is_ok_and(|length| length <= body_limit)
        });
        if declared_over_limit {
            return Err(HttpResponseError::BodyTooLarge { limit: body_limit });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|source| self.error(source))?
        {
            if body.len() + chunk.len() > body_limit {
                return Err(HttpResponseError::BodyTooLarge { limit: body_limit });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResponse {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }
}

/// An HTTP response. An error status is also a response, so the caller
/// must check [`Self::status`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The body as text, decoded as lossy UTF-8.
    pub body: String,
}

/// The reason an HTTP request returned no [`HttpResponse`].
#[derive(Debug, thiserror::Error)]
pub enum HttpResponseError {
    /// The request failed.
    #[error(transparent)]
    Transport(#[from] Socks5FetchError),
    /// The body is larger than the limit.
    #[error("the response body is larger than {limit} bytes")]
    BodyTooLarge {
        /// The limit in bytes.
        limit: usize,
    },
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
        GetRequest {
            socks5_proxy: Some(&proxy),
            url,
            request_timeout,
            connect_timeout,
        }
        .text()
        .await
    }

    /// Sends a GET request to `url` and returns the response. The body can
    /// be at most `body_limit` bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the body is larger than
    /// `body_limit`.
    pub async fn fetch_response(
        &self,
        url: &str,
        request_timeout: Duration,
        connect_timeout: Duration,
        body_limit: usize,
    ) -> Result<HttpResponse, HttpResponseError> {
        let proxy = self.socks5().to_string();
        GetRequest {
            socks5_proxy: Some(&proxy),
            url,
            request_timeout,
            connect_timeout,
        }
        .response(body_limit)
        .await
    }
}

/// The body at `url`, fetched untunneled, disclosing the client IP: the
/// nakednet leg of the price fetch a switched-off Mixnet Mode consents
/// to.
pub async fn fetch_text_untunneled(
    url: &str,
    request_timeout: Duration,
    connect_timeout: Duration,
) -> Result<String, Socks5FetchError> {
    GetRequest {
        socks5_proxy: None,
        url,
        request_timeout,
        connect_timeout,
    }
    .text()
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    const REQUEST_BOUND: Duration = Duration::from_secs(5);

    fn untunneled(url: &str) -> GetRequest<'_> {
        GetRequest {
            socks5_proxy: None,
            url,
            request_timeout: REQUEST_BOUND,
            connect_timeout: REQUEST_BOUND,
        }
    }

    async fn serve_raw(response: String) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let address = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("one connection");
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request).await;
            let _ = socket.write_all(response.as_bytes()).await;
        });
        format!("http://{address}/")
    }

    async fn serve_once(status_line: &'static str, body: &'static str) -> String {
        serve_raw(format!(
            "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ))
        .await
    }

    #[tokio::test]
    async fn an_error_status_is_a_response_with_its_status_code() {
        let url = serve_once("404 Not Found", "no such rate").await;

        let response = untunneled(&url)
            .response(1024)
            .await
            .expect("a 404 is a response");

        assert_eq!(
            response,
            HttpResponse {
                status: 404,
                body: "no such rate".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn a_body_of_undeclared_length_is_refused_past_the_limit() {
        let url =
            serve_raw("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n123456789".to_string()).await;

        let error = untunneled(&url)
            .response(8)
            .await
            .expect_err("a body over the limit is refused");
        assert!(
            matches!(error, HttpResponseError::BodyTooLarge { limit: 8 }),
            "the refusal must report the limit: {error}"
        );
    }

    #[tokio::test]
    async fn a_body_at_the_limit_is_returned_and_one_byte_more_is_refused() {
        let at_limit = serve_once("200 OK", "12345678").await;
        let response = untunneled(&at_limit)
            .response(8)
            .await
            .expect("a body at the limit is returned");
        assert_eq!(response.body, "12345678");

        let over_limit = serve_once("200 OK", "123456789").await;
        let error = untunneled(&over_limit)
            .response(8)
            .await
            .expect_err("a body over the limit is refused");
        assert!(
            matches!(error, HttpResponseError::BodyTooLarge { limit: 8 }),
            "the refusal must report the limit: {error}"
        );
    }

    /// The client-side bound every fabricated case shares, so a timeout
    /// verdict names a value the case can assert.
    const BOUND: Duration = Duration::from_millis(300);

    /// The local SOCKS5 address the proxied cases name in their chains.
    const PROXY: &str = "127.0.0.1:43210";

    /// The cause chain as the classifier reads it, one text per layer.
    fn chain(layers: &[&str]) -> Vec<String> {
        layers.iter().map(|layer| (*layer).to_string()).collect()
    }

    /// The signals for an error the transport reported as connect-related.
    fn connect() -> RequestSignals {
        RequestSignals {
            is_connect: true,
            ..Default::default()
        }
    }

    /// One fabricated input and the stage the table must reach from it.
    struct Case {
        why: &'static str,
        signals: RequestSignals,
        chain: Vec<String>,
        proxy: Option<&'static str>,
        expected: NetOpStage,
    }

    /// Every stage this classifier can reach is reached by one fabricated
    /// input, so a new arm must earn a case here before it can hide.
    #[test]
    fn every_reachable_stage_has_a_fabricated_input() {
        let cases = vec![
            Case {
                why: "an elapsed bound outranks every other signal",
                signals: RequestSignals {
                    is_timeout: true,
                    is_connect: true,
                    ..Default::default()
                },
                chain: chain(&["operation timed out"]),
                proxy: Some(PROXY),
                expected: NetOpStage::TimedOut { after_ms: 300 },
            },
            Case {
                why: "a connect failure naming the local proxy is the proxy's",
                signals: connect(),
                chain: chain(&[&format!("tcp connect error to {PROXY}")]),
                proxy: Some(PROXY),
                expected: NetOpStage::LocalProxyConnect,
            },
            Case {
                why: "a chain naming socks negotiation is the tunnel's handshake",
                signals: RequestSignals::default(),
                chain: chain(&["socks connect error: host unreachable"]),
                proxy: None,
                expected: NetOpStage::SocksHandshake,
            },
            Case {
                why: "a certificate rejection reached the target",
                signals: RequestSignals::default(),
                chain: chain(&["invalid peer certificate: Expired"]),
                proxy: None,
                expected: NetOpStage::RemoteTls,
            },
            Case {
                why: "a mid-handshake eof broke underneath the handshake",
                signals: RequestSignals::default(),
                chain: chain(&["tls handshake eof"]),
                proxy: None,
                expected: NetOpStage::TunnelTransport,
            },
            Case {
                why: "a status the server chose is the server's",
                signals: RequestSignals {
                    is_status: true,
                    ..Default::default()
                },
                chain: chain(&["HTTP status server error (502) for url"]),
                proxy: None,
                expected: NetOpStage::RemoteHttp,
            },
            Case {
                why: "a body that arrived and would not decode is the payload's",
                signals: RequestSignals {
                    is_decode_or_body: true,
                    ..Default::default()
                },
                chain: chain(&["error decoding response body"]),
                proxy: None,
                expected: NetOpStage::PayloadDecode,
            },
            Case {
                why: "a connect failure naming no proxy is the remote's",
                signals: connect(),
                chain: chain(&["tcp connect error: connection refused"]),
                proxy: None,
                expected: NetOpStage::RemoteConnect,
            },
            Case {
                why: "an unrevealing failure is the tunnel's",
                signals: RequestSignals::default(),
                chain: chain(&["connection reset by peer"]),
                proxy: None,
                expected: NetOpStage::TunnelTransport,
            },
        ];

        let mut reached = Vec::new();
        for case in cases {
            let verdict = classify_stage(case.signals, &case.chain, case.proxy, BOUND);
            assert_eq!(verdict, case.expected, "{}", case.why);
            reached.push(verdict);
        }

        for stage in [
            NetOpStage::TimedOut { after_ms: 300 },
            NetOpStage::LocalProxyConnect,
            NetOpStage::SocksHandshake,
            NetOpStage::RemoteTls,
            NetOpStage::RemoteHttp,
            NetOpStage::PayloadDecode,
            NetOpStage::RemoteConnect,
            NetOpStage::TunnelTransport,
        ] {
            assert!(
                reached.contains(&stage),
                "no fabricated input reaches {stage}"
            );
        }
    }

    /// The TLS arm fires on chain text alone, where the design's table at
    /// `docs/agents/net-diag-design.md` conjoins it with `is_connect()`.
    #[test]
    fn the_tls_arm_needs_no_connect_signal() {
        assert_eq!(
            classify_stage(
                RequestSignals::default(),
                &chain(&["invalid peer certificate: UnknownIssuer"]),
                None,
                BOUND,
            ),
            NetOpStage::RemoteTls,
            "chain text alone reaches the TLS arm"
        );
    }

    /// The TLS arm outranks `is_status()`, so a status error whose chain
    /// happens to name a certificate never reaches `RemoteHttp`.
    #[test]
    fn a_status_error_naming_a_certificate_classifies_as_tls() {
        assert_eq!(
            classify_stage(
                RequestSignals {
                    is_status: true,
                    ..Default::default()
                },
                &chain(&["HTTP status client error (400)", "certificate required"]),
                None,
                BOUND,
            ),
            NetOpStage::RemoteTls,
            "the earlier arm wins the error the later arm was written for"
        );
    }
}
