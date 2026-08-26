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
    /// The sweep reached no live exit at all: it acquired no transport, or
    /// every exit it drew carried nothing. Shared with every speed-priority
    /// operation, because neither failure is about indexers.
    #[error("the sweep reached no live exit")]
    Speed(#[source] crate::mixnet::speed::SpeedError),
    /// The survey ran through a proven exit but no indexer could be
    /// selected.
    #[error(transparent)]
    Selection(#[from] SweepError),
}

impl LightClient {
    /// Run one Server-Selection Sweep (ADR 0034) over `candidates`, returning
    /// the selected sync indexer, the transmit candidates that exclude its
    /// operator, and the height-ordered live cohort.
    ///
    /// `binary_path` is the `nym-proxy` binary the dedicated sweep proxy
    /// spawns from. The candidates are assigned to survey lanes at random
    /// and every one is probed and assigned a verdict (ruled 2026-08-14):
    /// a healthy `pin` wins outright, and otherwise the sync indexer is
    /// drawn among every healthy answer, all of which are draw-eligible for
    /// the session's operations, with the transmit candidates excluding the
    /// sync operator so different operations select different indexers.
    pub async fn run_server_selection_sweep(
        &self,
        binary_path: &Path,
        candidates: &[Uri],
        pin: Option<&Uri>,
        progress: impl Fn(SweepProgress),
    ) -> Result<Selection, ServerSelectionError> {
        let chain = lightd_chain_name(&self.chain_type());
        let acquirer = std::sync::Arc::new(crate::mixnet::acquire::Acquirer::Spawned(
            crate::mixnet::acquire::SpawnedBinary::at(binary_path.to_path_buf()),
        ));
        // Random lane assignment; the whole census is probed and assigned
        // before the drawn verdict. Acquisition, the Sentinel-ridden wave, and
        // the redraw of an exit that carries nothing are the shared
        // speed-priority loop's; this runner owns only the judgment.
        let order = sweep::wave_order(candidates, pin, SURVEY_WAVE_WIDTH, &mut rand::rngs::OsRng);
        let survey = IndexerSurvey {
            pools: self.destination_pools.clone(),
            acquirer,
            order: order.clone(),
            timeout: zingo_netutils::time::PROBE_LEG_TIMEOUT,
            history: self.indexer_history.clone(),
        };
        progress(SweepProgress::TransportBootstrapping);
        let (results, member) = crate::mixnet::speed::run_speed_prioritized(&survey)
            .await
            .map_err(ServerSelectionError::Speed)?;
        let socks5_addr = member.addr().ok_or(ServerSelectionError::Speed(
            crate::mixnet::speed::SpeedError::Transport(
                crate::mixnet::acquire::TransportError::DiedBeforeUse,
            ),
        ))?;
        progress(SweepProgress::Judging {
            answered: results.iter().filter(|r| r.reported.is_some()).count(),
            surveyed: results.len(),
        });

        let Some(chosen) =
            sweep::healthy_draw_verdict(&results, chain, pin, &mut rand::rngs::OsRng)
        else {
            // Every candidate was surveyed through a proven exit and none
            // was healthy, so the refusal is about the candidates.
            member.retire().await;
            return Err(SweepError::EmptyCohort {
                surveyed: results.len(),
                answered: results.iter().filter(|r| r.reported.is_some()).count(),
                causes: sweep::RefusalTally::of(&results),
            }
            .into());
        };
        let selection = sweep::assigned_selection(&results, chain, chosen);

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
        Ok(selection)
    }
}

/// The Server-Selection Sweep as a speed-priority operation: it races the
/// census through one Exit Node, assigning every candidate its verdict.
struct IndexerSurvey {
    pools: std::sync::Arc<crate::destination::pool::Pools>,
    acquirer: std::sync::Arc<crate::mixnet::acquire::Acquirer>,
    order: Vec<Uri>,
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
        dial: zingo_netutils::conduit::ConduitDial,
        target: Uri,
    ) -> impl std::future::Future<Output = SurveyResult> + Send {
        let timeout = self.timeout;
        let history = self.history.clone();
        async move {
            // The guard outlives the leg, so the conduit counts this survey
            // probe while it runs.
            let (reported, refusal) = probe_one(dial.socks5(), &target, timeout, &history).await;
            SurveyResult {
                uri: target,
                reported,
                refusal,
            }
        }
    }

    fn settled(&self, _outcomes: &[SurveyResult]) -> bool {
        // The IndexerSweep probes and assigns every indexer (ruled
        // 2026-08-14): no answer ends the wave early, so the whole census
        // earns its verdicts and every healthy indexer is draw-eligible.
        false
    }

    fn answered(&self, outcomes: &[SurveyResult]) -> bool {
        outcomes.iter().any(|result| result.reported.is_some())
    }

    fn acquire(
        &self,
    ) -> impl std::future::Future<
        Output = Result<
            (crate::mixnet::speed::Member, std::net::SocketAddr),
            crate::mixnet::acquire::TransportError,
        >,
    > + Send {
        // The sweep runs over the conduit boot proved for its role, so the
        // usual case costs no birth at all. A redraw finds the conduit
        // spent and births over the exit authority as before.
        let pools = self.pools.clone();
        let acquirer = self.acquirer.clone();
        async move {
            let birth = pools
                .conduit_for(
                    crate::mixnet::quartet::Role::IndexerSweep,
                    acquirer.as_ref(),
                )
                .await?;
            let member = crate::mixnet::speed::Member::new(birth.transport, birth.lease);
            let addr = member
                .addr()
                .ok_or(crate::mixnet::acquire::TransportError::DiedBeforeUse)?;
            Ok((member, addr))
        }
    }

    fn dispose(&self, spent: crate::mixnet::speed::Member) {
        // A finished survey is a completed round trip, so the exit's proof
        // renews as the client retires.
        self.pools.remember(
            spent.node().clone(),
            crate::destination::pool::exit_pool::ExitNodeHealthVerdict::EpochProven,
        );
        tokio::spawn(async move {
            spent.retire().await;
        });
    }

    fn abandon(&self, dead: crate::mixnet::speed::Member) {
        self.pools.remember(
            dead.node().clone(),
            crate::destination::pool::exit_pool::ExitNodeHealthVerdict::Failed,
        );
        tokio::spawn(async move {
            dead.retire().await;
        });
    }

    fn narrate(&self, phase: crate::mixnet::speed::SpeedProgress) {
        tracing::info!("server-selection sweep: {phase:?}");
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
    use crate::mixnet::Indicator;

    /// HYPOTHESIS: the survey never settles early — every indexer must be
    /// probed and assigned — so a healthy answer does not end the wave.
    /// Falsified if one healthy outcome settles the survey.
    #[test]
    fn a_healthy_answer_does_not_settle_the_survey() {
        let survey = IndexerSurvey {
            pools: crate::destination::pool::Pools::new(),
            acquirer: std::sync::Arc::new(crate::mixnet::acquire::Acquirer::Spawned(
                crate::mixnet::acquire::SpawnedBinary::at(std::path::PathBuf::from(
                    "/nonexistent/nym-proxy",
                )),
            )),
            order: Vec::new(),
            timeout: zingo_netutils::time::PROBE_LEG_TIMEOUT,
            history: crate::lightclient::indexer_history::IndexerHistoryHandle::default(),
        };
        let healthy = crate::mixnet::sweep::SurveyResult {
            uri: "https://a.example:443"
                .parse()
                .expect("the static uri parses"),
            reported: Some(crate::mixnet::probe::ProbeSuccess {
                chain: "main".to_string(),
                height: 100,
            }),
            refusal: None,
        };
        assert!(
            !crate::mixnet::speed::SpeedPrioritized::settled(&survey, &[healthy]),
            "one healthy answer must not end the survey"
        );
    }

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
    /// machinery, so the attempt it records in the session's history carries
    /// the latency it measured. Falsified if a sweep attempt lands with an
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
        let history = crate::lightclient::indexer_history::IndexerHistoryHandle::default();

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
            mode: Indicator::Died,
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
            mode: Indicator::Ready,
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
        let waiter = tokio::spawn(async move {
            crate::mixnet::supervisor::await_ready_endpoint(
                &mut receiver,
                zingo_netutils::time::NYM_LIFECYCLE_TIMEOUT,
            )
            .await
        });
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
        let refusal = crate::mixnet::supervisor::await_ready_endpoint(
            &mut receiver,
            zingo_netutils::time::NYM_LIFECYCLE_TIMEOUT,
        )
        .await
        .expect_err("no exit ever arrives");
        assert!(
            matches!(
                refusal,
                crate::mixnet::acquire::TransportError::NotReady { budget }
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
        let refusal = ServerSelectionError::Speed(crate::mixnet::speed::SpeedError::Transport(
            crate::mixnet::acquire::TransportError::ExitOutsideClutch {
                reported: vec![crate::mixnet::ExitNodeId::from("exit-foreign")],
            },
        ));
        let told = zingo_net_diag::chain_texts(&refusal).join(" ");
        assert!(
            !told.contains("could not acquire a transport"),
            "the bind refusal must not borrow the acquisition's story, got: {told}"
        );
        assert!(
            told.contains("no bound exit from the drawn clutch"),
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
        let died = crate::mixnet::supervisor::await_ready_endpoint(
            &mut death_receiver,
            zingo_netutils::time::NYM_LIFECYCLE_TIMEOUT,
        )
        .await
        .expect_err("a died transport never reaches readiness");

        let idle_publisher = crate::mixnet::status_publisher();
        let mut idle_receiver = idle_publisher.subscribe();
        let timed_out = crate::mixnet::supervisor::await_ready_endpoint(
            &mut idle_receiver,
            zingo_netutils::time::NYM_LIFECYCLE_TIMEOUT,
        )
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
                crate::mixnet::acquire::TransportError::NotReady { budget }
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
