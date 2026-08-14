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
    node: crate::mixnet::ExitNodeId,
    ledger: Weak<Mutex<ExitPool>>,
}

impl Reservation {
    /// The reserved Exit Node identity.
    pub(crate) fn node(&self) -> &crate::mixnet::ExitNodeId {
        &self.node
    }

    /// A ledgerless reservation for pool unit tests.
    #[cfg(test)]
    pub(crate) fn dangling_for_test(node: &str) -> Self {
        Reservation {
            node: crate::mixnet::ExitNodeId::from(node),
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

impl PartialEq for Reservation {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node
    }
}

impl Eq for Reservation {}

impl std::hash::Hash for Reservation {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.node.hash(state);
    }
}

impl std::borrow::Borrow<crate::mixnet::ExitNodeId> for Reservation {
    fn borrow(&self) -> &crate::mixnet::ExitNodeId {
        &self.node
    }
}

/// The node identities of a clutch, for the process seam's `--exit` args.
pub(crate) fn clutch_nodes(clutch: &HashSet<Reservation>) -> Vec<crate::mixnet::ExitNodeId> {
    clutch
        .iter()
        .map(|reservation| reservation.node().clone())
        .collect()
}

/// Takes the first reservation a ready transport reports as bound out of
/// `clutch`, recycling the rest, or `None` when the report names no drawn
/// node.
pub(crate) fn take_bound_lease(
    clutch: &mut HashSet<Reservation>,
    reported: &[crate::mixnet::ExitNodeId],
) -> Option<Reservation> {
    // The report is walked in announcement order, not the set's arbitrary
    // one, so a transport that names several drawn exits binds the one it
    // announced first and the same report always binds the same exit.
    reported.iter().find_map(|node| clutch.take(node))
}

/// What one completed or refused round trip showed about an Exit Node,
/// within the epoch that observation survives.
// TODO: implement sensitivity to, and policy around, Nym epochs: the
// network's epoch boundaries are queryable, and the sliding one-hour
// window from the observation instant is a stand-in for the real
// rotation edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExitNodeHealthVerdict {
    /// A round trip completed through the exit within the current epoch:
    /// it answered the Sentinel or carried a task.
    EpochProven,
    /// The exit refused, timed out, or stayed silent past budget, within
    /// the current epoch.
    Failed,
}

/// One exit's most recent verdict with the instant it was earned.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Observation {
    verdict: ExitNodeHealthVerdict,
    at: std::time::Instant,
}

impl Observation {
    /// A verdict earned now.
    pub(crate) fn earned(verdict: ExitNodeHealthVerdict, at: std::time::Instant) -> Self {
        Observation { verdict, at }
    }
}

/// The epoch-scoped health record of the exit census, keyed by node.
#[derive(Default)]
pub(crate) struct NodeHealthIndex(
    std::collections::HashMap<crate::mixnet::ExitNodeId, Observation>,
);

impl NodeHealthIndex {
    /// Keeps `observation` as the node's current verdict, superseding any
    /// earlier one.
    pub(crate) fn remember(&mut self, exit: crate::mixnet::ExitNodeId, observation: Observation) {
        self.0.insert(exit, observation);
    }

    /// Whether the node's proof completed within the last Nym epoch.
    pub(crate) fn epoch_proven(
        &self,
        exit: &crate::mixnet::ExitNodeId,
        now: std::time::Instant,
    ) -> bool {
        self.observed(exit, now, ExitNodeHealthVerdict::EpochProven)
    }

    /// Whether the node failed within the last Nym epoch, so a convicted
    /// node stands trial again once the topology that convicted it has
    /// rotated away.
    pub(crate) fn epoch_failed(
        &self,
        exit: &crate::mixnet::ExitNodeId,
        now: std::time::Instant,
    ) -> bool {
        self.observed(exit, now, ExitNodeHealthVerdict::Failed)
    }

    /// The instant the node's EpochProven observation stops being fresh,
    /// when one stands.
    pub(crate) fn proven_until(
        &self,
        exit: &crate::mixnet::ExitNodeId,
    ) -> Option<std::time::Instant> {
        self.0.get(exit).and_then(|seen| {
            (seen.verdict == ExitNodeHealthVerdict::EpochProven)
                .then(|| seen.at + zingo_netutils::time::NYM_EPOCH)
        })
    }

    /// Whether the node's current observation is `verdict`, still fresh.
    fn observed(
        &self,
        exit: &crate::mixnet::ExitNodeId,
        now: std::time::Instant,
        verdict: ExitNodeHealthVerdict,
    ) -> bool {
        self.0.get(exit).is_some_and(|seen| {
            seen.verdict == verdict
                && now.saturating_duration_since(seen.at) < zingo_netutils::time::NYM_EPOCH
        })
    }
}

/// The session's Exit Pool: one reservation per discovered node, issued to
/// at most one holder at a time.
#[derive(Default)]
pub(crate) struct ExitPool {
    population: HashSet<crate::mixnet::ExitNodeId>,
    issued: HashSet<crate::mixnet::ExitNodeId>,
    health: NodeHealthIndex,
}

impl ExitPool {
    /// Records the discovered population, once per session.
    pub(crate) fn seed(&mut self, discovered: impl IntoIterator<Item = crate::mixnet::ExitNodeId>) {
        self.population = discovered.into_iter().collect();
    }

    /// Whether the population is known yet.
    pub(crate) fn is_seeded(&self) -> bool {
        !self.population.is_empty()
    }

    /// Keeps `observation` as `exit`'s current verdict.
    pub(crate) fn remember(&mut self, exit: crate::mixnet::ExitNodeId, observation: Observation) {
        self.health.remember(exit, observation);
    }

    /// Whether `exit`'s proof completed within the last Nym epoch.
    pub(crate) fn epoch_proven(
        &self,
        exit: &crate::mixnet::ExitNodeId,
        now: std::time::Instant,
    ) -> bool {
        self.health.epoch_proven(exit, now)
    }

    /// The instant `exit`'s EpochProven observation stops being fresh,
    /// when one stands.
    pub(crate) fn proven_until(
        &self,
        exit: &crate::mixnet::ExitNodeId,
    ) -> Option<std::time::Instant> {
        self.health.proven_until(exit)
    }

    /// Whether `exit` failed within the last Nym epoch.
    // Exercised by the ProofAcquisition contract tests; production reads
    // failures only through the draw's own partition.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn epoch_failed(
        &self,
        exit: &crate::mixnet::ExitNodeId,
        now: std::time::Instant,
    ) -> bool {
        self.health.epoch_failed(exit, now)
    }

    /// Draws a Clutch of owning reservations, each of which recycles
    /// itself into `pool` when dropped, sampling fresh-Proven exits first,
    /// unknown ones next, and Failed ones only at exhaustion.
    pub(crate) fn draw_clutch(
        pool: &Arc<Mutex<ExitPool>>,
    ) -> Result<HashSet<Reservation>, ExitPoolError> {
        let now = std::time::Instant::now();
        let mut guarded = pool.lock().expect("exit pool mutex");
        if guarded.population.is_empty() {
            return Err(ExitPoolError::NotSeeded);
        }
        // The candidates are collected before any issue, so the ledger is
        // written while the population is no longer borrowed.
        let drawable: Vec<crate::mixnet::ExitNodeId> = guarded
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
        let mut proven = Vec::new();
        let mut unknown = Vec::new();
        let mut failed = Vec::new();
        for node in drawable {
            if guarded.health.epoch_proven(&node, now) {
                proven.push(node);
            } else if guarded.health.epoch_failed(&node, now) {
                failed.push(node);
            } else {
                unknown.push(node);
            }
        }
        // Each preference tier is shuffled for real within itself, so the
        // index orders tiers while chance still spreads load inside one:
        // sampling a tier at its own size returns iteration order verbatim,
        // which is why the shuffle is explicit.
        let mut ordered: Vec<crate::mixnet::ExitNodeId> = Vec::new();
        for mut tier in [proven, unknown, failed] {
            tier.shuffle(&mut rand::rngs::OsRng);
            ordered.extend(tier);
        }
        let clutch: HashSet<Reservation> = ordered
            .into_iter()
            .take(zingo_netutils::arm_race::RESERVATION_CLUTCH_SIZE)
            .map(|node| {
                guarded.issued.insert(node.clone());
                Reservation {
                    node,
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
    use zingo_netutils::arm_race::RESERVATION_CLUTCH_SIZE;

    fn seeded(count: usize) -> Arc<Mutex<ExitPool>> {
        let pool = Arc::new(Mutex::new(ExitPool::default()));
        pool.lock().unwrap().seed(
            (0..count)
                .map(|index| crate::mixnet::ExitNodeId::from(format!("exit-{index}").as_str())),
        );
        pool
    }

    /// HYPOTHESIS: the bind-time take yields exactly the reported
    /// reservation and leaves the rest for recycling. Falsified if it takes
    /// the wrong reservation or disturbs the remainder.
    #[test]
    fn the_bound_lease_is_taken_and_the_rest_remain() {
        let mut clutch: HashSet<Reservation> = [
            Reservation::dangling_for_test("exit-a"),
            Reservation::dangling_for_test("exit-b"),
            Reservation::dangling_for_test("exit-c"),
        ]
        .into_iter()
        .collect();
        let lease = take_bound_lease(
            &mut clutch,
            std::slice::from_ref(&crate::mixnet::ExitNodeId::from("exit-b")),
        )
        .expect("the reported exit is drawn");
        assert_eq!(lease.node(), &crate::mixnet::ExitNodeId::from("exit-b"));
        assert_eq!(clutch.len(), 2, "the unbound reservations remain");
    }

    /// HYPOTHESIS: a report naming several drawn exits binds the
    /// first-announced one, so the bind follows the transport's own order
    /// rather than the set's arbitrary one. Falsified if any trial binds a
    /// later announcement.
    #[test]
    fn the_bound_lease_follows_the_announcement_order() {
        /// Independently ordered clutches, enough that an arbitrary pick
        /// cannot agree with the announcement order by chance.
        const ORDER_TRIALS: usize = 16;

        let announced = [
            crate::mixnet::ExitNodeId::from("exit-b"),
            crate::mixnet::ExitNodeId::from("exit-c"),
            crate::mixnet::ExitNodeId::from("exit-a"),
        ];
        let first = announced.first().expect("the announcement names exits");
        for trial in 0..ORDER_TRIALS {
            let mut clutch: HashSet<Reservation> = [
                Reservation::dangling_for_test("exit-a"),
                Reservation::dangling_for_test("exit-b"),
                Reservation::dangling_for_test("exit-c"),
            ]
            .into_iter()
            .collect();
            let lease =
                take_bound_lease(&mut clutch, &announced).expect("the report names drawn exits");
            assert_eq!(
                lease.node(),
                first,
                "trial {trial} bound a later announcement"
            );
        }
    }

    /// HYPOTHESIS: a report naming no drawn node (foreign or empty) takes
    /// nothing, so the caller can refuse typed. Falsified if a lease is
    /// yielded or the clutch shrinks.
    #[test]
    fn a_foreign_or_empty_report_takes_no_lease() {
        let mut clutch: HashSet<Reservation> = [Reservation::dangling_for_test("exit-a")]
            .into_iter()
            .collect();
        let foreign = [crate::mixnet::ExitNodeId::from("exit-foreign")];
        assert!(take_bound_lease(&mut clutch, &foreign).is_none());
        assert!(take_bound_lease(&mut clutch, &[]).is_none());
        assert_eq!(clutch.len(), 1, "the clutch is undisturbed");
    }

    /// A tier three clutches wide, so a fixed four-exit subset is provable.
    const TIER_SPREAD_POPULATION: usize = RESERVATION_CLUTCH_SIZE * 3;

    /// Draws enough to make an unshuffled tier's repetition unmistakable.
    const TIER_SPREAD_DRAWS: usize = 10;

    /// HYPOTHESIS: the draw spreads load inside a tier by chance, so
    /// repeated draws over one wide tier reach beyond any fixed
    /// clutch-sized subset. Falsified if every draw binds the same exits.
    #[test]
    fn repeated_draws_spread_across_the_tier() {
        let pool = seeded(TIER_SPREAD_POPULATION);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..TIER_SPREAD_DRAWS {
            let clutch = ExitPool::draw_clutch(&pool).expect("the tier draws");
            for reservation in &clutch {
                seen.insert(reservation.node().clone());
            }
            drop(clutch);
        }
        assert!(
            seen.len() > RESERVATION_CLUTCH_SIZE,
            "every draw bound the same {RESERVATION_CLUTCH_SIZE} exits of \
             {TIER_SPREAD_POPULATION}: the tier is unshuffled"
        );
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

    /// HYPOTHESIS: a discovery naming one exit twice seeds a single
    /// reservation, so the pool can never double-issue that identity and
    /// counts it once. Falsified if the refusal reports a population larger
    /// than the identities the pool can issue.
    #[test]
    fn a_duplicated_discovery_seeds_one_reservation() {
        const DISTINCT_IDENTITIES: usize = 1;
        let pool = Arc::new(Mutex::new(ExitPool::default()));
        let repeated = crate::mixnet::ExitNodeId::from("exit-repeated");
        pool.lock()
            .unwrap()
            .seed(vec![repeated.clone(), repeated.clone()]);
        let clutch = ExitPool::draw_clutch(&pool).expect("the only clutch");
        assert_eq!(
            clutch.len(),
            DISTINCT_IDENTITIES,
            "the repeated identity reserves once"
        );
        match ExitPool::draw_clutch(&pool).expect_err("nothing is left to issue") {
            ExitPoolError::Exhausted { held, population } => {
                assert_eq!(held, DISTINCT_IDENTITIES, "the sole identity is held");
                assert_eq!(
                    population, DISTINCT_IDENTITIES,
                    "the population counts identities rather than repeats"
                );
            }
            other => panic!("the pool refused with {other:?} rather than exhaustion"),
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
