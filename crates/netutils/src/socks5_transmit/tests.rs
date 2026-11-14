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
        crate::diag::chain_texts(&error).join(": ")
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
/// Destination, never a verdict), and renders the bound it carries.
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
            "{code:?} must be worth another Destination"
        );
    }
}

/// HYPOTHESIS: an RPC status whose code is a server verdict is final, since
/// another Destination would hear the same request and say the same.
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
