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
    /// with `first` — the caller's pinned clearnet sync indexer, when one
    /// exists — guaranteed a lane in the opening wave, and the opening
    /// wave's verdict is offered to the session as soon as it forms: the
    /// remaining candidates keep being surveyed in the background purely as
    /// the health sweep, and the sweep transport recycles its exit when
    /// that finishes.
    pub async fn run_server_selection_sweep(
        &self,
        binary_path: &Path,
        candidates: &[Uri],
        first: Option<&Uri>,
        progress: impl Fn(SweepProgress),
    ) -> Result<Selection, ServerSelectionError> {
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
            return Err(ServerSelectionError::TransportAcquisition(
                crate::mixnet::acquire::TransportError::ExitOutsideClutch { reported: exits },
            ));
        };
        drop(clutch);
        let member: crate::correspondent::pool::Member<
            crate::mixnet::MixnetProxy,
            crate::correspondent::pool::Shared,
        > = crate::correspondent::pool::Member::new(proxy, lease);
        progress(SweepProgress::Surveying {
            candidates: candidates.len(),
        });
        let width = survey_tunnel_width(candidates.len());
        let order = sweep::wave_order(candidates, first, width, &mut rand::rngs::OsRng);

        // Waves run inline only until a verdict forms; every candidate a
        // formed verdict leaves unsurveyed continues in the background as
        // the health sweep, and the transport's exit recycles after it.
        let mut results: Vec<SurveyResult> = Vec::new();
        let mut surveyed_through = 0;
        let mut verdict: Option<Selection> = None;
        for wave in order.chunks(width) {
            results.extend(survey(socks5_addr, wave.to_vec(), &self.indexer_history).await);
            surveyed_through += wave.len();
            progress(SweepProgress::Judging {
                answered: results.iter().filter(|r| r.reported.is_some()).count(),
                surveyed: surveyed_through,
            });
            match sweep::select(&results, SWEEP_HEIGHT_TOLERANCE, &mut rand::rngs::OsRng) {
                Ok(selection) => {
                    verdict = Some(selection);
                    break;
                }
                Err(SweepError::EmptyCohort { .. }) if surveyed_through < order.len() => {}
                Err(refusal) => {
                    member.retire().await;
                    return Err(refusal.into());
                }
            }
        }
        let Some(selection) = verdict else {
            // Reached only by an empty candidate list, which surveys
            // nothing and forms no verdict.
            member.retire().await;
            return Err(SweepError::EmptyCohort {
                surveyed: results.len(),
                answered: results.iter().filter(|r| r.reported.is_some()).count(),
                causes: sweep::RefusalTally::of(&results),
            }
            .into());
        };

        let rest: Vec<Uri> = order[surveyed_through.min(order.len())..].to_vec();
        if rest.is_empty() {
            member.retire().await;
        } else {
            let history = self.indexer_history.clone();
            tokio::spawn(async move {
                let _health_only = survey(socks5_addr, rest, &history).await;
                // Exit Recycling: retiring the member kills the child and
                // recycles its lease, so no later traffic rides the exit
                // that observed the survey.
                member.retire().await;
            });
        }
        Ok(selection)
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
    .map_err(|refusal| match refusal {
        crate::mixnet::acquire::TransportError::DiedDuringBootstrap { detail } => {
            ServerSelectionError::TransportDied { detail }
        }
        crate::mixnet::acquire::TransportError::StatusChannelClosed => {
            ServerSelectionError::TransportStatusClosed
        }
        crate::mixnet::acquire::TransportError::NotReady { budget } => {
            ServerSelectionError::TransportTimeout { budget }
        }
        other => ServerSelectionError::TransportAcquisition(other),
    })
}

/// The saturation bound on concurrent survey tunnels: every tunnel shares
/// the one sweep exit's packet pipeline, a single measured round trip costs
/// seconds, and four concurrent TLS handshakes was the widest fan-out that
/// kept each near its solo cost instead of blowing every probe's
/// [`zingo_netutils::time::PROBE_LEG_TIMEOUT`] together — the 0-of-17
/// signature.
pub const MAX_SURVEY_TUNNEL_WIDTH: usize = 4;

/// The divisor bounding the fan-out to a fraction of the candidate list, so
/// a small census is never surveyed all at once.
const SURVEY_FANOUT_DIVISOR: usize = 4;

/// The narrowest survey: one tunnel, the sequential floor.
const MIN_SURVEY_TUNNEL_WIDTH: usize = 1;

/// The number of survey tunnels open at once for a survey of `candidates` —
/// a bounded function of the census size: at least one, at most a
/// `SURVEY_FANOUT_DIVISOR`th of the list, never past the measured
/// [`MAX_SURVEY_TUNNEL_WIDTH`] — counting connections through the one Nym
/// client rather than processes, so the same calibration serves a spawned
/// desktop binary and the in-process client Android and iOS host under the
/// single-process constraint.
pub fn survey_tunnel_width(candidates: usize) -> usize {
    candidates
        .div_ceil(SURVEY_FANOUT_DIVISOR)
        .clamp(MIN_SURVEY_TUNNEL_WIDTH, MAX_SURVEY_TUNNEL_WIDTH)
}

/// Survey every candidate over the sweep exit, at most
/// [`survey_tunnel_width`] tunnels at a time, recording each attempt in the
/// indexer history like any probe.
async fn survey(
    socks5_addr: std::net::SocketAddr,
    candidates: Vec<Uri>,
    history: &crate::lightclient::indexer_history::IndexerHistoryHandle,
) -> Vec<SurveyResult> {
    use futures::StreamExt as _;
    let timeout = zingo_netutils::time::PROBE_LEG_TIMEOUT;
    let width = survey_tunnel_width(candidates.len());
    futures::stream::iter(candidates)
        .map(|uri| async move {
            let (reported, refusal) = probe_one(socks5_addr, &uri, timeout, history).await;
            SurveyResult {
                uri,
                reported,
                refusal,
            }
        })
        .buffer_unordered(width)
        .collect()
        .await
}

/// One candidate's survey: `GetLatestBlock` over the sweep exit, its success
/// mapped to the reported tip height, any failure to a classified refusal.
async fn probe_one(
    socks5_addr: std::net::SocketAddr,
    uri: &Uri,
    timeout: Duration,
    history: &crate::lightclient::indexer_history::IndexerHistoryHandle,
) -> (
    Option<u64>,
    Option<crate::lightclient::indexer_history::FailureKind>,
) {
    use crate::lightclient::indexer_history::{
        AttemptKind, AttemptRoute, FailureKind, IndexerAttempt, now_unix_secs,
    };
    let host = crate::correspondent::Host::of_uri(uri);
    let result = zingo_netutils::get_latest_block_via_socks5(socks5_addr, uri, timeout).await;
    let (reported, refusal) = match &result {
        Ok(tip) => (Some(tip.height), None),
        Err(error) => (None, Some(FailureKind::classify(&error.to_string()))),
    };
    history.record(&IndexerAttempt {
        unix_secs: now_unix_secs(),
        host,
        route: AttemptRoute::Mixnet,
        kind: AttemptKind::Probe,
        millis: 0,
        phase: result
            .as_ref()
            .err()
            .map(|error| crate::mixnet::charge_phase(&crate::mixnet::socks5_transmit_stage(error))),
        exit: None,
        outcome: match refusal {
            None => Ok(()),
            Some(kind) => Err(kind),
        },
    });
    (reported, refusal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixnet::MixnetMode;

    /// HYPOTHESIS: the survey width is a bounded function of the census —
    /// at least one tunnel, at most a quarter of the candidates, never past
    /// the measured saturation bound — so a small census is never surveyed
    /// all at once and a large one never saturates the shared exit.
    /// Falsified if any bound moves.
    #[test]
    fn the_survey_width_is_bounded_by_the_census() {
        assert_eq!(survey_tunnel_width(0), MIN_SURVEY_TUNNEL_WIDTH);
        assert_eq!(survey_tunnel_width(1), 1);
        assert_eq!(survey_tunnel_width(4), 1);
        assert_eq!(survey_tunnel_width(5), 2);
        assert_eq!(survey_tunnel_width(16), 4);
        assert_eq!(survey_tunnel_width(17), MAX_SURVEY_TUNNEL_WIDTH);
        assert_eq!(survey_tunnel_width(100), MAX_SURVEY_TUNNEL_WIDTH);
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
        let publisher = crate::mixnet::status_publisher();
        publisher.send_replace(ready_with(Vec::new()));
        let mut receiver = publisher.subscribe();
        let waiter = tokio::spawn(async move { await_sweep_ready(&mut receiver).await });
        // Let the waiter observe the exitless Ready and park on the channel.
        for _ in 0..8 {
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
    /// exit exceeds the bootstrap budget as a typed timeout, so an empty
    /// announcement is a wait, never an ExitOutsideClutch refusal.
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
            matches!(refusal, ServerSelectionError::TransportTimeout { .. }),
            "an exitless Ready exhausts the budget as a timeout, got: {refusal}"
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
