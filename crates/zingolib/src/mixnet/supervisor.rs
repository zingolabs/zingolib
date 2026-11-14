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
//! mixnet-only surfaces fail closed rather than fall back to nakednet. Only a
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
use zl_netutils::time::{
    ATTACH_LISTENER_RETRY_PAUSE, ATTACH_WATCHDOG_INTERVAL, LOOPBACK_DIAL_BOUND,
    MIXNET_ROUND_TRIP_BOUND,
};
use zl_netutils::{NYM_EXIT_LINE_PREFIX, NYM_STATUS_LINE_PREFIX, SOCKS5_ADDR_LINE_PREFIX};

use crate::mixnet::Indicator;
use crate::mixnet::acquire;
use crate::mixnet::driver::{MixnetStatus, StatusPublisher};

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
    #[error("the session has no indexer for the attach readiness round trip")]
    NoSessionIndexer,
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
    pub detail: Option<zl_netutils::diag::NetOpFailure>,
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

    /// - Spawns the attached-state driver, whose readiness gate round-trips
    ///   through `socks5_addr` to `indexer`, the Session Indexer.
    /// - Publishes every lifecycle transition into `publisher`.
    pub(crate) fn attach(
        socks5_addr: SocketAddr,
        exits: &[crate::mixnet::ExitNodeId],
        indexer: http::Uri,
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
            attach_readiness(socks5_addr, indexer),
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
    pub fn death_detail(&self) -> Option<zl_netutils::diag::NetOpFailure> {
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

/// - Opens up to [`ATTACH_HEALTH_ATTEMPTS`] data round trips through
///   `socks5_addr` to `indexer`, the Session Indexer, each bounded by
///   [`MIXNET_ROUND_TRIP_BOUND`], because a listener that accepts TCP
///   proves nothing about the mixnet carrying data.
async fn attach_readiness(
    socks5_addr: SocketAddr,
    indexer: http::Uri,
) -> Result<(), zl_netutils::diag::NetOpFailure> {
    let indexer_text = indexer.to_string();
    let probe = zl_netutils::Socks5Indexer::new(socks5_addr, indexer, MIXNET_ROUND_TRIP_BOUND);
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
                    zl_netutils::diag::NetOpStage::LocalProxyConnect
                    | zl_netutils::diag::NetOpStage::SocksHandshake => socks5_addr.to_string(),
                    _ => indexer_text.clone(),
                };
                last_failure = Some(zl_netutils::diag::NetOpFailure::from_error(
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
    RFut: Future<Output = Result<(), zl_netutils::diag::NetOpFailure>>,
    P: FnMut() -> PFut,
    PFut: Future<Output = bool>,
{
    let die = |state: &Arc<Mutex<ProxyState>>, cause: zl_netutils::diag::NetOpFailure| {
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
                zl_netutils::diag::NetOpFailure::message(
                    zl_netutils::diag::NetOpStage::LocalProxyConnect,
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
                        detail: Some(zl_netutils::diag::NetOpFailure::message(
                            zl_netutils::diag::NetOpStage::ProxyLaunch,
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
    // transport is lost: refuse, never leak to nakednet. A stale address is
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
fn launch_failure_report(launch: &LaunchContext) -> zl_netutils::diag::NetOpFailure {
    let mut cause_chain = vec![
        "the nym-proxy exited during launch without speaking the wallet's stdout protocol; \
         an installed binary older than the wallet may not understand the launch arguments \
         (version skew)"
            .to_string(),
        format!("launch arguments: {}", launch.args.join(" ")),
    ];
    let tail = launch.stderr_tail.lock().expect("stderr tail mutex");
    cause_chain.extend(tail.iter().map(|line| format!("stderr: {line}")));
    zl_netutils::diag::NetOpFailure {
        stage: zl_netutils::diag::NetOpStage::ProxyLaunch,
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

impl crate::mixnet::pools::PoolTransport for MixnetProxy {
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
    // Every birth's announcement latency is measured here, at the one seam
    // every birth passes through, because the grace budget was chosen
    // without a distribution to choose it against (ADR 0045).
    let started = std::time::Instant::now();
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
                exit_deadline =
                    Some(tokio::time::Instant::now() + zl_netutils::time::EXIT_ANNOUNCEMENT_GRACE);
            }
            let changed = match exit_deadline {
                Some(deadline) => {
                    match tokio::time::timeout_at(deadline, receiver.changed()).await {
                        Ok(changed) => changed,
                        Err(_elapsed) => {
                            return Err(acquire::TransportError::NotReady {
                                budget: zl_netutils::time::EXIT_ANNOUNCEMENT_GRACE,
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
        Ok(ready) => {
            if ready.is_ok() {
                log::info!(
                    "birth announced its exit in {}ms of a {}ms grace",
                    started.elapsed().as_millis(),
                    budget.as_millis()
                );
            }
            ready
        }
        Err(_elapsed) => {
            log::info!(
                "birth announced no exit within its {}ms grace",
                budget.as_millis()
            );
            Err(acquire::TransportError::NotReady { budget })
        }
    }
}

/// Acquires one transport over `clutch`, publishing its lifecycle into
/// `publisher`, and waits until it is ready, yielding the transport with the
/// exits it announced as bound.
pub(crate) async fn acquire_ready_transport<A>(
    acquirer: &A,
    clutch: &[crate::mixnet::ExitNodeId],
    publisher: StatusPublisher,
) -> Result<(MixnetProxy, Vec<crate::mixnet::ExitNodeId>), acquire::TransportError>
where
    A: crate::mixnet::acquire::TransportAcquirable + ?Sized,
{
    let mut receiver = publisher.subscribe();
    let proxy = acquirer.acquire(clutch, publisher).await?;
    match await_ready_endpoint(&mut receiver, zl_netutils::time::NYM_LIFECYCLE_TIMEOUT).await {
        Ok((_addr, exits)) => Ok((proxy, exits)),
        Err(cause) => {
            proxy.stop().await;
            Err(cause)
        }
    }
}

#[cfg(test)]
mod tests;
