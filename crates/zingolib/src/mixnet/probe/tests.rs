use super::*;

use zl_netutils::time::test::FAST_STAGE_BOUND;

fn uri(text: &str) -> Uri {
    text.parse().expect("static uri")
}

/// A closed port fails at the first stage as a typed remote-connect
/// failure, and no later stage runs.
#[tokio::test]
async fn a_closed_port_fails_the_tcp_stage_and_stops() {
    // Bind then drop, so the port is closed when the probe dials it.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let probe = probe_sync_server(&uri(&format!("https://{closed}")), FAST_STAGE_BOUND).await;
    assert_eq!(probe.stages.len(), 1, "the run stops at the first failure");
    assert_eq!(probe.stages[0].step, SyncProbeStep::TcpConnect);
    let failure = probe.stages[0].failure.as_ref().expect("stage failed");
    assert_eq!(failure.stage, NetOpStage::RemoteConnect);
    assert!(probe.info.is_none());
}

/// A listener that accepts TCP but never speaks TLS passes the first
/// stage and fails the second within the bound — the staged separation
/// that a single connect phase cannot report.
#[tokio::test]
async fn a_silent_listener_passes_tcp_and_fails_the_channel_stage() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hold = tokio::spawn(async move {
        loop {
            let Ok((_sock, _)) = listener.accept().await else {
                return;
            };
            // Accept and hold; never answer.
        }
    });

    let probe = probe_sync_server(&uri(&format!("https://{addr}")), FAST_STAGE_BOUND).await;
    assert_eq!(probe.stages.len(), 2);
    assert!(probe.stages[0].failure.is_none(), "TCP is reachable");
    let failure = probe.stages[1].failure.as_ref().expect("channel must fail");
    assert!(
        matches!(
            failure.stage,
            NetOpStage::RemoteTls | NetOpStage::TimedOut { .. }
        ),
        "a non-TLS listener fails the channel stage as TLS or a bound: {failure}"
    );
    assert!(probe.info.is_none());
    hold.abort();
}

/// A URI without a port dials the scheme default.
#[test]
fn the_probe_port_falls_back_to_the_scheme_default() {
    assert_eq!(probe_port(&uri("https://zec.rocks")), 443);
    assert_eq!(probe_port(&uri("http://localhost")), 80);
    assert_eq!(probe_port(&uri("https://zec.rocks:9067")), 9067);
}

/// - Dials the mainnet Default Indexer over nakednet.
#[tokio::test]
#[ignore = "contacts the live Default Indexer over nakednet; the nightly runs it"]
async fn live_staged_probe_smoke() {
    let probe = probe_sync_server(
        &uri(zl_netutils::default_indexer::MAINNET_DEFAULT_INDEXER),
        zl_netutils::time::test::LIVE_STAGE_BOUND,
    )
    .await;
    assert_eq!(probe.stages.len(), 3, "all stages ran: {probe:?}");
    assert!(probe.info.is_some(), "a live indexer answers: {probe:?}");
}
