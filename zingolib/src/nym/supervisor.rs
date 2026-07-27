//! The Mixnet Mode proxy supervisor (ADR 0011, consumption model A).
//!
//! The wallet cannot link the mixnet transport in-process, so it bundles the
//! `nym-proxy` binary and spawns it as a child. This supervisor owns that
//! child's lifecycle: it starts the process, reads the local SOCKS5 address
//! the child announces on stdout, and drives the tri-state
//! [`MixnetMode`]. While the child is starting the
//! mode is `Bootstrapping`. It becomes `Ready` once the address arrives. If
//! the child's stdout later closes (during bootstrap or after ready) the
//! mode becomes `Died`, an unconsented loss of the transport that makes
//! mixnet-only surfaces fail closed rather than fall back to clearnet. Only a
//! deliberate [`MixnetProxy::stop`] yields `Off`.
//!
//! # The attached transport
//!
//! The spawned child is the desktop instance of a general rule (ADR 0011's
//! mobile amendment): the transport meets the wallet at a runtime boundary
//! carrying a SOCKS5 endpoint and a liveness signal. [`MixnetProxy::attach`]
//! is the other instance — a platform (a mobile app hosting the proxy as a
//! dynamic library) hands the wallet an already-running local SOCKS5
//! address. Readiness is gated on a data round trip through the endpoint,
//! never a bare TCP connect, because a listener that accepts connections
//! proves nothing about the mixnet carrying data; liveness is thereafter
//! observed by a periodic probe, whose failure lands `Died` and clears the
//! address, exactly as a closed stdout pipe does for the spawned child. The
//! mode semantics are identical for both transports.
//!
//! # Lifetime coupling
//!
//! The supervisor and its child behave as one unit (ADR 0011). Two mechanisms
//! bind the child's lifetime to the parent's without letting a terminal signal
//! tear the transport out from under a live session:
//!
//! - The child is spawned in its **own process group**, so a terminal `Ctrl-C`
//!   (delivered to the shell's foreground group) does not reach it. `Ctrl-C`
//!   aborts the wallet's current command while the mixnet keeps running.
//! - The supervisor holds the child's **stdin pipe** open for the child's
//!   life. The child watches that pipe. Any parent exit (clean, panic, or
//!   `SIGKILL`, which skips `kill_on_drop`) closes the pipe, the child reads
//!   EOF, disconnects from the mixnet, and exits. No orphaned proxy survives a
//!   dead parent.
#![forbid(unsafe_code)]

use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::task::JoinHandle;
use zingo_netutils::{NYM_STATUS_LINE_PREFIX, SOCKS5_ADDR_LINE_PREFIX};

use crate::nym::MixnetMode;

/// The indexer the attach readiness gate round-trips through. No wallet data
/// travels — a bare `GetLightdInfo` — and the target mirrors the spawned
/// binary's health-check indexer.
const ATTACH_HEALTH_INDEXER: &str = "https://zec.rocks:443";

/// Bound on one attach readiness round trip.
const ATTACH_HEALTH_TIMEOUT: Duration = Duration::from_secs(15);

/// Readiness attempts against the attached endpoint before declaring it
/// dead. Unlike the spawned binary's health gate, attach cannot redraw a
/// mixnet path — the platform owns the endpoint — so retrying buys recovery
/// only from a transient blip, and two attempts suffice.
const ATTACH_HEALTH_ATTEMPTS: usize = 2;

/// Pause between attach readiness attempts.
const ATTACH_HEALTH_RETRY_PAUSE: Duration = Duration::from_secs(1);

/// Cadence of the liveness probe against an attached endpoint.
const ATTACH_PROBE_INTERVAL: Duration = Duration::from_secs(30);

/// Bound on one liveness probe connect.
const ATTACH_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// A failure starting the mixnet proxy child process or attaching to a
/// platform-hosted endpoint.
#[derive(Debug, thiserror::Error)]
pub enum MixnetProxyError {
    /// The `nym-proxy` binary could not be spawned.
    #[error("failed to spawn the nym-proxy binary at {path}: {source}")]
    Spawn {
        /// The binary path that failed to spawn.
        path: String,
        /// The underlying spawn error.
        source: std::io::Error,
    },
    /// The spawned child exposed no stdout to read its address from.
    #[error("the nym-proxy child exposed no stdout")]
    NoStdout,
    /// The spawned child exposed no stdin to hold open as the liveness pipe.
    #[error("the nym-proxy child exposed no stdin")]
    NoStdin,
    /// The address handed to [`MixnetProxy::attach`] is not a socket address.
    #[error("the attached SOCKS5 address '{addr}' does not parse as a socket address")]
    InvalidAddress {
        /// The address that failed to parse.
        addr: String,
    },
}

/// The observable state shared between the supervisor and its stdout reader.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProxyState {
    mode: MixnetMode,
    socks5_addr: Option<String>,
    /// The child's latest bootstrap progress line, live only while
    /// [`MixnetMode::Bootstrapping`], so a user interface can narrate the
    /// connect race instead of showing an opaque wait.
    bootstrap_detail: Option<String>,
}

/// Supervises the mixnet proxy — a spawned child or an attached
/// platform-hosted endpoint — and exposes its state.
pub struct MixnetProxy {
    state: Arc<Mutex<ProxyState>>,
    transport: Transport,
}

/// How the proxy endpoint is provided (ADR 0011's mobile amendment): a child
/// process this supervisor spawned and owns, or an already-running endpoint a
/// platform handed the wallet. The state semantics are identical; only the
/// liveness mechanism differs.
enum Transport {
    /// The bundled `nym-proxy` binary, spawned as an owned child.
    Spawned {
        child: Child,
        reader: JoinHandle<()>,
        /// The child's stdin, held open for the child's life. The child
        /// watches this pipe and exits when it closes, so dropping this
        /// handle (on [`MixnetProxy::stop`] or when the whole process dies)
        /// tears the proxy down. Never written to. Its openness is the
        /// signal.
        _stdin: ChildStdin,
    },
    /// A platform-hosted endpoint, watched by the readiness/liveness driver.
    Attached { driver: JoinHandle<()> },
}

impl MixnetProxy {
    /// Spawn the `nym-proxy` binary at `binary_path`. Returns immediately with
    /// mode [`MixnetMode::Bootstrapping`]. Poll [`Self::mode`] for readiness.
    /// The child is killed if this supervisor is dropped, spawned in its own
    /// process group (terminal signals do not reach it) with its stdin piped
    /// (its closure is how the child learns the parent is gone).
    pub fn spawn(binary_path: &Path) -> Result<Self, MixnetProxyError> {
        let mut command = Command::new(binary_path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        // A distinct process group detaches the child from the terminal's
        // foreground group, so a user's Ctrl-C aborts the wallet command
        // without killing the privacy transport. Unix-only; on other targets
        // the child shares the group and Ctrl-C still reaches it (the
        // stdin-EOF watchdog remains the durable coupling).
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|source| MixnetProxyError::Spawn {
            path: binary_path.display().to_string(),
            source,
        })?;
        let stdout = child.stdout.take().ok_or(MixnetProxyError::NoStdout)?;
        let stdin = child.stdin.take().ok_or(MixnetProxyError::NoStdin)?;
        let state = Arc::new(Mutex::new(ProxyState {
            mode: MixnetMode::Bootstrapping,
            socks5_addr: None,
            bootstrap_detail: None,
        }));
        let reader = tokio::spawn(drive_state(stdout, Arc::clone(&state)));
        Ok(MixnetProxy {
            state,
            transport: Transport::Spawned {
                child,
                reader,
                _stdin: stdin,
            },
        })
    }

    /// Attach to an already-running, platform-hosted SOCKS5 endpoint
    /// (ADR 0011's mobile amendment) instead of spawning the bundled binary.
    /// Returns immediately with mode [`MixnetMode::Bootstrapping`]; poll
    /// [`Self::mode`]. Readiness is gated on a data round trip through the
    /// endpoint — never a bare TCP connect — and liveness is thereafter
    /// observed by a periodic probe. A failure of either lands
    /// [`MixnetMode::Died`], so an attached transport refuses rather than
    /// falls back to clearnet, exactly like a spawned one.
    pub fn attach(socks5_addr: &str) -> Result<Self, MixnetProxyError> {
        if socks5_addr.parse::<SocketAddr>().is_err() {
            return Err(MixnetProxyError::InvalidAddress {
                addr: socks5_addr.to_string(),
            });
        }
        let state = Arc::new(Mutex::new(ProxyState {
            mode: MixnetMode::Bootstrapping,
            socks5_addr: None,
            bootstrap_detail: Some("validating the attached mixnet endpoint".to_string()),
        }));
        let addr = socks5_addr.to_string();
        let probe_addr = addr.clone();
        let driver = tokio::spawn(drive_attached_state(
            Arc::clone(&state),
            addr.clone(),
            attach_readiness(addr),
            move || endpoint_alive(probe_addr.clone()),
            ATTACH_PROBE_INTERVAL,
        ));
        Ok(MixnetProxy {
            state,
            transport: Transport::Attached { driver },
        })
    }

    /// The current tri-state.
    pub fn mode(&self) -> MixnetMode {
        self.state.lock().expect("proxy state mutex").mode
    }

    /// The local SOCKS5 address, once the mode is [`MixnetMode::Ready`].
    pub fn socks5_addr(&self) -> Option<String> {
        self.state
            .lock()
            .expect("proxy state mutex")
            .socks5_addr
            .clone()
    }

    /// The child's latest bootstrap progress line, while
    /// [`MixnetMode::Bootstrapping`]. `None` before the first report and
    /// once the proxy is ready.
    pub fn bootstrap_detail(&self) -> Option<String> {
        self.state
            .lock()
            .expect("proxy state mutex")
            .bootstrap_detail
            .clone()
    }

    /// Shut the transport down deliberately and mark the mode
    /// [`MixnetMode::Off`]. The watcher task is aborted BEFORE the teardown,
    /// so this deliberate `Off` is never overwritten by the watcher's `Died`.
    pub async fn stop(self) {
        match self.transport {
            Transport::Spawned {
                mut child, reader, ..
            } => {
                reader.abort();
                let _ = child.kill().await;
            }
            Transport::Attached { driver } => driver.abort(),
        }
        self.state.lock().expect("proxy state mutex").mode = MixnetMode::Off;
    }
}

/// The attach readiness gate: a real data round trip through the endpoint to
/// [`ATTACH_HEALTH_INDEXER`], bounded per attempt and retried once, because a
/// listener that accepts TCP proves nothing about the mixnet carrying data —
/// the lesson behind the spawned binary's health gate.
async fn attach_readiness(socks5_addr: String) -> Result<(), String> {
    let indexer: http::Uri = ATTACH_HEALTH_INDEXER
        .parse()
        .expect("the static health-check URI parses");
    let mut last_failure = String::new();
    for attempt in 0..ATTACH_HEALTH_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(ATTACH_HEALTH_RETRY_PAUSE).await;
        }
        match zingo_netutils::get_lightd_info_via_socks5(
            &socks5_addr,
            &indexer,
            ATTACH_HEALTH_TIMEOUT,
        )
        .await
        {
            Ok(_) => return Ok(()),
            Err(error) => last_failure = error.to_string(),
        }
    }
    Err(last_failure)
}

/// One liveness probe: can the endpoint still be dialed? Local and cheap —
/// its job is to notice the platform host dying, not to re-validate the
/// mixnet path, which the send fan-out judges per arm.
async fn endpoint_alive(socks5_addr: String) -> bool {
    matches!(
        tokio::time::timeout(
            ATTACH_PROBE_TIMEOUT,
            tokio::net::TcpStream::connect(&socks5_addr),
        )
        .await,
        Ok(Ok(_))
    )
}

/// Drive an attached endpoint's state. The injected readiness round trip
/// gates `Ready`: success publishes the address; failure lands `Died`
/// without ever announcing a false `Ready`. After readiness, the injected
/// liveness probe runs every `interval`; a failed probe lands `Died` and
/// clears the address, exactly as a closed stdout pipe does for a spawned
/// child. Generic over both effects so the transitions are unit-tested on
/// paused time without a network; only [`MixnetProxy::stop`] sets `Off`, and
/// it aborts this task first.
async fn drive_attached_state<RFut, P, PFut>(
    state: Arc<Mutex<ProxyState>>,
    socks5_addr: String,
    readiness: RFut,
    mut probe: P,
    interval: Duration,
) where
    RFut: Future<Output = Result<(), String>>,
    P: FnMut() -> PFut,
    PFut: Future<Output = bool>,
{
    let die = |state: &Arc<Mutex<ProxyState>>| {
        let mut guarded = state.lock().expect("proxy state mutex");
        guarded.mode = MixnetMode::Died;
        guarded.socks5_addr = None;
        guarded.bootstrap_detail = None;
    };

    if readiness.await.is_err() {
        die(&state);
        return;
    }
    {
        let mut guarded = state.lock().expect("proxy state mutex");
        guarded.socks5_addr = Some(socks5_addr);
        guarded.bootstrap_detail = None;
        guarded.mode = MixnetMode::Ready;
    }
    loop {
        tokio::time::sleep(interval).await;
        if !probe().await {
            die(&state);
            return;
        }
    }
}

/// Read `stdout` for the child's whole life: the address announcement flips
/// the mode to `Ready`, progress lines update the live bootstrap detail, and,
/// the key coupling change, reading continues *past* `Ready` so a later close
/// is observed. When stdout closes at all, whether before or after the address
/// arrived, the mode becomes `Died`: an unexpected loss of the proxy, not a
/// consented `Off`. Only [`MixnetProxy::stop`] sets `Off`, and it aborts this
/// task first so its deliberate `Off` is never overwritten by this `Died`.
/// Generic over the reader so the state machine is unit-tested without a
/// process.
async fn drive_state<R: AsyncRead + Unpin>(stdout: R, state: Arc<Mutex<ProxyState>>) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(addr) = parse_socks5_addr_line(&line) {
            let mut guarded = state.lock().expect("proxy state mutex");
            guarded.socks5_addr = Some(addr.to_string());
            guarded.bootstrap_detail = None;
            guarded.mode = MixnetMode::Ready;
            // Keep reading: a close after this must be observed as Died.
            continue;
        }
        if let Some(detail) = parse_status_line(&line) {
            state.lock().expect("proxy state mutex").bootstrap_detail = Some(detail.to_string());
        }
    }
    // Stdout closed. The child exited without a deliberate stop(), so the
    // transport is lost: refuse, never leak to clearnet. A stale address is
    // cleared so no surface can dial a dead proxy.
    let mut guarded = state.lock().expect("proxy state mutex");
    guarded.mode = MixnetMode::Died;
    guarded.socks5_addr = None;
    guarded.bootstrap_detail = None;
}

/// Extract the SOCKS5 address from a child stdout line, if it is the
/// announcement line.
fn parse_socks5_addr_line(line: &str) -> Option<&str> {
    line.strip_prefix(SOCKS5_ADDR_LINE_PREFIX).map(str::trim)
}

/// Extract the progress detail from a child stdout line, if it is a
/// bootstrap status line.
fn parse_status_line(line: &str) -> Option<&str> {
    line.strip_prefix(NYM_STATUS_LINE_PREFIX).map(str::trim)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::ReadBuf;

    use super::*;

    fn bootstrapping() -> Arc<Mutex<ProxyState>> {
        Arc::new(Mutex::new(ProxyState {
            mode: MixnetMode::Bootstrapping,
            socks5_addr: None,
            bootstrap_detail: None,
        }))
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
        let handle = tokio::spawn(drive_state(OpenAfter::new(bytes), Arc::clone(&state)));
        for _ in 0..1000 {
            if state.lock().unwrap().mode != MixnetMode::Bootstrapping {
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
        assert_eq!(s.mode, MixnetMode::Ready);
        assert_eq!(s.socks5_addr.as_deref(), Some("127.0.0.1:43210"));
    }

    #[tokio::test]
    async fn ready_after_preamble_lines() {
        let s =
            state_over_open_stream(b"discovering gateways\nconnecting\nSOCKS5_ADDR=127.0.0.1:5\n")
                .await;
        assert_eq!(s.mode, MixnetMode::Ready);
        assert_eq!(s.socks5_addr.as_deref(), Some("127.0.0.1:5"));
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
        assert_eq!(s.mode, MixnetMode::Bootstrapping);
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
        assert_eq!(s.mode, MixnetMode::Ready);
        assert_eq!(s.socks5_addr.as_deref(), Some("127.0.0.1:7"));
        assert_eq!(s.bootstrap_detail, None, "ready has no bootstrap detail");
    }

    #[tokio::test]
    async fn died_when_stdout_closes_during_bootstrap() {
        let state = bootstrapping();
        drive_state(
            b"failed to reach any gateway\n".as_slice(),
            Arc::clone(&state),
        )
        .await;
        let s = state.lock().unwrap();
        assert_eq!(
            s.mode,
            MixnetMode::Died,
            "a proxy that closes without an address died; it is not consented Off"
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
        )
        .await;
        let s = state.lock().unwrap();
        assert_eq!(
            s.mode,
            MixnetMode::Died,
            "a proxy that dies after ready must not stay stale-Ready"
        );
        assert!(
            s.socks5_addr.is_none(),
            "the dead proxy's address must be cleared so nothing dials it"
        );
    }

    // ----- attached-transport falsifiers (issue #2503) -----

    /// HYPOTHESIS: a failed readiness round trip lands Died without ever
    /// announcing Ready, and the liveness probe never runs — an attached
    /// listener that accepts TCP but carries no data must not be published.
    /// Falsified if the driver reaches Ready, leaves an address set, or
    /// consults the probe after a dead readiness verdict.
    #[tokio::test(start_paused = true)]
    async fn attached_readiness_failure_lands_died_never_ready() {
        let state = bootstrapping();
        let probed = Arc::new(Mutex::new(false));
        let probed_flag = Arc::clone(&probed);
        drive_attached_state(
            Arc::clone(&state),
            "127.0.0.1:1080".to_string(),
            std::future::ready(Err("no data through the endpoint".to_string())),
            move || {
                *probed_flag.lock().unwrap() = true;
                std::future::ready(true)
            },
            Duration::from_secs(30),
        )
        .await;
        let s = state.lock().unwrap();
        assert_eq!(s.mode, MixnetMode::Died, "readiness failure is death");
        assert!(s.socks5_addr.is_none(), "no address may be published");
        assert!(
            !*probed.lock().unwrap(),
            "liveness probing must not start for an endpoint that never became ready"
        );
    }

    /// HYPOTHESIS (the attached zombie falsifier): readiness success
    /// publishes Ready with the address, and a later liveness-probe failure
    /// lands Died with the address cleared. The probe closure snapshots the
    /// state at each call, so the intermediate Ready is asserted without
    /// racing the driver. Falsified if Ready is never published, the probe
    /// cadence stalls, or the dead endpoint's address stays dialable. Runs
    /// on paused tokio time.
    #[tokio::test(start_paused = true)]
    async fn attached_probe_failure_lands_died_after_publishing_ready() {
        /// What the probe closure saw at one call: the mode and the
        /// published address at that instant.
        type ProbeSnapshot = (MixnetMode, Option<String>);

        let state = bootstrapping();
        let observed: Arc<Mutex<Vec<ProbeSnapshot>>> = Arc::new(Mutex::new(Vec::new()));
        let observer = Arc::clone(&observed);
        let observer_state = Arc::clone(&state);
        drive_attached_state(
            Arc::clone(&state),
            "127.0.0.1:1080".to_string(),
            std::future::ready(Ok(())),
            move || {
                let snapshot = observer_state.lock().unwrap();
                observer
                    .lock()
                    .unwrap()
                    .push((snapshot.mode, snapshot.socks5_addr.clone()));
                let calls = observer.lock().unwrap().len();
                std::future::ready(calls < 3)
            },
            Duration::from_secs(30),
        )
        .await;

        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 3, "two live probes, then the fatal third");
        assert_eq!(
            observed[0],
            (MixnetMode::Ready, Some("127.0.0.1:1080".to_string())),
            "readiness success must publish Ready with the address"
        );
        let s = state.lock().unwrap();
        assert_eq!(s.mode, MixnetMode::Died, "a failed probe is death");
        assert!(
            s.socks5_addr.is_none(),
            "the dead endpoint's address must be cleared so nothing dials it"
        );
    }

    /// HYPOTHESIS: attach validates the address synchronously, and stop() on
    /// an attached transport is the deliberate Off — never Died. Falsified
    /// if a malformed address is accepted, or if a stopped attachment
    /// reports anything but Off.
    #[tokio::test]
    async fn attach_validates_the_address_and_stop_is_a_deliberate_off() {
        assert!(matches!(
            MixnetProxy::attach("not-a-socket-address"),
            Err(MixnetProxyError::InvalidAddress { .. })
        ));

        // A refusing localhost endpoint: readiness will fail, but stop() must
        // win regardless of where the driver is when it lands.
        let proxy = MixnetProxy::attach("127.0.0.1:9").expect("a valid address attaches");
        assert_ne!(proxy.mode(), MixnetMode::Off, "attach never starts Off");
        proxy.stop().await;
    }

    /// HYPOTHESIS: an attached endpoint that dies refuses the route — the
    /// fail-closed invariant holds for the attached transport end to end.
    /// Attaches to a refusing localhost port, waits for the real readiness
    /// gate to land Died, and resolves the route. Falsified if the route
    /// ever yields clearnet or the mixnet for the dead endpoint.
    #[tokio::test]
    async fn an_attached_endpoint_that_dies_refuses_the_route() {
        use crate::nym::route::MixnetNotReady;
        use crate::nym::route::resolve_route;

        // Port 9 (discard) on localhost refuses; the readiness round trip
        // fails fast and the driver lands Died.
        let proxy = MixnetProxy::attach("127.0.0.1:9").expect("a valid address attaches");
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while proxy.mode() != MixnetMode::Died {
            assert!(
                std::time::Instant::now() < deadline,
                "the dead endpoint must be detected within the readiness budget"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            resolve_route(proxy.mode(), proxy.socks5_addr()),
            Err(MixnetNotReady::Died),
            "a died attachment must refuse, never route"
        );
        proxy.stop().await;
    }
}
