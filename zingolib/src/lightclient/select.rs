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
    /// spawns from. `pin` is an explicit user server: it is surveyed like any
    /// candidate and selected when live, and its absence from the live cohort
    /// fails [`SweepError::DeadPin`] rather than falling back to the draw.
    ///
    /// The sweep proxy is dropped before this returns, recycling its exit.
    pub async fn run_server_selection_sweep(
        &self,
        binary_path: &Path,
        candidates: &[Uri],
        pin: Option<&Uri>,
        progress: impl Fn(SweepProgress),
    ) -> Result<Selection, ServerSelectionError> {
        let chain = lightd_chain_name(&self.chain_type());
        // A dedicated status channel: the sweep proxy's lifecycle is private
        // to this call and must not touch the session's mixnet status.
        let publisher = crate::mixnet::status_publisher();
        let mut receiver = publisher.subscribe();
        // The sweep gates the Sync Session a user just asked to open.
        use zingo_netutils::responsiveness::{PrioritiseSpeed, Responsiveness as _};
        let acquirer = crate::mixnet::acquire::SpawnedBinary::at(binary_path.to_path_buf());
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
        progress(SweepProgress::Surveying {
            candidates: candidates.len(),
        });
        let results = survey(socks5_addr, candidates, &self.indexer_history).await;
        progress(SweepProgress::Judging {
            answered: results.iter().filter(|r| r.reported.is_some()).count(),
            surveyed: results.len(),
        });

        let selection = sweep::select(
            &results,
            chain,
            SWEEP_HEIGHT_TOLERANCE,
            pin,
            &mut rand::rngs::OsRng,
        );

        // Exit Recycling: retiring the member kills the child and recycles
        // its lease, so no later traffic rides the exit that observed the
        // survey. The judgment's verdict is held rather than propagated with
        // the question mark, because a refusal that returned early would drop
        // the member instead, recycling the reservation before the child's
        // death is confirmed. Retiring here covers every post-bind exit.
        member.retire().await;
        selection.map_err(ServerSelectionError::Selection)
    }
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

/// Survey every candidate over the sweep exit concurrently, recording each
/// attempt in the indexer history like any probe.
async fn survey(
    socks5_addr: std::net::SocketAddr,
    candidates: &[Uri],
    history: &crate::lightclient::indexer_history::IndexerHistoryHandle,
) -> Vec<SurveyResult> {
    let timeout = zingo_netutils::time::PROBE_LEG_TIMEOUT;
    futures::future::join_all(candidates.iter().map(|uri| async move {
        let reported = probe_one(socks5_addr, uri, timeout, history).await;
        SurveyResult {
            uri: uri.clone(),
            reported,
        }
    }))
    .await
}

/// One candidate's survey: the shared mixnet probe over the sweep exit,
/// which times and records the attempt, its success mapped to the reported
/// chain and height and any failure to `None`.
async fn probe_one(
    socks5_addr: std::net::SocketAddr,
    uri: &Uri,
    timeout: Duration,
    history: &crate::lightclient::indexer_history::IndexerHistoryHandle,
) -> Option<ProbeSuccess> {
    crate::mixnet::probe::probe_indexer(uri, socks5_addr, timeout, history)
        .await
        .leg
        .outcome
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixnet::MixnetMode;

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

        let reported = probe_one(
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
