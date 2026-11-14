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
