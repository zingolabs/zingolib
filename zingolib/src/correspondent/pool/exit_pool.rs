//! The Exit Pool: the session's sole issuer of Exit Node Reservations.
#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};

use rand::seq::SliceRandom as _;

/// How many standard deviations above the pool's mean failure count a node
/// must stand before Session Retirement withholds it.
const RETIREMENT_SIGMA_MULTIPLE: f64 = 1.0;

/// The fewest nodes whose failure evidence can support a retirement
/// judgment; below it a single unlucky node would retire itself.
const RETIREMENT_MINIMUM_POPULATION: usize = 3;

/// Why the pool could issue no clutch.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ExitPoolError {
    /// The session has not yet learned the exit population.
    #[error("the exit pool has no population yet")]
    NotSeeded,
    /// Every reservation is held or retired, so nothing can be drawn.
    #[error("every exit is held or retired: {held} held, {retired} retired, {population} known")]
    Exhausted {
        /// Reservations currently issued to some holder.
        held: usize,
        /// Nodes Session Retirement has withheld.
        retired: usize,
        /// The whole discovered population.
        population: usize,
    },
}

/// The session's Exit Pool: one reservation per discovered node, issued to
/// at most one holder at a time.
#[derive(Default)]
pub(crate) struct ExitPool {
    population: Vec<String>,
    issued: HashSet<String>,
    failures: HashMap<String, u32>,
    retired: HashSet<String>,
}

impl ExitPool {
    /// Records the discovered population, once per session.
    pub(crate) fn seed(&mut self, discovered: Vec<String>) {
        self.population = discovered;
    }

    /// Whether the population is known yet.
    pub(crate) fn is_seeded(&self) -> bool {
        !self.population.is_empty()
    }

    /// Draws a Clutch, transferring those reservations to the caller.
    pub(crate) fn draw_clutch(&mut self) -> Result<Vec<String>, ExitPoolError> {
        if self.population.is_empty() {
            return Err(ExitPoolError::NotSeeded);
        }
        let drawable: Vec<String> = self
            .population
            .iter()
            .filter(|node| !self.issued.contains(*node) && !self.retired.contains(*node))
            .cloned()
            .collect();
        if drawable.is_empty() {
            return Err(ExitPoolError::Exhausted {
                held: self.issued.len(),
                retired: self.retired.len(),
                population: self.population.len(),
            });
        }
        let clutch: Vec<String> = drawable
            .choose_multiple(
                &mut rand::rngs::OsRng,
                zingo_netutils::responsiveness::RESERVATION_CLUTCH_SIZE,
            )
            .cloned()
            .collect();
        for node in &clutch {
            self.issued.insert(node.clone());
        }
        Ok(clutch)
    }

    /// Returns reservations to the pool (Exit Recycling).
    pub(crate) fn recycle<I: IntoIterator<Item = String>>(&mut self, reservations: I) {
        for node in reservations {
            self.issued.remove(&node);
        }
    }

    /// Charges one connection failure against a node, then re-judges
    /// Session Retirement over the pool's evidence.
    pub(crate) fn note_failure(&mut self, node: &str) {
        *self.failures.entry(node.to_string()).or_default() += 1;
        self.retire_outliers();
    }

    /// Withholds every node whose failure count stands more than
    /// [`RETIREMENT_SIGMA_MULTIPLE`] deviations above the pool's mean.
    fn retire_outliers(&mut self) {
        if self.population.len() < RETIREMENT_MINIMUM_POPULATION {
            return;
        }
        let counts: Vec<f64> = self
            .population
            .iter()
            .map(|node| f64::from(self.failures.get(node).copied().unwrap_or_default()))
            .collect();
        let population = counts.len() as f64;
        let mean = counts.iter().sum::<f64>() / population;
        let variance = counts.iter().map(|c| (c - mean).powi(2)).sum::<f64>() / population;
        let threshold = mean + RETIREMENT_SIGMA_MULTIPLE * variance.sqrt();
        for (node, count) in self.population.iter().zip(counts) {
            if count > 0.0 && count > threshold {
                self.retired.insert(node.clone());
            }
        }
    }

    /// The nodes Session Retirement has withheld this session.
    #[cfg(test)]
    pub(crate) fn retired(&self) -> &HashSet<String> {
        &self.retired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zingo_netutils::responsiveness::RESERVATION_CLUTCH_SIZE;

    fn population(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("exit-{index}")).collect()
    }

    /// HYPOTHESIS: a drawn clutch is clutch-sized, and every reservation in
    /// it is transferred, so a second draw can never repeat one.
    #[test]
    fn a_reservation_is_never_issued_twice() {
        let mut pool = ExitPool::default();
        pool.seed(population(RESERVATION_CLUTCH_SIZE * 2));
        let first = pool.draw_clutch().expect("the first clutch");
        let second = pool.draw_clutch().expect("the second clutch");
        assert_eq!(first.len(), RESERVATION_CLUTCH_SIZE);
        assert_eq!(second.len(), RESERVATION_CLUTCH_SIZE);
        for node in &first {
            assert!(
                !second.contains(node),
                "{node} was issued to two holders at once"
            );
        }
    }

    /// HYPOTHESIS: recycling returns reservations, so an exhausted pool
    /// issues again only after its holders release.
    #[test]
    fn recycling_replenishes_an_exhausted_pool() {
        let mut pool = ExitPool::default();
        pool.seed(population(RESERVATION_CLUTCH_SIZE));
        let clutch = pool.draw_clutch().expect("the only clutch");
        let exhausted = pool.draw_clutch().expect_err("nothing is left to issue");
        assert!(matches!(exhausted, ExitPoolError::Exhausted { .. }));
        pool.recycle(clutch);
        pool.draw_clutch().expect("recycled reservations reissue");
    }

    /// HYPOTHESIS: a node failing far more than its peers is withheld from
    /// every later draw, while its unremarkable peers are not.
    #[test]
    fn session_retirement_withholds_the_outlier_alone() {
        let mut pool = ExitPool::default();
        let nodes = population(RESERVATION_CLUTCH_SIZE * 2);
        pool.seed(nodes.clone());
        for _ in 0..RESERVATION_CLUTCH_SIZE {
            pool.note_failure(&nodes[0]);
        }
        assert!(pool.retired().contains(&nodes[0]), "the outlier retires");
        assert_eq!(pool.retired().len(), 1, "its peers keep their standing");
    }

    /// HYPOTHESIS: an unseeded pool refuses rather than drawing nothing
    /// silently.
    #[test]
    fn an_unseeded_pool_refuses() {
        let mut pool = ExitPool::default();
        assert!(matches!(
            pool.draw_clutch().expect_err("no population"),
            ExitPoolError::NotSeeded
        ));
        assert!(!pool.is_seeded());
    }
}
