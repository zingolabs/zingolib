//! Embedded Nym SOCKS5 proxy for routing gRPC traffic through the Nym mixnet.
//!
//! This module wraps the nym-sdk `Socks5MixnetClient` lifecycle and provides
//! auto-discovery of public exit gateways (network requesters). It is gated
//! on the `nym` feature, whose dependencies resolve only in this crate's own
//! lockfile, never the parent workspace's, because nym-sdk's transitive
//! graph requires `crypto-common ^0.2`, which cannot coexist with the parent
//! workspace's `crypto-common =0.2.0-rc.1` pin. See
//! `docs/adr/0011-nym-mixnet-transmission.md`.
//!
//! # Architecture
//!
//! The Nym mixnet fragments traffic into Sphinx packets, shuffles them
//! through a three-layer mix network, and reassembles at an exit gateway.
//! The exit gateway runs a "network requester" service that makes the actual
//! TCP connections to the target server on behalf of the client.
//!
//! [`NymProxy`] embeds an in-process SOCKS5 proxy that connects to the
//! mixnet and listens on a localhost port. A consumer routes gRPC (or any
//! TCP) traffic through that local SOCKS5 address. The wallet-side transport
//! that dials it lives in the main workspace and needs only a SOCKS5 client,
//! not this nym-sdk stack.
//!
//! # Lifecycle
//!
//! 1. **Start**: [`NymProxy::start`] discovers public exit gateways and
//!    races hedged connect attempts across them (the pure plan lives in
//!    [`crate::arm_race`]), keeping the first winner.
//! 2. **Validate**: [`NymProxy::check_connectivity`] opens a test TCP tunnel
//!    through the proxy to verify end-to-end reachability of a target.
//! 3. **Use**: read the local SOCKS5 address from [`NymProxy::socks5_addr`].
//! 4. **Reconnect**: [`NymProxy::reconnect`] starts a fresh client on a new
//!    port, then disconnects the old one.
//! 5. **Disconnect**: [`NymProxy::disconnect`] shuts down the client cleanly.
#![forbid(unsafe_code)]

use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
};

use nym_sdk::mixnet::{MixnetClientBuilder, Socks5, Socks5MixnetClient};
use zingo_net_diag::{NetOpFailure, NetOpStage};

use crate::arm_race::acquisition_launch_policy;
use crate::arm_race::{LaunchPolicy, RaceAction, RaceEvent, RaceProgress, RaceState};
use crate::error::NymProxyError;
use crate::mixnet_connect::seeded_shuffle;

/// Default Nym API URL for mainnet.
const DEFAULT_NYM_API_URL: &str = "https://validator.nymtech.net/api/";

/// Draw a clutch of [`RESERVATION_CLUTCH_SIZE`] Exit Nodes, or every
/// discovered node when the population is smaller.
fn draw_clutch(mut discovered: Vec<String>) -> Vec<String> {
    seeded_shuffle(&mut discovered, time_entropy_seed());
    discovered.truncate(crate::arm_race::RESERVATION_CLUTCH_SIZE);
    discovered
}

/// Draw a reconnect clutch with every exit of the spent clutch excluded,
/// because a redraw that rebound the same exit would defeat the rotation it
/// exists for.
fn redraw_clutch(discovered: Vec<String>, spent: &[String]) -> Vec<String> {
    draw_clutch(
        discovered
            .into_iter()
            .filter(|exit_node| !spent.contains(exit_node))
            .collect(),
    )
}

use crate::time::{DISCOVERY_TIMEOUT, NYM_LIFECYCLE_TIMEOUT, PER_ATTEMPT_CONNECT_TIMEOUT};

/// One step of the bootstrap, carrying the Exit Node it concerns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BootstrapEvent {
    /// The Exit Node discovery query left for the Nym directory.
    DiscoveryStarted,
    /// The directory answered the discovery query.
    DiscoveryFinished {
        /// The count of Exit Nodes the directory advertised.
        candidate_count: usize,
    },
    /// A pull of this Exit Node launched.
    PullLaunched {
        /// The Exit Node address the pull races.
        exit_node: String,
    },
    /// The pull of this Exit Node failed.
    PullFailed {
        /// The Exit Node address whose pull failed.
        exit_node: String,
        /// The pull's typed failure record.
        failure: NetOpFailure,
    },
    /// The race kept this Exit Node and the local listener is up.
    Connected {
        /// The Exit Node address the proxy bound.
        exit_node: String,
    },
}

/// One step of a driven race, reported to the caller as it happens.
enum AcqStep<E> {
    /// A pull of this arm index launched.
    Launched {
        /// The arm index the pull races.
        arm: usize,
    },
    /// The pull of this arm index failed.
    Failed {
        /// The arm index whose pull failed.
        arm: usize,
        /// The pull's failure, untouched.
        error: E,
    },
    /// The race's counters changed.
    Progress(RaceProgress),
}

/// Embedded Nym SOCKS5 proxy that routes traffic through the Nym mixnet.
///
/// Manages the lifecycle of an in-process Nym SOCKS5 client connected to a
/// public exit gateway. The proxy listens on a localhost port.
pub struct NymProxy {
    client: Socks5MixnetClient,
    /// The loopback address this proxy told the mixnet client to bind, which
    /// is the address it announces to every caller.
    socks5_addr: SocketAddr,
    exit_node: String,
    /// The clutch this acquisition raced, reused by a later redraw.
    clutch: Vec<String>,
}

impl NymProxy {
    /// Start an embedded Nym SOCKS5 proxy over a clutch this call draws for
    /// itself, for a standalone run with no parent to draw one.
    pub async fn start() -> Result<Self, NymProxyError> {
        Self::start_observed(|_| {}).await
    }

    /// [`Self::start`], reporting each bootstrap step to `on_event` as a
    /// typed [`BootstrapEvent`].
    pub async fn start_observed(
        mut on_event: impl FnMut(BootstrapEvent),
    ) -> Result<Self, NymProxyError> {
        let discovered = Self::discover_observed(DEFAULT_NYM_API_URL, &mut on_event).await?;
        Self::start_over_observed(draw_clutch(discovered), on_event).await
    }

    /// Discover the directory's Exit Nodes at `nym_api_url`, reporting the
    /// query's departure and its answer to `on_event`.
    async fn discover_observed(
        nym_api_url: &str,
        on_event: &mut impl FnMut(BootstrapEvent),
    ) -> Result<Vec<String>, NymProxyError> {
        on_event(BootstrapEvent::DiscoveryStarted);
        let discovered = Self::discover_exit_nodes_at(nym_api_url).await?;
        on_event(BootstrapEvent::DiscoveryFinished {
            candidate_count: discovered.len(),
        });
        Ok(discovered)
    }

    /// Start over exactly `clutch`, the Exit Node Reservations the parent
    /// drew, reporting each bootstrap step to `on_progress`.
    pub async fn start_over(
        clutch: Vec<String>,
        on_progress: impl FnMut(String),
    ) -> Result<Self, NymProxyError> {
        Self::start_bounded(clutch, |_| {}, on_progress).await
    }

    /// [`Self::start_over`], reporting typed [`BootstrapEvent`]s instead of
    /// prose lines.
    pub async fn start_over_observed(
        clutch: Vec<String>,
        on_event: impl FnMut(BootstrapEvent),
    ) -> Result<Self, NymProxyError> {
        Self::start_bounded(clutch, on_event, |_| {}).await
    }

    async fn start_bounded(
        clutch: Vec<String>,
        on_event: impl FnMut(BootstrapEvent),
        on_progress: impl FnMut(String),
    ) -> Result<Self, NymProxyError> {
        tokio::time::timeout(
            NYM_LIFECYCLE_TIMEOUT,
            Self::start_inner(clutch, on_event, on_progress),
        )
        .await
        .map_err(|_| {
            NymProxyError::ConnectivityCheck(format!(
                "start timed out after {}s",
                NYM_LIFECYCLE_TIMEOUT.as_secs()
            ))
        })?
    }

    async fn start_inner(
        clutch: Vec<String>,
        on_event: impl FnMut(BootstrapEvent),
        mut on_progress: impl FnMut(String),
    ) -> Result<Self, NymProxyError> {
        if clutch.is_empty() {
            return Err(NymProxyError::NoExitNode);
        }
        on_progress(format!("racing a clutch of {} exits", clutch.len()));
        let mut proxy = Self::connect_across_exit_nodes(&clutch, on_event, on_progress).await?;
        proxy.clutch = clutch;
        Ok(proxy)
    }

    /// Race the clutch under the one hedged launch policy, each pull bounded
    /// by [`PER_ATTEMPT_CONNECT_TIMEOUT`] and binding its own fresh port; an
    /// arm wins by binding, and proof of the bound exit belongs to the layer
    /// above the SOCKS5 seam.
    async fn connect_across_exit_nodes(
        exit_nodes: &[String],
        mut on_event: impl FnMut(BootstrapEvent),
        mut on_progress: impl FnMut(String),
    ) -> Result<Self, NymProxyError> {
        // A panicked pull can lose its arm index, so the identity lookup
        // falls back the way short_exit_node_name does.
        let full_exit_node_name = |arm: usize| {
            exit_nodes
                .get(arm)
                .cloned()
                .unwrap_or_else(|| format!("exit node {arm}"))
        };
        let proxy = drive_acq_race(
            exit_nodes.len(),
            exit_nodes.len(),
            acquisition_launch_policy(),
            |arm| {
                let exit_node = exit_nodes[arm].clone();
                let target = short_exit_node_name(exit_nodes, arm);
                async move {
                    let port = Self::find_available_port()
                        .map_err(|e| exit_node_attempt_failure(&e, &target))?;
                    match tokio::time::timeout(
                        PER_ATTEMPT_CONNECT_TIMEOUT,
                        Self::start_with_config(&exit_node, port),
                    )
                    .await
                    {
                        Err(_elapsed) => Err(exit_node_attempt_failure(
                            &NymProxyError::AttemptTimeout(PER_ATTEMPT_CONNECT_TIMEOUT.as_secs()),
                            &target,
                        )),
                        Ok(Err(e)) => Err(exit_node_attempt_failure(&e, &target)),
                        Ok(Ok(proxy)) => Ok(proxy),
                    }
                }
            },
            |second_winner: NymProxy| async move { second_winner.disconnect().await },
            // A panicked pull task died mid-connect; its record carries the
            // panic text as the single-layer cause.
            |arm, text| {
                NetOpFailure::message(
                    NetOpStage::RemoteConnect,
                    short_exit_node_name(exit_nodes, arm),
                    text,
                )
            },
            |step| match step {
                AcqStep::Launched { arm } => on_event(BootstrapEvent::PullLaunched {
                    exit_node: full_exit_node_name(arm),
                }),
                AcqStep::Failed { arm, error } => on_event(BootstrapEvent::PullFailed {
                    exit_node: full_exit_node_name(arm),
                    failure: error,
                }),
                AcqStep::Progress(progress) => on_progress(progress.to_string()),
            },
        )
        .await
        .map_err(acq_race_loss_error)?;
        on_event(BootstrapEvent::Connected {
            exit_node: proxy.exit_node().to_string(),
        });
        Ok(proxy)
    }

    /// The Exit Nodes the Nym directory currently advertises, for
    /// callers that assign exits themselves (the pool discovery's
    /// uniform sampling without replacement, ADR 0029) instead of racing
    /// [`Self::start`]'s hedged draw.
    pub async fn discover_exit_nodes() -> Result<Vec<String>, NymProxyError> {
        Self::discover_exit_nodes_at(DEFAULT_NYM_API_URL).await
    }

    /// Start with a specific Exit Node address.
    ///
    /// Use this to pin a specific Nym network requester instead of
    /// auto-discovering one. The `exit_node_address` is a Nym `Recipient`
    /// address in base58 format (`<client_id>.<client_enc>@<gateway_id>`).
    /// Listens on a random available localhost port.
    pub async fn start_with_exit_node(exit_node_address: &str) -> Result<Self, NymProxyError> {
        let port = Self::find_available_port()?;
        Self::start_with_config(exit_node_address, port).await
    }

    /// Start with a specific Exit Node and custom local bind port.
    ///
    /// Useful when running multiple Nym proxies or when a specific port is
    /// required.
    pub async fn start_with_config(
        exit_node_address: &str,
        bind_port: u16,
    ) -> Result<Self, NymProxyError> {
        let socks5_cfg = Socks5::new(exit_node_address);
        let socks5_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bind_port);
        let client = MixnetClientBuilder::new_ephemeral()
            .socks5_config(Socks5 {
                bind_address: socks5_addr,
                ..socks5_cfg
            })
            .build()
            .map_err(|e| NymProxyError::Build(Box::new(e)))?
            .connect_to_mixnet_via_socks5()
            .await
            .map_err(|e| NymProxyError::Connect(Box::new(e)))?;
        Ok(Self {
            client,
            socks5_addr,
            exit_node: exit_node_address.to_string(),
            clutch: Vec::new(),
        })
    }

    /// The Exit Node identity this transport bound.
    pub fn exit_node(&self) -> &str {
        &self.exit_node
    }

    /// The local SOCKS5 proxy address, the loopback socket address this proxy
    /// listens on.
    pub fn socks5_addr(&self) -> SocketAddr {
        self.socks5_addr
    }

    /// The local bind port the SOCKS5 proxy listens on.
    pub fn bind_port(&self) -> u16 {
        self.socks5_addr.port()
    }

    /// Verify that a TCP connection can be established through this proxy to
    /// the given target host and port.
    ///
    /// Opens a SOCKS5 tunnel to the target, verifying end-to-end reachability
    /// through the Nym mixnet. The connection is dropped immediately after
    /// success.
    pub async fn check_connectivity(
        &self,
        target_host: &str,
        target_port: u16,
    ) -> Result<(), NymProxyError> {
        let _stream =
            tokio_socks::tcp::Socks5Stream::connect(self.socks5_addr(), (target_host, target_port))
                .await
                .map_err(|e| NymProxyError::ConnectivityCheck(e.to_string()))?;
        Ok(())
    }

    /// Disconnect the current mixnet client and start a fresh one.
    ///
    /// Rediscovers Exit Nodes and connects on a **new** local port to avoid
    /// binding conflicts with the still-running old client. The old client is
    /// disconnected only after the new one succeeds. If all connection
    /// attempts fail, the old client remains untouched and the error is
    /// returned. After a successful reconnect, [`socks5_addr`](Self::socks5_addr)
    /// returns the new port.
    pub async fn reconnect(&mut self) -> Result<(), NymProxyError> {
        self.reconnect_observed(|_| {}).await
    }

    /// [`Self::reconnect`], reporting each bootstrap step to `on_event` as a
    /// typed [`BootstrapEvent`].
    pub async fn reconnect_observed(
        &mut self,
        on_event: impl FnMut(BootstrapEvent),
    ) -> Result<(), NymProxyError> {
        tokio::time::timeout(NYM_LIFECYCLE_TIMEOUT, self.reconnect_inner(on_event))
            .await
            .map_err(|_| {
                NymProxyError::ConnectivityCheck(format!(
                    "reconnect timed out after {}s",
                    NYM_LIFECYCLE_TIMEOUT.as_secs()
                ))
            })?
    }

    async fn reconnect_inner(
        &mut self,
        mut on_event: impl FnMut(BootstrapEvent),
    ) -> Result<(), NymProxyError> {
        let discovered = Self::discover_observed(DEFAULT_NYM_API_URL, &mut on_event).await?;
        let clutch = redraw_clutch(discovered, &self.clutch);
        if clutch.is_empty() {
            return Err(NymProxyError::NoExitNode);
        }
        // Each attempt binds its own fresh port, which cannot collide with
        // the old client's still-bound port.
        let new_proxy = Self::connect_across_exit_nodes(&clutch, on_event, |_| {}).await?;

        // Swap only after the new client succeeded, so a failed reconnect
        // leaves the old client untouched.
        let old_client = std::mem::replace(&mut self.client, new_proxy.client);
        self.socks5_addr = new_proxy.socks5_addr;
        self.exit_node = new_proxy.exit_node;
        self.clutch = clutch;
        old_client.disconnect().await;
        Ok(())
    }

    /// Disconnect from the Nym mixnet and stop the local SOCKS5 proxy.
    pub async fn disconnect(self) {
        self.client.disconnect().await;
    }

    /// Find an available localhost port by briefly binding to port 0.
    ///
    /// # TOCTOU race
    ///
    /// There is an inherent race between dropping the listener and the Nym
    /// SDK rebinding to the same port: another process could claim it in
    /// between. This is a fundamental limitation of the bind-to-0-then-drop
    /// pattern (also used by `portpicker`). In practice the race is extremely
    /// unlikely and causes a connection retry, not a security issue, since
    /// `start()` retries across multiple gateways.
    fn find_available_port() -> Result<u16, NymProxyError> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| {
            NymProxyError::DiscoveryApi(format!("failed to find available port: {e}"))
        })?;
        let port = listener
            .local_addr()
            .map_err(|e| NymProxyError::DiscoveryApi(format!("failed to get port: {e}")))?
            .port();
        drop(listener);
        Ok(port)
    }

    /// Query the Nym API for active exit gateways running a network requester.
    ///
    /// Returns addresses shuffled for load distribution
    /// ([`seeded_shuffle`] on [`time_entropy_seed`], see its docs for why
    /// this is deliberately not cryptographic randomness). Callers should try
    /// multiple entries since individual gateways may be offline.
    async fn discover_exit_nodes_at(nym_api_url: &str) -> Result<Vec<String>, NymProxyError> {
        use nym_validator_client::nym_api::NymApiClientExt as _;

        let api_client = nym_http_api_client::Client::builder(nym_api_url)
            .map_err(|e| NymProxyError::DiscoveryApi(e.to_string()))?
            .build()
            .map_err(|e| NymProxyError::DiscoveryApi(e.to_string()))?;

        let described_nodes =
            tokio::time::timeout(DISCOVERY_TIMEOUT, api_client.get_all_described_nodes_v2())
                .await
                .map_err(|_| {
                    NymProxyError::DiscoveryApi(format!(
                        "discovery timed out after {}s",
                        DISCOVERY_TIMEOUT.as_secs()
                    ))
                })?
                .map_err(|e| NymProxyError::DiscoveryApi(e.to_string()))?;

        // Collect all nodes that have a network requester with an address.
        let mut exit_nodes: Vec<String> = described_nodes
            .iter()
            .filter_map(|node| node.description.network_requester.as_ref())
            .map(|nr| nr.address.clone())
            .filter(|addr| !addr.is_empty())
            .collect();

        if exit_nodes.is_empty() {
            return Err(NymProxyError::NoExitNode);
        }

        seeded_shuffle(&mut exit_nodes, time_entropy_seed());
        Ok(exit_nodes)
    }
}

/// The producer classifier for one raced connect pull: a pure, total match
/// from the pull's [`NymProxyError`] to its taxonomy stage
/// (`docs/agents/net-diag-design.md`). Classification lives at this seam
/// because this crate owns the error type. The match covers every variant,
/// but per pull only the port draw ([`NymProxyError::DiscoveryApi`]), the
/// client construction, the mixnet registration, and the per-attempt
/// timeout actually occur.
fn exit_node_attempt_stage(error: &NymProxyError) -> NetOpStage {
    match error {
        // Client construction, the local port draw, discovery, and an
        // exclusion-emptied draw all fail before this pull touches the
        // network.
        NymProxyError::Build(_) | NymProxyError::DiscoveryApi(_) | NymProxyError::NoExitNode => {
            NetOpStage::RouteResolution
        }
        // Registering with the Exit Node is the pull's remote
        // connect. An exhausted race never occurs per pull; it summarizes
        // the connects it is made of, so it classifies with them.
        NymProxyError::Connect(_) | NymProxyError::AttemptsExhausted { .. } => {
            NetOpStage::RemoteConnect
        }
        NymProxyError::ConnectivityCheck(_) => NetOpStage::TunnelTransport,
        NymProxyError::AttemptTimeout(secs) => NetOpStage::TimedOut {
            after_ms: secs * 1000,
        },
    }
}

/// One pull's typed failure record: the classified stage, the shortened
/// Exit Node name as the target, and the error's cause chain captured layer
/// by layer.
fn exit_node_attempt_failure(error: &NymProxyError, target: &str) -> NetOpFailure {
    NetOpFailure::from_error(exit_node_attempt_stage(error), target, error)
}

/// The terminal error of a lost race: [`NymProxyError::NoExitNode`] when
/// nothing was ever contacted, otherwise the typed per-Exit-Node attempts
/// carried whole into [`NymProxyError::AttemptsExhausted`].
fn acq_race_loss_error(lost: LostAcqRace<NetOpFailure>) -> NymProxyError {
    if lost.launched == 0 {
        NymProxyError::NoExitNode
    } else {
        NymProxyError::AttemptsExhausted {
            attempts: lost.launched,
            failures: lost.failures,
        }
    }
}

/// An Exit Node mix-address shortened for an error summary: the full base58
/// `Recipient` form runs to hundreds of characters, and ten of them would
/// drown the failure it is naming.
fn short_exit_node_name(exit_nodes: &[String], arm: usize) -> String {
    match exit_nodes.get(arm) {
        Some(exit_node) if exit_node.chars().count() > 15 => {
            let head: String = exit_node.chars().take(12).collect();
            format!("{head}…")
        }
        Some(exit_node) => exit_node.clone(),
        None => format!("exit node {arm}"),
    }
}

/// A lost race's terminal account: how many distinct pulls were pulled,
/// and every pull's typed failure in completion order. The driver
/// retains the failures whole (the planner keeps only the rendered line
/// each one fed to its progress narration), mirroring the transmission
/// escalation's shape (issue #2562).
#[derive(Debug)]
struct LostAcqRace<E> {
    /// Distinct pulls pulled before the race was lost.
    launched: usize,
    /// Every pull's failure, untouched, in completion order.
    failures: Vec<E>,
}

/// Execute a pure racing plan ([`crate::arm_race`]) over real tokio tasks:
/// launch pulls as the planner directs, keep at most one pending hedge timer,
/// feed completions back as events, and stop at the first winner, aborting
/// the pulls still in flight and handing any pull that had already finished as
/// a second winner to `abandon` rather than leaking it. A lost race returns
/// a [`LostAcqRace`] carrying every attempt's typed failure. A panicked pull is
/// a failed pull; `describe_panic` renders its record from the arm
/// index and the join error's text.
async fn drive_acq_race<T, E, F, Fut, D, DFut>(
    arms: usize,
    cap: usize,
    policy: LaunchPolicy,
    launch: F,
    abandon: D,
    describe_panic: impl Fn(usize, String) -> E,
    mut on_step: impl FnMut(AcqStep<E>),
) -> Result<T, LostAcqRace<E>>
where
    T: Send + 'static,
    E: Clone + std::fmt::Display + Send + 'static,
    F: Fn(usize) -> Fut,
    Fut: Future<Output = Result<T, E>> + Send + 'static,
    D: Fn(T) -> DFut,
    DFut: Future<Output = ()>,
{
    let mut acq_race = RaceState::new(arms, cap, policy);
    let mut pulls: tokio::task::JoinSet<(usize, Result<T, E>)> = tokio::task::JoinSet::new();
    let mut pull_arms: HashMap<tokio::task::Id, usize> = HashMap::new();
    let mut hedge_deadline: Option<tokio::time::Instant> = None;
    let mut lost = false;
    let mut failures: Vec<E> = Vec::new();

    let apply = |actions: Vec<RaceAction>,
                 pulls: &mut tokio::task::JoinSet<(usize, Result<T, E>)>,
                 pull_arms: &mut HashMap<tokio::task::Id, usize>,
                 hedge_deadline: &mut Option<tokio::time::Instant>,
                 lost: &mut bool,
                 launched: &mut Vec<usize>| {
        for action in actions {
            match action {
                RaceAction::Launch { arm } => {
                    let pull = launch(arm);
                    let handle = pulls.spawn(async move { (arm, pull.await) });
                    pull_arms.insert(handle.id(), arm);
                    launched.push(arm);
                }
                RaceAction::SetHedgeTimer(interval) => {
                    *hedge_deadline = Some(tokio::time::Instant::now() + interval);
                }
                RaceAction::GiveUp => *lost = true,
            }
        }
    };
    // The apply closure cannot call the generic `on_step` directly, so it
    // records launches and each call site drains them into steps.
    let mut launched: Vec<usize> = Vec::new();

    apply(
        acq_race.start(),
        &mut pulls,
        &mut pull_arms,
        &mut hedge_deadline,
        &mut lost,
        &mut launched,
    );
    for arm in launched.drain(..) {
        on_step(AcqStep::Launched { arm });
    }
    on_step(AcqStep::Progress(acq_race.progress()));

    loop {
        if lost {
            return Err(LostAcqRace {
                launched: acq_race.launched(),
                failures,
            });
        }
        tokio::select! {
            // Prefer completions over the hedge timer, so a finished pull is
            // never preempted by a simultaneous timer firing.
            biased;
            joined = pulls.join_next_with_id() => {
                let Some(joined) = joined else {
                    return Err(LostAcqRace {
                        launched: acq_race.launched(),
                        failures,
                    });
                };
                let (arm, outcome) = match joined {
                    Ok((id, (arm, outcome))) => {
                        pull_arms.remove(&id);
                        (arm, outcome)
                    }
                    Err(join_error) => {
                        // A panicked pull is a failed pull; recover which
                        // arm it pulled so the accounting stays exact.
                        let arm = pull_arms
                            .remove(&join_error.id())
                            .unwrap_or(usize::MAX);
                        let text = format!("pull task failed: {join_error}");
                        (arm, Err(describe_panic(arm, text)))
                    }
                };
                match outcome {
                    Ok(winner) => {
                        pulls.abort_all();
                        while let Some(late) = pulls.join_next().await {
                            if let Ok((_, Ok(second_winner))) = late {
                                abandon(second_winner).await;
                            }
                        }
                        return Ok(winner);
                    }
                    Err(error) => {
                        // The planner's event wants a rendered line for its
                        // progress narration; the typed failure is retained
                        // whole for the terminal account.
                        on_step(AcqStep::Failed {
                            arm,
                            error: error.clone(),
                        });
                        apply(
                            acq_race.on_event(RaceEvent::PullFailed {
                                arm,
                                error: error.to_string(),
                            }),
                            &mut pulls,
                            &mut pull_arms,
                            &mut hedge_deadline,
                            &mut lost,
                            &mut launched,
                        );
                        for arm in launched.drain(..) {
                            on_step(AcqStep::Launched { arm });
                        }
                        failures.push(error);
                        on_step(AcqStep::Progress(acq_race.progress()));
                    }
                }
            }
            () = tokio::time::sleep_until(
                hedge_deadline.unwrap_or_else(tokio::time::Instant::now)
            ), if hedge_deadline.is_some() => {
                hedge_deadline = None;
                apply(
                    acq_race.on_event(RaceEvent::HedgeElapsed),
                    &mut pulls,
                    &mut pull_arms,
                    &mut hedge_deadline,
                    &mut lost,
                    &mut launched,
                );
                for arm in launched.drain(..) {
                    on_step(AcqStep::Launched { arm });
                }
                on_step(AcqStep::Progress(acq_race.progress()));
            }
        }
    }
}

/// The entropy source for Exit Node shuffling: a hash of the current time.
/// The one effect feeding the pure [`seeded_shuffle`].
fn time_entropy_seed() -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut hasher = DefaultHasher::new();
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
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
            NymProxy::discover_observed("http://127.0.0.1:0/", &mut |event| events.push(event))
                .await;
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
    //   cargo test --manifest-path zingo-netutils/Cargo.toml --features nym -- --ignored

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
        let stream = tokio_socks::tcp::Socks5Stream::connect(proxy.socks5_addr(), "zec.rocks:443")
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
}
