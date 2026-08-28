//! The Nym mixnet proxy as a UniFFI component for mobile (ADR 0011 mobile
//! amendment, CP-3).
//!
//! iOS cannot spawn the `nym-proxy` child process the desktop model relies on,
//! and nym-sdk cannot link into the wallet's compile unit (the crypto-common
//! conflict). So on mobile the proxy is a dynamic library the app hosts: it
//! brings up a [`NymProxy`], exposes its local SOCKS5 address, and the app
//! hands that address to the wallet's `attach_mixnet` seam (CP-2). This crate
//! is that library's UniFFI surface — `start`, `stop`, and a death callback —
//! built from the standalone netutils workspace so nym-sdk resolves in its own
//! lockfile.
//!
//! The boundary is UniFFI rather than a hand-written C ABI specifically to
//! preserve `#![forbid(unsafe_code)]`: uniffi's proc-macros generate the FFI
//! `unsafe` scaffolding with macro hygiene that the lint does not fire on,
//! while any hand-written `unsafe` in this crate would still be rejected. So
//! this shim carries zero hand-written unsafe and keeps the workspace-wide
//! safety invariant intact — see the ADR amendment for the empirical check.
#![forbid(unsafe_code)]

use std::{
    mem,
    net::SocketAddr,
    sync::{Arc, Mutex, atomic},
    thread,
    time::{Duration, Instant},
};

use tokio::runtime::Runtime;
use zingo_netutils::NymProxy;
use zingo_netutils::time::{LISTENER_MONITOR_INTERVAL, LOOPBACK_DIAL_BOUND};

#[cfg(target_os = "android")]
mod debug_log;

/// Consecutive check failures required before the proxy is declared dead.
const LISTENER_MONITOR_STRIKES: u32 = 2;

uniffi::setup_scaffolding!();

/// The local SOCKS5 endpoint a running proxy listens on, as explicit parts
/// rather than a formatted `host:port` string, so the host language gets a
/// typed port and never re-parses.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Socks5Endpoint {
    /// The listener's IP address literal, e.g. `"127.0.0.1"`.
    pub host: String,
    /// The listener's TCP port.
    pub port: u16,
}

/// Why starting or driving the mixnet proxy failed, crossing the FFI as a
/// UniFFI error the host language can match on.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum ProxyFfiError {
    /// The tokio runtime that hosts the proxy could not be built.
    #[error("could not start the proxy runtime: {reason}")]
    Runtime {
        /// The underlying runtime-construction failure.
        reason: String,
    },
    /// The Nym mixnet client failed to connect.
    #[error("could not connect the mixnet proxy: {reason}")]
    Connect {
        /// The underlying NymProxy failure.
        reason: String,
    },
}

/// Why a running proxy was lost, named as the liveness probe observed it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ProxyDeathReason {
    /// The TCP connect to the listener failed.
    ListenerRefused {
        /// The underlying connect failure.
        detail: String,
    },
    /// The SOCKS5 greeting could not be written to the listener.
    GreetingUnwritable {
        /// The underlying write failure.
        detail: String,
    },
    /// The listener's method selection could not be read.
    MethodSelectionUnreadable {
        /// The underlying read failure.
        detail: String,
    },
    /// The listener answered a method selection other than no-auth.
    MethodSelectionRefused {
        /// The version byte the listener answered.
        version: u8,
        /// The method byte the listener answered.
        method: u8,
    },
    /// The liveness round trip missed its budget.
    CheckTimedOut {
        /// The budget the round trip missed, in milliseconds.
        budget_millis: u64,
    },
}

/// The host implements this to learn — at most once per proxy — that the
/// proxy died after it was running, so the app can redraw a fresh path and
/// re-attach under the proxy-owner-remediates contract.
#[uniffi::export(callback_interface)]
pub trait ProxyDeathObserver: Send + Sync {
    /// Called at most once when the running proxy is lost, after which the
    /// previously reported endpoint is dead.
    fn on_death(&self, reason: ProxyDeathReason);
}

/// A running mixnet proxy the mobile host owns, holding the tokio runtime
/// that keeps the [`NymProxy`] client and its SOCKS5 listener alive until
/// [`Self::stop`] or a drop of the handle tears both down.
#[derive(uniffi::Object)]
pub struct MixnetProxyHandle {
    proxy: Mutex<Option<ProxySlot>>,
    monitor: Option<tokio::task::AbortHandle>,
    endpoint: Socks5Endpoint,
    runtime: Mutex<Option<Runtime>>,
    stopping: Arc<atomic::AtomicBool>,
}

/// The client a handle tears down: the live one in production, a scripted
/// stand-in under test.
#[cfg_attr(test, expect(clippy::large_enum_variant))]
enum ProxySlot {
    Live(NymProxy),
    #[cfg(test)]
    Scripted(tests::ScriptedProxy),
}

impl ProxySlot {
    /// The Exit Node identity the slot's client bound.
    fn exit_node(&self) -> String {
        match self {
            ProxySlot::Live(proxy) => proxy.exit_node().to_string(),
            #[cfg(test)]
            ProxySlot::Scripted(proxy) => proxy.exit_node(),
        }
    }

    /// Runs the client's ordered disconnect.
    async fn disconnect(self) {
        match self {
            ProxySlot::Live(proxy) => proxy.disconnect().await,
            #[cfg(test)]
            ProxySlot::Scripted(proxy) => proxy.disconnect().await,
        }
    }
}

/// The bound a teardown's ordered disconnect gets before the runtime is
/// leaked, still running, instead of dropped.
pub const DISCONNECT_BOUND: Duration = Duration::from_secs(60);

/// How long a finished disconnect's runtime may wait for its last blocking
/// task before disposal abandons it.
pub const RUNTIME_DISPOSAL_GRACE: Duration = Duration::from_secs(5);

/// Runtimes this process has leaked because their teardown could not end.
static LEAKED_TEARDOWNS: atomic::AtomicUsize = atomic::AtomicUsize::new(0);

/// Leaked runtimes beyond which [`MixnetProxyHandle::start`] refuses to
/// build another.
const MAX_LEAKED_TEARDOWNS: usize = 4;

/// Teardowns whose disconnect is still in flight.
static ACTIVE_TEARDOWNS: atomic::AtomicUsize = atomic::AtomicUsize::new(0);

/// Counts and reports a runtime the teardown cannot dispose, then leaks it.
fn leak_runtime(runtime: Runtime, why: &str) {
    let leaks = LEAKED_TEARDOWNS.fetch_add(1, atomic::Ordering::SeqCst) + 1;
    tracing::error!(leaks, why, "leaking a mixnet runtime");
    mem::forget(runtime);
}

/// Runs the ordered disconnect under `disconnect_bound`, then disposes the
/// runtime within `disposal_grace`, leaking the runtime instead when the
/// disconnect does not finish cleanly.
fn teardown(
    runtime: Runtime,
    proxy: Option<ProxySlot>,
    disconnect_bound: Duration,
    disposal_grace: Duration,
) {
    if let Some(proxy) = proxy {
        let disconnect = runtime.spawn(proxy.disconnect());
        let finished =
            runtime.block_on(async { tokio::time::timeout(disconnect_bound, disconnect).await });
        match finished {
            Ok(Ok(())) => {}
            Ok(Err(died)) => {
                // The dead task's client state is unknown, and dropping the
                // runtime under it can abort (nymtech/nym#7108).
                tracing::error!(error = %died, "mixnet disconnect task died");
                leak_runtime(runtime, "disconnect task died");
                return;
            }
            Err(_) => {
                // Cancelling a late disconnect aborts (nymtech/nym#7108).
                leak_runtime(runtime, "disconnect missed its bound");
                return;
            }
        }
    }
    runtime.shutdown_timeout(disposal_grace);
}

/// Hands the runtime and client to a detached teardown thread, falling back
/// without an abort when the OS refuses the thread.
fn spawn_teardown(runtime: Runtime, proxy: Option<ProxySlot>) {
    ACTIVE_TEARDOWNS.fetch_add(1, atomic::Ordering::SeqCst);
    let payload = Arc::new(Mutex::new(Some((runtime, proxy))));
    let for_thread = Arc::clone(&payload);
    let spawned = thread::Builder::new()
        .name("nym-teardown".to_string())
        .spawn(move || {
            if let Some((runtime, proxy)) = take_teardown_payload(&for_thread) {
                teardown(runtime, proxy, DISCONNECT_BOUND, RUNTIME_DISPOSAL_GRACE);
            }
            ACTIVE_TEARDOWNS.fetch_sub(1, atomic::Ordering::SeqCst);
        });
    if spawned.is_err() {
        reclaim_unspawned_teardown(&payload);
        ACTIVE_TEARDOWNS.fetch_sub(1, atomic::Ordering::SeqCst);
    }
}

/// Waits, bounded by the teardown bounds, until no teardown thread is still
/// working, which fences a new client from every bounded teardown but not
/// from a leaked runtime.
fn await_teardowns_in_flight() {
    let deadline = Instant::now() + DISCONNECT_BOUND + RUNTIME_DISPOSAL_GRACE;
    while ACTIVE_TEARDOWNS.load(atomic::Ordering::SeqCst) > 0 {
        if Instant::now() >= deadline {
            tracing::warn!("starting a mixnet client beside an unfinished teardown");
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Takes the runtime and client out of a shared teardown payload.
fn take_teardown_payload(
    payload: &Mutex<Option<(Runtime, Option<ProxySlot>)>>,
) -> Option<(Runtime, Option<ProxySlot>)> {
    payload
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Completes a teardown whose thread the OS refused: inline where this
/// thread may block, by a deliberate leak where it may not.
fn reclaim_unspawned_teardown(payload: &Mutex<Option<(Runtime, Option<ProxySlot>)>>) {
    let Some((runtime, proxy)) = take_teardown_payload(payload) else {
        return;
    };
    if tokio::runtime::Handle::try_current().is_err() {
        teardown(runtime, proxy, DISCONNECT_BOUND, RUNTIME_DISPOSAL_GRACE);
    } else {
        // Dropping the runtime on a runtime-context thread panics, and
        // dropping the client raw aborts (nymtech/nym#7108). Leaking both is
        // the survivable arm.
        leak_runtime(
            runtime,
            "no thread for a teardown on a runtime-context thread",
        );
        mem::forget(proxy);
    }
}

/// Tears down through [`Self::stop`], so a dropped handle and a stopped one
/// take one path.
impl Drop for MixnetProxyHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Why one SOCKS5 method-selection round trip failed.
#[derive(Debug, thiserror::Error)]
enum HandshakeCheckError {
    /// The TCP connect to the listener failed.
    #[error("connect: {0}")]
    Connect(std::io::Error),
    /// The greeting write failed.
    #[error("greeting write: {0}")]
    GreetingWrite(std::io::Error),
    /// The method-selection read failed.
    #[error("method-selection read: {0}")]
    MethodSelectionRead(std::io::Error),
    /// The listener answered a method selection other than no-auth.
    #[error("unexpected method selection {reply:?}")]
    MethodSelection {
        /// The two reply bytes the listener answered.
        reply: [u8; 2],
    },
    /// The whole round trip missed its budget.
    #[error("check timed out after {}ms", budget.as_millis())]
    TimedOut {
        /// The budget the round trip missed.
        budget: Duration,
    },
}

/// One purely local SOCKS5 method-selection round trip against the listener,
/// answered `[0x05, 0x00]` when the no-auth method is accepted.
async fn socks5_handshake_check(
    endpoint: &Socks5Endpoint,
    check_timeout: Duration,
) -> Result<(), HandshakeCheckError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let attempt = async {
        let mut stream = tokio::net::TcpStream::connect((endpoint.host.as_str(), endpoint.port))
            .await
            .map_err(HandshakeCheckError::Connect)?;
        stream
            .write_all(&[0x05, 0x01, 0x00])
            .await
            .map_err(HandshakeCheckError::GreetingWrite)?;
        let mut reply = [0u8; 2];
        stream
            .read_exact(&mut reply)
            .await
            .map_err(HandshakeCheckError::MethodSelectionRead)?;
        if reply != [0x05, 0x00] {
            return Err(HandshakeCheckError::MethodSelection { reply });
        }
        Ok(())
    };
    tokio::time::timeout(check_timeout, attempt)
        .await
        .map_err(|_| HandshakeCheckError::TimedOut {
            budget: check_timeout,
        })?
}

/// Carry a liveness-probe failure across the FFI as the typed death it caused.
fn death_reason(failure: HandshakeCheckError) -> ProxyDeathReason {
    match failure {
        HandshakeCheckError::Connect(e) => ProxyDeathReason::ListenerRefused {
            detail: e.to_string(),
        },
        HandshakeCheckError::GreetingWrite(e) => ProxyDeathReason::GreetingUnwritable {
            detail: e.to_string(),
        },
        HandshakeCheckError::MethodSelectionRead(e) => {
            ProxyDeathReason::MethodSelectionUnreadable {
                detail: e.to_string(),
            }
        }
        HandshakeCheckError::MethodSelection { reply } => {
            ProxyDeathReason::MethodSelectionRefused {
                version: reply[0],
                method: reply[1],
            }
        }
        HandshakeCheckError::TimedOut { budget } => ProxyDeathReason::CheckTimedOut {
            budget_millis: u64::try_from(budget.as_millis()).unwrap_or(u64::MAX),
        },
    }
}

/// Checks the listener until `strikes_allowed` consecutive failures, then
/// reports the death exactly once and ends.
async fn monitor_listener(
    endpoint: Socks5Endpoint,
    observer: Box<dyn ProxyDeathObserver>,
    stopping: Arc<atomic::AtomicBool>,
    interval: Duration,
    check_timeout: Duration,
    strikes_allowed: u32,
) {
    let mut strikes = 0;
    loop {
        tokio::time::sleep(interval).await;
        match socks5_handshake_check(&endpoint, check_timeout).await {
            Ok(()) => strikes = 0,
            Err(failure) => {
                strikes += 1;
                if strikes >= strikes_allowed {
                    // The abort in stop() cannot recall a report already
                    // dispatched into the blocking pool, so the closure
                    // itself honours the deliberate stop.
                    let _ = tokio::task::spawn_blocking(move || {
                        if !stopping.load(atomic::Ordering::SeqCst) {
                            observer.on_death(death_reason(failure))
                        }
                    })
                    .await;
                    return;
                }
            }
        }
    }
}

/// Split a typed `NymProxy` listener address into the explicit parts the FFI
/// surface offers.
fn endpoint_from_listener_addr(addr: SocketAddr) -> Socks5Endpoint {
    Socks5Endpoint {
        host: addr.ip().to_string(),
        port: addr.port(),
    }
}

#[uniffi::export]
impl MixnetProxyHandle {
    /// Bring up a mixnet proxy and return, once its SOCKS5 listener is up, a
    /// handle whose [`Self::socks5_endpoint`] the app hands to the wallet's
    /// `attach_mixnet`, with a listener monitor that reports through
    /// `observer`, at most once, if the proxy is lost.
    #[uniffi::constructor]
    pub fn start(
        observer: Option<Box<dyn ProxyDeathObserver>>,
    ) -> Result<std::sync::Arc<Self>, ProxyFfiError> {
        #[cfg(target_os = "android")]
        debug_log::init();
        await_teardowns_in_flight();
        // Read after the fence, so a teardown that leaked during the wait
        // counts against the cap.
        let leaks = LEAKED_TEARDOWNS.load(atomic::Ordering::SeqCst);
        if leaks >= MAX_LEAKED_TEARDOWNS {
            return Err(ProxyFfiError::Runtime {
                reason: format!("{leaks} mixnet runtimes leaked; the process must restart"),
            });
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| ProxyFfiError::Runtime {
                reason: e.to_string(),
            })?;
        let proxy = runtime
            .block_on(NymProxy::start())
            .inspect_err(|e| tracing::error!(error = %e, "mixnet proxy start failed"))
            .map_err(|e| ProxyFfiError::Connect {
                reason: e.to_string(),
            })?;
        let endpoint = endpoint_from_listener_addr(proxy.socks5_addr());
        let stopping = Arc::new(atomic::AtomicBool::new(false));
        let monitor = observer.map(|observer| {
            runtime
                .spawn(monitor_listener(
                    endpoint.clone(),
                    observer,
                    Arc::clone(&stopping),
                    LISTENER_MONITOR_INTERVAL,
                    LOOPBACK_DIAL_BOUND,
                    LISTENER_MONITOR_STRIKES,
                ))
                .abort_handle()
        });
        Ok(std::sync::Arc::new(MixnetProxyHandle {
            runtime: Mutex::new(Some(runtime)),
            proxy: Mutex::new(Some(ProxySlot::Live(proxy))),
            endpoint,
            monitor,
            stopping,
        }))
    }

    /// The local SOCKS5 endpoint the app hands to `attach_mixnet`.
    pub fn socks5_endpoint(&self) -> Socks5Endpoint {
        self.endpoint.clone()
    }

    /// The Exit Node identity the running proxy bound, `None` once stopped.
    pub fn exit_node(&self) -> Option<String> {
        self.proxy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|slot| slot.exit_node())
    }

    /// Begin the ordered teardown on a thread of its own, idempotently,
    /// after silencing the monitor and any death report it has already
    /// dispatched.
    pub fn stop(&self) {
        self.stopping.store(true, atomic::Ordering::SeqCst);
        if let Some(monitor) = &self.monitor {
            monitor.abort();
        }
        // Both takes happen under the proxy lock, so concurrent stops can
        // never split the pair and strand a live client without its runtime.
        let (proxy, runtime) = {
            let mut proxy = self
                .proxy
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let runtime = self
                .runtime
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            (proxy.take(), runtime)
        };
        if let Some(runtime) = runtime {
            spawn_teardown(runtime, proxy);
        }
    }
}

#[cfg(test)]
mod tests {
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

    use zingo_netutils::time::test::MONITOR_CHECK_TIMEOUT;

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

    use zingo_netutils::time::test::MONITOR_CHECK_INTERVAL;

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

    use zingo_netutils::time::test::MOCK_OP_BOUND;

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
    fn release_last_reference_from(
        spawner: &tokio::runtime::Handle,
        handle: Arc<MixnetProxyHandle>,
    ) {
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
            teardown(
                runtime,
                None,
                Duration::from_millis(100),
                Duration::from_millis(100),
            );
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
            teardown(
                runtime,
                Some(ProxySlot::Scripted(proxy)),
                Duration::from_millis(100),
                Duration::from_millis(100),
            );
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
            MOCK_OP_BOUND,
            Duration::from_millis(100),
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
        let config = include_str!("../.config/nextest.toml");
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
            Duration::from_millis(100),
            Duration::from_millis(100),
        );
        assert!(
            LEAKED_TEARDOWNS.load(atomic::Ordering::SeqCst) > before,
            "a leaked runtime must be counted"
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
}
