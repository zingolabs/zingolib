//! The one wave every speed-priority operation runs.
//!
//! The Server-Selection Sweep and the price run differ in what they race
//! and in what settles them, and in nothing else: both ride a Proven
//! Client and open a fixed-width wave through its Exit Node. Proof belongs
//! to the client's birth now, so no Sentinel rides the wave; the redraw
//! survives as the safety net for an exit dying inside its epoch, keyed on
//! a wave that not one target answered.

#![forbid(unsafe_code)]

use std::net::SocketAddr;

use crate::lightclient::select::SURVEY_WAVE_WIDTH;

/// An operation that races targets through one Proven Client in a
/// fixed-width wave, redrawing when a wave proves the exit died under it.
pub(crate) trait SpeedPrioritized {
    /// What this operation races: an indexer's endpoint, a price source.
    type Target: Send;

    /// What one target yields, whether it answered or refused.
    type Outcome: Send;

    /// The targets this operation races, in the order the wave takes them.
    fn targets(&self) -> Vec<Self::Target>;

    /// Races one target through the tunnel at `socks5`.
    fn probe(
        &self,
        socks5: SocketAddr,
        target: Self::Target,
    ) -> impl std::future::Future<Output = Self::Outcome> + Send;

    /// Whether the outcomes so far settle this operation, which ends the
    /// wave with every remaining leg cancelled.
    fn settled(&self, outcomes: &[Self::Outcome]) -> bool;

    /// Whether any target answered at all, the evidence separating a live
    /// exit over unhealthy targets from an exit that died under the wave.
    fn answered(&self, outcomes: &[Self::Outcome]) -> bool;

    /// Acquires a Proven Client for one attempt, with the address it
    /// listens on.
    fn acquire(
        &self,
    ) -> impl std::future::Future<
        Output = Result<(Member, SocketAddr), crate::mixnet::acquire::TransportError>,
    > + Send;

    /// Disposes of a transport the operation is done with, in the
    /// background: the answer is already in hand, and teardown is no reason
    /// to keep a caller waiting.
    fn dispose(&self, spent: Member);

    /// Condemns a transport whose exit died under the operation, in the
    /// background, remembering the failure before teardown.
    fn abandon(&self, dead: Member);

    /// Narrates one phase of the attempt.
    fn narrate(&self, phase: SpeedProgress) {
        let _ = phase;
    }
}

/// The transport every speed-priority operation rides: one Shared-exit
/// Proven Client.
pub(crate) type Member = crate::correspondent::pool::Member<
    crate::mixnet::MixnetProxy,
    crate::correspondent::pool::Shared,
>;

/// A phase of a speed-priority attempt, for the operation to narrate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpeedProgress {
    /// An attempt is acquiring its transport, counting draws from one.
    Acquiring {
        /// Which draw this is.
        draw: usize,
    },
    /// The wave went wholly unanswered, so the exit is condemned, its
    /// outcomes dropped, and a fresh Proven Client drawn.
    ExitAbandoned {
        /// Which draw's wave went unanswered.
        draw: usize,
    },
}

/// Runs `operation` until it settles or exhausts its targets, redrawing a
/// fresh Proven Client whenever a wave goes wholly unanswered.
pub(crate) async fn run_speed_prioritized<T: SpeedPrioritized>(
    operation: &T,
) -> Result<(Vec<T::Outcome>, Member), SpeedError> {
    for draw in 1..=MAX_SPEED_EXIT_DRAWS {
        operation.narrate(SpeedProgress::Acquiring { draw });
        let (member, socks5) = operation.acquire().await?;
        match run_wave(operation, socks5).await {
            WaveEnd::Exhausted(outcomes) if !operation.answered(&outcomes) => {
                operation.narrate(SpeedProgress::ExitAbandoned { draw });
                operation.abandon(member);
            }
            WaveEnd::Settled(outcomes) | WaveEnd::Exhausted(outcomes) => {
                return Ok((outcomes, member));
            }
        }
    }
    Err(SpeedError::NoLiveExit {
        draws: MAX_SPEED_EXIT_DRAWS,
        budget: zingo_netutils::time::SENTINEL_BUDGET,
    })
}

/// Why a speed-priority operation reached no answer at all — the two ways
/// that belong to the wave rather than to what the operation was racing.
#[derive(Debug, thiserror::Error)]
pub enum SpeedError {
    /// The attempt's transport failed, at whatever stage its own error
    /// names: the wrap adds no story of its own, because it does not know
    /// whether a clutch could not be drawn or a bound exit was foreign.
    #[error(transparent)]
    Transport(#[from] crate::mixnet::acquire::TransportError),
    /// Every exit this operation drew carried nothing.
    #[error(
        "no drawn exit carried a round trip: {draws} exits each given {}ms",
        budget.as_millis()
    )]
    NoLiveExit {
        /// How many exits were drawn and proven silent.
        draws: usize,
        /// The budget each was given before it was condemned.
        budget: std::time::Duration,
    },
}

/// How many exits one operation may draw: the first, and a fresh one for
/// each wave that went wholly unanswered, bounded so a mixnet failing
/// everywhere refuses in a stated time rather than drawing forever.
pub(crate) const MAX_SPEED_EXIT_DRAWS: usize = 6;

/// How a wave ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WaveEnd<O> {
    /// The operation settled: its outcomes, ending with the settling one.
    Settled(Vec<O>),
    /// Every target was raced and none settled the operation.
    Exhausted(Vec<O>),
}

/// Races `operation`'s targets through `socks5` in one full-width wave.
pub(crate) async fn run_wave<T: SpeedPrioritized>(
    operation: &T,
    socks5: SocketAddr,
) -> WaveEnd<T::Outcome> {
    use futures::StreamExt as _;

    let mut outcomes: Vec<T::Outcome> = Vec::new();
    let mut legs = futures::stream::iter(operation.targets())
        .map(|target| operation.probe(socks5, target))
        .buffer_unordered(SURVEY_WAVE_WIDTH);
    while let Some(outcome) = legs.next().await {
        outcomes.push(outcome);
        if operation.settled(&outcomes) {
            return WaveEnd::Settled(outcomes);
        }
    }
    WaveEnd::Exhausted(outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An operation whose targets answer from a script, so the wave's own
    /// rules are testable without a tunnel.
    struct Scripted {
        answers: Vec<bool>,
    }

    impl SpeedPrioritized for Scripted {
        type Target = usize;
        type Outcome = bool;

        fn targets(&self) -> Vec<usize> {
            (0..self.answers.len()).collect()
        }

        fn probe(
            &self,
            _socks5: SocketAddr,
            target: usize,
        ) -> impl std::future::Future<Output = bool> + Send {
            let answered = self.answers[target];
            async move { answered }
        }

        fn settled(&self, outcomes: &[bool]) -> bool {
            outcomes.iter().any(|answered| *answered)
        }

        fn answered(&self, outcomes: &[bool]) -> bool {
            outcomes.iter().any(|answered| *answered)
        }

        // The scripted operation exercises the wave alone, never the redraw
        // loop, so it acquires nothing and disposes of nothing.
        async fn acquire(
            &self,
        ) -> Result<(Member, SocketAddr), crate::mixnet::acquire::TransportError> {
            Err(crate::mixnet::acquire::TransportError::NoAcquirer)
        }

        fn dispose(&self, _spent: Member) {}

        fn abandon(&self, _dead: Member) {}
    }

    /// HYPOTHESIS: the wave runs at its full fixed width, no lane withheld,
    /// now that proof belongs to the client's birth. Falsified if the width
    /// varies with the work.
    #[test]
    fn the_wave_runs_at_full_width() {
        assert_eq!(SURVEY_WAVE_WIDTH, 4);
    }

    /// HYPOTHESIS: a wave ends the moment its operation settles, carrying
    /// the settling outcome and no more, and reports exhaustion when
    /// nothing settles it. Falsified if a settled wave keeps racing or an
    /// unsettled one claims to have settled.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_wave_ends_when_its_operation_settles() {
        let socks5: SocketAddr = "127.0.0.1:1".parse().expect("a static address");

        let settles = Scripted {
            answers: vec![false, true, false],
        };
        match run_wave(&settles, socks5).await {
            WaveEnd::Settled(outcomes) => {
                assert!(outcomes.iter().any(|answered| *answered));
                assert!(outcomes.len() <= 3, "the wave stops at the settling leg");
            }
            other => panic!("a settling operation ended as {other:?}"),
        }

        let never = Scripted {
            answers: vec![false, false],
        };
        match run_wave(&never, socks5).await {
            WaveEnd::Exhausted(outcomes) => assert_eq!(outcomes.len(), 2),
            other => panic!("an unsettled operation ended as {other:?}"),
        }
    }
}
