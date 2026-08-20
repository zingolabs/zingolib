//! The Mixnet Mode proxy supervisor (ADR 0011, consumption model A).
//!
//! The wallet cannot link the mixnet transport in-process, so it bundles the
//! `nym-proxy` binary and spawns it as a child. This supervisor owns that
//! child's lifecycle: it starts the process, reads the local SOCKS5 address
//! the child announces on stdout, and drives the transport's lifecycle
//! states of [`Indicator`]. While the child is starting the
//! mode is `Bootstrapping`. It becomes `Ready` once the address arrives. If
//! the child's stdout later closes (during bootstrap or after ready) the
//! mode becomes `Died`, an unconsented loss of the transport that makes
//! mixnet-only surfaces fail closed rather than fall back to clearnet. Only a
//! deliberate `MixnetProxy::stop` tears down to `Unattached`; the consented
//! `SwitchedOff` is the wallet slot's to record, never this transport's.
//!
//! # The attached transport
//!
//! The spawned child is the desktop instance of a general rule (ADR 0011's
//! mobile amendment): the transport meets the wallet at a runtime boundary
//! carrying a SOCKS5 endpoint and a liveness signal. `MixnetProxy::attach`
//! is the other instance — a mobile platform (an app hosting the proxy as
//! a dynamic library) hands the wallet an already-running local SOCKS5
//! address. Readiness and continued life are judged by loopback dials
//! alone, whose failure lands `Died` exactly as the closed stdout pipe
//! does for the spawned child.
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

use std::collections::HashSet;
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
    ATTACH_LISTENER_RETRY_PAUSE, ATTACH_WATCHDOG_INTERVAL, LOOPBACK_DIAL_BOUND,
    MIXNET_ROUND_TRIP_BOUND,
};
use zingo_netutils::{NYM_EXIT_LINE_PREFIX, NYM_STATUS_LINE_PREFIX, SOCKS5_ADDR_LINE_PREFIX};

use crate::mixnet::Indicator;
use crate::mixnet::acquire;
use crate::mixnet::driver::{MixnetStatus, StatusPublisher};

/// The census indexer an attached session round-trips against to prove its
/// mixnet path carries data.
const ATTACH_HEALTH_INDEXER: &str = zingo_netutils::indexers::MIXNET_HEALTH_INDEXER;

/// Readiness round trips through the attached endpoint before it is declared
/// dead.
const ATTACH_HEALTH_ATTEMPTS: usize = 2;

/// A failure starting the mixnet proxy child process or attaching to a
/// mobile-platform-hosted endpoint.
#[derive(Debug, thiserror::Error)]
pub enum MixnetProxyError {
    /// The `nym-proxy` binary could not be spawned.
    #[error("failed to spawn the nym-proxy binary at {path}")]
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
    /// The spawned child exposed no stderr to collect its diagnostics from.
    #[error("the nym-proxy child exposed no stderr")]
    NoStderr,
    /// The address handed to `MixnetProxy::attach` is not a socket address.
    #[error("the attached SOCKS5 address '{addr}' does not parse as a socket address")]
    InvalidAddress {
        /// The address that failed to parse.
        addr: String,
    },
    /// The exit report handed to `MixnetProxy::attach` names no Exit Node.
    #[error("the attached endpoint reported no bound exit node")]
    NoExits,
    /// The session could not draw a ledgered Clutch for the spawn.
    #[error(transparent)]
    Acquisition(Box<acquire::TransportError>),
}

/// The observable state shared between the supervisor and its transport
/// driver.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProxyState {
    mode: Indicator,
    socks5_addr: Option<SocketAddr>,
    /// The Exit Node identities the transport reports as bound.
    exits: Vec<crate::mixnet::ExitNodeId>,
    /// The transport's latest bootstrap progress report, live only while
    /// [`Indicator::Bootstrapping`], so a user interface can narrate the
    /// connect race instead of showing an opaque wait.
    bootstrap_detail: Option<String>,
    /// The one sticky death latch, holding the moment and, when the watcher
    /// held one, the typed cause.
    death: Option<DeathReport>,
}

impl ProxyState {
    /// The subscriber-facing snapshot of this state, published into the
    /// session's status channel on every transition.
    fn snapshot(&self) -> MixnetStatus {
        MixnetStatus {
            mode: self.mode,
            socks5_addr: self.socks5_addr,
            // Ready-only evidence: the wire refuses exits in any other mode.
            exits: if self.mode == Indicator::Ready {
                self.exits.clone()
            } else {
                Vec::new()
            },
            bootstrap_detail: self.bootstrap_detail.clone(),
            death: self.death.clone(),
        }
    }
}

/// Publishes `guarded`'s snapshot into the session channel while the caller
/// still holds the [`ProxyState`] lock, so publications reach subscribers in
/// exactly the order the state changed.
fn publish_locked(guarded: &ProxyState, publisher: &StatusPublisher) {
    publisher.send_replace(guarded.snapshot());
}

/// One latched death, read whole: when it happened and, when the watcher
/// held one, the typed cause.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeathReport {
    /// When the death latched, by the system clock, crossing the wire as
    /// milliseconds since the Unix epoch.
    #[serde(with = "at_millis")]
    pub at: std::time::SystemTime,
    /// The typed cause, when one was held.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<zingo_net_diag::NetOpFailure>,
}

/// The serde codec carrying [`DeathReport::at`] as checked milliseconds
/// since the Unix epoch in a `u64`.
mod at_millis {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S: serde::Serializer>(at: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        let ms = at
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        s.serialize_u64(ms)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let ms = <u64 as serde::Deserialize>::deserialize(d)?;
        UNIX_EPOCH
            .checked_add(Duration::from_millis(ms))
            .ok_or_else(|| serde::de::Error::custom("death timestamp out of range"))
    }
}

impl DeathReport {
    /// How long ago the death latched, measured against `now` and clamped
    /// to zero when a stepped clock reads the moment later than `now`.
    pub fn age(&self, now: std::time::SystemTime) -> std::time::Duration {
        now.duration_since(self.at)
            .unwrap_or(std::time::Duration::ZERO)
    }
}

/// Supervises the mixnet proxy — a spawned child or an attached
/// mobile-platform-hosted endpoint — and exposes its state.
pub struct MixnetProxy {
    state: Arc<Mutex<ProxyState>>,
    transport: Transport,
}

/// How the proxy endpoint is provided: a child process this supervisor
/// spawned and owns, or an already-running endpoint a mobile platform
/// handed the wallet.
enum Transport {
    /// The bundled `nym-proxy` binary, spawned as an owned child.
    Spawned {
        child: Child,
        reader: JoinHandle<()>,
        /// The child's stdin, held open for the child's life and never
        /// written, whose closure on drop tears the proxy down.
        _stdin: ChildStdin,
    },
    /// A mobile-platform-hosted endpoint, watched by the readiness/liveness driver.
    Attached { driver: JoinHandle<()> },
}

impl MixnetProxy {
    /// Spawns the `nym-proxy` binary at `binary_path` over `clutch`,
    /// returning immediately with mode [`Indicator::Bootstrapping`]
    /// published into `publisher` along with every later transition.
    pub(crate) fn spawn(
        binary_path: &Path,
        publisher: StatusPublisher,
        clutch: &[crate::mixnet::ExitNodeId],
    ) -> Result<Self, MixnetProxyError> {
        let mut launch_args = Vec::new();
        for exit in clutch {
            launch_args.push("--exit".to_string());
            launch_args.push(exit.as_str().to_string());
        }
        let mut command = Command::new(binary_path);
        command.args(&launch_args);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
        let stderr = child.stderr.take().ok_or(MixnetProxyError::NoStderr)?;
        let stderr_tail = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        tokio::spawn(collect_stderr(stderr, Arc::clone(&stderr_tail)));
        let launch = LaunchContext {
            binary: binary_path.display().to_string(),
            args: launch_args,
            stderr_tail,
        };
        let state = Arc::new(Mutex::new(ProxyState {
            mode: Indicator::Bootstrapping,
            socks5_addr: None,
            exits: Vec::new(),
            bootstrap_detail: None,
            death: None,
        }));
        publish_locked(&state.lock().expect("proxy state mutex"), &publisher);
        let reader = tokio::spawn(drive_state(
            stdout,
            Arc::clone(&state),
            publisher,
            Some(launch),
        ));
        Ok(MixnetProxy {
            state,
            transport: Transport::Spawned {
                child,
                reader,
                _stdin: stdin,
            },
        })
    }

    /// Attaches to an already-running, mobile-platform-hosted SOCKS5 endpoint that
    /// bound `exits`, judging readiness and continued life by loopback dials
    /// alone.
    pub(crate) fn attach(
        socks5_addr: SocketAddr,
        exits: &[crate::mixnet::ExitNodeId],
        publisher: StatusPublisher,
    ) -> Result<Self, MixnetProxyError> {
        // Ready means the address AND a bound exit, at every door: an
        // attach that accepted an empty report would mint an exitless Ready
        // that no later announcement can correct.
        if exits.is_empty() {
            return Err(MixnetProxyError::NoExits);
        }
        let state = Arc::new(Mutex::new(ProxyState {
            mode: Indicator::Bootstrapping,
            socks5_addr: None,
            exits: exits.to_vec(),
            bootstrap_detail: Some("validating the attached mixnet endpoint".to_string()),
            death: None,
        }));
        publish_locked(&state.lock().expect("proxy state mutex"), &publisher);
        let driver = tokio::spawn(drive_attached_state(
            Arc::clone(&state),
            socks5_addr,
            attach_readiness(socks5_addr),
            move || endpoint_alive(socks5_addr),
            ATTACH_WATCHDOG_INTERVAL,
            publisher,
        ));
        Ok(MixnetProxy {
            state,
            transport: Transport::Attached { driver },
        })
    }

    /// The transport's current lifecycle state.
    pub fn mode(&self) -> Indicator {
        self.state.lock().expect("proxy state mutex").mode
    }

    /// The local SOCKS5 address, once the mode is [`Indicator::Ready`].
    pub fn socks5_addr(&self) -> Option<SocketAddr> {
        self.state.lock().expect("proxy state mutex").socks5_addr
    }

    /// The bound Exit Node identities, once the mode is
    /// [`Indicator::Ready`].
    pub fn exits(&self) -> Vec<crate::mixnet::ExitNodeId> {
        let guarded = self.state.lock().expect("proxy state mutex");
        if guarded.mode == Indicator::Ready {
            guarded.exits.clone()
        } else {
            Vec::new()
        }
    }

    /// The transport's latest bootstrap progress report, if any, while
    /// [`Indicator::Bootstrapping`].
    pub fn bootstrap_detail(&self) -> Option<String> {
        self.state
            .lock()
            .expect("proxy state mutex")
            .bootstrap_detail
            .clone()
    }

    /// Why the transport died — the failed stage, its target, and the cause
    /// chain — while the mode is [`Indicator::Died`] and the watcher held a
    /// typed cause, and `None` otherwise.
    pub fn death_detail(&self) -> Option<zingo_net_diag::NetOpFailure> {
        // Derived from the one latch, so this accessor and death_report
        // cannot disagree (the #2569 review).
        self.death_report().and_then(|report| report.detail)
    }

    /// The latched death read whole — its moment, and its typed cause when
    /// one was held — while the mode is [`Indicator::Died`], and `None` in
    /// every other mode.
    pub fn death_report(&self) -> Option<DeathReport> {
        let guarded = self.state.lock().expect("proxy state mutex");
        if guarded.mode == Indicator::Died {
            guarded.death.clone()
        } else {
            None
        }
    }

    /// Shuts the transport down deliberately, aborting and awaiting its
    /// driver task, marking the mode [`Indicator::Unattached`], and
    /// publishing nothing so the slot owner announces the settled state.
    pub(crate) async fn stop(self) {
        match self.transport {
            Transport::Spawned {
                mut child, reader, ..
            } => {
                reader.abort();
                let _ = reader.await;
                let _ = child.kill().await;
            }
            Transport::Attached { driver } => {
                driver.abort();
                let _ = driver.await;
            }
        }
        self.state.lock().expect("proxy state mutex").mode = Indicator::Unattached;
    }
}

/// The attach readiness gate: a data round trip through the endpoint to
/// [`ATTACH_HEALTH_INDEXER`], bounded per attempt and retried once, because a
/// listener that accepts TCP proves nothing about the mixnet carrying data.
async fn attach_readiness(socks5_addr: SocketAddr) -> Result<(), zingo_net_diag::NetOpFailure> {
    let indexer: http::Uri = ATTACH_HEALTH_INDEXER
        .parse()
        .expect("the static health-check URI parses");
    let probe = zingo_netutils::Socks5Indexer::new(socks5_addr, indexer, MIXNET_ROUND_TRIP_BOUND);
    let mut last_failure = None;
    for attempt in 0..ATTACH_HEALTH_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(ATTACH_LISTENER_RETRY_PAUSE).await;
        }
        match probe.get_latest_block().await {
            Ok(_) => return Ok(()),
            Err(error) => {
                let stage = crate::mixnet::socks5_transmit_stage(&error);
                let target = match stage {
                    zingo_net_diag::NetOpStage::LocalProxyConnect
                    | zingo_net_diag::NetOpStage::SocksHandshake => socks5_addr.to_string(),
                    _ => ATTACH_HEALTH_INDEXER.to_string(),
                };
                last_failure = Some(zingo_net_diag::NetOpFailure::from_error(
                    stage, target, &error,
                ));
            }
        }
    }
    Err(last_failure.expect("at least one readiness attempt ran"))
}

/// One watchdog tick: whether the local endpoint still accepts a dial.
async fn endpoint_alive(socks5_addr: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(
            LOOPBACK_DIAL_BOUND,
            tokio::net::TcpStream::connect(socks5_addr),
        )
        .await,
        Ok(Ok(_))
    )
}

/// Drives an attached endpoint's state: the injected readiness check gates
/// `Ready`, and a failed tick of the injected watchdog thereafter lands `Died`.
async fn drive_attached_state<RFut, P, PFut>(
    state: Arc<Mutex<ProxyState>>,
    socks5_addr: SocketAddr,
    readiness: RFut,
    mut watchdog: P,
    interval: Duration,
    publisher: StatusPublisher,
) where
    RFut: Future<Output = Result<(), zingo_net_diag::NetOpFailure>>,
    P: FnMut() -> PFut,
    PFut: Future<Output = bool>,
{
    let die = |state: &Arc<Mutex<ProxyState>>, cause: zingo_net_diag::NetOpFailure| {
        let mut guarded = state.lock().expect("proxy state mutex");
        guarded.mode = Indicator::Died;
        guarded.socks5_addr = None;
        guarded.exits.clear();
        guarded.bootstrap_detail = None;
        guarded.death = Some(DeathReport {
            at: std::time::SystemTime::now(),
            detail: Some(cause),
        });
        publish_locked(&guarded, &publisher);
    };

    if let Err(failure) = readiness.await {
        die(&state, failure);
        return;
    }
    {
        let mut guarded = state.lock().expect("proxy state mutex");
        guarded.socks5_addr = Some(socks5_addr);
        guarded.bootstrap_detail = None;
        guarded.mode = Indicator::Ready;
        publish_locked(&guarded, &publisher);
    }
    loop {
        tokio::time::sleep(interval).await;
        if !watchdog().await {
            die(
                &state,
                zingo_net_diag::NetOpFailure::message(
                    zingo_net_diag::NetOpStage::LocalProxyConnect,
                    socks5_addr.to_string(),
                    "the listener watchdog could not dial the attached endpoint",
                ),
            );
            return;
        }
    }
}

/// Reads `stdout` for the child's whole life, driving the mode from the
/// announcement lines and landing `Died` when the pipe closes.
async fn drive_state<R: AsyncRead + Unpin>(
    stdout: R,
    state: Arc<Mutex<ProxyState>>,
    publisher: StatusPublisher,
    launch: Option<LaunchContext>,
) {
    let mut spoke_protocol = false;
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(exit) = parse_exit_line(&line) {
            spoke_protocol = true;
            let mut guarded = state.lock().expect("proxy state mutex");
            guarded.exits.push(exit);
            // An exit announced after the address changes the Ready
            // snapshot, so it is published: a readiness waiter parked on an
            // address-first Ready wakes on this publication alone.
            if guarded.mode == Indicator::Ready {
                publish_locked(&guarded, &publisher);
            }
            continue;
        }
        if let Some(addr) = parse_socks5_addr_line(&line) {
            spoke_protocol = true;
            match addr.parse::<SocketAddr>() {
                Ok(addr) => {
                    let mut guarded = state.lock().expect("proxy state mutex");
                    guarded.socks5_addr = Some(addr);
                    guarded.bootstrap_detail = None;
                    guarded.mode = Indicator::Ready;
                    publish_locked(&guarded, &publisher);
                }
                Err(error) => {
                    // The announcement comes once, so an address that does
                    // not parse can never become Ready: the defect latches as
                    // the death cause now, instead of burning the caller's
                    // whole bootstrap budget with no diagnosis.
                    let mut guarded = state.lock().expect("proxy state mutex");
                    guarded.mode = Indicator::Died;
                    guarded.socks5_addr = None;
                    guarded.exits.clear();
                    guarded.bootstrap_detail = None;
                    guarded.death = Some(DeathReport {
                        at: std::time::SystemTime::now(),
                        detail: Some(zingo_net_diag::NetOpFailure::message(
                            zingo_net_diag::NetOpStage::ProxyLaunch,
                            addr,
                            format!(
                                "the nym-proxy announced an unparseable SOCKS5 address: {error}"
                            ),
                        )),
                    });
                    publish_locked(&guarded, &publisher);
                }
            }
            // Keep reading: a close after this must be observed as Died.
            continue;
        }
        if let Some(detail) = parse_status_line(&line) {
            spoke_protocol = true;
            let mut guarded = state.lock().expect("proxy state mutex");
            guarded.bootstrap_detail = Some(detail.to_string());
            publish_locked(&guarded, &publisher);
        }
    }
    // Stdout closed. The child exited without a deliberate stop(), so the
    // transport is lost: refuse, never leak to clearnet. A stale address is
    // cleared so no surface can dial a dead proxy.
    let detail = if spoke_protocol {
        // A closed pipe after protocol speech has no cause to hold.
        None
    } else {
        launch.as_ref().map(launch_failure_report)
    };
    let mut guarded = state.lock().expect("proxy state mutex");
    guarded.mode = Indicator::Died;
    guarded.socks5_addr = None;
    guarded.exits.clear();
    guarded.bootstrap_detail = None;
    // The latch is sticky: a close after a diagnosed defect (an unparseable
    // announcement) must not wash the held cause out with a cause-less one.
    if guarded.death.is_none() {
        guarded.death = Some(DeathReport {
            at: std::time::SystemTime::now(),
            detail,
        });
    }
    publish_locked(&guarded, &publisher);
}

/// The trailing child stderr lines a launch-death diagnosis carries.
const STDERR_TAIL_CAPACITY: usize = 8;

/// How the supervisor launched its child, held so a death before the child
/// ever speaks the stdout protocol can be diagnosed.
struct LaunchContext {
    binary: String,
    args: Vec<String>,
    stderr_tail: Arc<Mutex<std::collections::VecDeque<String>>>,
}

/// Drains the child's stderr for its whole life, logging each line and
/// keeping the trailing lines for the launch-death diagnosis.
async fn collect_stderr<R: AsyncRead + Unpin>(
    stderr: R,
    tail: Arc<Mutex<std::collections::VecDeque<String>>>,
) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::warn!("nym-proxy stderr: {line}");
        let mut guarded = tail.lock().expect("stderr tail mutex");
        if guarded.len() == STDERR_TAIL_CAPACITY {
            guarded.pop_front();
        }
        guarded.push_back(line);
    }
}

/// The typed cause for a child that exited before speaking the wallet's
/// stdout protocol: the launch arguments it may not have understood and the
/// stderr it left behind.
fn launch_failure_report(launch: &LaunchContext) -> zingo_net_diag::NetOpFailure {
    let mut cause_chain = vec![
        "the nym-proxy exited during launch without speaking the wallet's stdout protocol; \
         an installed binary older than the wallet may not understand the launch arguments \
         (version skew)"
            .to_string(),
        format!("launch arguments: {}", launch.args.join(" ")),
    ];
    let tail = launch.stderr_tail.lock().expect("stderr tail mutex");
    cause_chain.extend(tail.iter().map(|line| format!("stderr: {line}")));
    zingo_net_diag::NetOpFailure {
        stage: zingo_net_diag::NetOpStage::ProxyLaunch,
        target: launch.binary.clone(),
        cause_chain,
    }
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

/// Extract the Exit Node identity from a child stdout line, if it is an
/// exit announcement line naming a non-blank identity.
fn parse_exit_line(line: &str) -> Option<crate::mixnet::ExitNodeId> {
    line.strip_prefix(NYM_EXIT_LINE_PREFIX)
        .and_then(|identity| crate::mixnet::ExitNodeId::parse(identity).ok())
}

#[cfg(test)]
impl MixnetProxy {
    /// A transport already in [`Indicator::Ready`] with no child, watcher,
    /// or network behind it, for slot-mapping unit tests.
    pub(crate) fn ready_for_slot_tests(
        socks5_addr: SocketAddr,
        exits: Vec<crate::mixnet::ExitNodeId>,
    ) -> Self {
        MixnetProxy {
            state: Arc::new(Mutex::new(ProxyState {
                mode: Indicator::Ready,
                socks5_addr: Some(socks5_addr),
                exits,
                bootstrap_detail: None,
                death: None,
            })),
            transport: Transport::Attached {
                driver: tokio::spawn(async {}),
            },
        }
    }
}

impl crate::correspondent::pool::PoolTransport for MixnetProxy {
    fn socks5_addr(&self) -> Option<std::net::SocketAddr> {
        MixnetProxy::socks5_addr(self)
    }

    fn stop(self) -> impl std::future::Future<Output = ()> + Send {
        MixnetProxy::stop(self)
    }
}

/// Runs the proxy binary's discover mode and returns the Exit Nodes it
/// reports, the parent's one window onto the directory.
pub(crate) async fn discover_exit_nodes(
    binary_path: &Path,
) -> Result<HashSet<crate::mixnet::ExitNodeId>, acquire::TransportError> {
    let output = Command::new(binary_path)
        .arg("--discover")
        .output()
        .await
        .map_err(acquire::TransportError::DiscoverySpawn)?;
    if !output.status.success() {
        return Err(acquire::TransportError::DiscoveryFailed {
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(parse_discovery_output(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Reads every Exit Node identity the child's discover mode announced on
/// `stdout`.
fn parse_discovery_output(stdout: &str) -> HashSet<crate::mixnet::ExitNodeId> {
    stdout.lines().filter_map(parse_exit_line).collect()
}

/// Waits until the status channel reports Ready with an announced address
/// and at least one bound exit, yielding both, or fails typed when the
/// transport dies, the channel closes, `budget` elapses, or the first exit
/// misses its grace after the address.
pub(crate) async fn await_ready_endpoint(
    receiver: &mut tokio::sync::watch::Receiver<MixnetStatus>,
    budget: Duration,
) -> Result<(SocketAddr, Vec<crate::mixnet::ExitNodeId>), acquire::TransportError> {
    // The grace runs from the first addressed Ready, so a transport that
    // latches Ready and never binds an exit is refused well inside the
    // lifecycle budget instead of holding the user's go-online moment for
    // the whole of it.
    let mut exit_deadline: Option<tokio::time::Instant> = None;
    let outcome = tokio::time::timeout(budget, async {
        loop {
            let addressed = {
                let status = receiver.borrow_and_update();
                match status.mode {
                    Indicator::Ready => {
                        // Readiness is the address AND a bound exit: the
                        // child announces them on separate lines, so a Ready
                        // with no exit yet is a transient to wait through,
                        // never a defect to refuse.
                        if let (Some(addr), Some(_)) = (status.socks5_addr, status.exits.first()) {
                            return Ok((addr, status.exits.clone()));
                        }
                        status.socks5_addr.is_some()
                    }
                    Indicator::Died => {
                        return Err(acquire::TransportError::DiedDuringBootstrap {
                            detail: status.death.as_ref().and_then(|death| death.detail.clone()),
                        });
                    }
                    _ => false,
                }
            };
            if addressed && exit_deadline.is_none() {
                exit_deadline = Some(
                    tokio::time::Instant::now() + zingo_netutils::time::EXIT_ANNOUNCEMENT_GRACE,
                );
            }
            let changed = match exit_deadline {
                Some(deadline) => {
                    match tokio::time::timeout_at(deadline, receiver.changed()).await {
                        Ok(changed) => changed,
                        Err(_elapsed) => {
                            return Err(acquire::TransportError::NotReady {
                                budget: zingo_netutils::time::EXIT_ANNOUNCEMENT_GRACE,
                            });
                        }
                    }
                }
                None => receiver.changed().await,
            };
            if changed.is_err() {
                return Err(acquire::TransportError::StatusChannelClosed);
            }
        }
    })
    .await;
    match outcome {
        Ok(ready) => ready,
        Err(_elapsed) => Err(acquire::TransportError::NotReady { budget }),
    }
}

/// Acquires one transport over `clutch`, publishing its lifecycle into
/// `publisher`, and waits until it is ready, yielding the transport with the
/// exits it announced as bound.
pub(crate) async fn acquire_ready_transport(
    acquirer: &dyn crate::mixnet::acquire::TransportAcquirable,
    clutch: &[crate::mixnet::ExitNodeId],
    publisher: StatusPublisher,
) -> Result<(MixnetProxy, Vec<crate::mixnet::ExitNodeId>), acquire::TransportError> {
    let mut receiver = publisher.subscribe();
    let proxy = acquirer.acquire(clutch, publisher).await?;
    match await_ready_endpoint(&mut receiver, zingo_netutils::time::NYM_LIFECYCLE_TIMEOUT).await {
        Ok((_addr, exits)) => Ok((proxy, exits)),
        Err(cause) => {
            proxy.stop().await;
            Err(cause)
        }
    }
}

#[cfg(test)]
mod tests {
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
        let chain = zingo_net_diag::chain_texts(&failure);
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
        let s =
            state_over_open_stream(b"discovering gateways\nconnecting\nSOCKS5_ADDR=127.0.0.1:5\n")
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
        assert_eq!(detail.stage, zingo_net_diag::NetOpStage::ProxyLaunch);
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
        assert_eq!(detail.stage, zingo_net_diag::NetOpStage::ProxyLaunch);
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

    /// HYPOTHESIS: stop() on an attached transport is a deliberate teardown
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

    /// HYPOTHESIS: an attached endpoint that dies refuses the route — the
    /// fail-closed invariant holds for the attached transport end to end.
    /// Attaches to a refusing localhost port, waits for the real readiness
    /// gate to land Died, and resolves the route. Falsified if the route
    /// ever yields clearnet or the mixnet for the dead endpoint.
    #[tokio::test]
    async fn an_attached_endpoint_that_dies_refuses_the_route() {
        use crate::mixnet::route::MixnetNotReady;
        use crate::mixnet::route::resolve_route;

        // Port 9 (discard) on localhost refuses; the readiness round trip
        // fails fast and the driver lands Died.
        let proxy = MixnetProxy::attach(
            "127.0.0.1:9".parse().expect("the test address parses"),
            &[crate::mixnet::ExitNodeId::from("host-bound-exit")],
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
            resolve_route(proxy.mode(), proxy.socks5_addr()),
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
            await_ready_endpoint(&mut receiver, zingo_netutils::time::NYM_LIFECYCLE_TIMEOUT)
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
        let refusal =
            await_ready_endpoint(&mut receiver, zingo_netutils::time::NYM_LIFECYCLE_TIMEOUT)
                .await
                .expect_err("no exit ever arrives");
        assert!(
            matches!(
                refusal,
                acquire::TransportError::NotReady { budget }
                    if budget == zingo_netutils::time::EXIT_ANNOUNCEMENT_GRACE
            ),
            "the refusal names the grace it exceeded, got: {refusal}"
        );
        assert_eq!(
            started.elapsed(),
            zingo_netutils::time::EXIT_ANNOUNCEMENT_GRACE,
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
            zingo_netutils::time::ATTACH_READINESS_BUDGET,
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
}
