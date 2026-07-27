#![warn(missing_docs)]

//! Crate for ZEC price types, storage, and fetching.
//!
//! Currently only supports USD. The fetch defaults to clearnet; a caller that
//! wants to hide the client IP from the price source passes a local SOCKS5
//! proxy address (the Nym mixnet transport, ADR 0011) and the request is
//! routed through it instead.

use std::{
    collections::HashSet,
    io::{Read, Write},
    time::Duration,
};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use serde::Deserialize;
use zcash_encoding::{Optional, Vector};
use zingo_net_diag::{NetOpFailure, NetOpStage, chain_texts};

/// Errors with price requests and parsing.
// TODO: remove unused when historical data is implemented
#[derive(Debug, thiserror::Error)]
pub enum PriceError {
    /// The HTTP request failed. The typed [`NetOpFailure`] names the stage
    /// and target; the original [`reqwest::Error`] is preserved whole as the
    /// source, so nothing is flattened away. This `Display` prints its own
    /// layer only — the cause chain belongs to `source()`.
    #[error("price request failed at {} to {}", failure.stage, failure.target)]
    RequestFailed {
        /// Which stage failed, against what target, with the cause chain
        /// captured layer by layer.
        failure: NetOpFailure,
        /// The underlying reqwest failure, untouched.
        #[source]
        source: reqwest::Error,
    },
    /// The price source answered with fewer trades than requested, so the
    /// median position does not exist. A typed [`NetOpStage::PayloadDecode`]
    /// condition: the payload decoded but was structurally short.
    #[error("the price source returned {received} trades where {TRADES_REQUESTED} were requested")]
    InsufficientTrades {
        /// How many trades the response carried.
        received: usize,
    },
    /// Deserialization failed.
    #[error("deserialization failed. {0}")]
    DeserializationFailed(#[from] serde_json::Error),
    /// Parse error.
    #[error("parse error. {0}")]
    ParseError(#[from] std::num::ParseFloatError),
    /// Price list start time not set. Call `PriceList::set_start_time`.
    #[error("price list start time has not been set.")]
    PriceListNotInitialized,
    /// Decimal conversion error.
    #[error("decimal conversion error. {0}")]
    DecimalError(#[from] rust_decimal::Error),
    /// Invalid price.
    #[error("invalid price.")]
    InvalidPrice,
}

#[derive(Debug, Deserialize)]
struct CurrentPriceResponse {
    price: String,
    timestamp: u32,
}

/// Price of ZEC in USD at a given point in time.
#[derive(Debug, Clone, Copy)]
pub struct Price {
    /// Time in seconds.
    pub time: u32,
    /// ZEC price in USD.
    pub price_usd: f32,
}

/// Price list for wallets to maintain an updated list of daily ZEC prices.
#[derive(Debug)]
pub struct PriceList {
    /// Current price.
    current_price: Option<Price>,
    /// Historical price data by day.
    // TODO: currently unused
    daily_prices: Vec<Price>,
    /// Time of last historical price update in seconds.
    // TODO: currently unused
    time_historical_prices_last_updated: Option<u32>,
}

impl Default for PriceList {
    fn default() -> Self {
        Self::new()
    }
}

impl PriceList {
    /// Constructs a new price list from the time of wallet creation.
    #[must_use]
    pub fn new() -> Self {
        PriceList {
            current_price: None,
            daily_prices: Vec::new(),
            time_historical_prices_last_updated: None,
        }
    }

    /// Returns current price.
    #[must_use]
    pub fn current_price(&self) -> Option<Price> {
        self.current_price
    }

    /// Returns historical price data by day.
    #[must_use]
    pub fn daily_prices(&self) -> &[Price] {
        &self.daily_prices
    }

    /// Returns time historical prices were last updated.
    #[must_use]
    pub fn time_historical_prices_last_updated(&self) -> Option<u32> {
        self.time_historical_prices_last_updated
    }

    /// Price list requires a start time before it can be updated.
    ///
    /// Recommended start time is the time the wallet's birthday block height was mined.
    pub fn set_start_time(&mut self, time_of_birthday: u32) {
        self.time_historical_prices_last_updated = Some(time_of_birthday);
    }

    /// Records `price` as the current price, so it survives serialization
    /// and [`Self::current_price`] reflects the latest fetch.
    ///
    /// This is the storage half of a price update. A caller that must not
    /// hold a lock across the network wait runs [`fetch_current_price`]
    /// first, then records the result here under a briefly-held lock (the
    /// net-diag polling-blackout remedy).
    pub fn record_current_price(&mut self, price: Price) {
        self.current_price = Some(price);
    }

    /// Update, record, and return the current price of ZEC.
    ///
    /// Currently only USD is supported. When `socks5_proxy` is `Some`, the
    /// request is routed through that local SOCKS5 proxy (the Nym mixnet
    /// transport); `None` fetches over clearnet. The caller decides which
    /// route to use. The fetched price is recorded on `self` as
    /// [`Self::current_price`].
    pub async fn update_current_price(
        &mut self,
        socks5_proxy: Option<&str>,
    ) -> Result<Price, PriceError> {
        let price = fetch_current_price(socks5_proxy).await?;
        self.record_current_price(price);
        Ok(price)
    }

    /// Prunes historical price list to only retain prices for the days containing `transaction_times`.
    ///
    /// Will not remove prices above or equal to the `prune_below` threshold.
    // TODO: under development
    pub fn prune(&mut self, transaction_times: Vec<u32>, prune_below: u32) {
        let mut relevant_days = HashSet::new();

        for transaction_time in transaction_times {
            for daily_price in self.daily_prices() {
                if daily_price.time > transaction_time {
                    assert!(daily_price.time - transaction_time < 60 * 60 * 24);
                    relevant_days.insert(daily_price.time);
                    break;
                }
            }
        }

        self.daily_prices
            .retain(|price| relevant_days.contains(&price.time) || price.time >= prune_below);
    }

    fn serialized_version() -> u8 {
        0
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        let _version = reader.read_u8()?;

        let time_last_updated = Optional::read(
            &mut reader,
            byteorder::ReadBytesExt::read_u32::<LittleEndian>,
        )?;
        let current_price = Optional::read(&mut reader, |r| {
            Ok(Price {
                time: r.read_u32::<LittleEndian>()?,
                price_usd: r.read_f32::<LittleEndian>()?,
            })
        })?;
        let daily_prices = Vector::read(&mut reader, |r| {
            Ok(Price {
                time: r.read_u32::<LittleEndian>()?,
                price_usd: r.read_f32::<LittleEndian>()?,
            })
        })?;

        Ok(Self {
            current_price,
            daily_prices,
            time_historical_prices_last_updated: time_last_updated,
        })
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&self, mut writer: W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;

        Optional::write(
            &mut writer,
            self.time_historical_prices_last_updated(),
            byteorder::WriteBytesExt::write_u32::<LittleEndian>,
        )?;
        Optional::write(&mut writer, self.current_price(), |w, price| {
            w.write_u32::<LittleEndian>(price.time)?;
            w.write_f32::<LittleEndian>(price.price_usd)
        })?;
        Vector::write(&mut writer, self.daily_prices(), |w, price| {
            w.write_u32::<LittleEndian>(price.time)?;
            w.write_f32::<LittleEndian>(price.price_usd)
        })
    }
}

/// Installs rustls's `aws-lc-rs` provider as the process-level default if
/// none is installed yet (first-install-wins, so an embedder's choice is
/// kept). Required because reqwest is built with `rustls-tls-no-provider`.
/// Mirrors `zingo_netutils::ensure_default_crypto_provider`, which cannot be
/// used here: zingo-netutils sits above this crate in the dependency graph.
fn ensure_default_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
}

/// How many trades the fetch requests from the price source. Eleven, so the
/// median of the sorted list is a robust current price. [`GEMINI_ZECUSD_URL`]
/// embeds this value; a test pins the two together.
const TRADES_REQUESTED: usize = 11;

/// The median position in the sorted trades list.
const MEDIAN_INDEX: usize = TRADES_REQUESTED / 2;

/// The public price source: Gemini's recent-trades endpoint for the ZEC/USD
/// pair, requesting [`TRADES_REQUESTED`] trades.
const GEMINI_ZECUSD_URL: &str = "https://api.gemini.com/v1/trades/zecusd?limit_trades=11";

/// The client-side bound on the whole request. Twenty seconds keeps the
/// native bound under the mobile UI's 25-second watchdog, so the native call
/// ends (and releases whatever holds it) before or shortly after the UI
/// gives up. A hang through a half-dead tunnel becomes a typed
/// [`NetOpStage::TimedOut`] failure instead of an unbounded wait.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// The client-side bound on establishing the connection alone.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Fetch the current price of ZEC in USD from the public price source,
/// optionally through a local SOCKS5 proxy.
///
/// When `socks5_proxy` is `Some(addr)`, the request is proxied through
/// `socks5h://addr`, so the destination hostname is resolved at the proxy and
/// never leaked to the local clearnet resolver (the Nym mixnet transport).
/// `None` fetches over clearnet. This is a pure mechanism with no storage
/// side effects, so a caller can run it without holding any wallet lock and
/// store the result under a briefly-held lock afterwards (the net-diag
/// polling-blackout remedy).
pub async fn fetch_current_price(socks5_proxy: Option<&str>) -> Result<Price, PriceError> {
    get_current_price(
        socks5_proxy,
        GEMINI_ZECUSD_URL,
        REQUEST_TIMEOUT,
        CONNECT_TIMEOUT,
    )
    .await
}

/// The typed signals [`classify_stage`] reads from a [`reqwest::Error`],
/// extracted into a plain struct so the classification table is a pure
/// function testable with fabricated inputs (a `reqwest::Error` cannot be
/// constructed by hand).
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

/// The classification table (`docs/agents/net-diag-design.md`), checked in
/// order, pure over the extracted signals and the chain's layer texts.
///
/// The boundary between `RemoteTls` and `TunnelTransport` is genuinely fuzzy
/// from reqwest's chain alone: a handshake that reached the target and was
/// refused is `RemoteTls`, while a mid-handshake EOF (`tls handshake eof`,
/// the half-dead-tunnel signature) means the data path broke underneath the
/// handshake and is `TunnelTransport`. Chain-text inspection here is
/// classification input as the design's table specifies, not a decision on
/// the rendered stability contract.
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

/// The [`NetOpFailure`] for one failed request: stage from the
/// classification table, target from the failing leg (the SOCKS endpoint for
/// local stages, the price URL otherwise), cause chain captured layer by
/// layer. The `reqwest::Error` itself travels beside it, untouched, in
/// [`PriceError::RequestFailed`].
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

/// Get current price of ZEC in USD from `url`, optionally through a local
/// SOCKS5 proxy, bounded by the given timeouts.
///
/// Production callers go through [`fetch_current_price`]; tests point `url`
/// at a local server and shrink the bounds.
async fn get_current_price(
    socks5_proxy: Option<&str>,
    url: &str,
    request_timeout: Duration,
    connect_timeout: Duration,
) -> Result<Price, PriceError> {
    ensure_default_crypto_provider();
    let typed = |error: reqwest::Error| PriceError::RequestFailed {
        failure: classify_request(&error, socks5_proxy, url, request_timeout),
        source: error,
    };
    let mut builder = reqwest::Client::builder()
        .timeout(request_timeout)
        .connect_timeout(connect_timeout);
    if let Some(addr) = socks5_proxy {
        builder = builder.proxy(reqwest::Proxy::all(format!("socks5h://{addr}")).map_err(typed)?);
    }
    let httpget = builder
        .build()
        .map_err(typed)?
        .get(url)
        .send()
        .await
        .map_err(typed)?;
    let mut trades = httpget
        .json::<Vec<CurrentPriceResponse>>()
        .await
        .map_err(typed)?
        .iter()
        .map(|response| {
            let price_usd: f32 = response.price.parse()?;
            if !price_usd.is_finite() {
                return Err(PriceError::InvalidPrice);
            }

            Ok(Price {
                price_usd,
                time: response.timestamp,
            })
        })
        .collect::<Result<Vec<Price>, PriceError>>()?;

    trades.sort_by(|a, b| {
        a.price_usd
            .partial_cmp(&b.price_usd)
            .expect("trades are checked to be finite and comparable")
    });

    let received = trades.len();
    trades
        .get(MEDIAN_INDEX)
        .copied()
        .ok_or(PriceError::InsufficientTrades { received })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Generous bounds for tests whose subject is not the timeout.
    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    /// Serves exactly one Gemini-shaped trades response over plain HTTP and
    /// returns the URL to hit it. One connection, then the socket closes.
    async fn spawn_trades_server(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // Drain the request headers; the content is irrelevant to the stub.
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(response.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        });
        format!("http://{addr}/v1/trades/zecusd")
    }

    /// The clearnet fetch (`socks5_proxy = None`) performs the real HTTP round
    /// trip, deserializes the Gemini trades payload, and returns the median of
    /// the eleven trades (index 5 of the sorted list). Eleven deliberately
    /// out-of-order prices 100..=110 make the median 105 and prove the sort.
    #[tokio::test]
    async fn clearnet_fetch_returns_median_price() {
        let body = r#"[
            {"price":"110","timestamp":1},
            {"price":"100","timestamp":2},
            {"price":"105","timestamp":3},
            {"price":"101","timestamp":4},
            {"price":"109","timestamp":5},
            {"price":"102","timestamp":6},
            {"price":"108","timestamp":7},
            {"price":"103","timestamp":8},
            {"price":"107","timestamp":9},
            {"price":"104","timestamp":10},
            {"price":"106","timestamp":11}
        ]"#;
        let url = spawn_trades_server(body).await;

        let price = get_current_price(None, &url, TEST_TIMEOUT, TEST_TIMEOUT)
            .await
            .expect("the clearnet fetch parses a valid trades response");

        assert_eq!(
            price.price_usd, 105.0,
            "the returned price is the median (index 5) of the eleven sorted trades"
        );
    }

    /// Smoke test against the real Gemini endpoint over clearnet. Ignored by
    /// default (needs network and a live third party); run with
    /// `cargo test -p zingo-price -- --ignored` to confirm the live fetch.
    #[tokio::test]
    #[ignore = "hits the live Gemini API over clearnet"]
    async fn live_gemini_clearnet_fetch_smoke() {
        let price = fetch_current_price(None)
            .await
            .expect("the live Gemini fetch succeeds");
        assert!(
            price.price_usd > 0.0 && price.price_usd.is_finite(),
            "a live ZEC/USD price is positive and finite, got {}",
            price.price_usd
        );
    }

    /// The endpoint URL and [`TRADES_REQUESTED`] are two spellings of one
    /// spec value; this pins them together so neither drifts alone.
    #[test]
    fn the_endpoint_url_requests_the_named_trade_count() {
        assert!(
            GEMINI_ZECUSD_URL.ends_with(&format!("limit_trades={TRADES_REQUESTED}")),
            "GEMINI_ZECUSD_URL must request TRADES_REQUESTED trades: {GEMINI_ZECUSD_URL}"
        );
        assert_eq!(MEDIAN_INDEX, TRADES_REQUESTED / 2);
    }

    /// A recorded current price survives the write/read round trip, so the
    /// latest fetch is what a reloaded wallet reports.
    #[test]
    fn a_recorded_price_survives_the_serialization_round_trip() {
        let mut list = PriceList::new();
        assert!(list.current_price().is_none());
        list.record_current_price(Price {
            time: 1_753_000_000,
            price_usd: 42.25,
        });

        let mut bytes = Vec::new();
        list.write(&mut bytes).expect("an in-memory write succeeds");
        let reloaded = PriceList::read(bytes.as_slice()).expect("the bytes just written read back");

        let stored = reloaded
            .current_price()
            .expect("the recorded price survives");
        assert_eq!(stored.time, 1_753_000_000);
        assert_eq!(stored.price_usd, 42.25);
    }

    /// The classification table, exercised with fabricated signals and chain
    /// texts: every [`NetOpStage`] variant is reachable, and the documented
    /// boundary cases land where the table says. A `reqwest::Error` cannot
    /// be constructed by hand, which is why the table is pure over
    /// [`RequestSignals`].
    #[test]
    fn every_stage_is_reachable_from_fabricated_signals() {
        let chain = |texts: &[&str]| texts.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        let bound = Duration::from_secs(20);
        let signals = |timeout: bool, connect: bool, status: bool, decode: bool| RequestSignals {
            is_timeout: timeout,
            is_connect: connect,
            is_status: status,
            is_decode_or_body: decode,
        };

        let table: Vec<(RequestSignals, Vec<String>, Option<&str>, NetOpStage)> = vec![
            (
                signals(true, false, false, false),
                chain(&["operation timed out"]),
                None,
                NetOpStage::TimedOut { after_ms: 20_000 },
            ),
            (
                signals(false, true, false, false),
                chain(&["error sending request", "tcp connect error 127.0.0.1:1080"]),
                Some("127.0.0.1:1080"),
                NetOpStage::LocalProxyConnect,
            ),
            (
                signals(false, false, false, false),
                chain(&["error sending request", "socks connect refused by proxy"]),
                Some("127.0.0.1:1080"),
                NetOpStage::SocksHandshake,
            ),
            (
                signals(false, true, false, false),
                chain(&["error sending request", "invalid peer certificate"]),
                None,
                NetOpStage::RemoteTls,
            ),
            (
                signals(false, true, false, false),
                chain(&["error sending request", "tls handshake eof"]),
                None,
                NetOpStage::TunnelTransport,
            ),
            (
                signals(false, false, true, false),
                chain(&["HTTP status server error (503)"]),
                None,
                NetOpStage::RemoteHttp,
            ),
            (
                signals(false, false, false, true),
                chain(&["error decoding response body"]),
                None,
                NetOpStage::PayloadDecode,
            ),
            (
                signals(false, true, false, false),
                chain(&["error sending request", "connection refused"]),
                None,
                NetOpStage::RemoteConnect,
            ),
            (
                signals(false, false, false, false),
                chain(&["connection reset by peer"]),
                None,
                NetOpStage::TunnelTransport,
            ),
        ];

        let mut reached = std::collections::HashSet::new();
        for (signals, chain, socks, expected) in table {
            let stage = classify_stage(signals, &chain, socks, bound);
            assert_eq!(stage, expected, "signals {signals:?}, chain {chain:?}");
            reached.insert(stage.to_string());
        }
        // RouteResolution is refused upstream of any request, so the reqwest
        // table cannot produce it; every other variant must be reachable.
        assert_eq!(
            reached.len(),
            8,
            "every reqwest-reachable NetOpStage variant appears: {reached:?}"
        );
    }

    /// A black-holed server (accepts the TCP connection, never answers)
    /// resolves within the client bound as a typed `TimedOut` failure with
    /// the reqwest source preserved — the field failure that motivated the
    /// taxonomy (a five-minute unbounded hang) can no longer happen.
    #[tokio::test]
    async fn a_black_holed_server_times_out_typed_within_the_bound() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/trades/zecusd", listener.local_addr().unwrap());
        // Keep the listener alive but never accept-and-answer.
        let hold = tokio::spawn(async move {
            let _sock = listener.accept().await;
            std::future::pending::<()>().await;
        });

        let short = Duration::from_millis(300);
        let error = get_current_price(None, &url, short, short)
            .await
            .expect_err("a silent server cannot serve a price");
        match &error {
            PriceError::RequestFailed { failure, source } => {
                assert_eq!(failure.stage, NetOpStage::TimedOut { after_ms: 300 });
                assert!(source.is_timeout(), "the reqwest source keeps its kind");
                assert!(
                    !failure.cause_chain.is_empty(),
                    "the cause chain is captured layer by layer"
                );
            }
            other => panic!("expected a typed RequestFailed, got {other}"),
        }
        hold.abort();
    }

    /// A structurally short trades response is a typed refusal, never an
    /// index panic (the design's median-guard requirement).
    #[tokio::test]
    async fn fewer_trades_than_requested_is_a_typed_refusal() {
        let body = r#"[
            {"price":"110","timestamp":1},
            {"price":"100","timestamp":2}
        ]"#;
        let url = spawn_trades_server(body).await;

        let error = get_current_price(None, &url, TEST_TIMEOUT, TEST_TIMEOUT)
            .await
            .expect_err("two trades cannot yield the median of eleven");
        assert!(
            matches!(error, PriceError::InsufficientTrades { received: 2 }),
            "the refusal must be typed with the received count: {error}"
        );
    }
}
