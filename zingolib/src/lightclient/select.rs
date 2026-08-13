//! The Server-Selection Sweep runner (ADR 0034): the impure half that emits
//! the survey and turns it into a sync indexer.
//!
//! The runner spawns a dedicated sweep proxy — its own `nym-proxy` child on
//! its own status channel, so its bootstrap never churns the session's
//! send/price-fetch transport and its Exit Node is distinct by construction.
//! It surveys the candidates through that exit, hands the results to the
//! pure [`sweep::select`], and drops the proxy, which recycles its exit: the
//! transport that learned what was surveyed carries nothing after.
#![forbid(unsafe_code)]

use std::path::Path;
use std::time::Duration;

use http::Uri;

use super::LightClient;
use crate::mixnet::probe::ProbeSuccess;
use crate::mixnet::sweep::{self, Selection, SurveyResult, SweepError};

/// Two blocks: the height tolerance around the observed median that counts
/// as live (ADR 0034).
pub const SWEEP_HEIGHT_TOLERANCE: u64 = 2;

/// How many exits one sweep may draw: the first, and a fresh one for each
/// draw whose Sentinel proved its exit carries nothing, bounded so a mixnet
/// failing everywhere refuses in a stated time rather than drawing forever.
pub const MAX_SWEEP_EXIT_DRAWS: usize = 6;

/// A phase transition of a running Server-Selection Sweep, delivered to the
/// consumer's progress callback as the sweep reaches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SweepProgress {
    /// The dedicated sweep transport is bootstrapping toward its own exit.
    TransportBootstrapping,
    /// The survey is probing every candidate through the sweep exit.
    Surveying {
        /// How many candidates the survey covers.
        candidates: usize,
    },
    /// The draw's Sentinel proved the exit carries nothing, so the exit is
    /// abandoned, the draw's results are dropped, and a fresh exit drawn.
    ExitAbandoned {
        /// Which draw proved its exit carries nothing, counting from one.
        draw: usize,
    },
    /// The survey finished and the pure judgment is running.
    Judging {
        /// How many candidates answered the survey.
        answered: usize,
        /// How many candidates were surveyed.
        surveyed: usize,
    },
}

/// Why a Server-Selection Sweep produced no sync indexer.
#[derive(Debug, thiserror::Error)]
pub enum ServerSelectionError {
    /// The sweep could not draw a ledgered Clutch for its transport.
    #[error("the sweep could not acquire a transport")]
    TransportAcquisition(#[source] crate::mixnet::acquire::TransportError),
    /// The ready sweep transport bound an Exit Node outside the drawn Clutch.
    #[error(
        "the sweep transport bound an exit outside the drawn clutch ({} reported)",
        reported.len()
    )]
    ExitOutsideClutch {
        /// The exit identities the ready transport reported as bound.
        reported: Vec<crate::mixnet::ExitNodeId>,
    },
    /// The dedicated sweep proxy could not be spawned.
    #[error("the sweep proxy could not start")]
    ProxyStart(#[source] crate::mixnet::MixnetProxyError),
    /// The dedicated sweep proxy died before any survey ran.
    #[error("the sweep transport died before it became ready")]
    TransportDied {
        /// The typed cause the death report latched, when it held one.
        #[source]
        detail: Option<zingo_net_diag::NetOpFailure>,
    },
    /// The dedicated sweep proxy exceeded its bootstrap budget before any
    /// survey ran.
    #[error("the sweep transport did not become ready within {}s", budget.as_secs())]
    TransportTimeout {
        /// The bootstrap budget that elapsed without a readiness
        /// announcement.
        budget: Duration,
    },
    /// The dedicated sweep proxy's status channel closed before readiness.
    #[error("the sweep transport's status channel closed before readiness")]
    TransportStatusClosed,
    /// The survey ran but no sync indexer could be selected.
    #[error(transparent)]
    Selection(#[from] SweepError),
}

impl LightClient {
    /// Run one Server-Selection Sweep (ADR 0034) over `candidates`, returning
    /// the selected sync indexer, the transmit candidates that exclude its
    /// operator, and the height-ordered live cohort.
    ///
    /// `binary_path` is the `nym-proxy` binary the dedicated sweep proxy
    /// spawns from. The candidates are assigned to survey lanes at random,
    /// `pin` guaranteed an opening lane, and the first healthy answer is
    /// the verdict — the pin preempts while its own probe is pending and is
    /// chosen the moment it answers — offered to the session immediately:
    /// every unresolved candidate continues in the background as the health
    /// sweep, whose handle the session holds so revoking consent aborts it,
    /// and the sweep transport recycles its exit when that finishes.
    pub async fn run_server_selection_sweep(
        &self,
        binary_path: &Path,
        candidates: &[Uri],
        pin: Option<&Uri>,
        progress: impl Fn(SweepProgress),
    ) -> Result<Selection, ServerSelectionError> {
        let chain = lightd_chain_name(&self.chain_type());
        // The sweep gates the Sync Session a user just asked to open.
        use zingo_netutils::responsiveness::{PrioritiseSpeed, Responsiveness as _};
        let acquirer = crate::mixnet::acquire::SpawnedBinary::at(binary_path.to_path_buf());
        // A dead exit is abandoned for a fresh one, and its reservation is
        // held across the redraw so the pool cannot offer it again.
        let mut abandoned: Option<
            crate::correspondent::pool::Member<
                crate::mixnet::MixnetProxy,
                crate::correspondent::pool::Shared,
            >,
        > = None;
        for draw in 1..=MAX_SWEEP_EXIT_DRAWS {
            // A dedicated status channel per draw: the sweep proxy's lifecycle
            // is private to this attempt and must not touch the session's
            // mixnet status, nor carry a previous attempt's announcements.
            let publisher = crate::mixnet::status_publisher();
            let mut receiver = publisher.subscribe();
            // The sweep refuses without a ledgered Clutch; its reservations are
            // held for the sweep's life and recycled by drop on every return.
            let mut clutch = self
                .correspondent_pools
                .draw_clutch(&acquirer)
                .await
                .map_err(ServerSelectionError::TransportAcquisition)?;
            let nodes = crate::correspondent::pool::exit_pool::clutch_nodes(&clutch);
            let proxy = crate::mixnet::acquire::TransportAcquirable::acquire(
                &acquirer,
                PrioritiseSpeed::CLASS,
                &nodes,
                publisher,
            )
            .await
            .map_err(ServerSelectionError::TransportAcquisition)?;

            progress(SweepProgress::TransportBootstrapping);
            let (socks5_addr, exits) = await_sweep_ready(&mut receiver).await?;
            // Bind-time recycle: the survey's fan-out is a declared Shared use
            // of the one bound exit, and the unbound reservations return now.
            // The shared gate guarantees an announced exit, so this refusal
            // fires only on a genuinely foreign one (a version-skewed proxy),
            // typed at the user's go-online moment, never a panic.
            let Some(lease) =
                crate::correspondent::pool::exit_pool::take_bound_lease(&mut clutch, &exits)
            else {
                proxy.stop().await;
                return Err(ServerSelectionError::ExitOutsideClutch { reported: exits });
            };
            drop(clutch);
            let member: crate::correspondent::pool::Member<
                crate::mixnet::MixnetProxy,
                crate::correspondent::pool::Shared,
            > = crate::correspondent::pool::Member::new(proxy, lease);
            // This draw holds its own exit now, so a dead predecessor can go
            // back to the pool without risking its own reissue.
            if let Some(dead) = abandoned.take() {
                dead.retire().await;
            }
            progress(SweepProgress::Surveying {
                candidates: candidates.len(),
            });
            // Random lane assignment, the pin guaranteed an opening lane: the
            // first healthy answer is the verdict (the pin preempts while its
            // own probe is pending), offered to the session immediately. The
            // wave itself is the one every speed-priority operation runs.
            let order =
                sweep::wave_order(candidates, pin, SURVEY_WAVE_WIDTH, &mut rand::rngs::OsRng);
            let survey = IndexerSurvey {
                order: order.clone(),
                chain: chain.to_string(),
                pin: pin.cloned(),
                timeout: zingo_netutils::time::PROBE_LEG_TIMEOUT,
                history: self.indexer_history.clone(),
            };
            let (results, exit_carries_nothing) =
                match crate::mixnet::speed::run_wave(&survey, socks5_addr).await {
                    crate::mixnet::speed::WaveEnd::ExitCarriesNothing => (Vec::new(), true),
                    crate::mixnet::speed::WaveEnd::Settled(results)
                    | crate::mixnet::speed::WaveEnd::Exhausted(results) => (results, false),
                };
            let verdict = sweep::first_healthy_verdict(&results, chain, pin);

            if exit_carries_nothing && draw < MAX_SWEEP_EXIT_DRAWS {
                // The exit, not the candidates, failed: this draw's results
                // say nothing about any indexer, so they are dropped whole
                // and the sweep starts over on a fresh exit.
                progress(SweepProgress::ExitAbandoned { draw });
                abandoned = Some(member);
                continue;
            }
            progress(SweepProgress::Judging {
                answered: results.iter().filter(|r| r.reported.is_some()).count(),
                surveyed: results.len(),
            });

            let Some(chosen) = verdict else {
                // Every candidate was surveyed and none was healthy.
                member.retire().await;
                return Err(SweepError::EmptyCohort {
                    surveyed: results.len(),
                    answered: results.iter().filter(|r| r.reported.is_some()).count(),
                    causes: sweep::RefusalTally::of(&results),
                }
                .into());
            };
            let selection = sweep::first_healthy_selection(&results, chain, chosen);

            // Every candidate the verdict left unresolved continues in the
            // background purely as the health sweep; the session holds its
            // handle so `go_offline` can abort it, and retiring the member
            // afterward recycles the exit that observed the survey.
            let seen: std::collections::HashSet<Uri> =
                results.iter().map(|result| result.uri.clone()).collect();
            let rest: Vec<Uri> = order
                .into_iter()
                .filter(|candidate| !seen.contains(candidate))
                .collect();
            if rest.is_empty() {
                member.retire().await;
            } else {
                let history = self.indexer_history.clone();
                let continuation = tokio::spawn(async move {
                    health_survey(socks5_addr, rest, &history).await;
                    member.retire().await;
                });
                if let Some(superseded) = self
                    .health_sweep
                    .lock()
                    .expect("the health-sweep slot is never poisoned")
                    .replace(continuation)
                {
                    superseded.abort();
                }
            }
            return Ok(selection);
        }
        // The loop returns on every path a draw can reach.
        unreachable!("every sweep draw returns or abandons its exit")
    }
}

/// The Server-Selection Sweep as a speed-priority operation: it races the
/// census through one Exit Node and settles on the first healthy answer.
struct IndexerSurvey {
    order: Vec<Uri>,
    chain: String,
    pin: Option<Uri>,
    timeout: Duration,
    history: crate::lightclient::indexer_history::IndexerHistoryHandle,
}

impl crate::mixnet::speed::SpeedPrioritized for IndexerSurvey {
    type Target = Uri;
    type Outcome = SurveyResult;

    fn targets(&self) -> Vec<Uri> {
        self.order.clone()
    }

    fn probe(
        &self,
        socks5: std::net::SocketAddr,
        target: Uri,
    ) -> impl std::future::Future<Output = SurveyResult> + Send {
        let timeout = self.timeout;
        let history = self.history.clone();
        async move {
            let (reported, refusal) = probe_one(socks5, &target, timeout, &history).await;
            SurveyResult {
                uri: target,
                reported,
                refusal,
            }
        }
    }

    fn settled(&self, outcomes: &[SurveyResult]) -> bool {
        sweep::first_healthy_verdict(outcomes, &self.chain, self.pin.as_ref()).is_some()
    }
}

/// The wave width every speed-priority operation uses: four connections
/// through the one Nym client, which is the widest fan-out that kept each
/// near its solo cost instead of blowing every probe's budget together —
/// the 0-of-17 signature — and which counts connections rather than
/// processes, so one calibration serves the spawned desktop binary and the
/// client a mobile host runs in its own process.
pub const SURVEY_WAVE_WIDTH: usize = 4;

/// Survey `rest` purely for the indexer history — the health sweep — at the
/// width its own census slice warrants.
async fn health_survey(
    socks5_addr: std::net::SocketAddr,
    rest: Vec<Uri>,
    history: &crate::lightclient::indexer_history::IndexerHistoryHandle,
) {
    use futures::StreamExt as _;
    let timeout = zingo_netutils::time::PROBE_LEG_TIMEOUT;
    let width = SURVEY_WAVE_WIDTH;
    futures::stream::iter(rest)
        .map(|uri| async move {
            let _health_only = probe_one(socks5_addr, &uri, timeout, history).await;
        })
        .buffer_unordered(width)
        .collect::<Vec<()>>()
        .await;
}

/// The chain name a `GetLightdInfo` reply carries for `chain`, the
/// vocabulary the survey's liveness judgment must compare against.
fn lightd_chain_name(chain: &crate::config::ChainType) -> &'static str {
    match chain {
        crate::config::ChainType::Mainnet => "main",
        crate::config::ChainType::Testnet => "test",
        crate::config::ChainType::Regtest(_) => "regtest",
    }
}

/// Wait for the dedicated sweep proxy to reach readiness through the one
/// shared gate, mapping each typed outcome into the sweep's own vocabulary.
async fn await_sweep_ready(
    receiver: &mut tokio::sync::watch::Receiver<crate::mixnet::MixnetStatus>,
) -> Result<(std::net::SocketAddr, Vec<crate::mixnet::ExitNodeId>), ServerSelectionError> {
    crate::mixnet::supervisor::await_ready_endpoint(
        receiver,
        zingo_netutils::time::NYM_LIFECYCLE_TIMEOUT,
    )
    .await
    .map_err(sweep_refusal)
}

/// The sweep's own name for one transport refusal, mapping every variant by
/// hand so a new one is a compile error rather than a silent acquisition
/// story.
fn sweep_refusal(refusal: crate::mixnet::acquire::TransportError) -> ServerSelectionError {
    use crate::mixnet::acquire::TransportError;
    match refusal {
        TransportError::DiedDuringBootstrap { detail } => {
            ServerSelectionError::TransportDied { detail }
        }
        TransportError::StatusChannelClosed => ServerSelectionError::TransportStatusClosed,
        TransportError::NotReady { budget } => ServerSelectionError::TransportTimeout { budget },
        TransportError::ExitOutsideClutch { reported } => {
            ServerSelectionError::ExitOutsideClutch { reported }
        }
        acquisition @ (TransportError::NoAcquirer
        | TransportError::DiscoverySpawn(_)
        | TransportError::DiscoveryFailed { .. }
        | TransportError::ExitPoolNotSeeded
        | TransportError::ExitPoolExhausted { .. }
        | TransportError::Proxy(_)
        | TransportError::HostUnavailable(_)
        | TransportError::HostRefused(_)
        | TransportError::DiedBeforeUse) => ServerSelectionError::TransportAcquisition(acquisition),
    }
}

/// One candidate's survey: the shared mixnet probe over the sweep exit,
/// which times and records the attempt, its success mapped to the reported
/// chain and height and any failure to its classified refusal.
async fn probe_one(
    socks5_addr: std::net::SocketAddr,
    uri: &Uri,
    timeout: Duration,
    history: &crate::lightclient::indexer_history::IndexerHistoryHandle,
) -> (
    Option<ProbeSuccess>,
    Option<crate::lightclient::indexer_history::FailureKind>,
) {
    let leg = crate::mixnet::probe::probe_indexer(uri, socks5_addr, timeout, history)
        .await
        .leg;
    match leg.outcome {
        Ok(success) => (Some(success), None),
        Err(failure) => {
            let kind =
                crate::lightclient::indexer_history::FailureKind::classify(&failure.to_string());
            (None, Some(kind))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixnet::MixnetMode;

    /// HYPOTHESIS: revoking consent aborts the detached health sweep — the
    /// held task is cancelled and the slot empties — so no probe outlives
    /// `network off`. Falsified if the task survives going offline.
    #[tokio::test]
    async fn going_offline_aborts_the_health_sweep() {
        let wallet = crate::testutils::synthetic_wallet::SyntheticWalletBuilder::new(
            zingo_test_vectors::seeds::ABANDON_ART_SEED,
        )
        .build();
        let mut client = LightClient::new_for_test(wallet).await;
        let held = tokio::spawn(std::future::pending::<()>());
        *client
            .health_sweep
            .lock()
            .expect("the health-sweep slot is never poisoned") = Some(held);
        client.go_offline().await;
        assert!(
            client
                .health_sweep
                .lock()
                .expect("the health-sweep slot is never poisoned")
                .is_none(),
            "going offline empties the health-sweep slot"
        );
    }

    /// HYPOTHESIS: every speed-priority wave is four connections wide,
    /// whatever the census size, because the bound counts connections
    /// through the one Nym client rather than a fraction of the targets.
    /// Falsified if the width varies with the work.
    #[test]
    fn the_wave_width_is_fixed() {
        assert_eq!(SURVEY_WAVE_WIDTH, 4);
        assert_eq!(
            crate::mixnet::sweep::indexer_lanes(SURVEY_WAVE_WIDTH),
            3,
            "the Sentinel holds the fourth lane"
        );
    }

    /// HYPOTHESIS: the judgment compares against the wire's chain
    /// vocabulary (`main`, `test`, `regtest`), never `ChainType`'s own
    /// rendering (`mainnet`). Falsified if the mapping drifts back to the
    /// Display form, which emptied a 17-of-17 mainnet cohort on 2026-08-06.
    #[test]
    fn the_judgment_speaks_the_wire_chain_vocabulary() {
        use zingo_common_components::protocol::ActivationHeights;

        assert_eq!(
            lightd_chain_name(&crate::config::ChainType::Mainnet),
            "main"
        );
        assert_eq!(
            lightd_chain_name(&crate::config::ChainType::Testnet),
            "test"
        );
        assert_eq!(
            lightd_chain_name(&crate::config::ChainType::Regtest(
                ActivationHeights::default()
            )),
            "regtest"
        );
        assert_ne!(
            lightd_chain_name(&crate::config::ChainType::Mainnet),
            crate::config::ChainType::Mainnet.to_string(),
            "the Display form is not the wire form"
        );
    }

    /// The port value that asks the operating system to assign a free one.
    const ANY_PORT: u16 = 0;

    /// The fraction of its bound a probe against a proxy that never answers
    /// must be seen to spend, loose enough that a coarse clock cannot make a
    /// measured wait look unmeasured.
    const MEASURED_WAIT_DIVISOR: u32 = 2;

    /// The candidate the survey asks about, which the silent proxy never
    /// reaches.
    const UNANSWERED_CANDIDATE: &str = "https://sweep-candidate.example";

    /// HYPOTHESIS: the sweep's per-candidate probe rides the shared probe
    /// machinery, so the attempt it writes to the indexer diary carries the
    /// latency it measured. Falsified if a sweep attempt lands with an
    /// unmeasured duration while a liveness probe records a real one.
    #[tokio::test]
    async fn a_surveyed_candidate_records_the_latency_it_measured() {
        use zingo_netutils::time::test::FAST_STAGE_BOUND;

        // A stand-in proxy that accepts the dial and never speaks SOCKS5,
        // so the probe spends its whole bound waiting for an answer.
        let proxy = tokio::net::TcpListener::bind(std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            ANY_PORT,
        ))
        .await
        .expect("a loopback listener binds");
        let socks5_addr = proxy
            .local_addr()
            .expect("the listener reports its address");
        let silence = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = proxy.accept().await {
                held.push(socket);
            }
        });
        let dir = tempfile::tempdir().expect("temp dir");
        let history = crate::lightclient::indexer_history::IndexerHistoryHandle::beside_wallet(
            &dir.path().join("zingo-wallet.dat"),
        );
        history.set_recording(true);

        let (reported, refusal) = probe_one(
            socks5_addr,
            &UNANSWERED_CANDIDATE.parse().expect("the static uri parses"),
            FAST_STAGE_BOUND,
            &history,
        )
        .await;

        assert!(
            reported.is_none(),
            "a proxy that never answers reports no candidate"
        );
        assert!(
            refusal.is_some(),
            "a proxy that never answers classifies its refusal"
        );
        let attempt = history
            .load()
            .pop()
            .expect("the survey records its attempt");
        let floor = FAST_STAGE_BOUND / MEASURED_WAIT_DIVISOR;
        assert!(
            u128::from(attempt.millis) >= floor.as_millis(),
            "the recorded latency must be the measured wait, got {}ms against a {}ms floor",
            attempt.millis,
            floor.as_millis()
        );
        silence.abort();
    }

    /// The status a died sweep proxy publishes, carrying `detail` as its
    /// latched typed cause.
    fn died_with(detail: Option<zingo_net_diag::NetOpFailure>) -> crate::mixnet::MixnetStatus {
        crate::mixnet::MixnetStatus {
            mode: MixnetMode::Died,
            socks5_addr: None,
            exits: Vec::new(),
            bootstrap_detail: None,
            death: Some(crate::mixnet::DeathReport {
                at: std::time::SystemTime::UNIX_EPOCH,
                detail,
            }),
        }
    }

    /// The status a ready sweep proxy publishes, with `exits` as announced
    /// so far.
    fn ready_with(exits: Vec<crate::mixnet::ExitNodeId>) -> crate::mixnet::MixnetStatus {
        crate::mixnet::MixnetStatus {
            mode: MixnetMode::Ready,
            socks5_addr: Some("127.0.0.1:1080".parse().expect("the test address parses")),
            exits,
            bootstrap_detail: None,
            death: None,
        }
    }

    /// HYPOTHESIS: a Ready that has announced its address but no exit yet is
    /// a transient the waiter waits through, yielding once the exit arrives,
    /// so a healthy transport is never refused for its announcement order.
    /// Falsified if the waiter returns the empty announcement.
    #[tokio::test(start_paused = true)]
    async fn a_ready_without_an_exit_is_awaited_through() {
        /// Scheduler turns given to the waiter task, enough for it to
        /// observe the exitless Ready and park on the channel.
        const PARKING_YIELDS: usize = 8;

        let publisher = crate::mixnet::status_publisher();
        publisher.send_replace(ready_with(Vec::new()));
        let mut receiver = publisher.subscribe();
        let waiter = tokio::spawn(async move { await_sweep_ready(&mut receiver).await });
        for _ in 0..PARKING_YIELDS {
            tokio::task::yield_now().await;
        }
        publisher.send_replace(ready_with(vec![crate::mixnet::ExitNodeId::from(
            "exit-alpha",
        )]));
        let (_, exits) = waiter
            .await
            .expect("the waiter task completes")
            .expect("the transport becomes ready");
        assert_eq!(
            exits,
            vec![crate::mixnet::ExitNodeId::from("exit-alpha")],
            "the waiter yields the announced exit, never the empty transient"
        );
    }

    /// HYPOTHESIS: a transport that reaches Ready but never announces an
    /// exit refuses as a typed timeout naming the exit-announcement grace,
    /// so an empty announcement is a bounded wait, never an
    /// ExitOutsideClutch refusal and never the whole lifecycle budget.
    /// Falsified if the waiter yields the empty announcement to the bind.
    #[tokio::test(start_paused = true)]
    async fn a_ready_that_never_announces_an_exit_times_out() {
        let publisher = crate::mixnet::status_publisher();
        publisher.send_replace(ready_with(Vec::new()));
        let mut receiver = publisher.subscribe();
        let refusal = await_sweep_ready(&mut receiver)
            .await
            .expect_err("no exit ever arrives");
        assert!(
            matches!(
                refusal,
                ServerSelectionError::TransportTimeout { budget }
                    if budget == zingo_netutils::time::EXIT_ANNOUNCEMENT_GRACE
            ),
            "an exitless Ready refuses at the grace, got: {refusal}"
        );
    }

    /// HYPOTHESIS: the bind refusal tells its own story, naming the exit
    /// the ready transport bound outside the drawn Clutch, and never the
    /// acquisition's story of a Clutch that could not be drawn. Falsified
    /// if the refusal a foreign exit report produces reads as an
    /// acquisition failure.
    #[test]
    fn the_bind_refusal_never_tells_the_acquisition_story() {
        let refusal = sweep_refusal(crate::mixnet::acquire::TransportError::ExitOutsideClutch {
            reported: vec![crate::mixnet::ExitNodeId::from("exit-foreign")],
        });
        let told = refusal.to_string();
        assert!(
            !told.contains("could not acquire a transport"),
            "the bind refusal must not borrow the acquisition's story, got: {told}"
        );
        assert!(
            told.contains("outside the drawn clutch"),
            "the bind refusal names the exit outside the clutch, got: {told}"
        );
    }

    /// HYPOTHESIS: a sweep transport that dies and a sweep transport that
    /// exceeds its bootstrap budget refuse as two distinct typed variants,
    /// and the death carries its `NetOpFailure` down the source chain, so a
    /// caller separates the two without reading prose. Falsified if the two
    /// paths share one variant, or if the death's typed cause reaches the
    /// caller only as formatted text.
    #[tokio::test(start_paused = true)]
    async fn the_death_and_the_bootstrap_timeout_refuse_as_distinct_variants() {
        let handshake = zingo_net_diag::NetOpFailure::from_error(
            zingo_net_diag::NetOpStage::SocksHandshake,
            "127.0.0.1:1080",
            &std::io::Error::other("the handshake was refused"),
        );
        let death_publisher = crate::mixnet::status_publisher();
        death_publisher.send_replace(died_with(Some(handshake.clone())));
        let mut death_receiver = death_publisher.subscribe();
        let died = await_sweep_ready(&mut death_receiver)
            .await
            .expect_err("a died transport never reaches readiness");

        let idle_publisher = crate::mixnet::status_publisher();
        let mut idle_receiver = idle_publisher.subscribe();
        let timed_out = await_sweep_ready(&mut idle_receiver)
            .await
            .expect_err("a transport that never announces exceeds its budget");

        assert_ne!(
            std::mem::discriminant(&died),
            std::mem::discriminant(&timed_out),
            "a death and a bootstrap timeout are distinct refusals"
        );
        assert!(
            matches!(
                timed_out,
                ServerSelectionError::TransportTimeout { budget }
                    if budget == zingo_netutils::time::NYM_LIFECYCLE_TIMEOUT
            ),
            "the timeout refusal carries the budget it exceeded"
        );
        let cause = std::error::Error::source(&died)
            .expect("the death refusal carries its typed cause as a source");
        assert_eq!(
            cause.downcast_ref::<zingo_net_diag::NetOpFailure>(),
            Some(&handshake),
            "the typed failure reaches the caller whole"
        );
    }
}
