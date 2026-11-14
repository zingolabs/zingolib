use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use zl_netutils::diag::NetOpStage;

/// Generous bounds for tests whose subject is not the timeout.
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// One source's price through a conduit over `socks5`, for the tests
/// whose subject is the proxy leg itself.
async fn fetch_through_proxy(
    source: PriceSource,
    socks5: std::net::SocketAddr,
    url: &str,
    request_timeout: Duration,
    connect_timeout: Duration,
) -> Result<Price, PriceError> {
    let conduit = zl_netutils::conduit::MixnetConduit::over(socks5);
    get_source_price(
        source,
        &conduit.dial(),
        url,
        request_timeout,
        connect_timeout,
    )
    .await
}

/// HYPOTHESIS: the race report renders a source's two-link cause chain
/// exactly as the one sanctioned chain walk joined by this module's
/// separator does, so the report keeps no private copy of the walk.
/// Falsified if the two renderings differ by a single byte.
#[test]
fn the_race_report_matches_the_sanctioned_walk() {
    let parse_failure = "not a price"
        .parse::<f32>()
        .expect_err("the text is not a float");
    let error = PriceError::ParseError(parse_failure);
    let expected = format!(
        "{}: {}",
        PriceSource::Gemini.name(),
        chain_texts(&error).join(": ")
    );
    let failure = PriceRaceFailure {
        failures: vec![(PriceSource::Gemini, error)],
    };

    assert_eq!(failure.report(), expected);
}

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

/// The nakednet fetch (`socks5_proxy = None`) performs the real HTTP round
/// trip, deserializes the Gemini trades payload, and returns the median of
/// the eleven trades (index 5 of the sorted list). Eleven deliberately
/// out-of-order prices 100..=110 make the median 105 and prove the sort.
#[tokio::test]
async fn nakednet_fetch_returns_median_price() {
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

    let price = get_source_price_untunneled(PriceSource::Gemini, &url, TEST_TIMEOUT, TEST_TIMEOUT)
        .await
        .expect("the nakednet fetch parses a valid trades response");

    assert_eq!(
        price.price_usd, 105.0,
        "the returned price is the median (index 5) of the eleven sorted trades"
    );
}

/// Smoke test against a real price source over nakednet. Ignored by
/// default (needs network and a live third party); run with
/// `cargo test -p zl-price -- --ignored`. The race that once lived
/// here is the wallet's speed-priority wave, so this proves the fetch
/// this crate still owns.
#[tokio::test]
#[ignore = "hits a live price-source API over nakednet"]
async fn live_nakednet_price_fetch_smoke() {
    let source = PriceSource::Kraken;
    let price = get_source_price_untunneled(source, source.url(), REQUEST_TIMEOUT, CONNECT_TIMEOUT)
        .await
        .expect("the live price fetch succeeds");
    assert!(
        price.price_usd > 0.0 && price.price_usd.is_finite(),
        "a live ZEC/USD price is positive and finite, got {}",
        price.price_usd
    );
}

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
    let error = get_source_price_untunneled(PriceSource::Gemini, &url, short, short)
        .await
        .expect_err("a silent server cannot serve a price");
    match &error {
        PriceError::RequestFailed { failure, source } => {
            assert_eq!(failure.stage, NetOpStage::TimedOut { after_ms: 300 });
            assert!(
                std::error::Error::source(source).is_some(),
                "the transport source keeps its own cause beneath it"
            );
            assert!(
                !failure.cause_chain.is_empty(),
                "the cause chain is captured layer by layer"
            );
        }
        other => panic!("expected a typed RequestFailed, got {other}"),
    }
    hold.abort();
}

/// A dead SOCKS5 proxy (the port is closed when the fetch dials it)
/// surfaces as a typed connect-phase failure with the reqwest source
/// and the cause chain preserved whole, so a consumer distinguishes a
/// connect failure from a TLS or HTTP failure by fields instead of
/// parsing prose.
#[tokio::test]
async fn a_dead_proxy_failure_is_typed_with_its_source_intact() {
    // Bind then drop, so the port is closed when the fetch dials it.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };

    let short = Duration::from_millis(300);
    let error = fetch_through_proxy(
        PriceSource::Gemini,
        closed,
        "http://127.0.0.1:9/v1/trades/zecusd",
        short,
        short,
    )
    .await
    .expect_err("a closed port cannot serve a price");
    match &error {
        PriceError::RequestFailed { failure, source } => {
            assert!(
                std::error::Error::source(source).is_some(),
                "the transport failure must keep its own cause: {source}"
            );
            assert!(
                matches!(
                    failure.stage,
                    NetOpStage::LocalProxyConnect
                        | NetOpStage::SocksHandshake
                        | NetOpStage::RemoteConnect
                ),
                "a dead local proxy must classify as a connect-phase stage: {failure}"
            );
            assert!(
                !failure.cause_chain.is_empty(),
                "the cause chain is captured layer by layer"
            );
        }
        other => panic!("expected a typed RequestFailed, got {other}"),
    }
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

    let error = get_source_price_untunneled(PriceSource::Gemini, &url, TEST_TIMEOUT, TEST_TIMEOUT)
        .await
        .expect_err("two trades cannot yield the median of eleven");
    assert!(
        matches!(error, PriceError::InsufficientTrades { received: 2 }),
        "the refusal must be typed with the received count: {error}"
    );
}

/// Eleven Kraken-shaped trades around a 43.00 median. Trade entries are
/// [price, volume, time, buy/sell, market/limit, misc, id].
const KRAKEN_ELEVEN_TRADES: &str = r#"{
        "error": [],
        "result": {
            "XZECZUSD": [
                ["42.80","1.0",1700000001.1,"b","m","",1],
                ["43.20","0.5",1700000002.2,"s","l","",2],
                ["42.90","2.0",1700000003.3,"b","m","",3],
                ["43.10","1.1",1700000004.4,"s","m","",4],
                ["43.00","0.7",1700000005.5,"b","l","",5],
                ["42.70","0.2",1700000006.6,"s","m","",6],
                ["43.30","1.4",1700000007.7,"b","m","",7],
                ["42.60","0.9",1700000008.8,"s","l","",8],
                ["43.40","0.3",1700000009.9,"b","m","",9],
                ["42.95","1.8",1700000010.1,"s","m","",10],
                ["43.05","0.6",1700000011.2,"b","l","",11]
            ],
            "last": "1700000011200000000"
        }
    }"#;

#[test]
fn kraken_trades_parse_to_the_median_price() {
    let median = parse_kraken_trades(KRAKEN_ELEVEN_TRADES).expect("eleven finite trades parse");
    assert_eq!(median.price_usd, 43.00);
    assert_eq!(median.time, 1700000005);
}

#[test]
fn a_kraken_in_band_error_is_a_typed_source_report() {
    let body = r#"{"error":["EQuery:Unknown asset pair"],"result":{}}"#;
    let error = parse_kraken_trades(body).expect_err("a reported error is never a price");
    assert!(
        matches!(&error, PriceError::SourceReportedError(report)
            if report.contains("EQuery:Unknown asset pair")),
        "the report must carry the source's own words: {error}"
    );
}

#[test]
fn a_kraken_response_without_a_pair_is_a_typed_shape_refusal() {
    let body = r#"{"error":[],"result":{"last":"1700000011200000000"}}"#;
    let error = parse_kraken_trades(body).expect_err("no pair key, no trades");
    assert!(
        matches!(error, PriceError::UnexpectedShape(_)),
        "the refusal must name the missing shape: {error}"
    );
}

#[test]
fn coingecko_simple_price_parses_with_its_update_time() {
    let body = r#"{"zcash":{"usd":41.37,"last_updated_at":1700000123}}"#;
    let spot = parse_coingecko_simple(body).expect("a spot quote parses");
    assert_eq!(spot.price_usd, 41.37);
    assert_eq!(spot.time, 1700000123);
}

#[test]
fn a_non_finite_coingecko_price_is_a_typed_refusal() {
    let body = r#"{"zcash":{"usd":1e39,"last_updated_at":1700000123}}"#;
    let error = parse_coingecko_simple(body).expect_err("an overflowed float is no price");
    assert!(
        matches!(error, PriceError::InvalidPrice),
        "the refusal must be the typed invalid-price arm: {error}"
    );
}

/// A server that answers every connection with this body, until dropped.
async fn spawn_answering_server(body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(response.as_bytes()).await;
        }
    });
    url
}

/// A server that accepts and never answers, until dropped.
async fn spawn_silent_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _sock = listener.accept().await;
        std::future::pending::<()>().await;
    });
    url
}

/// HYPOTHESIS: the first source that answered supplies the quote, and
/// the sources that failed before it are discarded rather than
/// reported. Falsified if a later answer wins or a failure survives a
/// success.
#[tokio::test]
async fn the_first_answer_supplies_the_quote() {
    let garbage = spawn_answering_server("not json at all").await;
    let kraken = spawn_answering_server(KRAKEN_ELEVEN_TRADES).await;
    let short = Duration::from_millis(500);

    let refused = get_source_price_untunneled(PriceSource::Gemini, &garbage, short, short).await;
    let answered = get_source_price_untunneled(PriceSource::Kraken, &kraken, short, short).await;
    let won = first_quote(vec![
        (PriceSource::Gemini, refused),
        (PriceSource::Kraken, answered),
    ])
    .expect("one healthy source supplies the quote");
    assert_eq!(won.source, PriceSource::Kraken);
    assert_eq!(won.price.price_usd, 43.00);
}

/// HYPOTHESIS: when no source answered, the report carries every
/// source's typed failure and names each operator, so a total outage is
/// diagnosable per operator rather than as one opaque refusal.
/// Falsified if a failure or a name leaves the report.
#[tokio::test]
async fn a_run_where_every_source_fails_reports_them_all() {
    let garbage_one = spawn_answering_server("not json at all").await;
    let garbage_two = spawn_answering_server(r#"{"unexpected":true}"#).await;
    let silent = spawn_silent_server().await;
    let short = Duration::from_millis(300);

    let mut outcomes = Vec::new();
    for (source, url) in [
        (PriceSource::Gemini, garbage_one),
        (PriceSource::Kraken, garbage_two),
        (PriceSource::CoinGecko, silent),
    ] {
        outcomes.push((
            source,
            get_source_price_untunneled(source, &url, short, short).await,
        ));
    }
    let failure = first_quote(outcomes).expect_err("no source answered with a price");
    let named: Vec<&str> = failure
        .failures
        .iter()
        .map(|(source, _)| source.name())
        .collect();
    assert_eq!(failure.failures.len(), 3);
    for name in ["gemini", "kraken", "coingecko"] {
        assert!(named.contains(&name), "missing {name} in {named:?}");
        assert!(
            failure.to_string().contains(name),
            "the rendered report must name {name}: {failure}"
        );
    }
}
