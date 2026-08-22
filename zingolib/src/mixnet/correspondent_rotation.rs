//! Censorship-resistant transmission over the Nym mixnet: a hedged
//! Correspondent Rotation over the curated Correspondent list.
//!
//! The adversary is a Correspondent that suppresses a send (accepting the
//! connection but declining to relay, stalling silently, or misreporting
//! the outcome), so the send must be able to route around it to honest
//! indexers. One random Correspondent is submitted to first (Correspondent
//! Rotation, and the common success path). A further Correspondent learns
//! of the transaction only on evidence: an arm's failure, or
//! [`TRANSMISSION_HEDGE_INTERVAL`] of silence — an interval sized so a
//! responsive Correspondent's confirmed delivery ends the race before the
//! first hedge fires. At most [`RESERVATION_CLUTCH_SIZE`] arms fly at
//! once, the race stops at `MAX_TRANSMISSION_CORRESPONDENTS` distinct
//! Correspondents, and the first confirmed delivery wins with the rest
//! abandoned. See
//! `docs/adr/0040-sends-escalation-is-a-hedged-race-of-full-paths.md`.
//!
//! This orchestrates the shared per-submission policy (retry,
//! duplicate-in-mempool, queued-probe, delivery-check) rather than
//! duplicating it: each pull is a call to `resilient_transmit`
//! (`crate::lightclient::transmit`, crate-private, so no intra-doc link
//! from this public module), the same policy the clearnet path runs. The escalation logic itself is
//! the shared pure racing planner ([`zingo_netutils::arm_race`]) under its
//! [`LaunchPolicy::Hedged`]. This module drives the planner's actions over
//! borrowed futures and owns the single hedge timer the planner schedules.
//! The per-pull runner and the random-number generator are injected, so
//! the hedging, widening, and cap logic is exercised in CI without a live
//! mixnet, on the runtime's paused clock.
#![forbid(unsafe_code)]

use std::future::Future;

use futures::StreamExt as _;
use futures::stream::FuturesUnordered;
use http::Uri;
use rand::Rng;
use rand::seq::SliceRandom;
use zingo_netutils::arm_race::{
    LaunchPolicy, RESERVATION_CLUTCH_SIZE, RaceAction, RaceEvent, RaceState,
};
use zingo_netutils::time::TRANSMISSION_HEDGE_INTERVAL;

/// The maximum number of distinct Correspondents a single send may contact
/// before it surfaces failure (ADR 0011, schedule superseded by ADR 0040).
/// It is the circuit breaker for an untransmittable transaction, since the
/// client cannot classify a rejection.
pub(crate) const MAX_TRANSMISSION_CORRESPONDENTS: usize = 6;

/// One Correspondent's failed pull: which indexer was tried and its typed
/// failure, carried whole (`docs/agents/net-diag-design.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CorrespondentAttempt<E> {
    /// The Correspondent's host.
    pub correspondent: String,
    /// The pull's failure, untouched.
    pub failure: E,
}

/// Why an escalating transmission failed. The failed attempts are a vector of
/// typed records; the joined-prose rendering is a `Display` on top of that
/// vector for the existing string consumers, never the storage form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EscalationError<E> {
    /// The Correspondent list was empty.
    NoIndexers,
    /// The cap was reached without any Correspondent confirming delivery.
    AllFailed {
        /// The number of distinct Correspondents contacted.
        attempts: usize,
        /// Every Correspondent's failure: which indexers were tried and how
        /// each pull failed, in completion order.
        failures: Vec<CorrespondentAttempt<E>>,
    },
}

impl<E: std::fmt::Display> std::fmt::Display for EscalationError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EscalationError::NoIndexers => write!(f, "the Correspondent list is empty"),
            EscalationError::AllFailed { attempts, failures } => {
                write!(
                    f,
                    "no indexer confirmed delivery after contacting {attempts} of them: "
                )?;
                for (i, attempt) in failures.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{}: {}", attempt.correspondent, attempt.failure)?;
                }
                Ok(())
            }
        }
    }
}

impl<E: std::fmt::Display + std::fmt::Debug> std::error::Error for EscalationError<E> {}

/// Transmit to `indexers` as a hedged Correspondent Rotation, returning the
/// server-reported txid of the first Correspondent to confirm delivery.
/// `run_pull` submits to one indexer and resolves to `Ok(server_txid)` on
/// confirmed delivery or `Err(msg)` otherwise. `rng` chooses the random
/// order (Correspondent Rotation), and `cap` bounds the distinct
/// Correspondents contacted.
///
/// One pull launches first; a further pull launches only after
/// [`TRANSMISSION_HEDGE_INTERVAL`] of silence or immediately on a pull's
/// failure, holding at most [`RESERVATION_CLUTCH_SIZE`] pulls in flight,
/// so parallelism widens only as evidence of censorship, failure, or
/// suspicious silence accumulates. The first pull to confirm delivery wins
/// and the rest are abandoned.
///
/// `report` receives a succinct progress line whenever the race's shape
/// changes (a launch or a pull failure), rendering the planner's own
/// [`RaceProgress`](zingo_netutils::arm_race::RaceProgress) snapshot for display.
pub(crate) async fn escalating_transmit<A, F, E, R, P, T>(
    indexers: &[Uri],
    rng: &mut R,
    cap: usize,
    run_pull: A,
    report: P,
) -> Result<T, EscalationError<E>>
where
    A: Fn(Uri) -> F,
    F: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
    R: Rng + ?Sized,
    P: Fn(String),
{
    if indexers.is_empty() {
        return Err(EscalationError::NoIndexers);
    }

    // One shuffle yields both the initial random pick and a repetition-free
    // random escalation order: no indexer is contacted twice.
    let mut order: Vec<usize> = (0..indexers.len()).collect();
    order.shuffle(rng);

    let host_of = |arm: usize| {
        let indexer = &indexers[order[arm]];
        indexer
            .host()
            .map(str::to_string)
            .unwrap_or_else(|| indexer.to_string())
    };

    let mut race = RaceState::new(
        order.len(),
        cap,
        LaunchPolicy::Hedged {
            max_parallel: RESERVATION_CLUTCH_SIZE,
            hedge_interval: TRANSMISSION_HEDGE_INTERVAL,
        },
    );
    let mut pulls = FuturesUnordered::new();
    let mut lost = false;
    let mut failures: Vec<CorrespondentAttempt<E>> = Vec::new();
    // The planner schedules at most one hedge timer; the driver owns it.
    let mut hedge_timer: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;

    let apply = |actions: Vec<RaceAction>,
                 pulls: &mut FuturesUnordered<_>,
                 lost: &mut bool,
                 hedge_timer: &mut Option<std::pin::Pin<Box<tokio::time::Sleep>>>| {
        for action in actions {
            match action {
                RaceAction::Launch { arm } => {
                    let pull = run_pull(indexers[order[arm]].clone());
                    pulls.push(async move { (arm, pull.await) });
                }
                RaceAction::SetHedgeTimer(interval) => {
                    *hedge_timer = Some(Box::pin(tokio::time::sleep(interval)));
                }
                RaceAction::GiveUp => *lost = true,
            }
        }
    };

    enum Wake<T, E> {
        Pull(usize, Result<T, E>),
        HedgeElapsed,
        Drained,
    }

    apply(race.start(), &mut pulls, &mut lost, &mut hedge_timer);
    report(race.progress().to_string());

    while !lost {
        let wake = {
            let timer = async {
                match hedge_timer.as_mut() {
                    Some(sleep) => sleep.as_mut().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                next = pulls.next() => match next {
                    Some((arm, outcome)) => Wake::Pull(arm, outcome),
                    None => Wake::Drained,
                },
                () = timer => Wake::HedgeElapsed,
            }
        };
        match wake {
            Wake::Drained => break,
            // The first pull to confirm delivery wins; dropping `pulls`
            // abandons the remaining pulls.
            Wake::Pull(_, Ok(server_txid)) => return Ok(server_txid),
            Wake::Pull(arm, Err(error)) => {
                // The race planner's event wants a rendered line for its
                // progress narration; the typed failure itself is kept.
                apply(
                    race.on_event(RaceEvent::PullFailed {
                        arm,
                        error: error.to_string(),
                    }),
                    &mut pulls,
                    &mut lost,
                    &mut hedge_timer,
                );
                failures.push(CorrespondentAttempt {
                    correspondent: host_of(arm),
                    failure: error,
                });
                report(race.progress().to_string());
            }
            Wake::HedgeElapsed => {
                hedge_timer = None;
                apply(
                    race.on_event(RaceEvent::HedgeElapsed),
                    &mut pulls,
                    &mut lost,
                    &mut hedge_timer,
                );
                report(race.progress().to_string());
            }
        }
    }

    Err(EscalationError::AllFailed {
        attempts: race.launched(),
        failures,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    fn uris(hosts: &[&str]) -> Vec<Uri> {
        hosts
            .iter()
            .map(|h| format!("https://{h}:443").parse().unwrap())
            .collect()
    }

    fn host_of(indexer: &Uri) -> String {
        indexer.host().expect("indexer uri has a host").to_string()
    }

    /// A pull runner returning a scripted result per indexer host and recording
    /// every indexer it was asked to contact. Recording happens when the pull is
    /// created, so the race's full width is counted even when an early pull
    /// wins the race. A host in `hang` never resolves, the shape of a
    /// Correspondent that accepts the connection and stalls silently.
    struct MockArms {
        scripts: HashMap<String, Result<String, String>>,
        hang: std::collections::HashSet<String>,
        contacted: Mutex<Vec<String>>,
    }

    impl MockArms {
        fn new(scripts: &[(&str, Result<&str, &str>)]) -> Self {
            let scripts = scripts
                .iter()
                .map(|(host, result)| {
                    let owned = match result {
                        Ok(txid) => Ok((*txid).to_string()),
                        Err(message) => Err((*message).to_string()),
                    };
                    ((*host).to_string(), owned)
                })
                .collect();
            MockArms {
                scripts,
                hang: std::collections::HashSet::new(),
                contacted: Mutex::new(Vec::new()),
            }
        }

        fn hanging(hosts: &[&str]) -> Self {
            MockArms {
                scripts: HashMap::new(),
                hang: hosts.iter().map(|h| (*h).to_string()).collect(),
                contacted: Mutex::new(Vec::new()),
            }
        }

        fn run(&self, indexer: Uri) -> impl Future<Output = Result<String, String>> + '_ {
            let host = host_of(&indexer);
            self.contacted.lock().unwrap().push(host.clone());
            let hang = self.hang.contains(&host);
            let scripted = self
                .scripts
                .get(&host)
                .cloned()
                .unwrap_or_else(|| Err(format!("unscripted host {host}")));
            async move {
                if hang {
                    std::future::pending::<()>().await;
                }
                scripted
            }
        }

        fn contacted(&self) -> Vec<String> {
            self.contacted.lock().unwrap().clone()
        }
    }

    /// Reproduce the orchestrator's shuffle so a test can name the indexer a
    /// given seed contacts first versus later (under widening).
    fn shuffled_order(len: usize, seed: u64) -> Vec<usize> {
        let mut order: Vec<usize> = (0..len).collect();
        order.shuffle(&mut StdRng::seed_from_u64(seed));
        order
    }

    #[tokio::test]
    async fn empty_list_is_an_error() {
        let mock = MockArms::new(&[]);
        let err = escalating_transmit(
            &[],
            &mut StdRng::seed_from_u64(1),
            6,
            |u| mock.run(u),
            |_| (),
        )
        .await
        .expect_err("no indexers");
        assert!(matches!(err, EscalationError::NoIndexers));
        assert!(mock.contacted().is_empty());
    }

    #[tokio::test]
    async fn a_confirming_first_pull_contacts_exactly_one_correspondent() {
        // Every indexer would accept; the single first pick must end it, so
        // the happy path keeps its single-Correspondent discipline.
        let indexers = uris(&["a", "b", "c", "d"]);
        let mock = MockArms::new(&[
            ("a", Ok("txid")),
            ("b", Ok("txid")),
            ("c", Ok("txid")),
            ("d", Ok("txid")),
        ]);
        let ok = escalating_transmit(
            &indexers,
            &mut StdRng::seed_from_u64(7),
            6,
            |u| mock.run(u),
            |_| (),
        )
        .await
        .expect("first pick accepts");
        assert_eq!(ok, "txid");
        assert_eq!(mock.contacted().len(), 1, "only the first pull ran");
    }

    #[tokio::test]
    async fn a_failure_widens_by_exactly_one_replacement() {
        // Fail the seed's first pick; accept everywhere else. The failure
        // launches exactly one replacement pull, which succeeds, so exactly
        // two distinct indexers are contacted — hedged widening spends one
        // fresh Correspondent per piece of evidence, never a round of them.
        let hosts = ["a", "b", "c", "d", "e"];
        let indexers = uris(&hosts);
        let seed = 20;
        let order = shuffled_order(hosts.len(), seed);
        let first = hosts[order[0]];

        let scripts: Vec<(&str, Result<&str, &str>)> = hosts
            .iter()
            .map(|&h| {
                if h == first {
                    (h, Err("suppressed"))
                } else {
                    (h, Ok("txid"))
                }
            })
            .collect();
        let mock = MockArms::new(&scripts);

        let ok = escalating_transmit(
            &indexers,
            &mut StdRng::seed_from_u64(seed),
            6,
            |u| mock.run(u),
            |_| (),
        )
        .await
        .expect("the replacement accepts");
        assert_eq!(ok, "txid");
        assert_eq!(
            mock.contacted().len(),
            2,
            "one failure buys exactly one replacement"
        );
        assert_eq!(
            mock.contacted()[0],
            first,
            "the first pull is the single pick"
        );
    }

    #[tokio::test]
    async fn all_failing_stops_at_the_cap() {
        // Ten indexers, every one suppressing. Each failure launches one
        // replacement, and the walk must stop at the six-Correspondent cap,
        // not visit the whole list.
        let hosts = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
        let indexers = uris(&hosts);
        let scripts: Vec<(&str, Result<&str, &str>)> =
            hosts.iter().map(|&h| (h, Err("suppressed"))).collect();
        let mock = MockArms::new(&scripts);

        let err = escalating_transmit(
            &indexers,
            &mut StdRng::seed_from_u64(3),
            MAX_TRANSMISSION_CORRESPONDENTS,
            |u| mock.run(u),
            |_| (),
        )
        .await
        .expect_err("no indexer confirms");
        match err {
            EscalationError::AllFailed { attempts, .. } => assert_eq!(attempts, 6),
            other => panic!("expected AllFailed, got {other:?}"),
        }
        let contacted = mock.contacted();
        assert_eq!(contacted.len(), 6, "capped at six Correspondents");
        let distinct: std::collections::HashSet<_> = contacted.iter().collect();
        assert_eq!(distinct.len(), 6, "no indexer is contacted twice");
    }

    #[tokio::test]
    async fn cap_is_bounded_by_the_list_length() {
        // A cap larger than the list contacts every indexer once and no more.
        let hosts = ["a", "b", "c"];
        let indexers = uris(&hosts);
        let scripts: Vec<(&str, Result<&str, &str>)> =
            hosts.iter().map(|&h| (h, Err("down"))).collect();
        let mock = MockArms::new(&scripts);

        let err = escalating_transmit(
            &indexers,
            &mut StdRng::seed_from_u64(5),
            6,
            |u| mock.run(u),
            |_| (),
        )
        .await
        .expect_err("all down");
        match err {
            EscalationError::AllFailed { attempts, .. } => assert_eq!(attempts, 3),
            other => panic!("expected AllFailed, got {other:?}"),
        }
        assert_eq!(mock.contacted().len(), 3);
    }

    /// HYPOTHESIS: a failed escalation accounts for every Correspondent
    /// contacted and how each failed, as typed records — and the prose
    /// rendering on top still names each one. Falsified if any contacted host
    /// is missing from the typed attempts or the rendering.
    #[tokio::test]
    async fn a_failed_escalation_records_every_correspondent_and_its_failure() {
        let hosts = ["a", "b", "c"];
        let indexers = uris(&hosts);
        let scripts: Vec<(&str, Result<&str, &str>)> =
            hosts.iter().map(|&h| (h, Err("suppressed"))).collect();
        let mock = MockArms::new(&scripts);

        let err = escalating_transmit(
            &indexers,
            &mut StdRng::seed_from_u64(5),
            6,
            |u| mock.run(u),
            |_| (),
        )
        .await
        .expect_err("all suppressed");
        let rendered = err.to_string();
        let EscalationError::AllFailed { failures, .. } = err else {
            panic!("expected AllFailed");
        };
        for host in hosts {
            assert!(
                failures
                    .iter()
                    .any(|a| a.correspondent == host && a.failure == "suppressed"),
                "the typed attempts must record {host}: {failures:?}"
            );
            assert!(
                rendered.contains(&format!("{host}: suppressed")),
                "the rendering must name {host}: {rendered}"
            );
        }
    }

    /// HYPOTHESIS: the escalation narrates its shape at every launch and
    /// failure, so a heartbeat consumer can render the race live. Falsified
    /// if a fully failing transmission never reports the single opening pull
    /// or never reports reaching the cap.
    #[tokio::test]
    async fn a_failing_transmission_narrates_its_escalation() {
        let hosts = ["a", "b", "c", "d", "e", "f"];
        let indexers = uris(&hosts);
        let scripts: Vec<(&str, Result<&str, &str>)> =
            hosts.iter().map(|&h| (h, Err("suppressed"))).collect();
        let mock = MockArms::new(&scripts);

        let lines = Mutex::new(Vec::<String>::new());
        let _ = escalating_transmit(
            &indexers,
            &mut StdRng::seed_from_u64(11),
            MAX_TRANSMISSION_CORRESPONDENTS,
            |u| mock.run(u),
            |line| lines.lock().expect("narration mutex poisoned").push(line),
        )
        .await
        .expect_err("all suppressed");

        let lines = lines.into_inner().expect("narration mutex poisoned");
        assert!(
            lines
                .first()
                .is_some_and(|line| line.contains("1 in flight")),
            "the opening must narrate its single pull: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("attempt 6/6")),
            "the walk to the cap must be narrated: {lines:?}"
        );
    }

    /// HYPOTHESIS: silence alone widens the race — a Correspondent that
    /// accepts and stalls costs one hedge interval, after which a second
    /// Correspondent is contacted while the first pull stays in flight.
    /// Falsified if a silent stall leaves the race at width one.
    #[tokio::test(start_paused = true)]
    async fn silence_alone_widens_the_race_by_one_pull() {
        let hosts = ["a", "b"];
        let indexers = uris(&hosts);
        let mock = MockArms::hanging(&hosts);

        // Both pulls hang forever, so the race cannot resolve; observe it
        // for a bounded window on the paused clock, then cut it loose.
        const OBSERVATION_BUDGET: std::time::Duration =
            TRANSMISSION_HEDGE_INTERVAL.saturating_mul(4);
        let lines = Mutex::new(Vec::<String>::new());
        let raced = tokio::time::timeout(
            OBSERVATION_BUDGET,
            escalating_transmit(
                &indexers,
                &mut StdRng::seed_from_u64(2),
                MAX_TRANSMISSION_CORRESPONDENTS,
                |u| mock.run(u),
                |line| lines.lock().expect("narration mutex poisoned").push(line),
            ),
        )
        .await;

        assert!(raced.is_err(), "hanging pulls must never resolve the race");
        let lines = lines.into_inner().expect("narration mutex poisoned");
        assert!(
            lines.iter().any(|line| line.contains("2 in flight")),
            "the hedge must widen the race on silence: {lines:?}"
        );
        assert_eq!(
            mock.contacted().len(),
            2,
            "the silence hedge contacts exactly the second Correspondent"
        );
    }

    #[tokio::test]
    async fn a_seed_picks_the_same_first_correspondent_every_time() {
        // Correspondent Rotation is driven by the injected RNG, so a fixed
        // seed is reproducible: the same indexer opens the race across runs.
        let hosts = ["a", "b", "c", "d", "e"];
        let indexers = uris(&hosts);
        let scripts: Vec<(&str, Result<&str, &str>)> =
            hosts.iter().map(|&h| (h, Err("down"))).collect();

        let first_run = MockArms::new(&scripts);
        let _ = escalating_transmit(
            &indexers,
            &mut StdRng::seed_from_u64(99),
            6,
            |u| first_run.run(u),
            |_| (),
        )
        .await;

        let second_run = MockArms::new(&scripts);
        let _ = escalating_transmit(
            &indexers,
            &mut StdRng::seed_from_u64(99),
            6,
            |u| second_run.run(u),
            |_| (),
        )
        .await;

        assert_eq!(
            first_run.contacted()[0],
            second_run.contacted()[0],
            "the seed fixes the opening Correspondent"
        );
    }
}
