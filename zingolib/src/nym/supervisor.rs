//! The Mixnet Mode proxy supervisor (ADR 0011, consumption model A).
//!
//! The wallet cannot link the mixnet transport in-process, so it bundles the
//! `nym-proxy` binary and spawns it as a child. This supervisor owns that
//! child's lifecycle: it starts the process, reads the local SOCKS5 address
//! the child announces on stdout, and drives the transport's lifecycle
//! states of [`MixnetMode`]. While the child is starting the
//! mode is `Bootstrapping`. It becomes `Ready` once the address arrives. If
//! the child's stdout later closes (during bootstrap or after ready) the
//! mode becomes `Died`, an unconsented loss of the transport that makes
//! mixnet-only surfaces fail closed rather than fall back to clearnet. Only a
//! deliberate [`MixnetProxy::stop`] tears down to `Unattached`; the consented
//! `SwitchedOff` is the wallet slot's to record, never this transport's.
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
use zingo_netutils::time::{
    ATTACH_HEALTH_RETRY_PAUSE, ATTACH_PROBE_INTERVAL, LOOPBACK_DIAL_BOUND, MIXNET_ROUND_TRIP_BOUND,
};
use zingo_netutils::{NYM_STATUS_LINE_PREFIX, SOCKS5_ADDR_LINE_PREFIX};

use crate::nym::MixnetMode;

/// The indexer the attach readiness gate round-trips through: the census's
/// shared health target, the same one the spawned binary's gate uses (one
/// owner, issue #2565's rule; the census pins it to an active member). No
/// wallet data travels — a bare `GetLightdInfo`.
const ATTACH_HEALTH_INDEXER: &str = zingo_netutils::indexers::MIXNET_HEALTH_INDEXER;

/// Readiness attempts against the attached endpoint before declaring it
/// dead. Unlike the spawned binary's health gate, attach cannot redraw a
/// mixnet path — the platform owns the endpoint — so retrying buys recovery
/// only from a transient blip, and two attempts suffice.
const ATTACH_HEALTH_ATTEMPTS: usize = 2;

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
    /// The one death latch: its moment and, when the watcher held one, the
    /// typed cause (`docs/agents/net-diag-design.md`). A single field so a
    /// cause can never exist without its moment (the #2569 review); the
    /// latch is sticky by design (proxy-owner-remediates, issue #2564),
    /// and the mode enum itself is unchanged.
    death: Option<DeathReport>,
}

/// One latched death, read whole: when it happened and, when the watcher
/// held one, the typed cause. The timestamp is always present because every
/// death has a moment; the detail is `None` for a spawned child's closed
/// stdout pipe, whose diagnostic is the child's own stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeathReport {
    /// When the death latched, by the system clock — renderable as wall
    /// time and FFI-crossable, unlike a monotonic instant, at the price of
    /// NTP steps. Staleness math goes through [`DeathReport::age`], which
    /// absorbs a stepped clock.
    pub at: std::time::SystemTime,
    /// The typed cause, when one was held.
    pub detail: Option<zingo_net_diag::NetOpFailure>,
}

impl DeathReport {
    /// How long ago the death latched, measured against `now`. The system
    /// clock can step between the latch and the read, so a moment reading
    /// later than `now` clamps to zero: staleness never errors and never
    /// goes negative. Consumers rendering "died N minutes ago" go through
    /// this rather than subtracting timestamps themselves.
    pub fn age(&self, now: std::time::SystemTime) -> std::time::Duration {
        now.duration_since(self.at)
            .unwrap_or(std::time::Duration::ZERO)
    }
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
            death: None,
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
            death: None,
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

    /// The transport's current lifecycle state.
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

    /// Why the transport died, while the mode is [`MixnetMode::Died`] and
    /// the watcher held a typed cause: which stage failed, against what
    /// target, with the cause chain as a vector. `None` in every other
    /// mode, and `None` for a spawned child's death (a closed stdout pipe
    /// carries no cause; the child's own stderr is the diagnostic there).
    pub fn death_detail(&self) -> Option<zingo_net_diag::NetOpFailure> {
        // Derived from the one latch, so this accessor and death_report
        // cannot disagree (the #2569 review).
        self.death_report().and_then(|report| report.detail)
    }

    /// The latched death read whole — its moment, and its typed cause when
    /// one was held — while the mode is [`MixnetMode::Died`]; `None` in
    /// every other mode. The sticky latch (proxy-owner-remediates) makes
    /// the timestamp the difference between "attach timed out twenty
    /// minutes ago" and "the proxy is gone now" (issue #2564).
    pub fn death_report(&self) -> Option<DeathReport> {
        let guarded = self.state.lock().expect("proxy state mutex");
        if guarded.mode == MixnetMode::Died {
            guarded.death.clone()
        } else {
            None
        }
    }

    /// Shut the transport down deliberately and mark the mode
    /// [`MixnetMode::Unattached`]: the transport is gone, and a deliberate
    /// stop is neither a death nor the user's clearnet consent
    /// (`SwitchedOff` is the wallet's to record, never a torn-down
    /// transport's). The watcher task is aborted BEFORE the teardown, so
    /// this deliberate stop is never overwritten by the watcher's `Died`,
    /// and any stale reader of the handle refuses rather than routes.
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
        self.state.lock().expect("proxy state mutex").mode = MixnetMode::Unattached;
    }
}

/// The attach readiness gate: a real data round trip through the endpoint to
/// [`ATTACH_HEALTH_INDEXER`], bounded per attempt and retried once, because a
/// listener that accepts TCP proves nothing about the mixnet carrying data —
/// the lesson behind the spawned binary's health gate. The last attempt's
/// failure is returned typed: stage by typed match over the transmit error,
/// target the local endpoint for pre-tunnel stages and the health indexer
/// beyond, cause chain captured layer by layer.
async fn attach_readiness(socks5_addr: String) -> Result<(), zingo_net_diag::NetOpFailure> {
    let indexer: http::Uri = ATTACH_HEALTH_INDEXER
        .parse()
        .expect("the static health-check URI parses");
    let mut last_failure = None;
    for attempt in 0..ATTACH_HEALTH_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(ATTACH_HEALTH_RETRY_PAUSE).await;
        }
        match zingo_netutils::get_lightd_info_via_socks5(
            &socks5_addr,
            &indexer,
            MIXNET_ROUND_TRIP_BOUND,
        )
        .await
        {
            Ok(_) => return Ok(()),
            Err(error) => {
                let stage = crate::nym::socks5_transmit_stage(&error);
                let target = match stage {
                    zingo_net_diag::NetOpStage::LocalProxyConnect
                    | zingo_net_diag::NetOpStage::SocksHandshake => socks5_addr.as_str(),
                    _ => ATTACH_HEALTH_INDEXER,
                };
                last_failure = Some(zingo_net_diag::NetOpFailure::from_error(
                    stage, target, &error,
                ));
            }
        }
    }
    Err(last_failure.expect("at least one readiness attempt ran"))
}

/// One liveness probe: can the endpoint still be dialed? Local and cheap —
/// its job is to notice the platform host dying, not to re-validate the
/// mixnet path, which the send fan-out judges per arm.
async fn endpoint_alive(socks5_addr: String) -> bool {
    matches!(
        tokio::time::timeout(
            LOOPBACK_DIAL_BOUND,
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
/// paused time without a network; only [`MixnetProxy::stop`] tears down to
/// `Unattached`, and it aborts this task first.
async fn drive_attached_state<RFut, P, PFut>(
    state: Arc<Mutex<ProxyState>>,
    socks5_addr: String,
    readiness: RFut,
    mut probe: P,
    interval: Duration,
) where
    RFut: Future<Output = Result<(), zingo_net_diag::NetOpFailure>>,
    P: FnMut() -> PFut,
    PFut: Future<Output = bool>,
{
    let die = |state: &Arc<Mutex<ProxyState>>, cause: zingo_net_diag::NetOpFailure| {
        let mut guarded = state.lock().expect("proxy state mutex");
        guarded.mode = MixnetMode::Died;
        guarded.socks5_addr = None;
        guarded.bootstrap_detail = None;
        guarded.death = Some(DeathReport {
            at: std::time::SystemTime::now(),
            detail: Some(cause),
        });
    };

    if let Err(failure) = readiness.await {
        die(&state, failure);
        return;
    }
    {
        let mut guarded = state.lock().expect("proxy state mutex");
        guarded.socks5_addr = Some(socks5_addr.clone());
        guarded.bootstrap_detail = None;
        guarded.mode = MixnetMode::Ready;
    }
    loop {
        tokio::time::sleep(interval).await;
        if !probe().await {
            die(
                &state,
                zingo_net_diag::NetOpFailure::message(
                    zingo_net_diag::NetOpStage::LocalProxyConnect,
                    &socks5_addr,
                    "the liveness probe could not dial the attached endpoint",
                ),
            );
            return;
        }
    }
}

/// Read `stdout` for the child's whole life: the address announcement flips
/// the mode to `Ready`, progress lines update the live bootstrap detail, and,
/// the key coupling change, reading continues *past* `Ready` so a later close
/// is observed. When stdout closes at all, whether before or after the address
/// arrived, the mode becomes `Died`: an unexpected loss of the proxy, never
/// a consented clearnet. Only [`MixnetProxy::stop`] tears down to
/// `Unattached`, and it aborts this task first so a deliberate stop is never
/// overwritten by this `Died`.
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
    // A closed pipe has no cause to hold, but every death has a moment.
    guarded.death = Some(DeathReport {
        at: std::time::SystemTime::now(),
        detail: None,
    });
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
            death: None,
        }))
    }

    /// A fabricated readiness failure for the attach falsifiers.
    fn readiness_failure(text: &str) -> zingo_net_diag::NetOpFailure {
        zingo_net_diag::NetOpFailure::message(
            zingo_net_diag::NetOpStage::TunnelTransport,
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
            "a proxy that closes without an address died; it is never consented clearnet"
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
        let latch = s
            .death
            .as_ref()
            .expect("even a causeless death (closed pipe) must latch its moment");
        assert!(
            latch.detail.is_none(),
            "a closed pipe carries no cause; its diagnostic is the child's stderr"
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
            std::future::ready(Err(readiness_failure("no data through the endpoint"))),
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
    /// an attached transport is a deliberate teardown to Unattached — never
    /// Died, and never the wallet's SwitchedOff. Falsified if a malformed
    /// address is accepted, or if a live attachment reports anything but the
    /// transport lifecycle states.
    #[tokio::test]
    async fn attach_validates_the_address_and_stop_is_a_deliberate_teardown() {
        assert!(matches!(
            MixnetProxy::attach("not-a-socket-address"),
            Err(MixnetProxyError::InvalidAddress { .. })
        ));

        // A refusing localhost endpoint: readiness will fail, but stop() must
        // win regardless of where the driver is when it lands.
        let proxy = MixnetProxy::attach("127.0.0.1:9").expect("a valid address attaches");
        // The readiness gate races this assert (a refused port can land Died
        // fast), so assert only what is invariant: a live attachment is in
        // the transport lifecycle, never in a wallet slot state.
        assert!(
            !matches!(
                proxy.mode(),
                MixnetMode::Unattached | MixnetMode::SwitchedOff
            ),
            "a live attachment never reports a slot state"
        );
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
                guarded.mode = MixnetMode::Died;
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

    /// HYPOTHESIS (issue #2565's drift-test pattern): the named readiness
    /// budget equals the gate it summarizes — every attempt's round-trip
    /// bound plus the pauses between attempts. Falsified if any of the
    /// three constants is retuned without the others.
    #[test]
    fn the_readiness_budget_is_the_sum_of_its_gate() {
        let attempts = u32::try_from(ATTACH_HEALTH_ATTEMPTS).expect("a small count");
        assert_eq!(
            zingo_netutils::time::ATTACH_READINESS_BUDGET,
            MIXNET_ROUND_TRIP_BOUND * attempts + ATTACH_HEALTH_RETRY_PAUSE * (attempts - 1),
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
            guarded.mode = MixnetMode::Died;
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
            MixnetMode::Ready,
            MixnetMode::Bootstrapping,
            MixnetMode::Unattached,
            MixnetMode::SwitchedOff,
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
            guarded.mode = MixnetMode::Died;
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
            MixnetMode::Ready,
            MixnetMode::Bootstrapping,
            MixnetMode::Unattached,
            MixnetMode::SwitchedOff,
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
        )
        .await;
        let proxy = proxy_over(&state);
        assert_eq!(proxy.mode(), MixnetMode::Died);
        assert_eq!(
            proxy.death_detail(),
            None,
            "a closed stdout pipe carries no cause; nothing may be fabricated"
        );
    }
}
