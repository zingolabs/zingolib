//! Health: the wallet's per-Destination judgment, from real traffic alone.
#![forbid(unsafe_code)]

use std::collections::HashMap;

/// How many failures without a success mark a Destination unhealthy.
#[cfg_attr(not(feature = "nym"), allow(dead_code))]
pub(crate) const UNHEALTHY_FAILURE_THRESHOLD: u32 = 2;

/// The fewest Destinations a Health filter may leave eligible, so a
/// partition can never shrink the draw's anonymity set.
#[cfg_attr(not(feature = "nym"), allow(dead_code))]
const MINIMUM_ELIGIBLE_DESTINATIONS: usize = 4;

/// Which party a failed attempt is charged against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailurePhase {
    /// The tunnel failed before reaching the destination: the exit's.
    Tunnel,
    /// The destination answered badly or not at all: the Destination's.
    Destination,
    /// The evidence cannot say which party failed.
    Unattributed,
}

/// One Destination's standing this session.
#[derive(Clone, Copy, Debug, Default)]
struct Standing {
    successes: u32,
    failures: u32,
}

/// The session's per-Destination Health, updated by real traffic only.
#[derive(Debug, Default)]
pub struct Health {
    standings: HashMap<super::Host, Standing>,
}

impl Health {
    /// Charges one attempt's outcome against the host it contacted, counting
    /// a failure only when the evidence names this Destination.
    pub(crate) fn note(&mut self, host: &super::Host, failed: bool, phase: Option<FailurePhase>) {
        let standing = self.standings.entry(host.clone()).or_default();
        if !failed {
            standing.successes += 1;
        } else if phase == Some(FailurePhase::Destination) {
            standing.failures += 1;
        }
    }

    /// Whether this session's evidence leaves the host worth contacting.
    #[cfg_attr(not(feature = "nym"), allow(dead_code))]
    pub(crate) fn is_healthy(&self, host: &super::Host) -> bool {
        match self.standings.get(host) {
            None => true,
            Some(standing) => {
                standing.successes > 0 || standing.failures < UNHEALTHY_FAILURE_THRESHOLD
            }
        }
    }

    /// Filters `candidates` to the healthy ones, keeping the whole set when
    /// filtering would leave fewer than the floor.
    #[cfg_attr(not(feature = "nym"), allow(dead_code))]
    pub(crate) fn filter_with_floor(&self, candidates: Vec<http::Uri>) -> Vec<http::Uri> {
        let healthy: Vec<http::Uri> = candidates
            .iter()
            .filter(|uri| {
                uri.host()
                    .is_none_or(|host| self.is_healthy(&super::Host::of_host_str(host)))
            })
            .cloned()
            .collect();
        if healthy.len() < MINIMUM_ELIGIBLE_DESTINATIONS {
            candidates
        } else {
            healthy
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uris(hosts: &[&str]) -> Vec<http::Uri> {
        hosts
            .iter()
            .map(|host| format!("https://{host}:443").parse().unwrap())
            .collect()
    }

    fn host(name: &str) -> super::super::Host {
        super::super::Host::of_host_str(name)
    }

    /// HYPOTHESIS: only a Destination-phase failure counts against a
    /// Destination; a tunnel failure is the exit's and never demotes it.
    #[test]
    fn a_tunnel_failure_never_charges_the_destination() {
        let mut health = Health::default();
        for _ in 0..UNHEALTHY_FAILURE_THRESHOLD {
            health.note(&host("tunnelled.example"), true, Some(FailurePhase::Tunnel));
            health.note(&host("unknown.example"), true, None);
            health.note(
                &host("refusing.example"),
                true,
                Some(FailurePhase::Destination),
            );
        }
        assert!(health.is_healthy(&host("tunnelled.example")));
        assert!(health.is_healthy(&host("unknown.example")));
        assert!(!health.is_healthy(&host("refusing.example")));
    }

    /// HYPOTHESIS: one success redeems a Destination, so a transient
    /// outage never strands it for the session.
    #[test]
    fn a_success_redeems_a_failing_destination() {
        let mut health = Health::default();
        for _ in 0..UNHEALTHY_FAILURE_THRESHOLD {
            health.note(
                &host("flaky.example"),
                true,
                Some(FailurePhase::Destination),
            );
        }
        assert!(!health.is_healthy(&host("flaky.example")));
        health.note(&host("flaky.example"), false, None);
        assert!(health.is_healthy(&host("flaky.example")));
    }

    /// HYPOTHESIS: the filter removes the unhealthy while the floor holds,
    /// and returns the whole set rather than shrinking past it.
    #[test]
    fn the_floor_outranks_the_filter() {
        let mut health = Health::default();
        let roomy = uris(&["a", "b", "c", "d", "e", "f"]);
        for _ in 0..UNHEALTHY_FAILURE_THRESHOLD {
            health.note(&host("a"), true, Some(FailurePhase::Destination));
        }
        let filtered = health.filter_with_floor(roomy.clone());
        assert_eq!(filtered.len(), roomy.len() - 1, "the unhealthy one goes");

        let scarce = uris(&["a", "b", "c", "d"]);
        assert_eq!(
            health.filter_with_floor(scarce.clone()),
            scarce,
            "the floor keeps the draw's anonymity set whole"
        );
    }
}
