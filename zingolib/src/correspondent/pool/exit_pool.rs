//! The Exit Pool: the session's sole issuer of Exit Node Reservations.
#![forbid(unsafe_code)]

use std::collections::HashSet;

use rand::seq::SliceRandom as _;

/// Why the pool could issue no clutch.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ExitPoolError {
    /// The session has not yet learned the exit population.
    #[error("the exit pool has no population yet")]
    NotSeeded,
    /// Every reservation is held, so nothing can be drawn.
    #[error("every exit is held: {held} held, {population} known")]
    Exhausted {
        /// Reservations currently issued to some holder.
        held: usize,
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
            .filter(|node| !self.issued.contains(*node))
            .cloned()
            .collect();
        if drawable.is_empty() {
            return Err(ExitPoolError::Exhausted {
                held: self.issued.len(),
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
