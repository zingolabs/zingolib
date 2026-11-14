use std::net::{IpAddr, Ipv4Addr};

use super::*;

/// A runtime moved to a plain thread may still be blocked on and dropped,
/// which is what makes the spawned teardown thread legal.
#[test]
fn a_runtime_handed_to_a_plain_thread_still_blocks_and_drops() {
    let runtime = teardown_runtime();
    let moved = thread::spawn(move || {
        runtime.block_on(async { tokio::task::yield_now().await });
    });
    moved.join().expect("teardown thread joins");
}

#[test]
fn endpoint_round_trips_the_listener_address_nym_proxy_binds() {
    // NymProxy always binds IPv4 loopback; the derivation must preserve
    // both parts exactly through SocketAddr and back.
    let bound = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 43210);
    let endpoint = endpoint_from_listener_addr(bound);
    assert_eq!(endpoint.host, "127.0.0.1");
    assert_eq!(endpoint.port, 43210);
    let rebuilt: SocketAddr = format!("{}:{}", endpoint.host, endpoint.port)
        .parse()
        .unwrap();
    assert_eq!(rebuilt, bound);
}

use std::sync::Arc;

/// A local stand-in for the nym client's SOCKS5 listener: answers each
/// greeting with the given method-selection reply until dropped.
async fn mock_socks5_listener(reply: [u8; 2]) -> (Socks5Endpoint, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock listener");
    let endpoint =
        endpoint_from_listener_addr(listener.local_addr().expect("mock listener address"));
    let serving = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut greeting = [0u8; 3];
            if stream.read_exact(&mut greeting).await.is_ok() {
                let _ = stream.write_all(&reply).await;
            }
        }
    });
    (endpoint, serving)
}

use zl_netutils::time::test::MONITOR_CHECK_TIMEOUT;

#[tokio::test]
async fn check_passes_a_listener_that_completes_the_handshake() {
    let (endpoint, serving) = mock_socks5_listener([0x05, 0x00]).await;
    socks5_handshake_check(&endpoint, MONITOR_CHECK_TIMEOUT)
        .await
        .expect("a completed handshake must pass the check");
    serving.abort();
}

#[tokio::test]
async fn check_fails_a_listener_that_rejects_the_method() {
    // 0xff is SOCKS5 for "no acceptable method" — the listener is alive
    // but no longer serving, which must read as dead.
    let (endpoint, serving) = mock_socks5_listener([0x05, 0xff]).await;
    socks5_handshake_check(&endpoint, MONITOR_CHECK_TIMEOUT)
        .await
        .expect_err("a refusing listener must fail the check");
    serving.abort();
}

#[tokio::test]
async fn check_fails_when_nothing_listens() {
    let (endpoint, serving) = mock_socks5_listener([0x05, 0x00]).await;
    serving.abort();
    // The port is now free again; connecting must be refused.
    tokio::time::sleep(Duration::from_millis(50)).await;
    socks5_handshake_check(&endpoint, MONITOR_CHECK_TIMEOUT)
        .await
        .expect_err("a dead listener must fail the check");
}

/// Tests that the check fails with the typed method-selection variant,
/// both reply bytes intact, when the listener refuses the method.
#[tokio::test]
async fn check_names_the_refused_method_selection() {
    let (endpoint, serving) = mock_socks5_listener([0x05, 0xff]).await;
    let failure = socks5_handshake_check(&endpoint, MONITOR_CHECK_TIMEOUT)
        .await
        .expect_err("a refusing listener must fail the check");
    assert!(
        matches!(
            failure,
            super::HandshakeCheckError::MethodSelection {
                reply: [0x05, 0xff]
            }
        ),
        "the reply bytes travel typed: {failure:?}"
    );
    serving.abort();
}

/// Tests that each probe failure maps to its own death variant with its
/// payload intact.
#[test]
fn every_probe_failure_maps_to_its_own_death_variant() {
    let refused = death_reason(HandshakeCheckError::MethodSelection {
        reply: [0x05, 0xff],
    });
    assert_eq!(
        refused,
        ProxyDeathReason::MethodSelectionRefused {
            version: 0x05,
            method: 0xff,
        }
    );
    assert_eq!(
        death_reason(HandshakeCheckError::TimedOut {
            budget: Duration::from_millis(250),
        }),
        ProxyDeathReason::CheckTimedOut { budget_millis: 250 }
    );
    let io = || std::io::Error::other("boom");
    assert!(matches!(
        death_reason(HandshakeCheckError::Connect(io())),
        ProxyDeathReason::ListenerRefused { .. }
    ));
    assert!(matches!(
        death_reason(HandshakeCheckError::GreetingWrite(io())),
        ProxyDeathReason::GreetingUnwritable { .. }
    ));
    assert!(matches!(
        death_reason(HandshakeCheckError::MethodSelectionRead(io())),
        ProxyDeathReason::MethodSelectionUnreadable { .. }
    ));
}

#[derive(Default)]
struct RecordingObserver {
    deaths: Mutex<Vec<ProxyDeathReason>>,
}

impl ProxyDeathObserver for Arc<RecordingObserver> {
    fn on_death(&self, reason: ProxyDeathReason) {
        self.deaths.lock().unwrap().push(reason);
    }
}

use zl_netutils::time::test::MONITOR_CHECK_INTERVAL;

#[tokio::test]
async fn monitor_reports_death_exactly_once_after_the_listener_dies() {
    let (endpoint, serving) = mock_socks5_listener([0x05, 0x00]).await;
    let observer = Arc::new(RecordingObserver::default());
    let monitor = tokio::spawn(monitor_listener(
        endpoint,
        Box::new(Arc::clone(&observer)),
        not_stopping(),
        MONITOR_CHECK_INTERVAL,
        MONITOR_CHECK_TIMEOUT,
        LISTENER_MONITOR_STRIKES,
    ));
    // Several healthy intervals pass without a death report.
    tokio::time::sleep(MONITOR_CHECK_INTERVAL * 4).await;
    assert!(observer.deaths.lock().unwrap().is_empty());

    serving.abort();
    // The monitor ends itself after reporting; that is the at-most-once
    // guarantee, and joining it proves the report happened.
    tokio::time::timeout(MOCK_OP_BOUND, monitor)
        .await
        .expect("monitor must report within the strike window")
        .expect("monitor task must end cleanly");
    let deaths = observer.deaths.lock().unwrap();
    assert_eq!(deaths.len(), 1, "death must be reported exactly once");
    // The port is free once the mock stops serving, so the probe's connect
    // is refused and that refusal must survive typed to the observer.
    assert!(matches!(
        deaths[0],
        ProxyDeathReason::ListenerRefused { .. }
    ));
}

/// Tests that the death report reaches the host on a thread where
/// blocking is legal.
#[tokio::test]
async fn death_reaches_the_host_on_a_thread_that_may_block() {
    struct BlockingObserver(std::sync::mpsc::Sender<()>);
    impl ProxyDeathObserver for BlockingObserver {
        fn on_death(&self, _reason: ProxyDeathReason) {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build probe runtime")
                .block_on(async {});
            self.0.send(()).expect("report the blocking success");
        }
    }
    let (endpoint, serving) = mock_socks5_listener([0x05, 0x00]).await;
    serving.abort();
    let (blocked_tx, blocked_rx) = std::sync::mpsc::channel();
    tokio::spawn(monitor_listener(
        endpoint,
        Box::new(BlockingObserver(blocked_tx)),
        not_stopping(),
        MONITOR_CHECK_INTERVAL,
        MONITOR_CHECK_TIMEOUT,
        1,
    ));
    tokio::task::spawn_blocking(move || blocked_rx.recv_timeout(MOCK_OP_BOUND))
        .await
        .expect("join the wait")
        .expect("the callback must complete without panicking");
}

use zl_netutils::time::test::MOCK_OP_BOUND;

/// Teardown bounds short enough for a test to wait out.
const QUICK_BOUNDS: TeardownBounds = TeardownBounds {
    disconnect: Duration::from_millis(100),
    resume_grace: Duration::from_millis(100),
    disposal: Duration::from_millis(100),
};

/// Builds the multi-thread runtime the teardown tests hand their handles.
fn teardown_runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build runtime")
}

/// A handle over an empty proxy slot, owning the given runtime.
fn teardown_handle(runtime: Runtime) -> MixnetProxyHandle {
    MixnetProxyHandle {
        proxy: Mutex::new(None),
        monitor: None,
        endpoint: Socks5Endpoint {
            host: "127.0.0.1".to_string(),
            port: 1,
        },
        runtime: Mutex::new(Some(runtime)),
        stopping: Arc::new(atomic::AtomicBool::new(false)),
    }
}

/// A stopping flag no one ever raises.
fn not_stopping() -> Arc<atomic::AtomicBool> {
    Arc::new(atomic::AtomicBool::new(false))
}

/// The bound the drop tests observe under, strictly inside the disposal
/// grace so an inline-teardown regression reds reliably.
const DROP_OBSERVATION_BOUND: Duration = Duration::from_millis(2500);

/// Drops the last reference from a task the given spawner runs, waiting
/// for that drop to complete without a panic.
fn release_last_reference_from(spawner: &tokio::runtime::Handle, handle: Arc<MixnetProxyHandle>) {
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
    let held_by_task = Arc::clone(&handle);
    spawner.spawn(async move {
        release_rx.await.expect("release signal");
        drop(held_by_task);
        let _ = dropped_tx.send(());
    });
    drop(handle);
    release_tx.send(()).expect("the drop task is waiting");
    dropped_rx
        .recv_timeout(MOCK_OP_BOUND)
        .expect("the task's drop must complete without panicking");
}

/// Tests that teardown completes without a panic when a task on the
/// handle's own runtime drops the last reference.
#[test]
fn last_reference_may_drop_on_the_handles_own_runtime() {
    let handle = Arc::new(teardown_handle(teardown_runtime()));
    let spawner = handle
        .runtime
        .lock()
        .unwrap()
        .as_ref()
        .expect("a live handle owns its runtime")
        .handle()
        .clone();
    release_last_reference_from(&spawner, handle);
}

/// Tests that teardown completes when the observer blocks on the
/// handle's runtime, stops it, and releases the last reference inside
/// the death callback.
#[test]
fn observer_may_stop_and_release_the_handle_inside_the_callback() {
    struct TearingObserver {
        handle: Mutex<Option<Arc<MixnetProxyHandle>>>,
        torn_down: std::sync::mpsc::Sender<()>,
    }
    impl ProxyDeathObserver for TearingObserver {
        fn on_death(&self, _reason: ProxyDeathReason) {
            let handle = self
                .handle
                .lock()
                .unwrap()
                .take()
                .expect("the observer holds the last reference");
            handle
                .runtime
                .lock()
                .unwrap()
                .as_ref()
                .expect("a live handle owns its runtime")
                .block_on(async {});
            handle.stop();
            drop(handle);
            self.torn_down.send(()).expect("report the teardown");
        }
    }
    let handle = Arc::new(teardown_handle(teardown_runtime()));
    let (torn_down_tx, torn_down_rx) = std::sync::mpsc::channel();
    let observer = TearingObserver {
        handle: Mutex::new(Some(Arc::clone(&handle))),
        torn_down: torn_down_tx,
    };
    handle
        .runtime
        .lock()
        .unwrap()
        .as_ref()
        .expect("a live handle owns its runtime")
        .spawn(monitor_listener(
            handle.endpoint.clone(),
            Box::new(observer),
            not_stopping(),
            MONITOR_CHECK_INTERVAL,
            MONITOR_CHECK_TIMEOUT,
            1,
        ));
    drop(handle);
    torn_down_rx
        .recv_timeout(MOCK_OP_BOUND)
        .expect("the callback must tear the handle down without panicking");
}

/// Tests that teardown completes without a panic when a task on another
/// runtime drops the last reference.
#[test]
fn last_reference_may_drop_on_a_foreign_runtime() {
    let foreign = teardown_runtime();
    let handle = Arc::new(teardown_handle(teardown_runtime()));
    release_last_reference_from(foreign.handle(), handle);
}

/// Tests that a drop leads to every task's destruction within the mock
/// bound, now that teardown runs on a thread of its own.
#[test]
fn a_drop_destroys_every_task_within_the_bound() {
    let (down_tx, down_rx) = std::sync::mpsc::channel::<()>();
    let runtime = teardown_runtime();
    runtime.spawn(async move {
        let _held = down_tx;
        std::future::pending::<()>().await
    });
    drop(teardown_handle(runtime));
    // The task owns the sender, polled or not, so only its destruction
    // disconnects the channel.
    assert_eq!(
        down_rx.recv_timeout(DROP_OBSERVATION_BOUND),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected),
        "the teardown thread must destroy the task within the bound"
    );
}

/// Tests that the monitor ends cleanly when the observer panics.
#[tokio::test]
async fn a_panicking_observer_does_not_poison_the_monitor() {
    struct PanickingObserver;
    impl ProxyDeathObserver for PanickingObserver {
        fn on_death(&self, _reason: ProxyDeathReason) {
            panic!("host bug");
        }
    }
    let (endpoint, serving) = mock_socks5_listener([0x05, 0x00]).await;
    serving.abort();
    let monitor = tokio::spawn(monitor_listener(
        endpoint,
        Box::new(PanickingObserver),
        not_stopping(),
        MONITOR_CHECK_INTERVAL,
        MONITOR_CHECK_TIMEOUT,
        1,
    ));
    tokio::time::timeout(MOCK_OP_BOUND, monitor)
        .await
        .expect("monitor must end within the strike window")
        .expect("monitor task must end cleanly despite the panic");
}

/// Tests that a drop from a plain thread returns while a blocking task
/// still runs, instead of disposing the runtime inline.
#[test]
fn a_drop_from_a_plain_thread_returns_while_a_blocking_task_runs() {
    let runtime = teardown_runtime();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    runtime.spawn_blocking(move || {
        started_tx.send(()).expect("report the blocking start");
        let _ = release_rx.recv();
    });
    started_rx
        .recv_timeout(MOCK_OP_BOUND)
        .expect("the blocking task starts");
    let handle = teardown_handle(runtime);
    let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        drop(handle);
        let _ = dropped_tx.send(());
    });
    let outcome = dropped_rx.recv_timeout(DROP_OBSERVATION_BOUND);
    release_tx.send(()).expect("the blocking task is waiting");
    outcome.expect("drop must return while the blocking task still runs");
}

/// Tests that teardown disposes the runtime after the grace period even
/// when a blocking task never finishes.
#[test]
fn teardown_abandons_a_blocking_task_after_the_grace_period() {
    let runtime = teardown_runtime();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    runtime.spawn_blocking(move || {
        started_tx.send(()).expect("report the blocking start");
        let _ = release_rx.recv();
    });
    started_rx
        .recv_timeout(MOCK_OP_BOUND)
        .expect("the blocking task starts");
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        teardown(runtime, None, QUICK_BOUNDS);
        let _ = done_tx.send(());
    });
    let outcome = done_rx.recv_timeout(MOCK_OP_BOUND);
    release_tx.send(()).expect("the blocking task is waiting");
    outcome.expect("teardown must abandon a blocking task it cannot end");
}

/// Tests that a peer's close reaches `read` as zero bytes while
/// `peer_addr` still answers, which is why the live suite reads the
/// socket to observe teardown.
#[test]
fn a_peers_close_shows_in_read_and_not_in_peer_addr() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let mut client =
        std::net::TcpStream::connect(listener.local_addr().expect("addr")).expect("connect");
    let (accepted, _) = listener.accept().expect("accept");
    drop(accepted);
    client
        .set_read_timeout(Some(MOCK_OP_BOUND))
        .expect("read timeout");
    let mut byte = [0u8; 1];
    assert_eq!(client.read(&mut byte).expect("read"), 0);
    assert!(
        client.peer_addr().is_ok(),
        "peer_addr keeps answering on a half-closed socket"
    );
}

/// Tests that a blocking-pool thread sits inside the runtime context, so
/// no predicate on `Handle::try_current` finds every thread that may
/// block, which is why teardown always takes a thread of its own.
#[test]
fn a_blocking_pool_thread_sits_inside_the_runtime_context() {
    let runtime = teardown_runtime();
    let on_blocking_pool = runtime.block_on(async {
        tokio::task::spawn_blocking(|| tokio::runtime::Handle::try_current().is_ok())
            .await
            .expect("join the blocking probe")
    });
    assert!(
        on_blocking_pool,
        "a blocking-pool thread answers Ok from try_current"
    );
}

/// A stand-in client whose disconnect waits for the test's release
/// signal, then reports completion, or dies at once when scripted to.
pub(crate) struct ScriptedProxy {
    released: tokio::sync::oneshot::Receiver<()>,
    finished: std::sync::mpsc::Sender<()>,
    dies: bool,
}

impl ScriptedProxy {
    /// A fixed identity standing in for a live Exit Node.
    pub(crate) fn exit_node(&self) -> String {
        "scripted-exit-node".to_string()
    }

    /// Waits for the release signal, then reports the disconnect done,
    /// unless scripted to die first.
    pub(crate) async fn disconnect(self) {
        if self.dies {
            panic!("scripted disconnect death");
        }
        let _ = self.released.await;
        let _ = self.finished.send(());
    }
}

/// A scripted client whose disconnect dies as soon as it is polled.
fn dying_scripted_proxy() -> ScriptedProxy {
    let (proxy, _release_tx, _finished_rx) = scripted_proxy();
    ScriptedProxy {
        dies: true,
        ..proxy
    }
}

/// Builds a scripted client plus its release trigger and its completion
/// signal.
fn scripted_proxy() -> (
    ScriptedProxy,
    tokio::sync::oneshot::Sender<()>,
    std::sync::mpsc::Receiver<()>,
) {
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    (
        ScriptedProxy {
            released: release_rx,
            finished: finished_tx,
            dies: false,
        },
        release_tx,
        finished_rx,
    )
}

/// A handle whose proxy slot holds the given scripted client.
fn scripted_handle(runtime: Runtime, proxy: ScriptedProxy) -> MixnetProxyHandle {
    let handle = teardown_handle(runtime);
    *handle.proxy.lock().unwrap() = Some(ProxySlot::Scripted(proxy));
    handle
}

/// Tests that stop returns while the ordered disconnect is still
/// running, instead of blocking the caller's thread on it.
#[test]
fn stop_returns_while_the_disconnect_is_still_running() {
    let (proxy, release_tx, finished_rx) = scripted_proxy();
    let handle = Arc::new(scripted_handle(teardown_runtime(), proxy));
    let stopper = Arc::clone(&handle);
    let (stopped_tx, stopped_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        stopper.stop();
        let _ = stopped_tx.send(());
    });
    let outcome = stopped_rx.recv_timeout(MOCK_OP_BOUND);
    release_tx.send(()).expect("the disconnect is waiting");
    outcome.expect("stop must return while the disconnect still runs");
    finished_rx
        .recv_timeout(MOCK_OP_BOUND)
        .expect("the ordered disconnect still runs to completion");
}

/// Tests that a teardown whose disconnect never finishes still ends its
/// thread within the disconnect bound.
#[test]
fn teardown_leaks_rather_than_wait_out_a_disconnect_that_never_ends() {
    let (proxy, _release_tx, _finished_rx) = scripted_proxy();
    let runtime = teardown_runtime();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        teardown(runtime, Some(ProxySlot::Scripted(proxy)), QUICK_BOUNDS);
        let _ = done_tx.send(());
    });
    done_rx
        .recv_timeout(MOCK_OP_BOUND)
        .expect("teardown must end despite a disconnect that never finishes");
}

/// Tests that a disconnect task that dies leaves the runtime leaked,
/// never dropped under whatever the task left behind.
#[test]
fn a_dead_disconnect_task_leaks_the_runtime_rather_than_drop_it() {
    let (down_tx, down_rx) = std::sync::mpsc::channel::<()>();
    let runtime = teardown_runtime();
    runtime.spawn(async move {
        let _held = down_tx;
        std::future::pending::<()>().await
    });
    teardown(
        runtime,
        Some(ProxySlot::Scripted(dying_scripted_proxy())),
        TeardownBounds {
            disconnect: MOCK_OP_BOUND,
            ..QUICK_BOUNDS
        },
    );
    // A dropped runtime destroys the task and disconnects the channel.
    // A leaked one keeps it alive.
    assert_eq!(
        down_rx.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty),
        "a dead disconnect must leak the runtime, not drop it"
    );
}

/// Tests that the shim workspace itself serializes the live suite, so
/// any nextest invocation, local or CI, runs one mixnet client at a time.
#[test]
fn the_shim_workspace_serializes_its_live_suite_itself() {
    let config = include_str!("../../.config/nextest.toml");
    assert!(
        config.contains("binary(live_mixnet)") && config.contains("max-threads = 1"),
        "the live tests must not start two mixnet clients at once: {config}"
    );
}

/// Tests that a leaked runtime is counted, so the accumulation is
/// visible and start's refusal cap has something to read.
#[test]
fn a_missed_disconnect_bound_is_counted() {
    let (proxy, _release_tx, _finished_rx) = scripted_proxy();
    let before = LEAKED_TEARDOWNS.load(atomic::Ordering::SeqCst);
    teardown(
        teardown_runtime(),
        Some(ProxySlot::Scripted(proxy)),
        QUICK_BOUNDS,
    );
    assert!(
        LEAKED_TEARDOWNS.load(atomic::Ordering::SeqCst) > before,
        "a leaked runtime must be counted"
    );
}

/// Tests that a disconnect whose first bound was spent before it was
/// ever polled still runs to completion, the app resumed from suspension.
#[test]
fn a_disconnect_resumed_after_its_bound_finishes_instead_of_leaking() {
    let (proxy, release_tx, finished_rx) = scripted_proxy();
    // The released signal arrives while the resume grace runs, so the
    // first bound is guaranteed to elapse on an unpolled disconnect.
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        let _ = release_tx.send(());
    });
    let started = Instant::now();
    teardown(
        teardown_runtime(),
        Some(ProxySlot::Scripted(proxy)),
        TeardownBounds {
            disconnect: Duration::ZERO,
            resume_grace: MOCK_OP_BOUND,
            ..QUICK_BOUNDS
        },
    );
    finished_rx
        .try_recv()
        .expect("the resumed disconnect must run to completion");
    // Returning inside the grace is what tells a completed disconnect
    // apart from one the grace gave up on.
    assert!(
        started.elapsed() < MOCK_OP_BOUND,
        "a resumed disconnect must end inside its grace, not leak at the edge"
    );
}

/// Tests that a teardown which unwinds still leaves start's fence clear.
#[test]
fn an_unwound_teardown_leaves_no_teardown_in_flight() {
    static FENCE: atomic::AtomicUsize = atomic::AtomicUsize::new(0);
    let unwound = std::panic::catch_unwind(|| {
        let _in_flight = TeardownInFlight::enter(&FENCE);
        panic!("a teardown that unwinds");
    });
    assert!(unwound.is_err(), "the teardown must have unwound");
    assert_eq!(
        FENCE.load(atomic::Ordering::SeqCst),
        0,
        "an unwound teardown must not strand start's fence"
    );
}

/// Tests that start refuses to build a runtime once the process has
/// leaked its cap, instead of stacking leaks until the OS kills the app.
#[test]
fn start_refuses_once_leaks_reach_the_cap() {
    LEAKED_TEARDOWNS.fetch_add(MAX_LEAKED_TEARDOWNS, atomic::Ordering::SeqCst);
    let refused = MixnetProxyHandle::start(None);
    // Restore the process-global counter so this test cannot cap any
    // test that runs after it in the same process.
    LEAKED_TEARDOWNS.fetch_sub(MAX_LEAKED_TEARDOWNS, atomic::Ordering::SeqCst);
    assert!(
        matches!(refused, Err(ProxyFfiError::Runtime { .. })),
        "a capped process must refuse a new proxy"
    );
}

/// Tests that an in-flight disconnect is visible to start's fence, so a
/// new client never bootstraps beside a disconnecting one.
#[test]
fn a_running_disconnect_is_visible_to_the_start_fence() {
    let (proxy, release_tx, finished_rx) = scripted_proxy();
    spawn_teardown(teardown_runtime(), Some(ProxySlot::Scripted(proxy)));
    assert!(
        ACTIVE_TEARDOWNS.load(atomic::Ordering::SeqCst) >= 1,
        "an in-flight disconnect must be visible to start's fence"
    );
    release_tx.send(()).expect("the disconnect is waiting");
    finished_rx
        .recv_timeout(MOCK_OP_BOUND)
        .expect("the released disconnect completes");
}

/// Tests that the bound the drop tests observe under sits strictly
/// inside the disposal grace, so an inline-teardown regression reds
/// reliably instead of racing the scheduler.
#[test]
fn the_drop_observation_bound_nests_inside_the_disposal_grace() {
    assert!(
        DROP_OBSERVATION_BOUND < RUNTIME_DISPOSAL_GRACE,
        "equal bounds make an inline regression a scheduler coin flip"
    );
}

/// Tests that a death report dispatched into the blocking pool before a
/// deliberate stop never reaches the observer after it.
#[test]
fn a_deliberate_stop_silences_an_already_dispatched_death_report() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("build runtime");
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    runtime.spawn_blocking(move || {
        started_tx.send(()).expect("report the blocking start");
        let _ = release_rx.recv();
    });
    started_rx
        .recv_timeout(MOCK_OP_BOUND)
        .expect("the occupier starts");
    let observer = Arc::new(RecordingObserver::default());
    let handle = teardown_handle(runtime);
    handle
        .runtime
        .lock()
        .unwrap()
        .as_ref()
        .expect("a live handle owns its runtime")
        .spawn(monitor_listener(
            handle.endpoint.clone(),
            Box::new(Arc::clone(&observer)),
            Arc::clone(&handle.stopping),
            MONITOR_CHECK_INTERVAL,
            MONITOR_CHECK_TIMEOUT,
            1,
        ));
    // The endpoint is a dead port, so the monitor strikes on its first
    // check and its report queues behind the occupied blocking slot.
    thread::sleep(Duration::from_millis(400));
    handle.stop();
    release_tx.send(()).expect("the occupier is waiting");
    thread::sleep(Duration::from_millis(300));
    assert!(
        observer.deaths.lock().unwrap().is_empty(),
        "a deliberate stop must silence the dispatched report"
    );
}

/// Tests that the manifest itself enables tokio's `sync` feature for the
/// tests that call `tokio::sync::oneshot`, instead of borrowing it from
/// feature unification with the nym stack.
#[test]
fn the_manifest_declares_the_tokio_sync_feature_the_tests_use() {
    let manifest = include_str!("../Cargo.toml");
    let dev_dependencies = manifest
        .split("[dev-dependencies]")
        .nth(1)
        .expect("the manifest has a dev-dependencies table");
    let tokio_line = dev_dependencies
        .lines()
        .find(|line| line.trim_start().starts_with("tokio"))
        .expect("the dev-dependencies declare tokio");
    assert!(
        tokio_line.contains("\"sync\""),
        "tokio::sync compiles only through feature unification: {tokio_line}"
    );
}
