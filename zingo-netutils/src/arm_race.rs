//! The pure planner for racing arms over an ordered candidate list.
//!
//! Two escalation styles in this codebase share one skeleton: launch arms
//! against distinct candidates, never repeat a candidate, stop at a cap,
//! let the first success win, and accumulate every failure. The mixnet
//! bootstrap hedges (a new arm after a silence interval, or immediately when
//! an arm fails) and the send fan-out escalates in serially gated rounds
//! (ADR 0011). This module captures the shared skeleton as a pure state
//! machine — [`RaceState::start`] and [`RaceState::on_event`] map events to
//! [`RaceAction`]s with no I/O, no clock, and no randomness — and takes the
//! escalation style as data ([`LaunchPolicy`]). Effectful drivers execute
//! the actions: `NymProxy` drives a hedged race over tokio tasks, and
//! zingolib's `fanout_broadcast` drives escalating rounds over borrowed
//! futures. Deliberately NOT feature-gated, so the planner's tests run in
//! the default build without the nym-sdk stack.
//!
//! Every arm's outcome is retained ([`RaceState::failures`]), not just the
//! last: failures trigger immediate replacement launches, feed live
//! progress ([`RaceState::progress`]), and compose the terminal error
//! summary ([`RaceState::failure_summary`]).
#![forbid(unsafe_code)]

use std::time::Duration;

/// How a race widens.
#[derive(Clone, Copy, Debug)]
pub enum LaunchPolicy {
    /// Launch one arm, then hedge: a further arm after each `hedge_interval`
    /// of silence, or immediately when an arm fails, holding at most
    /// `max_parallel` arms in flight.
    Hedged {
        /// The most arms allowed in flight at once.
        max_parallel: usize,
        /// The silence interval after which another arm is launched.
        hedge_interval: Duration,
    },
    /// Escalating serially gated rounds: round `r` launches `r` arms, and
    /// round `r + 1` launches only after every arm of round `r` has failed.
    /// Uses no timer.
    EscalatingRounds,
}

/// One arm's failure, retained for replacement decisions, progress, and the
/// terminal summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmFailure {
    /// The candidate index (into the caller's ordered list) that failed.
    pub candidate: usize,
    /// The failure rendered for a human.
    pub error: String,
}

/// An input to the planner.
#[derive(Clone, Debug)]
pub enum RaceEvent {
    /// The arm racing `candidate` failed with `error`.
    ArmFailed {
        /// The candidate index whose arm failed.
        candidate: usize,
        /// The failure rendered for a human.
        error: String,
    },
    /// The pending hedge timer elapsed with no arm finishing meanwhile.
    HedgeElapsed,
}

/// An instruction to the effectful driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RaceAction {
    /// Launch an arm against this candidate index.
    Launch {
        /// The candidate index to race next.
        candidate: usize,
    },
    /// Replace the pending hedge timer with one firing after this duration.
    /// The driver keeps at most one hedge timer.
    ArmHedgeTimer(Duration),
    /// No further launches are possible and no arm is in flight: the race is
    /// lost. Read [`RaceState::failures`] for the full account.
    GiveUp,
}

/// A snapshot of the race for progress reporting. Renders via [`Display`]
/// as e.g. `attempt 4/10: 2 in flight, 2 failed`.
///
/// [`Display`]: std::fmt::Display
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RaceProgress {
    /// Arms launched so far (distinct candidates contacted).
    pub launched: usize,
    /// The most candidates this race may contact.
    pub limit: usize,
    /// Arms currently in flight.
    pub in_flight: usize,
    /// Arms that have failed.
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
    /// The most candidates this race may contact: `cap.min(candidates)`.
    limit: usize,
    /// The next unlaunched candidate index; also the count launched so far.
    next: usize,
    in_flight: usize,
    /// The current round size under [`LaunchPolicy::EscalatingRounds`].
    round_size: usize,
    failures: Vec<ArmFailure>,
}

impl RaceState {
    /// A race over `candidates` many candidates, contacting at most `cap` of
    /// them, widening per `policy`.
    pub fn new(candidates: usize, cap: usize, policy: LaunchPolicy) -> Self {
        RaceState {
            policy,
            limit: cap.min(candidates),
            next: 0,
            in_flight: 0,
            round_size: 1,
            failures: Vec::new(),
        }
    }

    /// Begin the race: the initial launch batch, or an immediate
    /// [`RaceAction::GiveUp`] when there is nothing to contact.
    pub fn start(&mut self) -> Vec<RaceAction> {
        if self.limit == 0 {
            return vec![RaceAction::GiveUp];
        }
        let mut actions = self.launch(1);
        self.arm_timer_if_hedging(&mut actions);
        actions
    }

    /// Advance the race on `event`, returning the driver's next actions.
    pub fn on_event(&mut self, event: RaceEvent) -> Vec<RaceAction> {
        let mut actions = match event {
            RaceEvent::ArmFailed { candidate, error } => {
                self.in_flight = self.in_flight.saturating_sub(1);
                self.failures.push(ArmFailure { candidate, error });
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
            self.arm_timer_if_hedging(&mut actions);
        }
        actions
    }

    /// Launch up to `count` fresh candidates, bounded by the limit.
    fn launch(&mut self, count: usize) -> Vec<RaceAction> {
        let launches = count.min(self.limit - self.next);
        let mut actions = Vec::with_capacity(launches);
        for _ in 0..launches {
            actions.push(RaceAction::Launch {
                candidate: self.next,
            });
            self.next += 1;
            self.in_flight += 1;
        }
        actions
    }

    /// Under [`LaunchPolicy::Hedged`], re-arm the hedge timer whenever another
    /// arm could still be launched by a future timer firing.
    fn arm_timer_if_hedging(&self, actions: &mut Vec<RaceAction>) {
        if let LaunchPolicy::Hedged {
            max_parallel,
            hedge_interval,
        } = self.policy
            && self.next < self.limit
            && self.in_flight < max_parallel
        {
            actions.push(RaceAction::ArmHedgeTimer(hedge_interval));
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

    /// Every arm failure so far, in the order they happened.
    pub fn failures(&self) -> &[ArmFailure] {
        &self.failures
    }

    /// Distinct candidates contacted so far.
    pub fn launched(&self) -> usize {
        self.next
    }

    /// The terminal account of a lost race: every failure, rendered through
    /// `name` (mapping a candidate index to something a human recognizes).
    pub fn failure_summary(&self, name: impl Fn(usize) -> String) -> String {
        if self.failures.is_empty() {
            return "no candidate was contacted".to_string();
        }
        let parts: Vec<String> = self
            .failures
            .iter()
            .map(|f| format!("{}: {}", name(f.candidate), f.error))
            .collect();
        format!(
            "all {} attempts failed — {}",
            self.failures.len(),
            parts.join("; ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEDGE: Duration = Duration::from_secs(5);

    fn hedged(max_parallel: usize) -> LaunchPolicy {
        LaunchPolicy::Hedged {
            max_parallel,
            hedge_interval: HEDGE,
        }
    }

    fn failed(candidate: usize) -> RaceEvent {
        RaceEvent::ArmFailed {
            candidate,
            error: format!("candidate {candidate} down"),
        }
    }

    #[test]
    fn start_launches_one_arm_and_arms_the_hedge_timer() {
        let mut race = RaceState::new(10, 10, hedged(3));
        assert_eq!(
            race.start(),
            vec![
                RaceAction::Launch { candidate: 0 },
                RaceAction::ArmHedgeTimer(HEDGE)
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
                RaceAction::Launch { candidate: 1 },
                RaceAction::ArmHedgeTimer(HEDGE)
            ]
        );
        // The third arm fills max_parallel, so no further timer is armed.
        assert_eq!(
            race.on_event(RaceEvent::HedgeElapsed),
            vec![RaceAction::Launch { candidate: 2 }]
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
                RaceAction::Launch { candidate: 1 },
                RaceAction::ArmHedgeTimer(HEDGE)
            ],
            "a failure is a signal to try elsewhere at once, not to wait"
        );
    }

    #[test]
    fn exhaustion_with_arms_in_flight_waits_rather_than_giving_up() {
        let mut race = RaceState::new(2, 10, hedged(3));
        race.start();
        race.on_event(RaceEvent::HedgeElapsed); // both candidates in flight
        assert_eq!(
            race.on_event(failed(0)),
            Vec::new(),
            "no fresh candidate, but candidate 1 still races"
        );
    }

    #[test]
    fn the_last_failure_with_nothing_left_gives_up_with_the_full_account() {
        let mut race = RaceState::new(2, 10, hedged(3));
        race.start();
        race.on_event(RaceEvent::HedgeElapsed);
        race.on_event(failed(0));
        assert_eq!(race.on_event(failed(1)), vec![RaceAction::GiveUp]);
        assert_eq!(race.failures().len(), 2, "every failure is retained");
        let summary = race.failure_summary(|i| format!("provider-{i}"));
        assert!(summary.contains("provider-0: candidate 0 down"));
        assert!(summary.contains("provider-1: candidate 1 down"));
    }

    #[test]
    fn an_empty_candidate_list_gives_up_at_start() {
        let mut race = RaceState::new(0, 10, hedged(3));
        assert_eq!(race.start(), vec![RaceAction::GiveUp]);
        let mut race = RaceState::new(10, 0, hedged(3));
        assert_eq!(race.start(), vec![RaceAction::GiveUp]);
    }

    #[test]
    fn the_cap_bounds_distinct_candidates() {
        let mut race = RaceState::new(10, 2, hedged(3));
        race.start();
        race.on_event(RaceEvent::HedgeElapsed);
        // Both capped candidates are in flight; a failure launches nothing new.
        assert_eq!(race.on_event(failed(0)), Vec::new());
        assert_eq!(race.launched(), 2);
    }

    #[test]
    fn rounds_escalate_one_two_three_gated_on_whole_round_failure() {
        let mut race = RaceState::new(10, 6, LaunchPolicy::EscalatingRounds);
        assert_eq!(race.start(), vec![RaceAction::Launch { candidate: 0 }]);
        // Round one fails: round two launches two arms, no timer ever.
        assert_eq!(
            race.on_event(failed(0)),
            vec![
                RaceAction::Launch { candidate: 1 },
                RaceAction::Launch { candidate: 2 }
            ]
        );
        // One of round two fails: the gate holds while its sibling races.
        assert_eq!(race.on_event(failed(1)), Vec::new());
        // The whole round has failed: round three launches three arms.
        assert_eq!(
            race.on_event(failed(2)),
            vec![
                RaceAction::Launch { candidate: 3 },
                RaceAction::Launch { candidate: 4 },
                RaceAction::Launch { candidate: 5 }
            ]
        );
        // All six capped arms have failed: the race is lost.
        race.on_event(failed(3));
        race.on_event(failed(4));
        assert_eq!(race.on_event(failed(5)), vec![RaceAction::GiveUp]);
        assert_eq!(race.launched(), 6, "the cap held");
        assert_eq!(race.failures().len(), 6);
    }

    #[test]
    fn a_round_is_bounded_by_the_remaining_candidates() {
        let mut race = RaceState::new(2, 6, LaunchPolicy::EscalatingRounds);
        race.start();
        assert_eq!(
            race.on_event(failed(0)),
            vec![RaceAction::Launch { candidate: 1 }],
            "round two wants two arms but only one candidate remains"
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
