//! Unit and integration-style tests for `zl-netutils`.
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

use crate::default_indexer;
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

use crate::time::test::LOCAL_TLS_TEST_BOUND;

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
    let pem = std::fs::read("test-data/localhost.pem").expect("missing test-data/localhost.pem");
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

    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
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

/// A path that goes silent while the channel is connecting must fail the
/// call, not hold it. This listener accepts the TCP connection and never
/// answers, which is what a dropped network looks like to a client that is
/// reconnecting: the TLS handshake waits for a reply that is not coming.
///
/// Before `INDEXER_CONNECT_TIMEOUT` the call below never returned, whatever
/// deadline it carried, because a request's deadline only starts once the
/// channel has a connection to send it on.
#[tokio::test]
async fn a_silent_indexer_fails_the_call_instead_of_hanging() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
    let addr = listener.local_addr().expect("local_addr failed");

    // Held open and never written to.
    let silent = tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (socket, _) = listener.accept().await.expect("accept failed");
            held.push(socket);
        }
    });

    let uri: http::Uri = format!("https://127.0.0.1:{}", addr.port())
        .parse()
        .expect("uri");
    let mut indexer = GrpcIndexer::new_lazy(uri).expect("lazy indexer");

    let outcome = timeout(
        crate::time::INDEXER_CONNECT_TIMEOUT + Duration::from_secs(10),
        indexer.get_latest_block(Duration::from_secs(1)),
    )
    .await;
    silent.abort();

    assert!(
        matches!(outcome, Ok(Err(_))),
        "expected the call to fail within the connect bound, got {outcome:?}"
    );
}

#[tokio::test]
async fn connects_to_public_mainnet_indexer_and_gets_info() {
    let endpoint = default_indexer::MAINNET_DEFAULT_INDEXER.to_string();

    let uri: http::Uri = endpoint.parse().expect("bad mainnet indexer URI");

    let response = GrpcIndexer::new(uri)
        .await
        .expect("URI to be valid.")
        .get_lightd_info(LOCAL_TLS_TEST_BOUND)
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

    let uri: http::Uri = default_indexer::MAINNET_DEFAULT_INDEXER
        .parse()
        .expect("MAINNET_DEFAULT_INDEXER is a literal that parses");
    let mut indexer = GrpcIndexer::new(uri).await.expect("valid URI");

    let tip = indexer
        .get_latest_block(LOCAL_TLS_TEST_BOUND)
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
        .get_block_range(range, LOCAL_TLS_TEST_BOUND)
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
