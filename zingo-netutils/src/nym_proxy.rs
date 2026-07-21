//! Embedded Nym SOCKS5 proxy for routing gRPC traffic through the Nym mixnet.
//!
//! This module wraps the nym-sdk `Socks5MixnetClient` lifecycle and provides
//! auto-discovery of public exit gateways (network requesters). It is gated
//! on the `nym` feature, whose dependencies resolve only in this crate's own
//! lockfile — never the parent workspace's — because nym-sdk's transitive
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
//! TCP) traffic through that local SOCKS5 address; the wallet-side transport
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
    time::Duration,
};

use nym_sdk::mixnet::{MixnetClientBuilder, Socks5, Socks5MixnetClient};

use crate::arm_race::{LaunchPolicy, RaceAction, RaceEvent, RaceProgress, RaceState};
use crate::error::NymProxyError;
use crate::mixnet_connect::{seeded_shuffle, strip_socks5_scheme};

/// Default Nym API URL for mainnet.
const DEFAULT_NYM_API_URL: &str = "https://validator.nymtech.net/api/";

/// Maximum number of providers to try before giving up.
const MAX_PROVIDER_ATTEMPTS: usize = 10;

/// The most simultaneous connect attempts the hedged bootstrap holds in
/// flight. Each attempt is a full ephemeral mixnet client registration, so
/// parallelism is deliberately narrow.
const MAX_PARALLEL_CONNECTS: usize = 3;

/// How long the hedged bootstrap stays quiet before launching another
/// provider in parallel. A responsive provider typically connects in well
/// under ten seconds, so an attempt this old is worth hedging against
/// without yet giving up on it.
const HEDGE_INTERVAL: Duration = Duration::from_secs(5);

/// Overall timeout for `start()` and `reconnect()` to prevent infinite hangs.
///
/// Nym SDK connection attempts can block indefinitely if a gateway is
/// unresponsive. This timeout caps total wall-clock time for the entire
/// retry loop; [`PER_ATTEMPT_CONNECT_TIMEOUT`] caps individual attempts.
const NYM_LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Timeout for a single provider connect attempt.
///
/// Without this bound, one unresponsive provider hangs
/// `connect_to_mixnet_via_socks5` until the whole [`NYM_LIFECYCLE_TIMEOUT`]
/// budget burns, and the retry engine never reaches the next provider. A
/// responsive provider bootstraps in well under ten seconds; six full
/// attempts fit inside the lifecycle budget.
const PER_ATTEMPT_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Timeout for the provider-discovery API query, which is otherwise
/// unbounded for the same reason as the connect attempts.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);

/// Embedded Nym SOCKS5 proxy that routes traffic through the Nym mixnet.
///
/// Manages the lifecycle of an in-process Nym SOCKS5 client connected to a
/// public exit gateway. The proxy listens on a localhost port.
pub struct NymProxy {
    client: Socks5MixnetClient,
    bind_port: u16,
}

impl NymProxy {
    /// Start an embedded Nym SOCKS5 proxy using an auto-discovered public exit gateway.
    ///
    /// Queries the Nym API for active exit gateways, then races hedged
    /// connect attempts across them, keeping the first winner. The proxy
    /// listens on a random available localhost port. This is the recommended
    /// entry point — no Nym-specific addresses are required.
    pub async fn start() -> Result<Self, NymProxyError> {
        Self::start_with_progress(|_| {}).await
    }

    /// [`Self::start`], reporting each bootstrap step to `on_progress` as a
    /// human-readable line. The spawnable binary forwards these lines to the
    /// wallet supervisor, so a user watching `nym status` sees the race
    /// advance instead of an opaque wait.
    pub async fn start_with_progress(
        on_progress: impl FnMut(String),
    ) -> Result<Self, NymProxyError> {
        tokio::time::timeout(NYM_LIFECYCLE_TIMEOUT, Self::start_inner(on_progress))
            .await
            .map_err(|_| {
                NymProxyError::ConnectivityCheck(format!(
                    "start timed out after {}s",
                    NYM_LIFECYCLE_TIMEOUT.as_secs()
                ))
            })?
    }

    async fn start_inner(mut on_progress: impl FnMut(String)) -> Result<Self, NymProxyError> {
        on_progress("discovering exit gateways".to_string());
        let providers = Self::discover_providers(DEFAULT_NYM_API_URL).await?;
        Self::connect_across_providers(&providers, on_progress).await
    }

    /// Race the pure hedged plan ([`crate::arm_race`]) over `providers`: one
    /// arm first, another after each quiet [`HEDGE_INTERVAL`] or immediately
    /// on a failure, at most [`MAX_PARALLEL_CONNECTS`] in flight and
    /// [`MAX_PROVIDER_ATTEMPTS`] contacted. Each arm is bounded by
    /// [`PER_ATTEMPT_CONNECT_TIMEOUT`] and binds a fresh port, since a
    /// timed-out arm may still hold the port it was given. A loser that
    /// finishes connecting after the winner is disconnected, not leaked.
    /// Shared by [`Self::start`] and [`Self::reconnect`].
    async fn connect_across_providers(
        providers: &[String],
        mut on_progress: impl FnMut(String),
    ) -> Result<Self, NymProxyError> {
        drive_race(
            providers.len(),
            MAX_PROVIDER_ATTEMPTS,
            LaunchPolicy::Hedged {
                max_parallel: MAX_PARALLEL_CONNECTS,
                hedge_interval: HEDGE_INTERVAL,
            },
            |candidate| {
                let provider = providers[candidate].clone();
                async move {
                    let port = Self::find_available_port().map_err(|e| e.to_string())?;
                    tokio::time::timeout(
                        PER_ATTEMPT_CONNECT_TIMEOUT,
                        Self::start_with_config(&provider, port),
                    )
                    .await
                    .map_err(|_| {
                        NymProxyError::AttemptTimeout(PER_ATTEMPT_CONNECT_TIMEOUT.as_secs())
                            .to_string()
                    })?
                    .map_err(|e| e.to_string())
                }
            },
            |second_winner: NymProxy| async move { second_winner.disconnect().await },
            |progress| on_progress(progress.to_string()),
        )
        .await
        .map_err(|race| {
            if race.launched() == 0 {
                NymProxyError::NoProvider
            } else {
                NymProxyError::AttemptsExhausted(
                    race.failure_summary(|i| short_provider_name(providers, i)),
                )
            }
        })
    }

    /// Start with a specific exit gateway provider address.
    ///
    /// Use this to pin a specific Nym network requester instead of
    /// auto-discovering one. The `provider_mix_address` is a Nym `Recipient`
    /// address in base58 format (`<client_id>.<client_enc>@<gateway_id>`).
    /// Listens on a random available localhost port.
    pub async fn start_with_provider(provider_mix_address: &str) -> Result<Self, NymProxyError> {
        let port = Self::find_available_port()?;
        Self::start_with_config(provider_mix_address, port).await
    }

    /// Start with a specific provider and custom local bind port.
    ///
    /// Useful when running multiple Nym proxies or when a specific port is
    /// required.
    pub async fn start_with_config(
        provider_mix_address: &str,
        bind_port: u16,
    ) -> Result<Self, NymProxyError> {
        let socks5_cfg = Socks5::new(provider_mix_address);
        let client = MixnetClientBuilder::new_ephemeral()
            .socks5_config(Socks5 {
                bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bind_port),
                ..socks5_cfg
            })
            .build()
            .map_err(|e| NymProxyError::Build(Box::new(e)))?
            .connect_to_mixnet_via_socks5()
            .await
            .map_err(|e| NymProxyError::Connect(Box::new(e)))?;
        Ok(Self { client, bind_port })
    }

    /// The local SOCKS5 proxy address (e.g., `"127.0.0.1:43210"`).
    pub fn socks5_addr(&self) -> String {
        strip_socks5_scheme(&self.client.socks5_url()).to_string()
    }

    /// The local bind port the SOCKS5 proxy listens on.
    pub fn bind_port(&self) -> u16 {
        self.bind_port
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
        let addr = self.socks5_addr();
        let _stream = tokio_socks::tcp::Socks5Stream::connect(&*addr, (target_host, target_port))
            .await
            .map_err(|e| NymProxyError::ConnectivityCheck(e.to_string()))?;
        Ok(())
    }

    /// Disconnect the current mixnet client and start a fresh one.
    ///
    /// Rediscovers providers and connects on a **new** local port to avoid
    /// binding conflicts with the still-running old client. The old client is
    /// disconnected only after the new one succeeds. If all connection
    /// attempts fail, the old client remains untouched and the error is
    /// returned. After a successful reconnect, [`socks5_addr`](Self::socks5_addr)
    /// returns the new port.
    pub async fn reconnect(&mut self) -> Result<(), NymProxyError> {
        tokio::time::timeout(NYM_LIFECYCLE_TIMEOUT, self.reconnect_inner())
            .await
            .map_err(|_| {
                NymProxyError::ConnectivityCheck(format!(
                    "reconnect timed out after {}s",
                    NYM_LIFECYCLE_TIMEOUT.as_secs()
                ))
            })?
    }

    async fn reconnect_inner(&mut self) -> Result<(), NymProxyError> {
        let providers = Self::discover_providers(DEFAULT_NYM_API_URL).await?;
        // Each attempt binds its own fresh port, which cannot collide with
        // the old client's still-bound port.
        let new_proxy = Self::connect_across_providers(&providers, |_| {}).await?;

        // Swap only after the new client succeeded, so a failed reconnect
        // leaves the old client untouched.
        let old_client = std::mem::replace(&mut self.client, new_proxy.client);
        self.bind_port = new_proxy.bind_port;
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
    /// ([`seeded_shuffle`] on [`time_entropy_seed`] — see its docs for why
    /// this is deliberately not cryptographic randomness). Callers should try
    /// multiple entries since individual gateways may be offline.
    async fn discover_providers(nym_api_url: &str) -> Result<Vec<String>, NymProxyError> {
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
        let mut providers: Vec<String> = described_nodes
            .iter()
            .filter_map(|node| node.description.network_requester.as_ref())
            .map(|nr| nr.address.clone())
            .filter(|addr| !addr.is_empty())
            .collect();

        if providers.is_empty() {
            return Err(NymProxyError::NoProvider);
        }

        seeded_shuffle(&mut providers, time_entropy_seed());
        Ok(providers)
    }
}

/// A provider mix-address shortened for an error summary: the full base58
/// `Recipient` form runs to hundreds of characters, and ten of them would
/// drown the failure it is naming.
fn short_provider_name(providers: &[String], candidate: usize) -> String {
    match providers.get(candidate) {
        Some(provider) if provider.chars().count() > 15 => {
            let head: String = provider.chars().take(12).collect();
            format!("{head}…")
        }
        Some(provider) => provider.clone(),
        None => format!("provider {candidate}"),
    }
}

/// Execute a pure racing plan ([`crate::arm_race`]) over real tokio tasks:
/// launch arms as the planner directs, keep at most one pending hedge timer,
/// feed completions back as events, and stop at the first winner — aborting
/// the arms still in flight and handing any arm that had already finished as
/// a second winner to `abandon` rather than leaking it. A lost race returns
/// the final [`RaceState`], which carries every attempt's failure for the
/// caller's summary.
async fn drive_race<T, F, Fut, D, DFut>(
    candidates: usize,
    cap: usize,
    policy: LaunchPolicy,
    launch: F,
    abandon: D,
    mut on_progress: impl FnMut(RaceProgress),
) -> Result<T, RaceState>
where
    T: Send + 'static,
    F: Fn(usize) -> Fut,
    Fut: Future<Output = Result<T, String>> + Send + 'static,
    D: Fn(T) -> DFut,
    DFut: Future<Output = ()>,
{
    let mut race = RaceState::new(candidates, cap, policy);
    let mut arms: tokio::task::JoinSet<(usize, Result<T, String>)> = tokio::task::JoinSet::new();
    let mut arm_candidates: HashMap<tokio::task::Id, usize> = HashMap::new();
    let mut hedge_deadline: Option<tokio::time::Instant> = None;
    let mut lost = false;

    let apply = |actions: Vec<RaceAction>,
                 arms: &mut tokio::task::JoinSet<(usize, Result<T, String>)>,
                 arm_candidates: &mut HashMap<tokio::task::Id, usize>,
                 hedge_deadline: &mut Option<tokio::time::Instant>,
                 lost: &mut bool| {
        for action in actions {
            match action {
                RaceAction::Launch { candidate } => {
                    let arm = launch(candidate);
                    let handle = arms.spawn(async move { (candidate, arm.await) });
                    arm_candidates.insert(handle.id(), candidate);
                }
                RaceAction::ArmHedgeTimer(interval) => {
                    *hedge_deadline = Some(tokio::time::Instant::now() + interval);
                }
                RaceAction::GiveUp => *lost = true,
            }
        }
    };

    apply(
        race.start(),
        &mut arms,
        &mut arm_candidates,
        &mut hedge_deadline,
        &mut lost,
    );
    on_progress(race.progress());

    loop {
        if lost {
            return Err(race);
        }
        tokio::select! {
            // Prefer completions over the hedge timer, so a finished arm is
            // never preempted by a simultaneous timer firing.
            biased;
            joined = arms.join_next_with_id() => {
                let Some(joined) = joined else { return Err(race) };
                let (candidate, outcome) = match joined {
                    Ok((id, (candidate, outcome))) => {
                        arm_candidates.remove(&id);
                        (candidate, outcome)
                    }
                    Err(join_error) => {
                        // A panicked arm is a failed arm; recover which
                        // candidate it raced so the accounting stays exact.
                        let candidate = arm_candidates
                            .remove(&join_error.id())
                            .unwrap_or(usize::MAX);
                        (candidate, Err(format!("arm task failed: {join_error}")))
                    }
                };
                match outcome {
                    Ok(winner) => {
                        arms.abort_all();
                        while let Some(late) = arms.join_next().await {
                            if let Ok((_, Ok(second_winner))) = late {
                                abandon(second_winner).await;
                            }
                        }
                        return Ok(winner);
                    }
                    Err(error) => {
                        apply(
                            race.on_event(RaceEvent::ArmFailed { candidate, error }),
                            &mut arms,
                            &mut arm_candidates,
                            &mut hedge_deadline,
                            &mut lost,
                        );
                        on_progress(race.progress());
                    }
                }
            }
            () = tokio::time::sleep_until(
                hedge_deadline.unwrap_or_else(tokio::time::Instant::now)
            ), if hedge_deadline.is_some() => {
                hedge_deadline = None;
                apply(
                    race.on_event(RaceEvent::HedgeElapsed),
                    &mut arms,
                    &mut arm_candidates,
                    &mut hedge_deadline,
                    &mut lost,
                );
                on_progress(race.progress());
            }
        }
    }
}

/// The entropy source for provider shuffling: a hash of the current time.
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
    use super::*;

    // The scheme-stripping and retry-engine logic is tested in
    // `mixnet_connect`, where the tests call the REAL functions in the
    // default build; the earlier copies of that logic here tested a
    // transcription of the expression, not the code.

    #[test]
    fn find_available_port_returns_nonzero() {
        let port = NymProxy::find_available_port().expect("find_available_port");
        assert!(port > 0);
    }

    fn hedged(max_parallel: usize) -> LaunchPolicy {
        LaunchPolicy::Hedged {
            max_parallel,
            hedge_interval: HEDGE_INTERVAL,
        }
    }

    async fn no_abandon(_: &str) {}

    /// HYPOTHESIS: a provider whose connect hangs costs only the hedge
    /// interval before a parallel arm can win — not the per-attempt timeout,
    /// and never the lifecycle budget (the regression behind live 120s
    /// startup hangs). Falsified if the race waits for the dud to time out.
    /// Runs on paused tokio time, so no live network and no real waiting.
    #[tokio::test(start_paused = true)]
    async fn a_hedged_arm_rescues_a_hanging_provider_at_the_hedge_interval() {
        let started = tokio::time::Instant::now();
        let winner = drive_race(
            2,
            MAX_PROVIDER_ATTEMPTS,
            hedged(MAX_PARALLEL_CONNECTS),
            |candidate| async move {
                if candidate == 0 {
                    std::future::pending::<Result<&str, String>>().await
                } else {
                    Ok("rescued")
                }
            },
            no_abandon,
            |_| {},
        )
        .await
        .expect("the hedged arm wins");
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
    /// still frees the wedged slot, so a hanging provider costs one timeout
    /// rather than the lifecycle budget. Falsified if the race hangs past
    /// [`PER_ATTEMPT_CONNECT_TIMEOUT`] with a single slot.
    #[tokio::test(start_paused = true)]
    async fn the_per_attempt_timeout_frees_a_wedged_slot() {
        let started = tokio::time::Instant::now();
        let winner = drive_race(
            2,
            MAX_PROVIDER_ATTEMPTS,
            hedged(1),
            |candidate| async move {
                if candidate == 0 {
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
            |_| {},
        )
        .await
        .expect("the second provider wins after the timeout");
        assert_eq!(winner, "second");

        let elapsed = started.elapsed();
        assert!(elapsed >= PER_ATTEMPT_CONNECT_TIMEOUT);
        assert!(elapsed < NYM_LIFECYCLE_TIMEOUT);
    }

    /// HYPOTHESIS: a lost race accounts for EVERY attempt, not just the last
    /// failure — the information the retired retry engine discarded.
    /// Falsified if any attempt's failure is missing from the summary.
    #[tokio::test(start_paused = true)]
    async fn a_lost_race_accounts_for_every_attempt() {
        let race = drive_race(
            2,
            MAX_PROVIDER_ATTEMPTS,
            hedged(MAX_PARALLEL_CONNECTS),
            |candidate| async move { Err::<&str, _>(format!("candidate {candidate} refused")) },
            no_abandon,
            |_| {},
        )
        .await
        .expect_err("both providers refuse");

        let summary = race.failure_summary(|i| format!("p{i}"));
        assert!(summary.contains("p0: candidate 0 refused"), "{summary}");
        assert!(summary.contains("p1: candidate 1 refused"), "{summary}");
    }

    /// HYPOTHESIS: when two arms finish as winners in the same instant,
    /// exactly one wins and the other is handed to `abandon` (production
    /// disconnects it) rather than being dropped silently. Falsified if the
    /// second winner vanishes without the abandon callback.
    #[tokio::test(start_paused = true)]
    async fn a_simultaneous_second_winner_is_abandoned_not_leaked() {
        let abandoned = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&abandoned);
        // Arm 0 launches at t=0 and finishes at t=6; arm 1 launches at the
        // t=5 hedge and finishes at t=6 as well.
        let winner = drive_race(
            2,
            MAX_PROVIDER_ATTEMPTS,
            hedged(MAX_PARALLEL_CONNECTS),
            |candidate| async move {
                let finish = if candidate == 0 {
                    Duration::from_secs(6)
                } else {
                    Duration::from_secs(1)
                };
                tokio::time::sleep(finish).await;
                Ok::<_, String>(if candidate == 0 { "arm0" } else { "arm1" })
            },
            move |second_winner| {
                let sink = std::sync::Arc::clone(&sink);
                async move { sink.lock().unwrap().push(second_winner) }
            },
            |_| {},
        )
        .await
        .expect("one arm wins");

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
    async fn progress_lines_narrate_the_race() {
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&lines);
        let _ = drive_race(
            2,
            MAX_PROVIDER_ATTEMPTS,
            hedged(MAX_PARALLEL_CONNECTS),
            |candidate| async move { Err::<&str, _>(format!("candidate {candidate} refused")) },
            no_abandon,
            move |progress| sink.lock().unwrap().push(progress.to_string()),
        )
        .await;
        let lines = lines.lock().unwrap();
        assert!(
            lines.iter().any(|l| l.contains("2 failed")),
            "progress must narrate the accumulated failures, got {lines:?}"
        );
    }

    #[test]
    fn long_provider_names_are_shortened_for_the_summary() {
        let providers = vec!["a".repeat(200), "short".to_string()];
        let shortened = short_provider_name(&providers, 0);
        assert!(
            shortened.chars().count() == 13,
            "twelve chars plus ellipsis"
        );
        assert!(shortened.ends_with('…'));
        assert_eq!(short_provider_name(&providers, 1), "short");
        assert_eq!(short_provider_name(&providers, 9), "provider 9");
    }

    // Integration tests below require a live Nym network. Run with:
    //   cargo test --manifest-path zingo-netutils/Cargo.toml --features nym -- --ignored

    /// Start the embedded proxy and verify it reports a valid localhost address.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires live Nym network"]
    async fn nym_proxy_starts_and_reports_address() {
        let proxy = NymProxy::start().await.expect("NymProxy::start");
        let addr = proxy.socks5_addr();
        assert!(
            addr.starts_with("127.0.0.1:"),
            "expected localhost address, got {addr}"
        );
        let port: u16 = addr
            .split(':')
            .next_back()
            .unwrap()
            .parse()
            .expect("port should be numeric");
        assert!(port > 0, "port should be non-zero");
        proxy.disconnect().await;
    }

    /// Start the proxy and verify a SOCKS5 TCP tunnel works end-to-end.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires live Nym network"]
    async fn nym_proxy_socks5_tunnel_works() {
        let proxy = NymProxy::start().await.expect("NymProxy::start");
        let addr = proxy.socks5_addr();

        let stream = tokio_socks::tcp::Socks5Stream::connect(&*addr, "zec.rocks:443")
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
