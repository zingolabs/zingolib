use std::io::Cursor;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::ReadBuf;

use super::*;

/// The number of times one detail may appear across a whole cause chain.
const DETAIL_RENDERINGS: usize = 1;

/// HYPOTHESIS: a proxy spawn failure names the path it tried without
/// restating its source, so the underlying detail reaches the reader
/// exactly once across the sanctioned chain walk. Falsified if the
/// detail appears more than once.
#[test]
fn a_proxy_spawn_failure_renders_its_detail_once() {
    const DETAIL: &str = "the nym-proxy binary is absent";
    let failure = MixnetProxyError::Spawn {
        path: "/no/such/nym-proxy".to_string(),
        source: std::io::Error::other(DETAIL),
    };
    let chain = zl_netutils::diag::chain_texts(&failure);
    assert_eq!(
        chain.join("\n").matches(DETAIL).count(),
        DETAIL_RENDERINGS,
        "the chain rendered was {chain:?}"
    );
}

/// The identities a duplicating child names, counted once each.
const DISTINCT_DISCOVERED: usize = 2;

/// HYPOTHESIS: a child announcing one identity on two lines discovers
/// that identity once, so the discovery edge carries a population
/// rather than a transcript. Falsified if the repeated line reaches the
/// caller twice.
#[test]
fn a_duplicated_child_line_discovers_once() {
    let stdout = format!(
        "{NYM_EXIT_LINE_PREFIX}exit-a\n\
             {NYM_EXIT_LINE_PREFIX}exit-b\n\
             {NYM_EXIT_LINE_PREFIX}exit-a\n"
    );
    assert_eq!(
        parse_discovery_output(&stdout).len(),
        DISTINCT_DISCOVERED,
        "the repeated announcement names one exit"
    );
}

fn bootstrapping() -> Arc<Mutex<ProxyState>> {
    Arc::new(Mutex::new(ProxyState {
        mode: Indicator::Bootstrapping,
        socks5_addr: None,
        exits: Vec::new(),
        bootstrap_detail: None,
        death: None,
    }))
}

/// A throwaway session channel for tests that exercise the state
/// machine without asserting on publications.
fn test_publisher() -> StatusPublisher {
    crate::mixnet::status_publisher()
}

/// A fabricated readiness failure for the attach falsifiers.
fn readiness_failure(text: &str) -> zl_netutils::diag::NetOpFailure {
    zl_netutils::diag::NetOpFailure::message(
        zl_netutils::diag::NetOpStage::TunnelTransport,
        "zec.rocks:443",
        text,
    )
}

/// A reader that yields `bytes` once and then stays pending forever, never
/// reaching EOF, modelling a live child whose stdout is open but idle
/// after its announcement. A plain byte slice EOFs immediately, which
/// `drive_state` now (correctly) reads as the child dying. These tests
/// want to observe the intermediate `Ready` while the stream is still
/// open, so the reader must not close.
struct OpenAfter(Cursor<Vec<u8>>);

impl OpenAfter {
    fn new(bytes: &[u8]) -> Self {
        OpenAfter(Cursor::new(bytes.to_vec()))
    }
}

impl AsyncRead for OpenAfter {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let position = self.0.position() as usize;
        let source = self.0.get_ref();
        if position >= source.len() {
            // Delivered everything; stay open, never signal EOF.
            return Poll::Pending;
        }
        let count = (source.len() - position).min(buf.remaining());
        buf.put_slice(&source[position..position + count]);
        self.0.set_position((position + count) as u64);
        Poll::Ready(Ok(()))
    }
}

/// Runs `drive_state` against an open-then-idle stream carrying `bytes`,
/// yields until the mode leaves `Bootstrapping` (or a bound elapses), and
/// returns the observed state without the stream ever closing.
async fn state_over_open_stream(bytes: &[u8]) -> ProxyState {
    let state = bootstrapping();
    let handle = tokio::spawn(drive_state(
        OpenAfter::new(bytes),
        Arc::clone(&state),
        test_publisher(),
        None,
    ));
    for _ in 0..1000 {
        if state.lock().unwrap().mode != Indicator::Bootstrapping {
            break;
        }
        tokio::task::yield_now().await;
    }
    handle.abort();
    let guard = state.lock().expect("proxy state mutex");
    ProxyState::clone(&guard)
}

#[test]
fn parses_the_announcement_line() {
    assert_eq!(
        parse_socks5_addr_line("SOCKS5_ADDR=127.0.0.1:43210"),
        Some("127.0.0.1:43210")
    );
}

#[test]
fn trims_trailing_whitespace_and_carriage_return() {
    assert_eq!(
        parse_socks5_addr_line("SOCKS5_ADDR=127.0.0.1:9 \r"),
        Some("127.0.0.1:9")
    );
}

#[test]
fn ignores_non_announcement_lines() {
    assert_eq!(parse_socks5_addr_line("connecting to mixnet"), None);
    assert_eq!(parse_socks5_addr_line(""), None);
}

#[tokio::test]
async fn ready_when_the_address_is_announced() {
    let s = state_over_open_stream(b"SOCKS5_ADDR=127.0.0.1:43210\n").await;
    assert_eq!(s.mode, Indicator::Ready);
    assert_eq!(
        s.socks5_addr,
        Some("127.0.0.1:43210".parse().expect("the test address parses"))
    );
}

#[tokio::test]
async fn ready_after_preamble_lines() {
    let s = state_over_open_stream(b"discovering gateways\nconnecting\nSOCKS5_ADDR=127.0.0.1:5\n")
        .await;
    assert_eq!(s.mode, Indicator::Ready);
    assert_eq!(
        s.socks5_addr,
        Some("127.0.0.1:5".parse().expect("the test address parses"))
    );
}

/// HYPOTHESIS: an exit announcement before the address is recorded and
/// surfaces as Ready-only evidence in the snapshot.
#[tokio::test]
async fn ready_carries_the_announced_exit() {
    let s = state_over_open_stream(b"NYM_EXIT=exit-alpha\nSOCKS5_ADDR=127.0.0.1:5\n").await;
    assert_eq!(s.mode, Indicator::Ready);
    assert_eq!(s.exits, vec![crate::mixnet::ExitNodeId::from("exit-alpha")]);
    assert_eq!(
        s.snapshot().exits,
        vec![crate::mixnet::ExitNodeId::from("exit-alpha")]
    );
}

/// HYPOTHESIS: a bare or whitespace-only exit announcement records no
/// exit, so no blank identity reaches the snapshot. Falsified if the
/// blank is pushed as an `ExitNodeId`.
#[tokio::test]
async fn a_blank_exit_announcement_records_no_exit() {
    let s = state_over_open_stream(b"NYM_EXIT=\nNYM_EXIT=   \nSOCKS5_ADDR=127.0.0.1:5\n").await;
    assert_eq!(s.mode, Indicator::Ready);
    assert_eq!(s.exits, Vec::<crate::mixnet::ExitNodeId>::new());
}

/// HYPOTHESIS: an unparseable address announcement latches `Died` with a
/// typed cause while the stream is still open, so the caller fails fast
/// instead of burning its whole bootstrap budget. Falsified if the line
/// is dropped silently and the mode stays `Bootstrapping`.
#[tokio::test]
async fn an_unparseable_address_announcement_dies_with_a_cause() {
    let s = state_over_open_stream(b"SOCKS5_ADDR=not-a-socket\n").await;
    assert_eq!(s.mode, Indicator::Died);
    assert_eq!(s.socks5_addr, None);
    let death = s.death.expect("the defect latches a death report");
    let detail = death.detail.expect("the death carries the typed cause");
    assert_eq!(detail.stage, zl_netutils::diag::NetOpStage::ProxyLaunch);
    assert_eq!(detail.target, "not-a-socket");
}

/// HYPOTHESIS: the close after a diagnosed defect keeps that defect as
/// the death cause. Falsified if the close overwrites the sticky latch
/// with a cause-less report.
#[tokio::test]
async fn the_close_preserves_the_diagnosed_cause() {
    let state = bootstrapping();
    drive_state(
        b"SOCKS5_ADDR=not-a-socket\n".as_slice(),
        Arc::clone(&state),
        test_publisher(),
        None,
    )
    .await;
    let s = state.lock().expect("proxy state mutex");
    assert_eq!(s.mode, Indicator::Died);
    let death = s.death.clone().expect("the defect latches a death report");
    assert!(
        death.detail.is_some(),
        "the diagnosed cause survives the close"
    );
}

/// HYPOTHESIS: a status line updates the live bootstrap detail while the
/// mode stays `Bootstrapping`. Falsified if the line is ignored as noise
/// or flips the state.
#[tokio::test]
async fn a_status_line_updates_the_detail_and_keeps_bootstrapping() {
    // No address arrives, so the mode stays Bootstrapping and the helper
    // returns after its bound with the live detail retained. This isolates
    // the detail-tracking from the close transition (tested separately).
    let s = state_over_open_stream(
        b"NYM_STATUS=discovering exit gateways\nNYM_STATUS=attempt 2/10: 2 in flight, 0 failed\n",
    )
    .await;
    assert_eq!(s.mode, Indicator::Bootstrapping);
    assert_eq!(
        s.bootstrap_detail.as_deref(),
        Some("attempt 2/10: 2 in flight, 0 failed"),
        "the LATEST status line is retained while bootstrapping"
    );
}

/// HYPOTHESIS: the address announcement still wins after status lines,
/// and readiness clears the now-stale detail. Falsified if a status line
/// masks the announcement or the detail lingers past bootstrap.
#[tokio::test]
async fn the_address_wins_after_status_lines_and_clears_the_detail() {
    let s = state_over_open_stream(
        b"NYM_STATUS=attempt 1/10: 1 in flight, 0 failed\nSOCKS5_ADDR=127.0.0.1:7\n",
    )
    .await;
    assert_eq!(s.mode, Indicator::Ready);
    assert_eq!(
        s.socks5_addr,
        Some("127.0.0.1:7".parse().expect("the test address parses"))
    );
    assert_eq!(s.bootstrap_detail, None, "ready has no bootstrap detail");
}

#[tokio::test]
async fn died_when_stdout_closes_during_bootstrap() {
    let state = bootstrapping();
    drive_state(
        b"failed to reach any gateway\n".as_slice(),
        Arc::clone(&state),
        test_publisher(),
        None,
    )
    .await;
    let s = state.lock().unwrap();
    assert_eq!(
        s.mode,
        Indicator::Died,
        "a proxy that closes without an address died; it is never consented nakednet"
    );
    assert!(s.socks5_addr.is_none());
}

/// HYPOTHESIS: the reader keeps watching past Ready, so a proxy that dies
/// AFTER announcing its address lands in Died with the stale address
/// cleared, the exact zombie path the coupling closes. Falsified if the
/// reader stops at Ready (mode stuck Ready) or leaves the dead address
/// dialable.
#[tokio::test]
async fn died_when_stdout_closes_after_ready() {
    let state = bootstrapping();
    // Address announced (Ready), then stdout closes (the child exited).
    drive_state(
        b"SOCKS5_ADDR=127.0.0.1:43210\n".as_slice(),
        Arc::clone(&state),
        test_publisher(),
        None,
    )
    .await;
    let s = state.lock().unwrap();
    assert_eq!(
        s.mode,
        Indicator::Died,
        "a proxy that dies after ready must not stay stale-Ready"
    );
    assert!(
        s.socks5_addr.is_none(),
        "the dead proxy's address must be cleared so nothing dials it"
    );
    let latch = s
        .death
        .as_ref()
        .expect("even a causeless death (closed pipe) must latch its moment");
    assert!(
        latch.detail.is_none(),
        "a closed pipe carries no cause; its diagnostic is the child's stderr"
    );
}

/// A launch context whose stderr tail already holds `lines`.
fn launch_context(lines: &[&str]) -> LaunchContext {
    LaunchContext {
        binary: "/opt/zingo/nym-proxy".to_string(),
        args: vec!["--exit".to_string(), "exit-alpha".to_string()],
        stderr_tail: Arc::new(Mutex::new(
            lines.iter().map(|line| line.to_string()).collect(),
        )),
    }
}

/// HYPOTHESIS: a child that closes stdout without ever speaking the
/// protocol latches a typed proxy-launch cause naming the binary, the
/// launch arguments, and its stderr tail — the version-skew diagnosis.
/// Falsified if the death report stays bare.
#[tokio::test]
async fn a_launch_death_diagnoses_version_skew() {
    let state = bootstrapping();
    drive_state(
        b"error: unrecognized flag\n".as_slice(),
        Arc::clone(&state),
        test_publisher(),
        Some(launch_context(&["unknown argument: --frobnicate"])),
    )
    .await;
    let s = state.lock().unwrap();
    assert_eq!(s.mode, Indicator::Died);
    let detail = s
        .death
        .as_ref()
        .and_then(|report| report.detail.as_ref())
        .expect("a pre-protocol death carries the launch diagnosis");
    assert_eq!(detail.stage, zl_netutils::diag::NetOpStage::ProxyLaunch);
    assert_eq!(detail.target, "/opt/zingo/nym-proxy");
    assert!(
        detail
            .cause_chain
            .iter()
            .any(|text| text.contains("version skew"))
    );
    assert!(
        detail
            .cause_chain
            .iter()
            .any(|text| text.contains("--exit exit-alpha"))
    );
    assert!(
        detail
            .cause_chain
            .iter()
            .any(|text| text.contains("unknown argument: --frobnicate"))
    );
}

/// HYPOTHESIS: one protocol line proves the child speaks the wallet's
/// stdout grammar, so its later death carries no launch diagnosis.
/// Falsified if every spawned death now claims a version skew.
#[tokio::test]
async fn a_protocol_line_disarms_the_launch_diagnosis() {
    let state = bootstrapping();
    drive_state(
        b"NYM_STATUS=discovering exit gateways\n".as_slice(),
        Arc::clone(&state),
        test_publisher(),
        Some(launch_context(&[])),
    )
    .await;
    let s = state.lock().unwrap();
    assert_eq!(s.mode, Indicator::Died);
    assert!(
        s.death
            .as_ref()
            .is_some_and(|report| report.detail.is_none()),
        "a death after protocol speech is not a launch failure"
    );
}

// ----- attached-transport falsifiers (issue #2503) -----

/// HYPOTHESIS: a failed readiness check lands Died without ever
/// announcing Ready, and the watchdog never runs. Falsified if the
/// driver reaches Ready, leaves an address set, or consults the
/// watchdog after a dead readiness verdict.
#[tokio::test(start_paused = true)]
async fn attached_readiness_failure_lands_died_never_ready() {
    let state = bootstrapping();
    let probed = Arc::new(Mutex::new(false));
    let probed_flag = Arc::clone(&probed);
    drive_attached_state(
        Arc::clone(&state),
        "127.0.0.1:1080".parse().expect("the test address parses"),
        std::future::ready(Err(readiness_failure("no data through the endpoint"))),
        move || {
            *probed_flag.lock().unwrap() = true;
            std::future::ready(true)
        },
        Duration::from_secs(30),
        test_publisher(),
    )
    .await;
    let s = state.lock().unwrap();
    assert_eq!(s.mode, Indicator::Died, "readiness failure is death");
    assert!(s.socks5_addr.is_none(), "no address may be published");
    assert_eq!(
        s.death.as_ref().and_then(|latch| latch.detail.clone()),
        Some(readiness_failure("no data through the endpoint")),
        "the death must carry the typed readiness failure"
    );
    assert!(
        !*probed.lock().unwrap(),
        "liveness probing must not start for an endpoint that never became ready"
    );
}

/// HYPOTHESIS (the attached zombie falsifier): readiness success
/// publishes Ready with the address, and a later watchdog failure
/// lands Died with the address cleared. The watchdog closure snapshots
/// the state at each call, so the intermediate Ready is asserted
/// without racing the driver. Falsified if Ready is never published,
/// the watchdog cadence stalls, or the dead endpoint's address stays
/// dialable. Runs on paused tokio time.
#[tokio::test(start_paused = true)]
async fn attached_watchdog_failure_lands_died_after_publishing_ready() {
    /// What the watchdog closure saw at one call: the mode and the
    /// published address at that instant.
    type ProbeSnapshot = (Indicator, Option<std::net::SocketAddr>);

    let state = bootstrapping();
    let observed: Arc<Mutex<Vec<ProbeSnapshot>>> = Arc::new(Mutex::new(Vec::new()));
    let observer = Arc::clone(&observed);
    let observer_state = Arc::clone(&state);
    drive_attached_state(
        Arc::clone(&state),
        "127.0.0.1:1080".parse().expect("the test address parses"),
        std::future::ready(Ok(())),
        move || {
            let snapshot = observer_state.lock().unwrap();
            observer
                .lock()
                .unwrap()
                .push((snapshot.mode, snapshot.socks5_addr));
            let calls = observer.lock().unwrap().len();
            std::future::ready(calls < 3)
        },
        Duration::from_secs(30),
        test_publisher(),
    )
    .await;

    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 3, "two live ticks, then the fatal third");
    assert_eq!(
        observed[0],
        (
            Indicator::Ready,
            Some("127.0.0.1:1080".parse().expect("the test address parses"))
        ),
        "readiness success must publish Ready with the address"
    );
    let s = state.lock().unwrap();
    assert_eq!(s.mode, Indicator::Died, "a failed tick is death");
    assert!(
        s.socks5_addr.is_none(),
        "the dead endpoint's address must be cleared so nothing dials it"
    );
}

/// HYPOTHESIS: a passing readiness gate publishes Ready carrying the
/// host-bound Exit Node identities the attach seam seeded. Falsified if
/// Ready drops or invents the identities.
#[tokio::test]
async fn attached_ready_carries_the_host_bound_exits() {
    let state = Arc::new(Mutex::new(ProxyState {
        mode: Indicator::Bootstrapping,
        socks5_addr: None,
        exits: vec!["host-bound-exit".into()],
        bootstrap_detail: None,
        death: None,
    }));
    let publisher = test_publisher();
    let mut receiver = publisher.subscribe();
    // The watchdog sleeps far past the test; the readiness publish is
    // what this isolates, observed on the channel before teardown.
    let driver = tokio::spawn(drive_attached_state(
        Arc::clone(&state),
        "127.0.0.1:1080".parse().expect("the test address parses"),
        std::future::ready(Ok(())),
        || std::future::ready(true),
        Duration::from_secs(3_600),
        publisher.clone(),
    ));
    loop {
        receiver.changed().await.expect("the publisher stays open");
        if receiver.borrow().mode == Indicator::Ready {
            break;
        }
    }
    assert_eq!(
        receiver.borrow().exits,
        vec![crate::mixnet::ExitNodeId::from("host-bound-exit")],
        "an attached Ready must carry the host-bound Exit Node"
    );
    driver.abort();
}

/// HYPOTHESIS: an attach whose report names no Exit Node refuses typed,
/// so the attach door and the acquisition gate share one definition of
/// Ready. Falsified if an exitless report mints a permanent Ready.
#[tokio::test]
async fn an_exitless_report_refuses_the_attach() {
    match MixnetProxy::attach(
        "127.0.0.1:1080".parse().expect("the test address parses"),
        &[],
        crate::mixnet::test_indexer(),
        test_publisher(),
    ) {
        Err(MixnetProxyError::NoExits) => {}
        Err(other) => panic!("the attach refused with '{other}' rather than the empty report"),
        Ok(proxy) => {
            proxy.stop().await;
            panic!("an exitless report must never mint an attached transport")
        }
    }
}

/// stop() on an attached transport is a deliberate teardown
/// to Unattached — never Died, and never the wallet's SwitchedOff.
/// Falsified if a live attachment reports anything but the transport
/// lifecycle states. Address validation lives at the consumer seam,
/// because `attach` now takes the parsed address by type.
#[tokio::test]
async fn attached_stop_is_a_deliberate_teardown() {
    // A refusing localhost endpoint: readiness will fail, but stop() must
    // win regardless of where the driver is when it lands.
    let proxy = MixnetProxy::attach(
        "127.0.0.1:9".parse().expect("the test address parses"),
        &[crate::mixnet::ExitNodeId::from("host-bound-exit")],
        crate::mixnet::test_indexer(),
        test_publisher(),
    )
    .expect("a valid address and a named exit attach");
    // The readiness gate races this assert (a refused port can land Died
    // fast), so assert only what is invariant: a live attachment is in
    // the transport lifecycle, never in a wallet slot state.
    assert!(
        !matches!(proxy.mode(), Indicator::Unattached | Indicator::SwitchedOff),
        "a live attachment never reports a slot state"
    );
    proxy.stop().await;
}

/// Asserts that an attached endpoint that dies refuses the route — the
/// fail-closed invariant holds for the attached transport end to end.
/// Attaches to a refusing localhost port, waits for the real readiness
/// gate to land Died, and resolves the route. Falsified if the route
/// ever yields nakednet or the mixnet for the dead endpoint.
#[tokio::test]
async fn an_attached_endpoint_that_dies_refuses_the_route() {
    use crate::mixnet::route::MixnetNotReady;
    use crate::mixnet::route::resolve_mixnet_only_route;

    // Port 9 (discard) on localhost refuses; the readiness round trip
    // fails fast and the driver lands Died.
    let proxy = MixnetProxy::attach(
        "127.0.0.1:9".parse().expect("the test address parses"),
        &[crate::mixnet::ExitNodeId::from("host-bound-exit")],
        crate::mixnet::test_indexer(),
        test_publisher(),
    )
    .expect("a valid address and a named exit attach");
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while proxy.mode() != Indicator::Died {
        assert!(
            std::time::Instant::now() < deadline,
            "the dead endpoint must be detected within the readiness budget"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        resolve_mixnet_only_route(
            proxy.mode(),
            proxy
                .socks5_addr()
                .map(zl_netutils::conduit::MixnetConduit::over)
        ),
        Err(MixnetNotReady::Died),
        "a died attachment must refuse, never route"
    );
    proxy.stop().await;
}

// ----- session-channel publication falsifiers (the driver arc) -----

/// HYPOTHESIS: an exit announced after the address wakes a readiness
/// waiter that is already parked on the session channel, so a healthy
/// transport is never stopped as not ready for its announcement order.
#[tokio::test(start_paused = true)]
async fn an_exit_after_the_address_wakes_the_parked_waiter() {
    let publisher = test_publisher();
    let mut receiver = publisher.subscribe();
    let state = bootstrapping();
    let handle = tokio::spawn(drive_state(
        OpenAfter::new(b"SOCKS5_ADDR=127.0.0.1:43210\nNYM_EXIT=exit-alpha\n"),
        Arc::clone(&state),
        Arc::clone(&publisher),
        None,
    ));
    let (addr, exits) =
        await_ready_endpoint(&mut receiver, zl_netutils::time::NYM_LIFECYCLE_TIMEOUT)
            .await
            .expect("the announced exit reaches the parked waiter");
    assert_eq!(
        addr,
        "127.0.0.1:43210".parse().expect("the test address parses"),
        "the waiter yields the announced address"
    );
    assert_eq!(
        exits,
        vec![crate::mixnet::ExitNodeId::from("exit-alpha")],
        "the waiter yields the exit announced after the address"
    );
    handle.abort();
}

/// HYPOTHESIS: a Ready that announces its address but never an Exit
/// Node refuses once the exit-announcement grace elapses, so the
/// go-online moment is not held for the whole lifecycle budget.
#[tokio::test(start_paused = true)]
async fn an_exitless_ready_refuses_after_the_grace() {
    let publisher = test_publisher();
    let mut receiver = publisher.subscribe();
    publisher.send_replace(MixnetStatus {
        mode: Indicator::Ready,
        socks5_addr: Some("127.0.0.1:1080".parse().expect("the test address parses")),
        exits: Vec::new(),
        bootstrap_detail: None,
        death: None,
    });
    let started = tokio::time::Instant::now();
    let refusal = await_ready_endpoint(&mut receiver, zl_netutils::time::NYM_LIFECYCLE_TIMEOUT)
        .await
        .expect_err("no exit ever arrives");
    assert!(
        matches!(
            refusal,
            acquire::TransportError::NotReady { budget }
                if budget == zl_netutils::time::EXIT_ANNOUNCEMENT_GRACE
        ),
        "the refusal names the grace it exceeded, got: {refusal}"
    );
    assert_eq!(
        started.elapsed(),
        zl_netutils::time::EXIT_ANNOUNCEMENT_GRACE,
        "the wait ends at the grace, never at the lifecycle budget"
    );
}

/// HYPOTHESIS: transport transitions are published into the session
/// channel as they happen — a subscriber observes Ready with the
/// address while the stream is still open — without polling any pull
/// accessor. Falsified if the channel stays at its initial value or
/// lags the state machine.
#[tokio::test]
async fn transitions_reach_the_session_channel() {
    let publisher = test_publisher();
    let mut receiver = publisher.subscribe();
    let state = bootstrapping();
    let handle = tokio::spawn(drive_state(
        OpenAfter::new(b"SOCKS5_ADDR=127.0.0.1:43210\n"),
        Arc::clone(&state),
        Arc::clone(&publisher),
        None,
    ));
    {
        let ready = receiver
            .wait_for(|status| status.mode == Indicator::Ready)
            .await
            .expect("the publisher outlives the wait");
        assert_eq!(
            ready.socks5_addr,
            Some("127.0.0.1:43210".parse().expect("the test address parses"))
        );
    }
    handle.abort();
}

/// HYPOTHESIS: a spawned child's death is published whole — mode Died,
/// address cleared, the causeless latch present — so a subscriber needs
/// no pull accessor to learn the transport is gone. Falsified if the
/// published snapshot omits the latch or leaves the dead address set.
#[tokio::test]
async fn a_death_is_published_whole() {
    let publisher = test_publisher();
    let receiver = publisher.subscribe();
    let state = bootstrapping();
    drive_state(
        b"SOCKS5_ADDR=127.0.0.1:43210\n".as_slice(),
        Arc::clone(&state),
        Arc::clone(&publisher),
        None,
    )
    .await;
    let latest = receiver.borrow().clone();
    assert_eq!(latest.mode, Indicator::Died);
    assert!(
        latest.socks5_addr.is_none(),
        "the dead proxy's address must not be published"
    );
    let death = latest
        .death
        .expect("every published death carries its latch");
    assert!(death.detail.is_none(), "a closed pipe carries no cause");
}

// ----- the death-detail accessor's mode gate (issue #2563) -----

/// A `MixnetProxy` over a fabricated state, with an inert attached
/// driver: the accessor under test reads only the state, so the
/// transport variant carries no behavior here.
fn proxy_over(state: &Arc<Mutex<ProxyState>>) -> MixnetProxy {
    MixnetProxy {
        state: Arc::clone(state),
        transport: Transport::Attached {
            driver: tokio::spawn(std::future::ready(())),
        },
    }
}

/// HYPOTHESIS (the #2569 review): staleness math survives a stepping
/// clock — [`DeathReport::age`] clamps a latch moment reading later
/// than now (an NTP step between the latch and the read) to zero.
/// Falsified if age panics on a future moment or reports it nonzero.
#[test]
fn death_age_clamps_a_stepping_clock_to_zero() {
    let epoch = std::time::SystemTime::UNIX_EPOCH;
    let now = epoch + std::time::Duration::from_secs(100);
    let latched_before = DeathReport {
        at: epoch + std::time::Duration::from_secs(40),
        detail: None,
    };
    assert_eq!(latched_before.age(now), std::time::Duration::from_secs(60));
    let latched_in_the_future = DeathReport {
        at: now + std::time::Duration::from_secs(10),
        detail: None,
    };
    assert_eq!(
        latched_in_the_future.age(now),
        std::time::Duration::ZERO,
        "a stepped clock reads as zero age, never an error"
    );
}

/// HYPOTHESIS (the #2569 review): the two death accessors derive from
/// one latch and can never disagree — whenever the mode is `Died`,
/// `death_detail()` equals the detail inside `death_report()`, whether
/// the death carried a cause (attach failure) or none (closed pipe).
/// The folded latch makes a cause without a moment unrepresentable;
/// this pins the derivation over every representable death.
#[tokio::test]
async fn the_death_accessors_cannot_disagree() {
    let state = bootstrapping();
    let proxy = proxy_over(&state);
    let deaths = [
        Some(readiness_failure("an attach failure with its moment")),
        None,
    ];
    for detail in deaths {
        {
            let mut guarded = state.lock().unwrap();
            guarded.mode = Indicator::Died;
            guarded.death = Some(DeathReport {
                at: std::time::SystemTime::UNIX_EPOCH,
                detail: detail.clone(),
            });
        }
        assert_eq!(
            proxy.death_detail(),
            proxy.death_report().and_then(|report| report.detail),
            "one latch, one answer: the detail must never outrun the report"
        );
        assert_eq!(proxy.death_detail(), detail);
    }
}

/// HYPOTHESIS: the named readiness budget equals the round-trip gate it
/// summarizes.
#[test]
fn the_readiness_budget_is_the_sum_of_its_gate() {
    let attempts = u32::try_from(ATTACH_HEALTH_ATTEMPTS).expect("a small count");
    assert_eq!(
        zl_netutils::time::ATTACH_READINESS_BUDGET,
        MIXNET_ROUND_TRIP_BOUND * attempts + ATTACH_LISTENER_RETRY_PAUSE * (attempts - 1),
        "retune the budget with its gate, never apart"
    );
}

/// HYPOTHESIS (issue #2564): a death latches its moment, `death_report`
/// surfaces moment and cause together while the mode is `Died`, and the
/// report is gated exactly like the detail. Falsified if a died mode
/// with a recorded moment yields no report, or a stale report leaks out
/// of another mode.
#[tokio::test]
async fn death_report_carries_the_latch_moment_and_respects_the_mode_gate() {
    let state = bootstrapping();
    {
        let mut guarded = state.lock().unwrap();
        guarded.mode = Indicator::Died;
        guarded.death = Some(DeathReport {
            at: std::time::SystemTime::UNIX_EPOCH,
            detail: Some(readiness_failure("no data through the endpoint")),
        });
    }
    let proxy = proxy_over(&state);
    let report = proxy.death_report().expect("a died transport reports");
    assert_eq!(report.at, std::time::SystemTime::UNIX_EPOCH);
    assert_eq!(
        report.detail,
        Some(readiness_failure("no data through the endpoint"))
    );

    for stale_mode in [
        Indicator::Ready,
        Indicator::Bootstrapping,
        Indicator::Unattached,
        Indicator::SwitchedOff,
    ] {
        state.lock().unwrap().mode = stale_mode;
        assert_eq!(
            proxy.death_report(),
            None,
            "a stale report must not leak out of {stale_mode:?}"
        );
    }
}

/// HYPOTHESIS (the Connection Doctor's contract, issue #2563):
/// `death_detail` surfaces the recorded cause whole while the mode is
/// `Died`, and `None` in every other mode even when a stale record
/// remains in the state. Falsified if a stale record leaks out of a
/// mode that is not `Died`.
#[tokio::test]
async fn death_detail_is_gated_on_the_died_mode() {
    let state = bootstrapping();
    {
        let mut guarded = state.lock().unwrap();
        guarded.mode = Indicator::Died;
        guarded.death = Some(DeathReport {
            at: std::time::SystemTime::UNIX_EPOCH,
            detail: Some(readiness_failure("no data through the endpoint")),
        });
    }
    let proxy = proxy_over(&state);
    assert_eq!(
        proxy.death_detail(),
        Some(readiness_failure("no data through the endpoint")),
        "a died transport surfaces its typed cause whole"
    );

    for stale_mode in [
        Indicator::Ready,
        Indicator::Bootstrapping,
        Indicator::Unattached,
        Indicator::SwitchedOff,
    ] {
        state.lock().unwrap().mode = stale_mode;
        assert_eq!(
            proxy.death_detail(),
            None,
            "a stale record must not leak out of {stale_mode:?}"
        );
    }
}

/// HYPOTHESIS (issue #2563): a spawned child's death — a closed stdout
/// pipe — carries no typed cause, and the accessor answers `None`
/// rather than a fabricated record. Falsified if the death path invents
/// a cause.
#[tokio::test]
async fn a_spawned_childs_death_surfaces_no_fabricated_cause() {
    let state = bootstrapping();
    // Address announced (Ready), then stdout closes: the child exited.
    drive_state(
        b"SOCKS5_ADDR=127.0.0.1:43210\n".as_slice(),
        Arc::clone(&state),
        test_publisher(),
        None,
    )
    .await;
    let proxy = proxy_over(&state);
    assert_eq!(proxy.mode(), Indicator::Died);
    assert_eq!(
        proxy.death_detail(),
        None,
        "a closed stdout pipe carries no cause; nothing may be fabricated"
    );
}
