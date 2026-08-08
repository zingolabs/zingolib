//! Censorship-resistant transmission over the Nym mixnet: an escalating,
//! serially gated Correspondent Rotation over the curated Correspondent list.
//!
//! The adversary is a Correspondent that suppresses a send (accepting the
//! connection but declining to relay, or misreporting the outcome), so the
//! send must be able to route around it to honest indexers. The first round
//! submits to a single random Correspondent (Correspondent Rotation, and the
//! common success path). Only if that fails to confirm delivery does the send
//! escalate, submitting to two fresh Correspondents in parallel, then three,
//! each round gated on the complete failure of the round before it. The
//! escalation stops at `MAX_TRANSMISSION_CORRESPONDENTS` distinct
//! Correspondents, which the one-two-three schedule reaches at the end of the
//! third round. Within a round the first Correspondent to confirm delivery
//! wins and the rest are abandoned. See
//! `docs/adr/0011-nym-mixnet-transmission.md`.
//!
//! This orchestrates the shared per-submission policy (retry,
//! duplicate-in-mempool, queued-probe, delivery-check) rather than
//! duplicating it: each pull is a call to `resilient_transmit`
//! (`crate::lightclient::transmit`, crate-private, so no intra-doc link
//! from this public module), the same policy the clearnet path runs. The escalation logic itself is
//! the shared pure racing planner ([`zingo_netutils::arm_race`]) under its
//! serially gated [`LaunchPolicy::EscalatingRounds`]. This module drives the
//! planner's actions over borrowed futures and keeps the ratified schedule.
//! The per-pull runner and the random-number generator are injected, so the
//! round, escalation, and cap logic is exercised in CI without a live mixnet
//! or real time.
#![forbid(unsafe_code)]

use std::future::Future;

use futures::StreamExt as _;
use futures::stream::FuturesUnordered;
use http::Uri;
use rand::Rng;
use rand::seq::SliceRandom;
use zingo_netutils::arm_race::{LaunchPolicy, RaceAction, RaceEvent, RaceState};

/// The maximum number of distinct Correspondents a single send may contact
/// before it surfaces failure (ADR 0011). The escalating one-two-three
/// schedule reaches this at the end of the third round. It is the circuit
/// breaker for an untransmittable transaction, since the client cannot
/// classify a rejection.
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

/// Transmit to `indexers` as an escalating, serially gated Correspondent
/// Rotation, returning the server-reported txid of the first Correspondent to
/// confirm delivery. `run_pull` submits to one indexer and resolves to
/// `Ok(server_txid)` on confirmed delivery or `Err(msg)` otherwise. `rng`
/// chooses the random order (Correspondent Rotation), and `cap` bounds the
/// distinct Correspondents contacted.
///
/// Round `r` submits to `r` fresh indexers in parallel, and round `r + 1` runs
/// only after every pull of round `r` fails, so parallelism widens only as
/// evidence of censorship or failure accumulates. Within a round the first pull
/// to confirm delivery wins and the rest are abandoned. The one-two-three
/// schedule stops once `cap` distinct indexers have been contacted.
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

    let mut race = RaceState::new(order.len(), cap, LaunchPolicy::EscalatingRounds);
    let mut pulls = FuturesUnordered::new();
    let mut lost = false;
    let mut failures: Vec<CorrespondentAttempt<E>> = Vec::new();

    let launch = |actions: Vec<RaceAction>, pulls: &mut FuturesUnordered<_>, lost: &mut bool| {
        for action in actions {
            match action {
                RaceAction::Launch { arm } => {
                    let pull = run_pull(indexers[order[arm]].clone());
                    pulls.push(async move { (arm, pull.await) });
                }
                // The serially gated rounds policy never hedges on time.
                RaceAction::SetHedgeTimer(_) => {}
                RaceAction::GiveUp => *lost = true,
            }
        }
    };

    launch(race.start(), &mut pulls, &mut lost);
    report(race.progress().to_string());

    while !lost {
        let Some((arm, outcome)) = pulls.next().await else {
            break;
        };
        match outcome {
            // The first pull to confirm delivery wins; dropping `pulls`
            // abandons the round's remaining pulls.
            Ok(server_txid) => return Ok(server_txid),
            Err(error) => {
                // The race planner's event wants a rendered line for its
                // progress narration; the typed failure itself is kept.
                launch(
                    race.on_event(RaceEvent::PullFailed {
                        arm,
                        error: error.to_string(),
                    }),
                    &mut pulls,
                    &mut lost,
                );
                failures.push(CorrespondentAttempt {
                    correspondent: host_of(arm),
                    failure: error,
                });
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
    /// created (which the orchestrator does for all of a round's pulls before it
    /// awaits any), so a round's full width is counted even when an early pull
    /// wins the race.
    struct MockArms {
        scripts: HashMap<String, Result<String, String>>,
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
                contacted: Mutex::new(Vec::new()),
            }
        }

        fn run(&self, indexer: Uri) -> impl Future<Output = Result<String, String>> + '_ {
            let host = host_of(&indexer);
            self.contacted.lock().unwrap().push(host.clone());
            let scripted = self
                .scripts
                .get(&host)
                .cloned()
                .unwrap_or_else(|| Err(format!("unscripted host {host}")));
            async move { scripted }
        }

        fn contacted(&self) -> Vec<String> {
            self.contacted.lock().unwrap().clone()
        }
    }

    /// Reproduce the orchestrator's shuffle so a test can name the indexer a
    /// given seed contacts first (round one) versus later (escalation).
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
    async fn first_round_success_contacts_exactly_one_correspondent() {
        // Every indexer would accept; the single round-one pick must end it, so
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
        assert_eq!(mock.contacted().len(), 1, "only round one ran");
    }

    #[tokio::test]
    async fn escalates_to_the_second_round_when_the_first_fails() {
        // Fail the seed's round-one pick; accept everywhere else. The send must
        // escalate to round two (two more indexers) and succeed, contacting
        // exactly 1 + 2 = 3 distinct indexers.
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
        .expect("round two accepts");
        assert_eq!(ok, "txid");
        assert_eq!(
            mock.contacted().len(),
            3,
            "round one plus a full second round"
        );
        assert_eq!(mock.contacted()[0], first, "round one is the single pick");
    }

    #[tokio::test]
    async fn all_failing_stops_at_the_cap() {
        // Ten indexers, every one suppressing. The escalation must stop at the
        // six-Correspondent cap (1 + 2 + 3), not walk the whole list.
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
    /// if a fully failing transmission never reports the single-pull round one
    /// or a widened later round.
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
            "round one must narrate its single pull: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("2 in flight")),
            "escalation to a widened round must be narrated: {lines:?}"
        );
    }

    #[tokio::test]
    async fn a_seed_picks_the_same_first_correspondent_every_time() {
        // Correspondent Rotation is driven by the injected RNG, so a fixed
        // seed is reproducible: the same indexer carries round one across runs.
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
            "the seed fixes the round-one Correspondent"
        );
    }
}
