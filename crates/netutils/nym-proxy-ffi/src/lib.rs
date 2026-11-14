//! The Nym mixnet proxy as a UniFFI component for mobile (zingolib/0011
//! mobile amendment, CP-3).
//!
//! iOS cannot spawn the `nym-proxy` child process the desktop model relies on,
//! and the proxy stays a separate artifact from the wallet library on every
//! platform (ADR 0011, mobile amendment). So on mobile the proxy is a dynamic
//! library the app hosts: it brings up a [`NymProxy`], exposes its local
//! SOCKS5 address, and the app hands that address to the wallet's
//! `attach_mixnet` seam (CP-2). This crate is that library's UniFFI surface —
//! `start`, `stop`, and a death callback — a member of the root workspace,
//! where its `nym` feature of netutils resolves in the shared lockfile.
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
use zl_netutils::NymProxy;
use zl_netutils::time::{LISTENER_MONITOR_INTERVAL, LOOPBACK_DIAL_BOUND};

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

/// The second bound the disconnect gets once the first is spent, which a
/// process resumed from suspension needs because it wakes with the first
/// bound already gone.
pub const DISCONNECT_RESUME_GRACE: Duration = Duration::from_secs(10);

/// How long a finished disconnect's runtime may wait for its last blocking
/// task before disposal abandons it.
pub const RUNTIME_DISPOSAL_GRACE: Duration = Duration::from_secs(5);

/// The bounds one teardown runs under.
#[derive(Clone, Copy, Debug)]
struct TeardownBounds {
    /// What the ordered disconnect gets before the resume grace.
    disconnect: Duration,
    /// What a disconnect gets after a spent bound, before the runtime leaks.
    resume_grace: Duration,
    /// What the runtime's disposal gets for its last blocking task.
    disposal: Duration,
}

impl TeardownBounds {
    /// The bounds every teardown outside the tests runs under.
    const LIVE: Self = Self {
        disconnect: DISCONNECT_BOUND,
        resume_grace: DISCONNECT_RESUME_GRACE,
        disposal: RUNTIME_DISPOSAL_GRACE,
    };

    /// The longest a teardown under these bounds holds its thread.
    fn longest(self) -> Duration {
        self.disconnect + self.resume_grace + self.disposal
    }
}

/// Runtimes this process has leaked because their teardown could not end.
static LEAKED_TEARDOWNS: atomic::AtomicUsize = atomic::AtomicUsize::new(0);

/// Leaked runtimes beyond which [`MixnetProxyHandle::start`] refuses to
/// build another.
const MAX_LEAKED_TEARDOWNS: usize = 4;

/// Teardowns whose disconnect is still in flight.
static ACTIVE_TEARDOWNS: atomic::AtomicUsize = atomic::AtomicUsize::new(0);

/// Holds one teardown in a fence counter for as long as the guard lives.
struct TeardownInFlight(&'static atomic::AtomicUsize);

impl TeardownInFlight {
    /// Registers a teardown as in flight until the returned guard drops.
    fn enter(fence: &'static atomic::AtomicUsize) -> Self {
        fence.fetch_add(1, atomic::Ordering::SeqCst);
        Self(fence)
    }
}

impl Drop for TeardownInFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, atomic::Ordering::SeqCst);
    }
}

/// Counts and reports a runtime the teardown cannot dispose, then leaks it.
fn leak_runtime(runtime: Runtime, why: &str) {
    let leaks = LEAKED_TEARDOWNS.fetch_add(1, atomic::Ordering::SeqCst) + 1;
    tracing::error!(leaks, why, "leaking a mixnet runtime");
    mem::forget(runtime);
}

/// Runs the ordered disconnect under `bounds`, then disposes the runtime,
/// leaking the runtime instead when the disconnect does not finish cleanly.
fn teardown(runtime: Runtime, proxy: Option<ProxySlot>, bounds: TeardownBounds) {
    if let Some(proxy) = proxy {
        let mut disconnect = runtime.spawn(proxy.disconnect());
        let finished = runtime.block_on(async {
            match tokio::time::timeout(bounds.disconnect, &mut disconnect).await {
                // A bound spent while the process was suspended gave the
                // disconnect no poll at all, and leaking there spends a leak
                // credit on a teardown that never ran.
                Err(_) => tokio::time::timeout(bounds.resume_grace, &mut disconnect).await,
                polled => polled,
            }
        });
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
    runtime.shutdown_timeout(bounds.disposal);
}

/// Hands the runtime and client to a detached teardown thread, falling back
/// without an abort when the OS refuses the thread.
fn spawn_teardown(runtime: Runtime, proxy: Option<ProxySlot>) {
    // Guards, and not paired decrements, because an unwind out of a teardown
    // would strand the count and make every later start wait out the fence.
    // The entry kept here holds the count up until a refused spawn has run
    // its reclaim.
    let _here = TeardownInFlight::enter(&ACTIVE_TEARDOWNS);
    let on_thread = TeardownInFlight::enter(&ACTIVE_TEARDOWNS);
    let payload = Arc::new(Mutex::new(Some((runtime, proxy))));
    let for_thread = Arc::clone(&payload);
    let spawned = thread::Builder::new()
        .name("nym-teardown".to_string())
        .spawn(move || {
            let _in_flight = on_thread;
            if let Some((runtime, proxy)) = take_teardown_payload(&for_thread) {
                teardown(runtime, proxy, TeardownBounds::LIVE);
            }
        });
    if spawned.is_err() {
        reclaim_unspawned_teardown(&payload);
    }
}

/// Waits, bounded by the teardown bounds, until no teardown thread is still
/// working, which fences a new client from every bounded teardown but not
/// from a leaked runtime.
fn await_teardowns_in_flight() {
    let deadline = Instant::now() + TeardownBounds::LIVE.longest();
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
        teardown(runtime, proxy, TeardownBounds::LIVE);
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
mod tests;
