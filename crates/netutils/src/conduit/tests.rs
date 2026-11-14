use super::*;

fn conduit() -> MixnetConduit {
    MixnetConduit::over("127.0.0.1:9050".parse().expect("the test address parses"))
}

/// HYPOTHESIS: a conduit counts its outstanding uses, so dropping the
/// last guard is observable. Falsified if a dropped guard still counts.
#[test]
fn a_dropped_guard_stops_counting() {
    let conduit = conduit();
    assert_eq!(conduit.in_flight(), 0);
    let first = conduit.dial();
    let second = conduit.dial();
    assert_eq!(conduit.in_flight(), 2);
    drop(first);
    assert_eq!(conduit.in_flight(), 1);
    drop(second);
    assert_eq!(conduit.in_flight(), 0);
}

/// HYPOTHESIS: retirement waits for the work, so a superseded conduit
/// with a guard outstanding is not retired. Falsified if superseding
/// alone retires it.
#[test]
fn a_superseded_conduit_retires_only_when_idle() {
    let conduit = conduit();
    assert_eq!(conduit.state(), ConduitState::Serving);
    let held = conduit.dial();
    conduit.supersede();
    assert_eq!(conduit.state(), ConduitState::Superseded);
    drop(held);
    assert_eq!(conduit.state(), ConduitState::Retired);
}

/// HYPOTHESIS: the states order as the life runs, so a later state
/// compares greater. Falsified if the derive ever reorders them.
#[test]
fn the_states_order_as_the_life_runs() {
    assert!(ConduitState::Serving < ConduitState::Superseded);
    assert!(ConduitState::Superseded < ConduitState::Retired);
}

/// HYPOTHESIS: a clone shares one count and is not itself a use, so a
/// conduit handed to two holders reports their uses together and only
/// their uses. Falsified if cloning forks the count or inflates it,
/// the latter leaving a superseded conduit permanently unretired.
#[test]
fn a_clone_shares_one_count_and_is_not_a_use() {
    let conduit = conduit();
    let shared = conduit.clone();
    assert_eq!(conduit.in_flight(), 0, "holding a conduit is not using it");
    let held = shared.dial();
    assert_eq!(conduit.in_flight(), 1);
    drop(held);
    assert_eq!(conduit.in_flight(), 0);
    assert_eq!(shared.state(), ConduitState::Serving);
}

/// HYPOTHESIS: retirement is visible from every holder, so the rotation
/// that supersedes a conduit can watch a clone the route resolver
/// handed out. Falsified if a clone reports its own state.
#[test]
fn every_holder_sees_the_same_retirement() {
    let conduit = conduit();
    let routed = conduit.clone();
    let held = routed.dial();
    conduit.supersede();
    assert_eq!(routed.state(), ConduitState::Superseded);
    drop(held);
    assert_eq!(conduit.state(), ConduitState::Retired);
    assert_eq!(routed.state(), ConduitState::Retired);
}
