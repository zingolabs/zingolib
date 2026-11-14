use super::*;

use crate::time::test::PLANNER_HEDGE;

fn hedged(max_parallel: usize) -> LaunchPolicy {
    LaunchPolicy::Hedged {
        max_parallel,
        hedge_interval: PLANNER_HEDGE,
    }
}

fn failed(arm: usize) -> RaceEvent {
    RaceEvent::PullFailed {
        arm,
        error: format!("arm {arm} down"),
    }
}

#[test]
fn start_launches_one_pull_and_sets_the_hedge_timer() {
    let mut race = RaceState::new(10, 10, hedged(3));
    assert_eq!(
        race.start(),
        vec![
            RaceAction::Launch { arm: 0 },
            RaceAction::SetHedgeTimer(PLANNER_HEDGE)
        ]
    );
}

#[test]
fn hedge_elapse_widens_until_max_parallel() {
    let mut race = RaceState::new(10, 10, hedged(3));
    race.start();
    assert_eq!(
        race.on_event(RaceEvent::HedgeElapsed),
        vec![
            RaceAction::Launch { arm: 1 },
            RaceAction::SetHedgeTimer(PLANNER_HEDGE)
        ]
    );
    // The third pull fills max_parallel, so no further timer is set.
    assert_eq!(
        race.on_event(RaceEvent::HedgeElapsed),
        vec![RaceAction::Launch { arm: 2 }]
    );
    // At max_parallel a timer firing launches nothing.
    assert_eq!(race.on_event(RaceEvent::HedgeElapsed), Vec::new());
}

#[test]
fn a_failure_launches_a_replacement_immediately() {
    let mut race = RaceState::new(10, 10, hedged(3));
    race.start();
    let actions = race.on_event(failed(0));
    assert_eq!(
        actions,
        vec![
            RaceAction::Launch { arm: 1 },
            RaceAction::SetHedgeTimer(PLANNER_HEDGE)
        ],
        "a failure is a signal to try elsewhere at once, not to wait"
    );
}

#[test]
fn exhaustion_with_pulls_in_flight_waits_rather_than_giving_up() {
    let mut race = RaceState::new(2, 10, hedged(3));
    race.start();
    race.on_event(RaceEvent::HedgeElapsed); // both arms in flight
    assert_eq!(
        race.on_event(failed(0)),
        Vec::new(),
        "no fresh arm, but arm 1's pull still races"
    );
}

#[test]
fn the_last_failure_with_nothing_left_gives_up_with_the_full_account() {
    let mut race = RaceState::new(2, 10, hedged(3));
    race.start();
    race.on_event(RaceEvent::HedgeElapsed);
    race.on_event(failed(0));
    assert_eq!(race.on_event(failed(1)), vec![RaceAction::GiveUp]);
    assert_eq!(
        race.failures(),
        &[
            PullFailure {
                arm: 0,
                error: "arm 0 down".to_string(),
            },
            PullFailure {
                arm: 1,
                error: "arm 1 down".to_string(),
            },
        ],
        "every failure is retained, in order"
    );
}

#[test]
fn an_empty_arm_list_gives_up_at_start() {
    let mut race = RaceState::new(0, 10, hedged(3));
    assert_eq!(race.start(), vec![RaceAction::GiveUp]);
    let mut race = RaceState::new(10, 0, hedged(3));
    assert_eq!(race.start(), vec![RaceAction::GiveUp]);
}

#[test]
fn the_cap_bounds_distinct_arms() {
    let mut race = RaceState::new(10, 2, hedged(3));
    race.start();
    race.on_event(RaceEvent::HedgeElapsed);
    // Both capped arms are in flight; a failure launches nothing new.
    assert_eq!(race.on_event(failed(0)), Vec::new());
    assert_eq!(race.launched(), 2);
}

#[test]
fn rounds_escalate_one_two_three_gated_on_whole_round_failure() {
    let mut race = RaceState::new(10, 6, LaunchPolicy::EscalatingRounds);
    assert_eq!(race.start(), vec![RaceAction::Launch { arm: 0 }]);
    // Round one fails: round two launches two pulls, no timer ever.
    assert_eq!(
        race.on_event(failed(0)),
        vec![RaceAction::Launch { arm: 1 }, RaceAction::Launch { arm: 2 }]
    );
    // One of round two fails: the gate holds while its sibling races.
    assert_eq!(race.on_event(failed(1)), Vec::new());
    // The whole round has failed: round three launches three pulls.
    assert_eq!(
        race.on_event(failed(2)),
        vec![
            RaceAction::Launch { arm: 3 },
            RaceAction::Launch { arm: 4 },
            RaceAction::Launch { arm: 5 }
        ]
    );
    // All six capped pulls have failed: the race is lost.
    race.on_event(failed(3));
    race.on_event(failed(4));
    assert_eq!(race.on_event(failed(5)), vec![RaceAction::GiveUp]);
    assert_eq!(race.launched(), 6, "the cap held");
    assert_eq!(race.failures().len(), 6);
}

#[test]
fn a_round_is_bounded_by_the_remaining_arms() {
    let mut race = RaceState::new(2, 6, LaunchPolicy::EscalatingRounds);
    race.start();
    assert_eq!(
        race.on_event(failed(0)),
        vec![RaceAction::Launch { arm: 1 }],
        "round two wants two pulls but only one arm remains"
    );
    assert_eq!(race.on_event(failed(1)), vec![RaceAction::GiveUp]);
}

#[test]
fn progress_renders_the_race_snapshot() {
    let mut race = RaceState::new(10, 10, hedged(3));
    race.start();
    race.on_event(RaceEvent::HedgeElapsed);
    race.on_event(failed(0));
    assert_eq!(
        race.progress().to_string(),
        "attempt 3/10: 2 in flight, 1 failed"
    );
}
