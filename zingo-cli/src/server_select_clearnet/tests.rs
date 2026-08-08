#![allow(clippy::disallowed_methods)]

use std::time::Duration;

use super::{ProbeStage, probe_servers};

/// A budget generous enough that a fast local failure is never misread as a timeout.
const CLASSIFICATION_BUDGET: Duration = Duration::from_secs(10);

fn uri(text: &str) -> http::Uri {
    text.parse().expect("static uri")
}

/// A census entry whose domain no longer resolves fails at the connect
/// stage with a DNS cause in its rendered chain, never as a silent skip.
#[tokio::test]
async fn an_unresolvable_domain_is_root_caused_as_dns_at_connect() {
    let (ranked, failures) = probe_servers(
        vec![uri("https://indexer.invalid:443")],
        CLASSIFICATION_BUDGET,
    )
    .await;
    assert!(ranked.is_empty());
    assert_eq!(failures.len(), 1);
    assert!(
        matches!(failures[0].stage, ProbeStage::Connect(_)),
        "{}",
        failures[0].stage
    );
    let rendered = failures[0].stage.to_string();
    assert!(rendered.to_lowercase().contains("dns"), "{rendered}");
}

/// An endpoint whose address resolves but refuses the connection is a
/// connect-stage failure naming the refusal.
#[tokio::test]
async fn a_refused_port_is_root_caused_at_connect() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
        listener.local_addr().expect("bound address").port()
    };
    let (ranked, failures) = probe_servers(
        vec![uri(&format!("http://127.0.0.1:{port}"))],
        CLASSIFICATION_BUDGET,
    )
    .await;
    assert!(ranked.is_empty());
    assert_eq!(failures.len(), 1);
    assert!(
        matches!(failures[0].stage, ProbeStage::Connect(_)),
        "{}",
        failures[0].stage
    );
    let rendered = failures[0].stage.to_string().to_lowercase();
    assert!(rendered.contains("refused"), "{rendered}");
}

/// An endpoint that accepts the connection but never answers is
/// root-caused at the RPC stage by the call's own expired timeout.
#[tokio::test]
async fn a_silent_listener_is_root_caused_as_an_rpc_timeout() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let port = listener.local_addr().expect("bound address").port();
    let hold_sockets_open = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    let (ranked, failures) = probe_servers(
        vec![uri(&format!("http://127.0.0.1:{port}"))],
        Duration::from_millis(250),
    )
    .await;
    hold_sockets_open.abort();
    assert!(ranked.is_empty());
    assert_eq!(failures.len(), 1);
    assert!(
        matches!(failures[0].stage, ProbeStage::Rpc(_)),
        "{}",
        failures[0].stage
    );
    let rendered = failures[0].stage.to_string().to_lowercase();
    assert!(rendered.contains("timeout"), "{rendered}");
}

/// The outer guard's narration names the exhausted budget; its trigger (a
/// connect-stage hang such as a SYN blackhole) has no hermetic test.
#[test]
fn the_probe_timeout_narration_names_the_budget() {
    assert_eq!(
        ProbeStage::TimedOut(Duration::from_secs(5)).to_string(),
        "no answer within 5s"
    );
}

/// A live endpoint ranks with a measured latency, so an all-failed sweep
/// indicts the census entries rather than the probe mechanism.
#[tokio::test]
async fn a_live_endpoint_ranks_instead_of_failing() {
    use zingo_grpc_proxy::{
        CompactTxStreamerServer, ConfigurableMockStreamer, MethodHandler, MockConfig,
    };

    let config = MockConfig::unimplemented().with_get_lightd_info(MethodHandler::from_response(
        zingo_grpc_proxy::service::LightdInfo::default(),
    ));
    let svc = CompactTxStreamerServer::new(ConfigurableMockStreamer::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let port = listener.local_addr().expect("bound address").port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(async move {
        zingo_grpc_proxy::tonic_reexport::transport::Server::builder()
            .add_service(svc)
            .serve_with_incoming(incoming)
            .await
            .ok();
    });

    let (ranked, failures) = probe_servers(
        vec![uri(&format!("http://127.0.0.1:{port}"))],
        CLASSIFICATION_BUDGET,
    )
    .await;
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(ranked.len(), 1);
}
