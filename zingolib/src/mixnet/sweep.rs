//! The pure core of the Server-Selection Sweep (ADR 0034).
//!
//! Selection has two halves: a survey that emits network traffic, and the
//! judgment that turns its results into a sync indexer. This module is the
//! judgment, kept pure so every rule the sweep rests on is pinned without a
//! transport. It takes the survey's per-candidate outcomes and yields the
//! live cohort, the drawn sync indexer, and the transmit candidates the
//! selection excludes. A user's pinned clearnet sync indexer never enters:
//! the caller holds it out of the sweep.
#![forbid(unsafe_code)]

use http::Uri;
use rand::seq::SliceRandom as _;

/// One candidate's survey outcome: the endpoint and the tip height its
/// `GetLatestBlock` reported, or `None` when it did not answer over the
/// mixnet.
#[derive(Clone, Debug)]
pub struct SurveyResult {
    /// The surveyed endpoint.
    pub uri: Uri,
    /// The tip height the endpoint reported, or `None` on any failure.
    pub reported: Option<u64>,
}

/// A candidate that answered the survey and passed the cohort's liveness
/// test: its tip height sat within the median tolerance.
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
    /// No candidate passed the liveness test.
    #[error("no live indexer: {answered} of {surveyed} answered, none within the cohort")]
    EmptyCohort {
        /// How many candidates were surveyed.
        surveyed: usize,
        /// How many answered at all (before the cohort test).
        answered: usize,
    },
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

/// The live cohort of a survey: every result whose tip height sits within
/// `tolerance` of the answered set's median height. Height-descending. The
/// median, never the maximum, so one inflated report cannot lift the bar
/// (ADR 0034).
pub fn live_cohort(results: &[SurveyResult], tolerance: u64) -> Vec<LiveCandidate> {
    let mut answered: Vec<(&Uri, u64)> = results
        .iter()
        .filter_map(|r| r.reported.map(|height| (&r.uri, height)))
        .collect();
    if answered.is_empty() {
        return Vec::new();
    }
    answered.sort_by_key(|(_, height)| std::cmp::Reverse(*height));
    let heights: Vec<u64> = answered.iter().map(|(_, h)| *h).collect();
    let mid = median(&heights);
    answered
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
/// the live cohort with `rng`.
pub fn select(
    results: &[SurveyResult],
    tolerance: u64,
    rng: &mut impl rand::Rng,
) -> Result<Selection, SweepError> {
    let cohort = live_cohort(results, tolerance);
    if cohort.is_empty() {
        return Err(SweepError::EmptyCohort {
            surveyed: results.len(),
            answered: answered_count(results),
        });
    }
    let sync_indexer = draw_one_per_operator(&cohort, rng);

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

    fn answered(u: &str, height: u64) -> SurveyResult {
        SurveyResult {
            uri: uri(u),
            reported: Some(height),
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
            answered("https://a.example:443", 100),
            answered("https://b.example:443", 101),
            answered("https://c.example:443", 100),
            answered("https://liar.example:443", 100_000),
        ];
        let cohort = live_cohort(&results, 2);
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
            answered("https://a.example:443", 100),
            answered("https://b.example:443", 102),
            answered("https://c.example:443", 101),
        ];
        let cohort = live_cohort(&results, 2);
        assert_eq!(
            cohort.iter().map(|c| c.height).collect::<Vec<_>>(),
            vec![102, 101, 100]
        );
    }

    /// A silent candidate is excluded from the cohort.
    #[test]
    fn silent_candidates_are_excluded() {
        let results = vec![
            answered("https://main.example:443", 100),
            silent("https://dead.example:443"),
        ];
        let cohort = live_cohort(&results, 2);
        assert_eq!(cohort.len(), 1);
        assert_eq!(cohort[0].uri.host(), Some("main.example"));
    }

    /// A candidate two blocks off the median stays; three blocks off drops.
    #[test]
    fn the_tolerance_is_within_two_blocks_of_the_median() {
        let results = vec![
            answered("https://a.example:443", 100),
            answered("https://b.example:443", 100),
            answered("https://c.example:443", 100),
            answered("https://lag.example:443", 98),
            answered("https://old.example:443", 97),
        ];
        let cohort = live_cohort(&results, 2);
        let hosts: Vec<&str> = cohort.iter().filter_map(|c| c.uri.host()).collect();
        assert!(hosts.contains(&"lag.example"), "two blocks off is live");
        assert!(!hosts.contains(&"old.example"), "three blocks off is not");
    }

    /// The selected sync indexer's whole operator is excluded from the
    /// transmit candidates, not merely the one chosen endpoint.
    #[test]
    fn the_sync_operator_is_excluded_from_transmit_candidates() {
        let results = vec![
            answered("https://na.zec.rocks:443", 100),
            answered("https://eu.zec.rocks:443", 100),
            answered("https://other.example:443", 100),
        ];
        // Seed chosen so the draw lands on the zec.rocks operator; the test
        // asserts the exclusion regardless by checking the operator of the
        // winner against the transmit set.
        let selection =
            select(&results, 2, &mut StdRng::seed_from_u64(1)).expect("a live cohort selects");
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
            answered("https://na.zec.rocks:443", 100),
            answered("https://eu.zec.rocks:443", 100),
            answered("https://sa.zec.rocks:443", 100),
            answered("https://solo.example:443", 100),
        ];
        let mut solo_wins = 0;
        let mut rocks_wins = 0;
        for seed in 0..2_000 {
            let selection = select(&results, 2, &mut StdRng::seed_from_u64(seed)).expect("selects");
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

    /// An empty cohort reports how many were surveyed and how many answered.
    #[test]
    fn an_empty_cohort_reports_its_counts() {
        let results = vec![
            silent("https://a.example:443"),
            silent("https://b.example:443"),
        ];
        let error = select(&results, 2, &mut StdRng::seed_from_u64(0)).expect_err("empty");
        assert_eq!(
            error,
            SweepError::EmptyCohort {
                surveyed: 2,
                answered: 0,
            }
        );
    }
}
