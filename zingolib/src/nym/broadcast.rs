//! Censorship-resistant broadcast over the Nym mixnet: an escalating, serially
//! gated fan-out over the curated Broadcast Indexer list.
//!
//! The adversary is a Broadcast Indexer that suppresses a send (accepting the
//! connection but declining to relay, or misreporting the outcome), so the
//! send must be able to route around it to honest indexers. The first round
//! submits to a single random indexer (witness rotation, and the common
//! success path). Only if that fails to confirm delivery does the send
//! escalate, submitting to two fresh indexers in parallel, then three, each
//! round gated on the complete failure of the round before it. The fan-out
//! stops at `MAX_BROADCAST_WITNESSES` distinct indexers, which the one-two-
//! three schedule reaches at the end of the third round. Within a round the
//! first indexer to confirm delivery wins and the rest are abandoned. See
//! `docs/adr/0011-nym-mixnet-transmission.md`.
//!
//! This orchestrates the shared per-submission policy (retry,
//! duplicate-in-mempool, queued-probe, delivery-check) rather than
//! duplicating it: each arm is a call to `resilient_transmit`
//! (`crate::lightclient::transmit`, crate-private, so no intra-doc link
//! from this public module), the same policy the clearnet path runs. The escalation logic itself is
//! the shared pure racing planner ([`zingo_netutils::arm_race`]) under its
//! serially gated [`LaunchPolicy::EscalatingRounds`]. This module drives the
//! planner's actions over borrowed futures and keeps the ratified schedule.
//! The per-arm runner and the random-number generator are injected, so the
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

/// The maximum number of distinct Broadcast Indexers a single send may contact
/// before it surfaces failure (ADR 0011). The escalating one-two-three fan-out
/// reaches this at the end of the third round. It is the circuit breaker for an
/// unbroadcastable transaction, since the client cannot classify a rejection.
pub(crate) const MAX_BROADCAST_WITNESSES: usize = 6;

/// Why a fan-out broadcast failed.
#[derive(Clone, Debug, thiserror::Error)]
pub(crate) enum FanoutError {
    /// The Broadcast Indexer list was empty.
    #[error("the Broadcast Indexer list is empty")]
    NoIndexers,
    /// The cap was reached without any indexer confirming delivery.
    #[error("no indexer confirmed delivery after contacting {attempts} of them: {summary}")]
    AllFailed {
        /// The number of distinct indexers contacted.
        attempts: usize,
        /// Every witness's failure, surfaced for the user: which indexers
        /// were tried and how each one failed.
        summary: String,
    },
}

/// Broadcast to `indexers` as an escalating, serially gated fan-out, returning
/// the server-reported txid of the first indexer to confirm delivery. `run_arm`
/// submits to one indexer and resolves to `Ok(server_txid)` on confirmed
/// delivery or `Err(msg)` otherwise. `rng` chooses the random order (witness
/// rotation), and `cap` bounds the distinct indexers contacted.
///
/// Round `r` submits to `r` fresh indexers in parallel, and round `r + 1` runs
/// only after every arm of round `r` fails, so parallelism widens only as
/// evidence of censorship or failure accumulates. Within a round the first arm
/// to confirm delivery wins and the rest are abandoned. The one-two-three
/// schedule stops once `cap` distinct indexers have been contacted.
///
/// `report` receives a succinct progress line whenever the race's shape
/// changes (a launch or an arm failure), rendering the planner's own
/// [`RaceProgress`] snapshot for display.
pub(crate) async fn fanout_broadcast<A, F, R, P>(
    indexers: &[Uri],
    rng: &mut R,
    cap: usize,
    run_arm: A,
    report: P,
) -> Result<String, FanoutError>
where
    A: Fn(Uri) -> F,
    F: Future<Output = Result<String, String>>,
    R: Rng + ?Sized,
    P: Fn(String),
{
    if indexers.is_empty() {
        return Err(FanoutError::NoIndexers);
    }

    // One shuffle yields both the initial random pick and a repetition-free
    // random escalation order: no indexer is contacted twice.
    let mut order: Vec<usize> = (0..indexers.len()).collect();
    order.shuffle(rng);

    let host_of = |candidate: usize| {
        let indexer = &indexers[order[candidate]];
        indexer
            .host()
            .map(str::to_string)
            .unwrap_or_else(|| indexer.to_string())
    };

    let mut race = RaceState::new(order.len(), cap, LaunchPolicy::EscalatingRounds);
    let mut arms = FuturesUnordered::new();
    let mut lost = false;

    let launch = |actions: Vec<RaceAction>, arms: &mut FuturesUnordered<_>, lost: &mut bool| {
        for action in actions {
            match action {
                RaceAction::Launch { candidate } => {
                    let arm = run_arm(indexers[order[candidate]].clone());
                    arms.push(async move { (candidate, arm.await) });
                }
                // The serially gated rounds policy never hedges on time.
                RaceAction::ArmHedgeTimer(_) => {}
                RaceAction::GiveUp => *lost = true,
            }
        }
    };

    launch(race.start(), &mut arms, &mut lost);
    report(race.progress().to_string());

    while !lost {
        let Some((candidate, outcome)) = arms.next().await else {
            break;
        };
        match outcome {
            // The first arm to confirm delivery wins; dropping `arms`
            // abandons the round's remaining arms.
            Ok(server_txid) => return Ok(server_txid),
            Err(error) => {
                launch(
                    race.on_event(RaceEvent::ArmFailed { candidate, error }),
                    &mut arms,
                    &mut lost,
                );
                report(race.progress().to_string());
            }
        }
    }

    Err(FanoutError::AllFailed {
        attempts: race.launched(),
        summary: race.failure_summary(host_of),
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

    /// An arm runner returning a scripted result per indexer host and recording
    /// every indexer it was asked to contact. Recording happens when the arm is
    /// created (which the orchestrator does for all of a round's arms before it
    /// awaits any), so a round's full width is counted even when an early arm
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
        let err = fanout_broadcast(
            &[],
            &mut StdRng::seed_from_u64(1),
            6,
            |u| mock.run(u),
            |_| (),
        )
        .await
        .expect_err("no indexers");
        assert!(matches!(err, FanoutError::NoIndexers));
        assert!(mock.contacted().is_empty());
    }

    #[tokio::test]
    async fn first_round_success_contacts_exactly_one_witness() {
        // Every indexer would accept; the single round-one pick must end it, so
        // the happy path keeps its single-witness discipline.
        let indexers = uris(&["a", "b", "c", "d"]);
        let mock = MockArms::new(&[
            ("a", Ok("txid")),
            ("b", Ok("txid")),
            ("c", Ok("txid")),
            ("d", Ok("txid")),
        ]);
        let ok = fanout_broadcast(
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

        let ok = fanout_broadcast(
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
        // Ten indexers, every one suppressing. The fan-out must stop at the
        // six-witness cap (1 + 2 + 3), not walk the whole list.
        let hosts = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
        let indexers = uris(&hosts);
        let scripts: Vec<(&str, Result<&str, &str>)> =
            hosts.iter().map(|&h| (h, Err("suppressed"))).collect();
        let mock = MockArms::new(&scripts);

        let err = fanout_broadcast(
            &indexers,
            &mut StdRng::seed_from_u64(3),
            MAX_BROADCAST_WITNESSES,
            |u| mock.run(u),
            |_| (),
        )
        .await
        .expect_err("no indexer confirms");
        match err {
            FanoutError::AllFailed { attempts, .. } => assert_eq!(attempts, 6),
            other => panic!("expected AllFailed, got {other:?}"),
        }
        let contacted = mock.contacted();
        assert_eq!(contacted.len(), 6, "capped at six witnesses");
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

        let err = fanout_broadcast(
            &indexers,
            &mut StdRng::seed_from_u64(5),
            6,
            |u| mock.run(u),
            |_| (),
        )
        .await
        .expect_err("all down");
        match err {
            FanoutError::AllFailed { attempts, .. } => assert_eq!(attempts, 3),
            other => panic!("expected AllFailed, got {other:?}"),
        }
        assert_eq!(mock.contacted().len(), 3);
    }

    /// HYPOTHESIS: a failed fan-out accounts for every witness contacted and
    /// how each failed, down to the first. Falsified if any
    /// contacted host is missing from the error summary.
    #[tokio::test]
    async fn a_failed_fanout_names_every_witness_and_its_failure() {
        let hosts = ["a", "b", "c"];
        let indexers = uris(&hosts);
        let scripts: Vec<(&str, Result<&str, &str>)> =
            hosts.iter().map(|&h| (h, Err("suppressed"))).collect();
        let mock = MockArms::new(&scripts);

        let err = fanout_broadcast(
            &indexers,
            &mut StdRng::seed_from_u64(5),
            6,
            |u| mock.run(u),
            |_| (),
        )
        .await
        .expect_err("all suppressed");
        let FanoutError::AllFailed { summary, .. } = err else {
            panic!("expected AllFailed");
        };
        for host in hosts {
            assert!(
                summary.contains(&format!("{host}: suppressed")),
                "the summary must name {host}: {summary}"
            );
        }
    }

    /// HYPOTHESIS: the fan-out narrates its shape at every launch and
    /// failure, so a heartbeat consumer can render the race live. Falsified
    /// if a fully failing fan-out never reports the single-arm round one or a
    /// widened later round.
    #[tokio::test]
    async fn a_failing_fanout_narrates_its_escalation() {
        let hosts = ["a", "b", "c", "d", "e", "f"];
        let indexers = uris(&hosts);
        let scripts: Vec<(&str, Result<&str, &str>)> =
            hosts.iter().map(|&h| (h, Err("suppressed"))).collect();
        let mock = MockArms::new(&scripts);

        let lines = Mutex::new(Vec::<String>::new());
        let _ = fanout_broadcast(
            &indexers,
            &mut StdRng::seed_from_u64(11),
            MAX_BROADCAST_WITNESSES,
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
            "round one must narrate its single arm: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("2 in flight")),
            "escalation to a widened round must be narrated: {lines:?}"
        );
    }

    #[tokio::test]
    async fn a_seed_picks_the_same_first_witness_every_time() {
        // Witness rotation is driven by the injected RNG, so a fixed seed is
        // reproducible: the same indexer carries round one across runs.
        let hosts = ["a", "b", "c", "d", "e"];
        let indexers = uris(&hosts);
        let scripts: Vec<(&str, Result<&str, &str>)> =
            hosts.iter().map(|&h| (h, Err("down"))).collect();

        let first_run = MockArms::new(&scripts);
        let _ = fanout_broadcast(
            &indexers,
            &mut StdRng::seed_from_u64(99),
            6,
            |u| first_run.run(u),
            |_| (),
        )
        .await;

        let second_run = MockArms::new(&scripts);
        let _ = fanout_broadcast(
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
            "the seed fixes the round-one witness"
        );
    }
}
