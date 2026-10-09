//! Exit identity, health, Exit Pool.
#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};

use rand::seq::SliceRandom as _;

/// Exit Node identity, as announced.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(into = "String", try_from = "String")]
pub struct ExitNodeId(String);

impl ExitNodeId {
    /// Wire string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Trims; refuses blank.
    pub fn parse(candidate: &str) -> Result<Self, BlankExitNodeId> {
        let identity = candidate.trim();
        if identity.is_empty() {
            return Err(BlankExitNodeId);
        }
        Ok(ExitNodeId(identity.to_string()))
    }
}

/// Blank candidate.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("an Exit Node identity cannot be blank")]
pub struct BlankExitNodeId;

impl TryFrom<String> for ExitNodeId {
    type Error = BlankExitNodeId;

    fn try_from(candidate: String) -> Result<Self, Self::Error> {
        ExitNodeId::parse(&candidate)
    }
}

impl From<ExitNodeId> for String {
    fn from(identity: ExitNodeId) -> Self {
        identity.0
    }
}

impl std::fmt::Display for ExitNodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(any(test, feature = "testutils"))]
impl From<&str> for ExitNodeId {
    fn from(identity: &str) -> Self {
        ExitNodeId::parse(identity).expect("test exit identities are non-blank")
    }
}

#[cfg(test)]
mod identity_tests {
    use super::{BlankExitNodeId, ExitNodeId};

    /// HYPOTHESIS: minting trims the candidate and refuses a blank, so the
    /// spawned and hosted paths cannot mint unequal spellings of one exit or
    /// an empty identity. Falsified if whitespace survives or a blank mints.
    #[test]
    fn minting_trims_and_refuses_a_blank() {
        assert_eq!(
            ExitNodeId::parse(" exit-alpha ").expect("non-blank mints"),
            ExitNodeId::parse("exit-alpha").expect("non-blank mints"),
        );
        assert_eq!(ExitNodeId::parse(""), Err(BlankExitNodeId));
        assert_eq!(ExitNodeId::parse("   "), Err(BlankExitNodeId));
        assert_eq!(
            ExitNodeId::try_from(String::from("  ")),
            Err(BlankExitNodeId)
        );
    }
}

/// Draw refusal.
#[derive(Debug, thiserror::Error)]
pub enum ExitPoolError {
    /// Population unknown.
    #[error("the exit pool has no population yet")]
    NotSeeded,
    /// All reservations held.
    #[error("every exit is held: {held} held, {population} known")]
    Exhausted {
        /// Reservations issued.
        held: usize,
        /// Population size.
        population: usize,
    },
}

/// Issued reservation; recycles on drop.
pub struct Reservation {
    node: ExitNodeId,
    ledger: Weak<Mutex<ExitPool>>,
}

impl Reservation {
    /// Reserved identity.
    pub fn node(&self) -> &ExitNodeId {
        &self.node
    }

    /// Ledgerless reservation, for tests.
    #[cfg(any(test, feature = "testutils"))]
    pub fn dangling_for_test(node: &str) -> Self {
        Reservation {
            node: ExitNodeId::from(node),
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

impl std::borrow::Borrow<ExitNodeId> for Reservation {
    fn borrow(&self) -> &ExitNodeId {
        &self.node
    }
}

/// Round-trip verdict, epoch-scoped.
// TODO: implement sensitivity to, and policy around, Nym epochs: the
// network's epoch boundaries are queryable, and the sliding one-hour
// window from the observation instant is a stand-in for the real
// rotation edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitNodeHealthVerdict {
    /// Round trip completed.
    EpochProven,
    /// Refused, timed out, or silent.
    Failed,
}

/// Latest verdict with its instant.
#[derive(Clone, Copy, Debug)]
pub struct Observation {
    verdict: ExitNodeHealthVerdict,
    at: std::time::Instant,
}

impl Observation {
    /// Verdict earned at `at`.
    pub fn earned(verdict: ExitNodeHealthVerdict, at: std::time::Instant) -> Self {
        Observation { verdict, at }
    }
}

/// Health record, keyed by node.
#[derive(Default)]
pub struct NodeHealthIndex(std::collections::HashMap<ExitNodeId, Observation>);

impl NodeHealthIndex {
    /// Supersedes any earlier verdict.
    pub fn remember(&mut self, exit: ExitNodeId, observation: Observation) {
        self.0.insert(exit, observation);
    }

    /// Proven within the epoch.
    pub fn epoch_proven(&self, exit: &ExitNodeId, now: std::time::Instant) -> bool {
        self.observed(exit, now, ExitNodeHealthVerdict::EpochProven)
    }

    /// Failed within the epoch.
    pub fn epoch_failed(&self, exit: &ExitNodeId, now: std::time::Instant) -> bool {
        self.observed(exit, now, ExitNodeHealthVerdict::Failed)
    }

    /// When the proof goes stale.
    pub fn proven_until(&self, exit: &ExitNodeId) -> Option<std::time::Instant> {
        self.0.get(exit).and_then(|seen| {
            (seen.verdict == ExitNodeHealthVerdict::EpochProven)
                .then(|| seen.at + crate::time::NYM_EPOCH)
        })
    }

    fn observed(
        &self,
        exit: &ExitNodeId,
        now: std::time::Instant,
        verdict: ExitNodeHealthVerdict,
    ) -> bool {
        self.0.get(exit).is_some_and(|seen| {
            seen.verdict == verdict
                && now.saturating_duration_since(seen.at) < crate::time::NYM_EPOCH
        })
    }
}

/// One reservation per node, one holder at a time.
#[derive(Default)]
pub struct ExitPool {
    population: HashSet<ExitNodeId>,
    issued: HashSet<ExitNodeId>,
    health: NodeHealthIndex,
}

impl ExitPool {
    /// Records the population, once.
    pub fn seed(&mut self, discovered: impl IntoIterator<Item = ExitNodeId>) {
        self.population = discovered.into_iter().collect();
    }

    /// Population known.
    pub fn is_seeded(&self) -> bool {
        !self.population.is_empty()
    }

    /// Supersedes `exit`'s verdict.
    pub fn remember(&mut self, exit: ExitNodeId, observation: Observation) {
        self.health.remember(exit, observation);
    }

    /// Proven within the epoch.
    pub fn epoch_proven(&self, exit: &ExitNodeId, now: std::time::Instant) -> bool {
        self.health.epoch_proven(exit, now)
    }

    /// When `exit`'s proof goes stale.
    pub fn proven_until(&self, exit: &ExitNodeId) -> Option<std::time::Instant> {
        self.health.proven_until(exit)
    }

    /// Failed within the epoch.
    // Exercised by the ProofAcquisition contract tests; production reads
    // failures only through the draw's own partition.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn epoch_failed(&self, exit: &ExitNodeId, now: std::time::Instant) -> bool {
        self.health.epoch_failed(exit, now)
    }

    /// Draws one reservation: Proven, then unknown, then Failed.
    pub fn draw_exit(pool: &Arc<Mutex<ExitPool>>) -> Result<Reservation, ExitPoolError> {
        let now = std::time::Instant::now();
        let mut guarded = pool.lock().expect("exit pool mutex");
        if guarded.population.is_empty() {
            return Err(ExitPoolError::NotSeeded);
        }
        // The candidates are collected before any issue, so the ledger is
        // written while the population is no longer borrowed.
        let drawable: Vec<ExitNodeId> = guarded
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
        let mut ordered: Vec<ExitNodeId> = Vec::new();
        for mut tier in [proven, unknown, failed] {
            tier.shuffle(&mut rand::rngs::OsRng);
            ordered.extend(tier);
        }
        // One exit per birth: the preference order above decides what a
        // birth uses, because nothing downstream can overrule it.
        let node = ordered.into_iter().next().expect("drawable is non-empty");
        guarded.issued.insert(node.clone());
        Ok(Reservation {
            node,
            ledger: Arc::downgrade(pool),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A population wide enough that repeated draws reaching only a few
    /// exits would be unmistakable.
    const TIER_SPREAD_POPULATION: usize = 12;

    /// Draws enough to make an unshuffled tier's repetition unmistakable.
    const TIER_SPREAD_DRAWS: usize = 10;

    /// The smallest population that can be exhausted by one draw.
    const SOLE_EXIT: usize = 1;

    fn seeded(count: usize) -> Arc<Mutex<ExitPool>> {
        let pool = Arc::new(Mutex::new(ExitPool::default()));
        pool.lock()
            .unwrap()
            .seed((0..count).map(|index| ExitNodeId::from(format!("exit-{index}").as_str())));
        pool
    }

    /// HYPOTHESIS: the draw spreads load inside a tier by chance, so
    /// repeated draws over one wide tier reach beyond any fixed subset.
    /// Falsified if every draw binds the same exit.
    #[test]
    fn repeated_draws_spread_across_the_tier() {
        let pool = seeded(TIER_SPREAD_POPULATION);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..TIER_SPREAD_DRAWS {
            let lease = ExitPool::draw_exit(&pool).expect("the tier draws");
            seen.insert(lease.node().clone());
            drop(lease);
        }
        assert!(
            seen.len() > SOLE_EXIT,
            "every draw bound the same exit of {TIER_SPREAD_POPULATION}: \
             the tier is unshuffled"
        );
    }

    /// HYPOTHESIS: a draw transfers its reservation, so a second draw can
    /// never repeat it. Falsified if one identity reaches two holders.
    #[test]
    fn a_reservation_is_never_issued_twice() {
        let pool = seeded(TIER_SPREAD_POPULATION);
        let first = ExitPool::draw_exit(&pool).expect("the first draw");
        let second = ExitPool::draw_exit(&pool).expect("the second draw");
        assert_ne!(
            first.node(),
            second.node(),
            "{} was issued to two holders at once",
            first.node()
        );
    }

    /// HYPOTHESIS: a discovery naming one exit twice seeds a single
    /// reservation, so the pool can never double-issue that identity and
    /// counts it once. Falsified if the refusal reports a population larger
    /// than the identities the pool can issue.
    #[test]
    fn a_duplicated_discovery_seeds_one_reservation() {
        let pool = Arc::new(Mutex::new(ExitPool::default()));
        let repeated = ExitNodeId::from("exit-repeated");
        pool.lock()
            .unwrap()
            .seed(vec![repeated.clone(), repeated.clone()]);
        let lease = ExitPool::draw_exit(&pool).expect("the only draw");
        assert_eq!(
            lease.node(),
            &repeated,
            "the repeated identity reserves once"
        );
        match ExitPool::draw_exit(&pool).expect_err("nothing is left to issue") {
            ExitPoolError::Exhausted { held, population } => {
                assert_eq!(held, SOLE_EXIT, "the sole identity is held");
                assert_eq!(
                    population, SOLE_EXIT,
                    "the population counts identities rather than repeats"
                );
            }
            other => panic!("an exhausted pool refuses as Exhausted, got {other:?}"),
        }
    }

    /// HYPOTHESIS: dropping a reservation recycles it, so an exhausted pool
    /// issues again only after its holder releases.
    #[test]
    fn dropping_replenishes_an_exhausted_pool() {
        let pool = seeded(SOLE_EXIT);
        let lease = ExitPool::draw_exit(&pool).expect("the only draw");
        let exhausted = ExitPool::draw_exit(&pool).expect_err("nothing is left to issue");
        assert!(matches!(exhausted, ExitPoolError::Exhausted { .. }));
        drop(lease);
        ExitPool::draw_exit(&pool).expect("a dropped reservation reissues");
    }

    /// HYPOTHESIS: cancelling a future that owns a reservation recycles it,
    /// the leak shape of an abandoned birth.
    #[test]
    fn a_cancelled_owner_recycles_by_drop() {
        let pool = seeded(SOLE_EXIT);
        let lease = ExitPool::draw_exit(&pool).expect("the only draw");
        let owner = async move {
            let _held = lease;
            std::future::pending::<()>().await;
        };
        drop(owner);
        ExitPool::draw_exit(&pool).expect("the cancelled owner's reservation reissues");
    }

    /// HYPOTHESIS: a reservation outliving its pool recycles into nothing
    /// rather than panicking.
    #[test]
    fn an_orphaned_reservation_drops_quietly() {
        let pool = seeded(SOLE_EXIT);
        let lease = ExitPool::draw_exit(&pool).expect("the only draw");
        drop(pool);
        drop(lease);
    }

    /// HYPOTHESIS: an unseeded pool refuses rather than drawing nothing
    /// silently.
    #[test]
    fn an_unseeded_pool_refuses() {
        let pool = Arc::new(Mutex::new(ExitPool::default()));
        assert!(matches!(
            ExitPool::draw_exit(&pool).expect_err("no population"),
            ExitPoolError::NotSeeded
        ));
        assert!(!pool.lock().unwrap().is_seeded());
    }
}
