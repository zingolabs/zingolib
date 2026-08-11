//! The Exit Pool: the session's sole issuer of Exit Node Reservations.
#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};

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

/// One issued Exit Node Reservation; dropping it recycles the node.
pub(crate) struct Reservation {
    node: crate::nym::ExitNodeId,
    ledger: Weak<Mutex<ExitPool>>,
}

impl Reservation {
    /// The reserved Exit Node identity.
    pub(crate) fn node(&self) -> &crate::nym::ExitNodeId {
        &self.node
    }

    /// A ledgerless reservation for pool unit tests.
    #[cfg(test)]
    pub(crate) fn dangling_for_test(node: &str) -> Self {
        Reservation {
            node: crate::nym::ExitNodeId::from(node),
            ledger: Weak::new(),
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(ledger) = self.ledger.upgrade() {
            ledger
                .lock()
                .expect("exit pool mutex")
                .issued
                .remove(&self.node);
        }
    }
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reservation")
            .field("node", &self.node)
            .finish_non_exhaustive()
    }
}

/// The node identities of a clutch, for the process seam's `--exit` args.
pub(crate) fn clutch_nodes(clutch: &[Reservation]) -> Vec<crate::nym::ExitNodeId> {
    clutch
        .iter()
        .map(|reservation| reservation.node().clone())
        .collect()
}

/// The session's Exit Pool: one reservation per discovered node, issued to
/// at most one holder at a time.
#[derive(Default)]
pub(crate) struct ExitPool {
    population: Vec<crate::nym::ExitNodeId>,
    issued: HashSet<crate::nym::ExitNodeId>,
}

impl ExitPool {
    /// Records the discovered population, once per session.
    pub(crate) fn seed(&mut self, discovered: Vec<crate::nym::ExitNodeId>) {
        self.population = discovered;
    }

    /// Whether the population is known yet.
    pub(crate) fn is_seeded(&self) -> bool {
        !self.population.is_empty()
    }

    /// Draws a Clutch of owning reservations, each of which recycles
    /// itself into `pool` when dropped.
    pub(crate) fn draw_clutch(
        pool: &Arc<Mutex<ExitPool>>,
    ) -> Result<Vec<Reservation>, ExitPoolError> {
        let mut guarded = pool.lock().expect("exit pool mutex");
        if guarded.population.is_empty() {
            return Err(ExitPoolError::NotSeeded);
        }
        let drawable: Vec<crate::nym::ExitNodeId> = guarded
            .population
            .iter()
            .filter(|node| !guarded.issued.contains(*node))
            .cloned()
            .collect();
        if drawable.is_empty() {
            return Err(ExitPoolError::Exhausted {
                held: guarded.issued.len(),
                population: guarded.population.len(),
            });
        }
        let clutch: Vec<Reservation> = drawable
            .choose_multiple(
                &mut rand::rngs::OsRng,
                zingo_netutils::responsiveness::RESERVATION_CLUTCH_SIZE,
            )
            .map(|node| {
                guarded.issued.insert(node.clone());
                Reservation {
                    node: node.clone(),
                    ledger: Arc::downgrade(pool),
                }
            })
            .collect();
        Ok(clutch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zingo_netutils::responsiveness::RESERVATION_CLUTCH_SIZE;

    fn seeded(count: usize) -> Arc<Mutex<ExitPool>> {
        let pool = Arc::new(Mutex::new(ExitPool::default()));
        pool.lock().unwrap().seed(
            (0..count)
                .map(|index| crate::nym::ExitNodeId::from(format!("exit-{index}")))
                .collect(),
        );
        pool
    }

    /// HYPOTHESIS: a drawn clutch is clutch-sized, and every reservation in
    /// it is transferred, so a second draw can never repeat one.
    #[test]
    fn a_reservation_is_never_issued_twice() {
        let pool = seeded(RESERVATION_CLUTCH_SIZE * 2);
        let first = ExitPool::draw_clutch(&pool).expect("the first clutch");
        let second = ExitPool::draw_clutch(&pool).expect("the second clutch");
        assert_eq!(first.len(), RESERVATION_CLUTCH_SIZE);
        assert_eq!(second.len(), RESERVATION_CLUTCH_SIZE);
        for reservation in &first {
            assert!(
                !second
                    .iter()
                    .any(|other| other.node() == reservation.node()),
                "{} was issued to two holders at once",
                reservation.node()
            );
        }
    }

    /// HYPOTHESIS: dropping reservations recycles them, so an exhausted
    /// pool issues again only after its holders release.
    #[test]
    fn dropping_replenishes_an_exhausted_pool() {
        let pool = seeded(RESERVATION_CLUTCH_SIZE);
        let clutch = ExitPool::draw_clutch(&pool).expect("the only clutch");
        let exhausted = ExitPool::draw_clutch(&pool).expect_err("nothing is left to issue");
        assert!(matches!(exhausted, ExitPoolError::Exhausted { .. }));
        drop(clutch);
        ExitPool::draw_clutch(&pool).expect("dropped reservations reissue");
    }

    /// HYPOTHESIS: cancelling a future that owns a reservation recycles it,
    /// the leak shape of a hedged race's losing pull.
    #[test]
    fn a_cancelled_owner_recycles_by_drop() {
        let pool = seeded(RESERVATION_CLUTCH_SIZE);
        let clutch = ExitPool::draw_clutch(&pool).expect("the only clutch");
        let owner = async move {
            let _held = clutch;
            std::future::pending::<()>().await;
        };
        drop(owner);
        ExitPool::draw_clutch(&pool).expect("the cancelled owner's reservations reissue");
    }

    /// HYPOTHESIS: a reservation outliving its pool recycles into nothing
    /// rather than panicking.
    #[test]
    fn an_orphaned_reservation_drops_quietly() {
        let pool = seeded(RESERVATION_CLUTCH_SIZE);
        let clutch = ExitPool::draw_clutch(&pool).expect("the only clutch");
        drop(pool);
        drop(clutch);
    }

    /// HYPOTHESIS: an unseeded pool refuses rather than drawing nothing
    /// silently.
    #[test]
    fn an_unseeded_pool_refuses() {
        let pool = Arc::new(Mutex::new(ExitPool::default()));
        assert!(matches!(
            ExitPool::draw_clutch(&pool).expect_err("no population"),
            ExitPoolError::NotSeeded
        ));
        assert!(!pool.lock().unwrap().is_seeded());
    }
}
