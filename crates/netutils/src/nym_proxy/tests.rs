use std::time::Duration;

use super::*;
use crate::arm_race::RESERVATION_CLUTCH_SIZE;
use crate::time::HEDGE_INTERVAL;

// The shuffling and retry-engine logic is tested in `mixnet_connect`,
// where the tests call the REAL functions in the default build; the
// earlier copies of that logic here tested a transcription of the
// expression, not the code.

#[test]
fn find_available_port_returns_nonzero() {
    let port = NymProxy::find_available_port().expect("find_available_port");
    assert!(port > 0);
}

/// HYPOTHESIS: the proxy announces the socket address it listens on, so
/// no caller ever parses an address out of text; falsified if
/// [`NymProxy::socks5_addr`] returns anything other than a
/// [`SocketAddr`].
#[test]
fn the_proxy_announces_a_typed_socket_address() {
    let announce: fn(&NymProxy) -> SocketAddr = NymProxy::socks5_addr;
    let _ = announce;
}

/// The arm count the paused-time races below run over; the clutch is
/// the race's whole width, so the candidate count is also its cap.
const RACED_ARMS: usize = 2;

/// HYPOTHESIS: a locally drawn clutch holds exactly
/// [`RESERVATION_CLUTCH_SIZE`] of the discovered nodes, takes every node
/// when the population is smaller, and never repeats one.
#[test]
fn a_drawn_clutch_is_clutch_sized_and_repetition_free() {
    let discovered: Vec<String> = (0..RESERVATION_CLUTCH_SIZE * 3)
        .map(|index| format!("exit-{index}"))
        .collect();
    let clutch = draw_clutch(discovered.clone());
    assert_eq!(clutch.len(), RESERVATION_CLUTCH_SIZE);
    let distinct: std::collections::HashSet<_> = clutch.iter().collect();
    assert_eq!(distinct.len(), clutch.len(), "no node is drawn twice");
    assert!(clutch.iter().all(|drawn| discovered.contains(drawn)));

    let scarce = vec!["only".to_string()];
    assert_eq!(draw_clutch(scarce.clone()), scarce, "a short population");
    assert!(draw_clutch(Vec::new()).is_empty());
}

/// HYPOTHESIS: a reconnect redraw never rebinds a spent exit and still
/// draws a full clutch from the fresh remainder; falsified if a spent
/// exit reappears, or if a spent-out population yields anything but an
/// empty draw.
#[test]
fn a_reconnect_redraw_never_rebinds_a_spent_exit() {
    let population: Vec<String> = (0..RESERVATION_CLUTCH_SIZE * 3)
        .map(|index| format!("exit-{index}"))
        .collect();
    let spent: Vec<String> = population[..RESERVATION_CLUTCH_SIZE].to_vec();

    let redrawn = redraw_clutch(population.clone(), &spent);
    assert_eq!(redrawn.len(), RESERVATION_CLUTCH_SIZE);
    assert!(redrawn.iter().all(|drawn| !spent.contains(drawn)));
    assert!(redrawn.iter().all(|drawn| population.contains(drawn)));

    assert!(
        redraw_clutch(spent.clone(), &spent).is_empty(),
        "a spent-out population leaves nothing to draw"
    );
}

fn hedged(max_parallel: usize) -> LaunchPolicy {
    LaunchPolicy::Hedged {
        max_parallel,
        hedge_interval: HEDGE_INTERVAL,
    }
}

async fn no_abandon(_: &str) {}

/// The tests race `String`-failing pulls; a panic's record is its text.
fn panic_text(_: usize, text: String) -> String {
    text
}

/// HYPOTHESIS: an Exit Node whose connect hangs costs only the hedge
/// interval before a parallel pull can win, not the per-attempt timeout,
/// and never the lifecycle budget (the regression behind live 120s
/// startup hangs). Falsified if the race waits for the dud to time out.
/// Runs on paused tokio time, so no live network and no real waiting.
#[tokio::test(start_paused = true)]
async fn a_hedged_pull_rescues_a_hanging_exit_node_at_the_hedge_interval() {
    let started = tokio::time::Instant::now();
    let winner = drive_acq_race(
        RACED_ARMS,
        RACED_ARMS,
        hedged(RESERVATION_CLUTCH_SIZE),
        |arm| async move {
            if arm == 0 {
                std::future::pending::<Result<&str, String>>().await
            } else {
                Ok("rescued")
            }
        },
        no_abandon,
        panic_text,
        |_| {},
    )
    .await
    .expect("the hedged pull wins");
    assert_eq!(winner, "rescued");

    let elapsed = started.elapsed();
    assert!(
        elapsed >= HEDGE_INTERVAL,
        "the hedge waits its interval, elapsed {elapsed:?}"
    );
    assert!(
        elapsed < PER_ATTEMPT_CONNECT_TIMEOUT,
        "the rescue must not wait for the per-attempt timeout, elapsed {elapsed:?}"
    );
}

/// HYPOTHESIS: when parallelism is exhausted the per-attempt timeout
/// still frees the wedged slot, so a hanging Exit Node costs one timeout
/// rather than the lifecycle budget. Falsified if the race hangs past
/// [`PER_ATTEMPT_CONNECT_TIMEOUT`] with a single slot.
#[tokio::test(start_paused = true)]
async fn the_per_attempt_timeout_frees_a_wedged_slot() {
    let started = tokio::time::Instant::now();
    let winner = drive_acq_race(
        RACED_ARMS,
        RACED_ARMS,
        hedged(1),
        |arm| async move {
            if arm == 0 {
                tokio::time::timeout(
                    PER_ATTEMPT_CONNECT_TIMEOUT,
                    std::future::pending::<Result<&str, String>>(),
                )
                .await
                .unwrap_or_else(|_| Err("attempt timed out".to_string()))
            } else {
                Ok("second")
            }
        },
        no_abandon,
        panic_text,
        |_| {},
    )
    .await
    .expect("the second exit node wins after the timeout");
    assert_eq!(winner, "second");

    let elapsed = started.elapsed();
    assert!(elapsed >= PER_ATTEMPT_CONNECT_TIMEOUT);
    assert!(elapsed < NYM_LIFECYCLE_TIMEOUT);
}

/// HYPOTHESIS: a lost race accounts for EVERY attempt, down to the first
/// failure, the information the retired retry engine discarded — and it
/// accounts for them as typed records, not a joined sentence (issue
/// #2562). Falsified if any attempt's failure is missing or flattened.
#[tokio::test(start_paused = true)]
async fn a_lost_acq_race_accounts_for_every_attempt() {
    let lost = drive_acq_race(
        RACED_ARMS,
        RACED_ARMS,
        hedged(RESERVATION_CLUTCH_SIZE),
        |arm| async move { Err::<&str, _>(format!("arm {arm} refused")) },
        no_abandon,
        panic_text,
        |_| {},
    )
    .await
    .expect_err("both exit nodes refuse");

    assert_eq!(lost.launched, 2, "both arms were contacted");
    assert!(
        lost.failures.contains(&"arm 0 refused".to_string()),
        "{:?}",
        lost.failures
    );
    assert!(
        lost.failures.contains(&"arm 1 refused".to_string()),
        "{:?}",
        lost.failures
    );
}

/// HYPOTHESIS: when two pulls finish as winners in the same instant,
/// exactly one wins and the other is handed to `abandon` (production
/// disconnects it) rather than being dropped silently. Falsified if the
/// second winner vanishes without the abandon callback.
#[tokio::test(start_paused = true)]
async fn a_simultaneous_second_winner_is_abandoned_not_leaked() {
    let abandoned = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&abandoned);
    // Pull 0 launches at t=0 and finishes at t=6; pull 1 launches at the
    // t=5 hedge and finishes at t=6 as well.
    let winner = drive_acq_race(
        RACED_ARMS,
        RACED_ARMS,
        hedged(RESERVATION_CLUTCH_SIZE),
        |arm| async move {
            let finish = if arm == 0 {
                Duration::from_secs(6)
            } else {
                Duration::from_secs(1)
            };
            tokio::time::sleep(finish).await;
            Ok::<_, String>(if arm == 0 { "arm0" } else { "arm1" })
        },
        move |second_winner| {
            let sink = std::sync::Arc::clone(&sink);
            async move { sink.lock().unwrap().push(second_winner) }
        },
        panic_text,
        |_| {},
    )
    .await
    .expect("one pull wins");

    let abandoned = abandoned.lock().unwrap();
    assert_eq!(abandoned.len(), 1, "the second winner is abandoned once");
    let mut both = vec![winner, abandoned[0]];
    both.sort_unstable();
    assert_eq!(both, vec!["arm0", "arm1"]);
}

/// HYPOTHESIS: progress reaches the observer as the race advances, so a
/// bootstrap is narratable rather than opaque. Falsified if no progress
/// line mentions the widened race.
#[tokio::test(start_paused = true)]
async fn progress_lines_narrate_the_acq_race() {
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&lines);
    let _ = drive_acq_race(
        RACED_ARMS,
        RACED_ARMS,
        hedged(RESERVATION_CLUTCH_SIZE),
        |arm| async move { Err::<&str, _>(format!("arm {arm} refused")) },
        no_abandon,
        panic_text,
        move |step| {
            if let AcqStep::Progress(progress) = step {
                sink.lock().unwrap().push(progress.to_string())
            }
        },
    )
    .await;
    let lines = lines.lock().unwrap();
    assert!(
        lines.iter().any(|l| l.contains("2 failed")),
        "progress must narrate the accumulated failures, got {lines:?}"
    );
}

/// HYPOTHESIS: the driver reports every launch and every failure as a
/// typed step carrying the arm index, so the caller can name the Exit
/// Node each step concerns. Falsified if a launched arm goes
/// unreported, a failure loses its index, or a failure loses its text.
#[tokio::test(start_paused = true)]
async fn the_driver_reports_each_launch_and_failure_with_its_arm() {
    let steps = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&steps);
    let _ = drive_acq_race(
        RACED_ARMS,
        RACED_ARMS,
        hedged(RESERVATION_CLUTCH_SIZE),
        |arm| async move { Err::<&str, _>(format!("arm {arm} refused")) },
        no_abandon,
        panic_text,
        move |step| match step {
            AcqStep::Launched { arm } => sink.lock().unwrap().push(format!("launched {arm}")),
            AcqStep::Failed { arm, error } => {
                sink.lock().unwrap().push(format!("failed {arm}: {error}"))
            }
            AcqStep::Progress(_) => {}
        },
    )
    .await;
    let steps = steps.lock().unwrap();
    for arm in 0..RACED_ARMS {
        assert!(
            steps.contains(&format!("launched {arm}")),
            "every arm's launch is reported, got {steps:?}"
        );
        assert!(
            steps.contains(&format!("failed {arm}: arm {arm} refused")),
            "every arm's failure carries its index and text, got {steps:?}"
        );
    }
}

/// HYPOTHESIS: the shared discovery narration reports the query's
/// departure before its outcome, so a failed directory query is still
/// visible on the typed channel that both bootstrap and reconnect
/// report through; falsified if a failing discovery emits nothing or
/// claims to have finished.
#[tokio::test]
async fn a_failed_discovery_still_reports_its_departure() {
    let mut events = Vec::new();
    // Port 0 is unconnectable, so the query fails without the network.
    let outcome =
        NymProxy::discover_observed("http://127.0.0.1:0/", &mut |event| events.push(event)).await;
    outcome.expect_err("no directory answers on port 0");
    assert_eq!(events, vec![BootstrapEvent::DiscoveryStarted]);
}

#[test]
fn long_exit_node_names_are_shortened_for_the_summary() {
    let exit_nodes = vec!["a".repeat(200), "short".to_string()];
    let shortened = short_exit_node_name(&exit_nodes, 0);
    assert!(
        shortened.chars().count() == 13,
        "twelve chars plus ellipsis"
    );
    assert!(shortened.ends_with('…'));
    assert_eq!(short_exit_node_name(&exit_nodes, 1), "short");
    assert_eq!(short_exit_node_name(&exit_nodes, 9), "exit node 9");
}

/// The per-pull classifier is a pure, total match (issue #2562): the
/// constructible variants pin their stages, the timeout carries its
/// bound in milliseconds, and the record captures the target and the
/// cause chain.
#[test]
fn exit_node_attempt_failures_classify_stage_target_and_chain() {
    let timeout = NymProxyError::AttemptTimeout(PER_ATTEMPT_CONNECT_TIMEOUT.as_secs());
    let failure = exit_node_attempt_failure(&timeout, "Emq7Gc3PLdp…");
    assert_eq!(
        failure.stage,
        NetOpStage::TimedOut {
            after_ms: PER_ATTEMPT_CONNECT_TIMEOUT.as_secs() * 1000
        }
    );
    assert_eq!(failure.target, "Emq7Gc3PLdp…");
    assert_eq!(failure.cause_chain, vec![timeout.to_string()]);

    // The port draw fails before the arm touches the network.
    let port_draw =
        NymProxyError::DiscoveryApi("failed to find available port: denied".to_string());
    assert_eq!(
        exit_node_attempt_stage(&port_draw),
        NetOpStage::RouteResolution
    );
    let tunnel = NymProxyError::ConnectivityCheck("no data".to_string());
    assert_eq!(
        exit_node_attempt_stage(&tunnel),
        NetOpStage::TunnelTransport
    );
}

/// A lost race maps to `NoExitNode` only when nothing was contacted;
/// otherwise the typed attempts are carried whole into
/// `AttemptsExhausted`, never flattened (issue #2562).
#[test]
fn an_acq_race_loss_carries_its_typed_attempts_whole() {
    assert!(matches!(
        acq_race_loss_error(LostAcqRace {
            launched: 0,
            failures: Vec::new(),
        }),
        NymProxyError::NoExitNode
    ));

    let records = vec![
        NetOpFailure::message(NetOpStage::RemoteConnect, "p0", "gateway refused"),
        NetOpFailure::message(NetOpStage::TimedOut { after_ms: 20_000 }, "p1", "timed out"),
    ];
    match acq_race_loss_error(LostAcqRace {
        launched: 2,
        failures: records.clone(),
    }) {
        NymProxyError::AttemptsExhausted { attempts, failures } => {
            assert_eq!(attempts, 2);
            assert_eq!(failures, records, "the records survive untouched");
        }
        other => panic!("expected AttemptsExhausted, got {other:?}"),
    }
}

// Integration tests below require a live Nym network. Run with:
//   cargo test --package zl-netutils --features nym -- --ignored

/// Start the embedded proxy and verify it reports a valid localhost address.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires live Nym network"]
async fn nym_proxy_starts_and_reports_address() {
    let proxy = NymProxy::start().await.expect("NymProxy::start");
    let addr = proxy.socks5_addr();
    assert_eq!(
        addr.ip(),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        "expected a loopback address, got {addr}"
    );
    assert!(addr.port() > 0, "port should be non-zero");
    assert_eq!(addr.port(), proxy.bind_port(), "one bound port, one report");
    proxy.disconnect().await;
}

/// Start the proxy and verify a SOCKS5 TCP tunnel works end-to-end.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires live Nym network"]
async fn nym_proxy_socks5_tunnel_works() {
    let proxy = NymProxy::start().await.expect("NymProxy::start");
    let stream = tokio_socks::tcp::Socks5Stream::connect(
        proxy.socks5_addr(),
        crate::default_indexer::MAINNET_DEFAULT_INDEXER.trim_start_matches("https://"),
    )
    .await
    .expect("SOCKS5 connect");

    drop(stream);
    proxy.disconnect().await;
}

/// Start and disconnect cleanly with no panic.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires live Nym network"]
async fn nym_proxy_disconnect_clean() {
    let proxy = NymProxy::start().await.expect("NymProxy::start");
    proxy.disconnect().await;
}
