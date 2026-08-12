//! The pure core of the Server-Selection Sweep (ADR 0034).
//!
//! Selection has two halves: a survey that emits network traffic, and the
//! judgment that turns its results into a sync indexer. This module is the
//! judgment, kept pure so every rule the sweep rests on is pinned without a
//! transport. It takes the survey's per-candidate outcomes and yields the
//! live cohort, the drawn or pinned sync indexer, and the transmit
//! candidates the selection excludes.
#![forbid(unsafe_code)]

use http::Uri;
use rand::seq::SliceRandom as _;

use super::probe::ProbeSuccess;

/// One candidate's survey outcome: the endpoint and what its `GetLightdInfo`
/// reported, or `None` when it did not answer over the mixnet.
#[derive(Clone, Debug)]
pub struct SurveyResult {
    /// The surveyed endpoint.
    pub uri: Uri,
    /// The endpoint's reported chain and height, or `None` on any failure.
    pub reported: Option<ProbeSuccess>,
}

/// A candidate that answered the survey and passed the cohort's liveness
/// test: its chain matched and its height sat within the median tolerance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveCandidate {
    /// The live endpoint.
    pub uri: Uri,
    /// The height it reported, retained for the height-descending order.
    pub height: u64,
}

/// Why a sweep selected no sync indexer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SweepError {
    /// No candidate passed the liveness test, and no server was pinned.
    #[error("no live indexer: {answered} of {surveyed} answered, none within the cohort")]
    EmptyCohort {
        /// How many candidates were surveyed.
        surveyed: usize,
        /// How many answered at all (before the cohort test).
        answered: usize,
    },
    /// A server was pinned, but it did not pass the liveness test.
    #[error("pinned server '{0}' is not live over the mixnet")]
    DeadPin(Uri),
}

/// A sweep's verdict: the chosen sync indexer, the transmit candidates that
/// exclude it, and the live cohort in height-descending failover order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The sync indexer this Sync Session attaches to.
    pub sync_indexer: Uri,
    /// The live candidates that serve transmit operations: the cohort minus
    /// every endpoint of the sync indexer's operator (ADR 0034 — the sync
    /// operator is never also a transmit target).
    pub transmit_candidates: Vec<Uri>,
    /// The full live cohort, height-descending, the sync-attach failover
    /// sequence and the report order.
    pub cohort: Vec<LiveCandidate>,
}

use crate::correspondent::Operator;

/// The median of a nonempty height list, taking the lower of the two middle
/// values for an even count. Integer-only, so the tolerance test needs no
/// float.
fn median(sorted_desc: &[u64]) -> u64 {
    // Reads from a height-descending slice; the middle index is the median
    // either way, and for an even count this picks the lower-middle.
    sorted_desc[sorted_desc.len() / 2]
}

/// The live cohort of a survey: every result whose chain matches `chain` and
/// whose height sits within `tolerance` of the answered set's median height.
/// Height-descending. The median, never the maximum, so one inflated report
/// cannot lift the bar (ADR 0034).
pub fn live_cohort(results: &[SurveyResult], chain: &str, tolerance: u64) -> Vec<LiveCandidate> {
    let mut on_chain: Vec<(&Uri, u64)> = results
        .iter()
        .filter_map(|r| r.reported.as_ref().map(|info| (&r.uri, info)))
        .filter(|(_, info)| info.chain == chain)
        .map(|(uri, info)| (uri, info.height))
        .collect();
    if on_chain.is_empty() {
        return Vec::new();
    }
    on_chain.sort_by_key(|(_, height)| std::cmp::Reverse(*height));
    let heights: Vec<u64> = on_chain.iter().map(|(_, h)| *h).collect();
    let mid = median(&heights);
    on_chain
        .into_iter()
        .filter(|(_, h)| h.abs_diff(mid) <= tolerance)
        .map(|(uri, height)| LiveCandidate {
            uri: uri.clone(),
            height,
        })
        .collect()
}

/// How many candidates answered the survey at all, for the empty-cohort
/// report.
fn answered_count(results: &[SurveyResult]) -> usize {
    results.iter().filter(|r| r.reported.is_some()).count()
}

/// Judge a survey into a [`Selection`], drawing one ticket per operator over
/// the live cohort with `rng`. `pin` is an explicit user server: when set,
/// it is selected if it is in the cohort and the sweep fails [`SweepError::DeadPin`]
/// otherwise, never falling back to the draw (ADR 0034).
pub fn select(
    results: &[SurveyResult],
    chain: &str,
    tolerance: u64,
    pin: Option<&Uri>,
    rng: &mut impl rand::Rng,
) -> Result<Selection, SweepError> {
    let cohort = live_cohort(results, chain, tolerance);

    let sync_indexer = match pin {
        Some(pinned) => {
            if cohort.iter().any(|c| &c.uri == pinned) {
                pinned.clone()
            } else {
                return Err(SweepError::DeadPin(pinned.clone()));
            }
        }
        None => {
            if cohort.is_empty() {
                return Err(SweepError::EmptyCohort {
                    surveyed: results.len(),
                    answered: answered_count(results),
                });
            }
            draw_one_per_operator(&cohort, rng)
        }
    };

    let sync_operator = Operator::of_uri(&sync_indexer);
    let transmit_candidates = cohort
        .iter()
        .filter(|c| Operator::of_uri(&c.uri) != sync_operator)
        .map(|c| c.uri.clone())
        .collect();

    Ok(Selection {
        sync_indexer,
        transmit_candidates,
        cohort,
    })
}

/// Whether `result` is a healthy answer for `chain`: it answered the survey
/// and its reported chain matches.
fn healthy(result: &SurveyResult, chain: &str) -> bool {
    result
        .reported
        .as_ref()
        .is_some_and(|info| info.chain == chain)
}

/// The first-healthy verdict over the results seen so far: the pin the
/// moment it answers on `chain`, any healthy candidate once the pin has
/// refused or no pin exists, and `None` while the decision still awaits the
/// pin or a first healthy answer.
pub fn first_healthy_verdict(
    results: &[SurveyResult],
    chain: &str,
    pin: Option<&Uri>,
) -> Option<Uri> {
    let first_healthy = || {
        results
            .iter()
            .find(|result| healthy(result, chain))
            .map(|result| result.uri.clone())
    };
    match pin {
        Some(pin) => match results.iter().find(|result| &result.uri == pin) {
            Some(of_pin) if healthy(of_pin, chain) => Some(pin.clone()),
            Some(_refused) => first_healthy(),
            None => None,
        },
        None => first_healthy(),
    }
}

/// The [`Selection`] a first-healthy verdict yields: the verdict as the
/// sync indexer, every healthy answer seen so far as the height-descending
/// cohort, and the transmit candidates that exclude the verdict's operator.
pub fn first_healthy_selection(
    results: &[SurveyResult],
    chain: &str,
    sync_indexer: Uri,
) -> Selection {
    let mut cohort: Vec<LiveCandidate> = results
        .iter()
        .filter(|result| healthy(result, chain))
        .map(|result| LiveCandidate {
            uri: result.uri.clone(),
            height: result
                .reported
                .as_ref()
                .expect("healthy answers carry a report")
                .height,
        })
        .collect();
    cohort.sort_by_key(|candidate| std::cmp::Reverse(candidate.height));
    let sync_operator = Operator::of_uri(&sync_indexer);
    let transmit_candidates = cohort
        .iter()
        .filter(|candidate| Operator::of_uri(&candidate.uri) != sync_operator)
        .map(|candidate| candidate.uri.clone())
        .collect();
    Selection {
        sync_indexer,
        transmit_candidates,
        cohort,
    }
}

/// The survey order for `candidates`: uniformly shuffled with `rng` so the
/// lane assignment is random, except that `first`, when present, is swapped
/// into the opening `width` lanes, because the user's own selection must
/// get its answer before any first-healthy verdict can supersede it.
pub fn wave_order(
    candidates: &[Uri],
    first: Option<&Uri>,
    width: usize,
    rng: &mut impl rand::Rng,
) -> Vec<Uri> {
    let mut order: Vec<Uri> = candidates.to_vec();
    order.shuffle(rng);
    if let Some(first) = first
        && let Some(position) = order.iter().position(|candidate| candidate == first)
        && position >= width
    {
        order.swap(0, position);
    }
    order
}

/// Draw one live endpoint by the sync-attach rule: one ticket per operator,
/// a uniform draw among the operators, then any live endpoint of the winner.
/// `cohort` is nonempty here.
fn draw_one_per_operator(cohort: &[LiveCandidate], rng: &mut impl rand::Rng) -> Uri {
    let mut operators: Vec<Option<Operator>> =
        cohort.iter().map(|c| Operator::of_uri(&c.uri)).collect();
    operators.sort();
    operators.dedup();
    let winner = operators
        .choose(rng)
        .expect("a nonempty cohort has at least one operator");
    let endpoints: Vec<&LiveCandidate> = cohort
        .iter()
        .filter(|c| &Operator::of_uri(&c.uri) == winner)
        .collect();
    endpoints
        .choose(rng)
        .expect("the winning operator has at least one endpoint")
        .uri
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng as _;
    use rand::rngs::StdRng;

    fn uri(text: &str) -> Uri {
        text.parse().expect("static uri")
    }

    fn answered(u: &str, chain: &str, height: u64) -> SurveyResult {
        SurveyResult {
            uri: uri(u),
            reported: Some(ProbeSuccess {
                chain: chain.to_string(),
                height,
            }),
        }
    }

    fn silent(u: &str) -> SurveyResult {
        SurveyResult {
            uri: uri(u),
            reported: None,
        }
    }

    /// An inflated outlier cannot lift the bar: the median holds, and the
    /// liar itself falls outside the tolerance and drops from the cohort.
    #[test]
    fn the_median_defends_against_an_inflated_height() {
        let results = vec![
            answered("https://a.example:443", "main", 100),
            answered("https://b.example:443", "main", 101),
            answered("https://c.example:443", "main", 100),
            answered("https://liar.example:443", "main", 100_000),
        ];
        let cohort = live_cohort(&results, "main", 2);
        let hosts: Vec<&str> = cohort.iter().filter_map(|c| c.uri.host()).collect();
        assert_eq!(hosts, vec!["b.example", "a.example", "c.example"]);
        assert!(
            !hosts.contains(&"liar.example"),
            "the inflated report is outside the cohort"
        );
    }

    /// The cohort is height-descending, the failover and report order.
    #[test]
    fn the_cohort_is_height_descending() {
        let results = vec![
            answered("https://a.example:443", "main", 100),
            answered("https://b.example:443", "main", 102),
            answered("https://c.example:443", "main", 101),
        ];
        let cohort = live_cohort(&results, "main", 2);
        assert_eq!(
            cohort.iter().map(|c| c.height).collect::<Vec<_>>(),
            vec![102, 101, 100]
        );
    }

    /// A wrong-chain responder and a silent candidate are both excluded.
    #[test]
    fn wrong_chain_and_silent_candidates_are_excluded() {
        let results = vec![
            answered("https://main.example:443", "main", 100),
            answered("https://test.example:443", "test", 100),
            silent("https://dead.example:443"),
        ];
        let cohort = live_cohort(&results, "main", 2);
        assert_eq!(cohort.len(), 1);
        assert_eq!(cohort[0].uri.host(), Some("main.example"));
    }

    /// A candidate two blocks off the median stays; three blocks off drops.
    #[test]
    fn the_tolerance_is_within_two_blocks_of_the_median() {
        let results = vec![
            answered("https://a.example:443", "main", 100),
            answered("https://b.example:443", "main", 100),
            answered("https://c.example:443", "main", 100),
            answered("https://lag.example:443", "main", 98),
            answered("https://old.example:443", "main", 97),
        ];
        let cohort = live_cohort(&results, "main", 2);
        let hosts: Vec<&str> = cohort.iter().filter_map(|c| c.uri.host()).collect();
        assert!(hosts.contains(&"lag.example"), "two blocks off is live");
        assert!(!hosts.contains(&"old.example"), "three blocks off is not");
    }

    /// The selected sync indexer's whole operator is excluded from the
    /// transmit candidates, not merely the one chosen endpoint.
    #[test]
    fn the_sync_operator_is_excluded_from_transmit_candidates() {
        let results = vec![
            answered("https://na.zec.rocks:443", "main", 100),
            answered("https://eu.zec.rocks:443", "main", 100),
            answered("https://other.example:443", "main", 100),
        ];
        // Seed chosen so the draw lands on the zec.rocks operator; the test
        // asserts the exclusion regardless by checking the operator of the
        // winner against the transmit set.
        let selection = select(&results, "main", 2, None, &mut StdRng::seed_from_u64(1))
            .expect("a live cohort selects");
        let sync_op = Operator::of_uri(&selection.sync_indexer);
        for candidate in &selection.transmit_candidates {
            assert_ne!(
                Operator::of_uri(candidate),
                sync_op,
                "no transmit candidate shares the sync operator"
            );
        }
        // The cohort itself still holds every live endpoint.
        assert_eq!(selection.cohort.len(), 3);
    }

    /// The draw gives one ticket per operator: a two-endpoint operator is no
    /// likelier to win than a one-endpoint operator.
    #[test]
    fn the_draw_is_one_ticket_per_operator() {
        let results = vec![
            answered("https://na.zec.rocks:443", "main", 100),
            answered("https://eu.zec.rocks:443", "main", 100),
            answered("https://sa.zec.rocks:443", "main", 100),
            answered("https://solo.example:443", "main", 100),
        ];
        let mut solo_wins = 0;
        let mut rocks_wins = 0;
        for seed in 0..2_000 {
            let selection = select(&results, "main", 2, None, &mut StdRng::seed_from_u64(seed))
                .expect("selects");
            let winner = Operator::of_uri(&selection.sync_indexer)
                .expect("every selected sync indexer has a host");
            match winner.to_string().as_str() {
                "solo.example" => solo_wins += 1,
                "zec.rocks" => rocks_wins += 1,
                other => panic!("unexpected operator {other}"),
            }
        }
        // Two operators, one ticket each: both near half despite zec.rocks
        // holding three of the four endpoints. A per-endpoint draw would
        // give zec.rocks ~75%.
        assert!(
            (800..1200).contains(&solo_wins),
            "solo won {solo_wins}/2000; the draw is per-operator, not per-endpoint"
        );
        assert!((800..1200).contains(&rocks_wins), "rocks won {rocks_wins}");
    }

    /// A pinned server that is live is selected, bypassing the draw.
    #[test]
    fn a_live_pin_is_selected() {
        let results = vec![
            answered("https://pinned.example:443", "main", 100),
            answered("https://other.example:443", "main", 100),
        ];
        let pin = uri("https://pinned.example:443");
        let selection = select(
            &results,
            "main",
            2,
            Some(&pin),
            &mut StdRng::seed_from_u64(0),
        )
        .expect("a live pin selects");
        assert_eq!(selection.sync_indexer, pin);
    }

    /// A pinned server that is not live fails DeadPin, never falling back to
    /// the draw over the rest of the cohort.
    #[test]
    fn a_dead_pin_fails_without_fallback() {
        let results = vec![
            answered("https://pinned.example:443", "main", 90),
            answered("https://a.example:443", "main", 100),
            answered("https://b.example:443", "main", 100),
        ];
        let pin = uri("https://pinned.example:443");
        let error = select(
            &results,
            "main",
            2,
            Some(&pin),
            &mut StdRng::seed_from_u64(0),
        )
        .expect_err("a lagging pin is dead");
        assert_eq!(error, SweepError::DeadPin(pin));
    }

    /// An empty cohort without a pin reports how many were surveyed and how
    /// many answered.
    #[test]
    fn an_empty_cohort_reports_its_counts() {
        let results = vec![
            silent("https://a.example:443"),
            silent("https://b.example:443"),
        ];
        let error =
            select(&results, "main", 2, None, &mut StdRng::seed_from_u64(0)).expect_err("empty");
        assert_eq!(
            error,
            SweepError::EmptyCohort {
                surveyed: 2,
                answered: 0,
            }
        );
    }

    /// HYPOTHESIS: without a pin, the first healthy answer is the verdict
    /// the moment it exists, and a wrong-chain answer is not healthy.
    /// Falsified if silence or a wrong-chain answer forms the verdict.
    #[test]
    fn the_first_healthy_answer_is_the_unpinned_verdict() {
        let mut seen = vec![
            silent("https://a.example:443"),
            answered("https://test.example:443", "test", 100),
        ];
        assert_eq!(first_healthy_verdict(&seen, "main", None), None);
        seen.push(answered("https://b.example:443", "main", 100));
        assert_eq!(
            first_healthy_verdict(&seen, "main", None),
            Some(uri("https://b.example:443"))
        );
    }

    /// HYPOTHESIS: a pinned decision waits for the pin's own outcome — an
    /// earlier healthy answer does not preempt it — and chooses the pin the
    /// moment it answers. Falsified if another candidate wins first.
    #[test]
    fn the_pin_preempts_earlier_answers_when_it_answers() {
        let pin = uri("https://pin.example:443");
        let mut seen = vec![answered("https://fast.example:443", "main", 100)];
        assert_eq!(first_healthy_verdict(&seen, "main", Some(&pin)), None);
        seen.push(answered("https://pin.example:443", "main", 100));
        assert_eq!(first_healthy_verdict(&seen, "main", Some(&pin)), Some(pin));
    }

    /// HYPOTHESIS: once the pin has refused, the verdict is the first
    /// healthy answer already seen. Falsified if the refusal blocks it.
    #[test]
    fn a_refused_pin_yields_to_the_first_healthy_answer() {
        let pin = uri("https://pin.example:443");
        let seen = vec![
            answered("https://fast.example:443", "main", 100),
            silent("https://pin.example:443"),
        ];
        assert_eq!(
            first_healthy_verdict(&seen, "main", Some(&pin)),
            Some(uri("https://fast.example:443"))
        );
    }

    /// HYPOTHESIS: the first-healthy selection carries every healthy answer
    /// as its height-descending cohort and excludes the verdict's operator
    /// from the transmit candidates. Falsified if either property drifts.
    #[test]
    fn the_first_healthy_selection_reports_its_evidence() {
        let results = vec![
            answered("https://na.zec.rocks:443", "main", 101),
            answered("https://other.example:443", "main", 102),
            silent("https://dead.example:443"),
        ];
        let selection = first_healthy_selection(&results, "main", uri("https://na.zec.rocks:443"));
        assert_eq!(
            selection
                .cohort
                .iter()
                .map(|candidate| candidate.height)
                .collect::<Vec<_>>(),
            vec![102, 101]
        );
        assert_eq!(
            selection.transmit_candidates,
            vec![uri("https://other.example:443")]
        );
    }

    /// HYPOTHESIS: the wave order always seats `first` in the opening
    /// `width` lanes, whatever the shuffle drew, because the pin must get
    /// its answer before any first-healthy verdict can supersede it.
    /// Falsified if any seed leaves it outside the opening lanes.
    #[test]
    fn the_first_candidate_always_rides_the_opening_lanes() {
        let candidates: Vec<Uri> = (0..17)
            .map(|n| uri(&format!("https://c{n}.example:443")))
            .collect();
        let pin = uri("https://c13.example:443");
        const WIDTH: usize = 4;
        for seed in 0..200 {
            let order = wave_order(
                &candidates,
                Some(&pin),
                WIDTH,
                &mut StdRng::seed_from_u64(seed),
            );
            assert_eq!(order.len(), candidates.len(), "a permutation, whole");
            assert!(
                order[..WIDTH].contains(&pin),
                "seed {seed} left the pin outside the opening lanes: {order:?}"
            );
        }
    }
}
