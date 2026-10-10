//! The pure planner for racing pulls over an ordered list of arms.
//!
//! Two escalation styles in this codebase share one skeleton: launch pulls
//! against distinct arms, never pull the same arm twice, stop at a cap,
//! let the first success win, and accumulate every failure. The mixnet
//! bootstrap hedges (a new pull after a silence interval, or immediately when
//! a pull fails) and the send escalation widens in serially gated rounds
//! (ADR 0011). This module captures the shared skeleton as a pure state
//! machine ([`RaceState::start`] and [`RaceState::on_event`] map events to
//! [`RaceAction`]s with no I/O, no clock, and no randomness) and takes the
//! escalation style as data ([`LaunchPolicy`]). Effectful drivers execute
//! the actions: `NymProxy` drives a hedged race over tokio tasks, and
//! zingolib's `escalating_transmit` drives escalating rounds over borrowed
//! futures. Deliberately NOT feature-gated, so the planner's tests run in
//! the default build without the nym-sdk stack.
//!
//! An arm is a contender available to be tried, and a pull is one trial of
//! it (ADR 0035's bandit vocabulary). This race pulls each arm at most
//! once, so the two counts coincide today; the accounting is nevertheless
//! kept in pulls, because the reservation model may pull an arm twice.
//!
//! Every pull's outcome is retained ([`RaceState::failures`]), down to the
//! first: failures trigger immediate replacement launches and feed live
//! progress ([`RaceState::progress`]). The terminal account of a lost race
//! belongs to the driver, which keeps each pull's failure as a typed record
//! (issue #2562); the planner retains only the rendered line each failure
//! fed to its progress narration.
#![forbid(unsafe_code)]

use std::time::Duration;

/// How a race widens.
#[derive(Clone, Copy, Debug)]
pub enum LaunchPolicy {
    /// Launch one pull, then hedge: a further pull after each
    /// `hedge_interval` of silence, or immediately when a pull fails,
    /// holding at most `max_parallel` pulls in flight.
    Hedged {
        /// The most pulls allowed in flight at once.
        max_parallel: usize,
        /// The silence interval after which another pull is launched.
        hedge_interval: Duration,
    },
    /// Escalating serially gated rounds: round `r` launches `r` pulls, and
    /// round `r + 1` launches only after every pull of round `r` has failed.
    /// Uses no timer.
    EscalatingRounds,
}

/// ```
/// // The number of exit reservations an acquisition draws to attempt one
/// // connection.
/// use zingo_netutils::arm_race::RESERVATION_CLUTCH_SIZE;
/// assert_eq!(RESERVATION_CLUTCH_SIZE, 4);
/// ```
pub const RESERVATION_CLUTCH_SIZE: usize = 4;

/// ```
/// // Every Acquisition Race runs under the one hedged launch policy,
/// // capped at the clutch size and paced by the hedge interval.
/// use zingo_netutils::arm_race::{
///     LaunchPolicy, RESERVATION_CLUTCH_SIZE, acquisition_launch_policy,
/// };
/// use zingo_netutils::time::HEDGE_INTERVAL;
///
/// match acquisition_launch_policy() {
///     LaunchPolicy::Hedged {
///         max_parallel,
///         hedge_interval,
///     } => {
///         assert_eq!(max_parallel, RESERVATION_CLUTCH_SIZE);
///         assert_eq!(hedge_interval, HEDGE_INTERVAL);
///     }
///     other => panic!("the one policy is hedged, got {other:?}"),
/// }
/// ```
pub fn acquisition_launch_policy() -> LaunchPolicy {
    LaunchPolicy::Hedged {
        max_parallel: RESERVATION_CLUTCH_SIZE,
        hedge_interval: crate::time::HEDGE_INTERVAL,
    }
}

/// One pull's failure, retained for replacement decisions and progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullFailure {
    /// The arm index (into the caller's ordered list) whose pull failed.
    pub arm: usize,
    /// The failure rendered for a human.
    pub error: String,
}

/// An input to the planner.
#[derive(Clone, Debug)]
pub enum RaceEvent {
    /// The pull of `arm` failed with `error`.
    PullFailed {
        /// The arm index whose pull failed.
        arm: usize,
        /// The failure rendered for a human.
        error: String,
    },
    /// The pending hedge timer elapsed with no pull finishing meanwhile.
    HedgeElapsed,
}

/// An instruction to the effectful driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RaceAction {
    /// Launch a pull of this arm index.
    Launch {
        /// The arm index to pull next.
        arm: usize,
    },
    /// Replace the pending hedge timer with one firing after this duration.
    /// The driver keeps at most one hedge timer.
    SetHedgeTimer(Duration),
    /// No further launches are possible and no pull is in flight: the race is
    /// lost. Read [`RaceState::failures`] for the full account.
    GiveUp,
}

/// A snapshot of the race for progress reporting. Renders via [`Display`]
/// as e.g. `attempt 4/10: 2 in flight, 2 failed`.
///
/// [`Display`]: std::fmt::Display
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RaceProgress {
    /// Pulls launched so far (distinct arms pulled).
    pub launched: usize,
    /// The most arms this race may pull.
    pub limit: usize,
    /// Pulls currently in flight.
    pub in_flight: usize,
    /// Pulls that have failed.
    pub failed: usize,
}

impl std::fmt::Display for RaceProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "attempt {}/{}: {} in flight, {} failed",
            self.launched, self.limit, self.in_flight, self.failed
        )
    }
}

/// The pure racing state machine. See the module docs for the contract.
#[derive(Debug)]
pub struct RaceState {
    policy: LaunchPolicy,
    /// The most arms this race may pull: `cap.min(arms)`.
    limit: usize,
    /// The next unpulled arm index. Also the count of pulls launched so far.
    next: usize,
    in_flight: usize,
    /// The current round size under [`LaunchPolicy::EscalatingRounds`].
    round_size: usize,
    failures: Vec<PullFailure>,
}

impl RaceState {
    /// A race over `arms` many arms, pulling at most `cap` of them, widening
    /// per `policy`.
    pub fn new(arms: usize, cap: usize, policy: LaunchPolicy) -> Self {
        RaceState {
            policy,
            limit: cap.min(arms),
            next: 0,
            in_flight: 0,
            round_size: 1,
            failures: Vec::new(),
        }
    }

    /// Begin the race: the initial launch batch, or an immediate
    /// [`RaceAction::GiveUp`] when there is nothing to pull.
    pub fn start(&mut self) -> Vec<RaceAction> {
        if self.limit == 0 {
            return vec![RaceAction::GiveUp];
        }
        let initial = match self.policy {
            LaunchPolicy::Hedged { .. } | LaunchPolicy::EscalatingRounds => 1,
        };
        let mut actions = self.launch(initial);
        self.set_timer_if_hedging(&mut actions);
        actions
    }

    /// Advance the race on `event`, returning the driver's next actions.
    pub fn on_event(&mut self, event: RaceEvent) -> Vec<RaceAction> {
        let mut actions = match event {
            RaceEvent::PullFailed { arm, error } => {
                self.in_flight = self.in_flight.saturating_sub(1);
                self.failures.push(PullFailure { arm, error });
                match self.policy {
                    LaunchPolicy::Hedged { .. } => self.launch(1),
                    LaunchPolicy::EscalatingRounds => {
                        if self.in_flight > 0 {
                            // The round is not over; the gate holds.
                            Vec::new()
                        } else {
                            self.round_size += 1;
                            self.launch(self.round_size)
                        }
                    }
                }
            }
            RaceEvent::HedgeElapsed => match self.policy {
                LaunchPolicy::Hedged { max_parallel, .. } => {
                    if self.in_flight < max_parallel {
                        self.launch(1)
                    } else {
                        Vec::new()
                    }
                }
                LaunchPolicy::EscalatingRounds => Vec::new(),
            },
        };

        if self.in_flight == 0 && actions.is_empty() {
            actions.push(RaceAction::GiveUp);
        } else {
            self.set_timer_if_hedging(&mut actions);
        }
        actions
    }

    /// Launch pulls of up to `count` fresh arms, bounded by the limit.
    fn launch(&mut self, count: usize) -> Vec<RaceAction> {
        let launches = count.min(self.limit - self.next);
        let mut actions = Vec::with_capacity(launches);
        for _ in 0..launches {
            actions.push(RaceAction::Launch { arm: self.next });
            self.next += 1;
            self.in_flight += 1;
        }
        actions
    }

    /// Under [`LaunchPolicy::Hedged`], re-set the hedge timer whenever another
    /// pull could still be launched by a future timer firing.
    fn set_timer_if_hedging(&self, actions: &mut Vec<RaceAction>) {
        if let LaunchPolicy::Hedged {
            max_parallel,
            hedge_interval,
        } = self.policy
            && self.next < self.limit
            && self.in_flight < max_parallel
        {
            actions.push(RaceAction::SetHedgeTimer(hedge_interval));
        }
    }

    /// A snapshot for progress reporting.
    pub fn progress(&self) -> RaceProgress {
        RaceProgress {
            launched: self.next,
            limit: self.limit,
            in_flight: self.in_flight,
            failed: self.failures.len(),
        }
    }

    /// Every pull failure so far, in the order they happened.
    pub fn failures(&self) -> &[PullFailure] {
        &self.failures
    }

    /// Distinct arms pulled so far.
    pub fn launched(&self) -> usize {
        self.next
    }
}

#[cfg(test)]
mod tests {
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
}
