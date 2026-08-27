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

use std::{net::SocketAddr, sync::Mutex, time::Duration};

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
    proxy: Mutex<Option<NymProxy>>,
    monitor: Option<tokio::task::AbortHandle>,
    endpoint: Socks5Endpoint,
    runtime: Option<Runtime>,
}

/// Where a handle's teardown can run, decided by whether the dropping thread
/// may block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TeardownSite {
    /// The dropping thread may block, so teardown finishes before `drop`
    /// returns.
    Inline,
    /// The dropping thread is a runtime worker, so teardown moves to a thread
    /// of its own.
    OffRuntime,
}

/// Pure: a runtime worker may neither block on the disconnect nor drop the
/// runtime, so a drop that lands on one moves its teardown elsewhere.
fn teardown_site(dropping_on_runtime_thread: bool) -> TeardownSite {
    if dropping_on_runtime_thread {
        TeardownSite::OffRuntime
    } else {
        TeardownSite::Inline
    }
}

/// Disconnects the mixnet client in order, then drops the runtime that hosted
/// it.
fn teardown(runtime: Runtime, proxy: Option<NymProxy>) {
    // The disconnect carries no deadline on purpose. Cancelling it drops the
    // runtime with SOCKS5 connections still open, and `SocksClient::drop`
    // aborts the process when it reports a closing connection to a controller
    // the same cancellation already took down (nymtech/nym#7108). A teardown
    // that hangs leaks one thread. A teardown that gives up kills the app.
    if let Some(proxy) = proxy {
        runtime.block_on(proxy.disconnect());
    }
}

/// Tears down safely on whatever thread drops the last reference, including
/// the runtime's own workers.
impl Drop for MixnetProxyHandle {
    fn drop(&mut self) {
        if let Some(monitor) = &self.monitor {
            monitor.abort();
        }
        let proxy = self
            .proxy
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        match teardown_site(tokio::runtime::Handle::try_current().is_ok()) {
            TeardownSite::Inline => teardown(runtime, proxy),
            TeardownSite::OffRuntime => {
                std::thread::spawn(move || teardown(runtime, proxy));
            }
        }
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
                    let _ = tokio::task::spawn_blocking(move || {
                        observer.on_death(death_reason(failure))
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
        let monitor = observer.map(|observer| {
            runtime
                .spawn(monitor_listener(
                    endpoint.clone(),
                    observer,
                    LISTENER_MONITOR_INTERVAL,
                    LOOPBACK_DIAL_BOUND,
                    LISTENER_MONITOR_STRIKES,
                ))
                .abort_handle()
        });
        Ok(std::sync::Arc::new(MixnetProxyHandle {
            runtime: Some(runtime),
            proxy: Mutex::new(Some(proxy)),
            endpoint,
            monitor,
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
            .map(|proxy| proxy.exit_node().to_string())
    }

    /// Disconnect the mixnet client and stop the local SOCKS5 proxy,
    /// idempotently, cancelling the liveness monitor before the listener goes
    /// down so a deliberate stop never fires the death observer.
    pub fn stop(&self) {
        if let Some(monitor) = &self.monitor {
            monitor.abort();
        }
        let taken = self
            .proxy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let (Some(proxy), Some(runtime)) = (taken, self.runtime.as_ref()) {
            runtime.block_on(proxy.disconnect());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    #[test]
    fn a_worker_drop_moves_its_teardown_off_the_runtime() {
        assert_eq!(teardown_site(true), TeardownSite::OffRuntime);
        assert_eq!(teardown_site(false), TeardownSite::Inline);
    }

    /// The whole teardown design rests on `Handle::try_current` telling a
    /// runtime worker apart from a thread that may block, so pin both answers.
    #[test]
    fn try_current_separates_a_worker_from_a_thread_that_may_block() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let on_worker = runtime.block_on(async { tokio::runtime::Handle::try_current().is_ok() });
        assert_eq!(teardown_site(on_worker), TeardownSite::OffRuntime);
        assert_eq!(
            teardown_site(tokio::runtime::Handle::try_current().is_ok()),
            TeardownSite::Inline
        );
    }

    /// A runtime moved to a plain thread may still be blocked on and dropped,
    /// which is what makes the `OffRuntime` arm legal where the worker was not.
    #[test]
    fn a_runtime_handed_to_a_plain_thread_still_blocks_and_drops() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let moved = std::thread::spawn(move || {
            runtime.block_on(async { tokio::task::yield_now().await });
            true
        });
        assert!(moved.join().expect("teardown thread joins"));
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
            runtime: Some(runtime),
        }
    }

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
            .as_ref()
            .expect("a live handle owns its runtime")
            .spawn(monitor_listener(
                handle.endpoint.clone(),
                Box::new(observer),
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

    /// Tests that every task is destroyed before drop returns, given a drop
    /// away from any runtime.
    #[test]
    fn drop_off_runtime_outlives_no_task() {
        let (down_tx, down_rx) = std::sync::mpsc::channel::<()>();
        let runtime = teardown_runtime();
        runtime.spawn(async move {
            let _held = down_tx;
            std::future::pending::<()>().await
        });
        drop(teardown_handle(runtime));
        // The task owns the sender, polled or not, so only its destruction
        // disconnects the channel. Empty means the task outlived the drop.
        assert_eq!(
            down_rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected),
            "the task must already be destroyed when drop returns"
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
            MONITOR_CHECK_INTERVAL,
            MONITOR_CHECK_TIMEOUT,
            1,
        ));
        tokio::time::timeout(MOCK_OP_BOUND, monitor)
            .await
            .expect("monitor must end within the strike window")
            .expect("monitor task must end cleanly despite the panic");
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
