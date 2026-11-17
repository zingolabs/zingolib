//! The one batch of concurrent requests every speed-priority operation
//! runs.
//!
//! The price run uses a Proven Client and sends one fixed-size batch
//! through its Exit Node. Proof belongs to the client's birth, so the batch
//! sends no Sentinel; the redraw survives as the safety net for an exit
//! dying inside its epoch, keyed on a batch where no target responded.

#![forbid(unsafe_code)]

use std::net::SocketAddr;

/// How many requests a speed-priority operation has in flight at once:
/// four connections through the one Nym client, which is the widest
/// fan-out that kept each near its solo cost instead of blowing every
/// request's budget together — the 0-of-17 signature — and which counts
/// connections rather than processes, so one calibration serves the
/// spawned desktop binary and the client a mobile host runs in its own
/// process.
pub const MAX_CONCURRENT_REQUESTS: usize = 4;

/// An operation that sends requests to its targets through one Proven
/// Client in one batch, redrawing when a batch shows the exit died.
pub(crate) trait SpeedPrioritized {
    /// What this operation sends requests to: an indexer's endpoint, a
    /// price source.
    type Target: Send;

    /// What one target returns, whether it responded or failed.
    type Response: Send;

    /// The label the runner logs this operation under.
    fn log_label(&self) -> &'static str;

    /// The targets of this operation, in the order the batch takes them.
    fn targets(&self) -> Vec<Self::Target>;

    /// Sends one request to `target` through `dial`'s conduit, keeping the
    /// guard for the request so the conduit counts it as outstanding.
    fn request(
        &self,
        dial: zl_netutils::conduit::ConduitDial,
        target: Self::Target,
    ) -> impl std::future::Future<Output = Self::Response> + Send;

    /// Whether the responses so far complete this operation, which ends
    /// the batch with every remaining request cancelled.
    fn is_complete(&self, responses: &[Self::Response]) -> bool;

    /// Whether any target responded at all, the evidence separating a live
    /// exit over unhealthy targets from an exit that died during the batch.
    fn has_response(&self, responses: &[Self::Response]) -> bool;

    /// Acquires a Proven Client for one attempt, with the address it
    /// listens on.
    fn acquire(
        &self,
    ) -> impl std::future::Future<
        Output = Result<(Member, SocketAddr), crate::mixnet::acquire::TransportError>,
    > + Send;

    /// Releases a transport the operation is done with, in the background:
    /// the response is already in hand, and teardown is no reason to keep a
    /// caller waiting.
    fn release(&self, used: Member);

    /// Records the failure of a transport whose exit died during the
    /// operation, then closes it, in the background.
    fn mark_exit_failed(&self, failed: Member);
}

/// The transport every speed-priority operation uses: one Proven Client
/// whose exit serves the whole batch.
pub(crate) type Member = crate::mixnet::pools::Member<crate::mixnet::MixnetProxy>;

/// Runs `operation` until it completes or exhausts its targets, redrawing a
/// fresh Proven Client whenever no target in a batch responds.
pub(crate) async fn run_speed_prioritized<T: SpeedPrioritized>(
    operation: &T,
) -> Result<(Vec<T::Response>, Member), SpeedError> {
    // One shared deadline bounds the whole run — every redraw, birth, and
    // batch together — so the draw and birth budgets can no longer multiply
    // into an unbounded go-online wait.
    let deadline = tokio::time::Instant::now() + zl_netutils::time::SPEED_ACQUISITION_DEADLINE;
    let refusal = |draws: usize| SpeedError::DeadlineExhausted {
        draws,
        budget: zl_netutils::time::SPEED_ACQUISITION_DEADLINE,
    };
    for draw in 1..=MAX_SPEED_EXIT_DRAWS {
        tracing::info!(
            "{}: acquiring transport, draw {draw}",
            operation.log_label()
        );
        let Ok(acquired) = tokio::time::timeout_at(deadline, operation.acquire()).await else {
            return Err(refusal(draw - 1));
        };
        let (member, socks5) = acquired?;
        // One conduit per draw, so the count reflects the requests this
        // batch has in flight and nothing outside it.
        let conduit = zl_netutils::conduit::MixnetConduit::over(socks5);
        let Ok(batch) = tokio::time::timeout_at(deadline, run_batch(operation, &conduit)).await
        else {
            // The deadline, not the exit, ended this batch: the member
            // retires without a verdict, because nothing was proven.
            member.retire().await;
            return Err(refusal(draw));
        };
        match batch {
            BatchEnd::Exhausted(responses) if !operation.has_response(&responses) => {
                operation.mark_exit_failed(member);
            }
            BatchEnd::Completed(responses) | BatchEnd::Exhausted(responses) => {
                return Ok((responses, member));
            }
        }
    }
    Err(SpeedError::NoLiveExit {
        draws: MAX_SPEED_EXIT_DRAWS,
        budget: zl_netutils::time::SENTINEL_BUDGET,
    })
}

/// Why a speed-priority operation got no response at all — the ways that
/// belong to the batch rather than to the targets of the operation.
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
    /// The operation's one shared deadline elapsed before any batch
    /// completed.
    #[error(
        "the speed-prioritized operation exceeded its {}s acquisition deadline after {draws} draws",
        budget.as_secs()
    )]
    DeadlineExhausted {
        /// How many draws completed before the deadline elapsed.
        draws: usize,
        /// The whole operation's shared deadline.
        budget: std::time::Duration,
    },
}

/// How many exits one operation may draw: the first, and a fresh one for
/// each batch where no target responded, bounded so a mixnet failing
/// everywhere refuses in a stated time rather than drawing forever.
pub(crate) const MAX_SPEED_EXIT_DRAWS: usize = 6;

/// How a batch ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BatchEnd<R> {
    /// The operation completed: its responses, ending with the completing
    /// one.
    Completed(Vec<R>),
    /// Every target got a request and none completed the operation.
    Exhausted(Vec<R>),
}

/// Sends `operation`'s requests through `conduit` in one batch, at most
/// [`MAX_CONCURRENT_REQUESTS`] in flight.
pub(crate) async fn run_batch<T: SpeedPrioritized>(
    operation: &T,
    conduit: &zl_netutils::conduit::MixnetConduit,
) -> BatchEnd<T::Response> {
    use futures::StreamExt as _;

    let mut responses: Vec<T::Response> = Vec::new();
    let mut requests = futures::stream::iter(operation.targets())
        .map(|target| operation.request(conduit.dial(), target))
        .buffer_unordered(MAX_CONCURRENT_REQUESTS);
    while let Some(response) = requests.next().await {
        responses.push(response);
        if operation.is_complete(&responses) {
            return BatchEnd::Completed(responses);
        }
    }
    BatchEnd::Exhausted(responses)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An operation whose acquisition never completes, so only a shared
    /// deadline can end the run.
    struct HangingAcquire;

    impl SpeedPrioritized for HangingAcquire {
        type Target = usize;
        type Response = bool;

        fn log_label(&self) -> &'static str {
            "hanging acquire"
        }

        fn targets(&self) -> Vec<usize> {
            Vec::new()
        }

        fn request(
            &self,
            _dial: zl_netutils::conduit::ConduitDial,
            _target: usize,
        ) -> impl std::future::Future<Output = bool> + Send {
            std::future::ready(false)
        }

        fn is_complete(&self, _responses: &[bool]) -> bool {
            false
        }

        fn has_response(&self, _responses: &[bool]) -> bool {
            false
        }

        fn acquire(
            &self,
        ) -> impl std::future::Future<
            Output = Result<(Member, SocketAddr), crate::mixnet::acquire::TransportError>,
        > + Send {
            std::future::pending()
        }

        fn release(&self, _used: Member) {}

        fn mark_exit_failed(&self, _failed: Member) {}
    }

    /// HYPOTHESIS: one shared deadline bounds the whole run, so an
    /// acquisition that never completes refuses at the ruled ten-proof
    /// budget instead of holding the go-online moment open. Falsified if
    /// the run outlives the deadline.
    #[tokio::test(start_paused = true)]
    async fn the_shared_deadline_ends_a_hanging_acquisition() {
        let ended = tokio::time::timeout(
            zl_netutils::time::SPEED_ACQUISITION_DEADLINE + zl_netutils::time::SENTINEL_BUDGET,
            run_speed_prioritized(&HangingAcquire),
        )
        .await;
        let Ok(Err(refusal)) = ended else {
            panic!("the run must refuse at its deadline, not hang past it");
        };
        assert!(
            matches!(
                refusal,
                SpeedError::DeadlineExhausted { budget, .. }
                    if budget == zl_netutils::time::SPEED_ACQUISITION_DEADLINE
            ),
            "the refusal names the ruled deadline, got: {refusal}"
        );
    }

    /// An operation whose targets respond from a script, so the batch's own
    /// rules are testable without a tunnel.
    struct Scripted {
        responds: Vec<bool>,
    }

    impl SpeedPrioritized for Scripted {
        type Target = usize;
        type Response = bool;

        fn log_label(&self) -> &'static str {
            "scripted"
        }

        fn targets(&self) -> Vec<usize> {
            (0..self.responds.len()).collect()
        }

        fn request(
            &self,
            _dial: zl_netutils::conduit::ConduitDial,
            target: usize,
        ) -> impl std::future::Future<Output = bool> + Send {
            let responded = self.responds[target];
            async move { responded }
        }

        fn is_complete(&self, responses: &[bool]) -> bool {
            responses.iter().any(|responded| *responded)
        }

        fn has_response(&self, responses: &[bool]) -> bool {
            responses.iter().any(|responded| *responded)
        }

        // The scripted operation exercises the batch alone, never the redraw
        // loop, so it acquires nothing and releases nothing.
        async fn acquire(
            &self,
        ) -> Result<(Member, SocketAddr), crate::mixnet::acquire::TransportError> {
            Err(crate::mixnet::acquire::TransportError::NoAcquirer)
        }

        fn release(&self, _used: Member) {}

        fn mark_exit_failed(&self, _failed: Member) {}
    }

    /// HYPOTHESIS: a batch keeps the calibrated number of requests in
    /// flight, now that proof belongs to the client's birth. Falsified if
    /// the limit varies with the work.
    #[test]
    fn a_batch_keeps_four_requests_in_flight() {
        assert_eq!(MAX_CONCURRENT_REQUESTS, 4);
    }

    /// HYPOTHESIS: a batch ends the moment its operation completes,
    /// carrying the completing response and no more, and reports
    /// exhaustion when nothing completes it. Falsified if a completed
    /// batch keeps sending or an incomplete one claims to have completed.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_batch_ends_when_its_operation_completes() {
        let conduit = zl_netutils::conduit::MixnetConduit::over(
            "127.0.0.1:1".parse().expect("a static address"),
        );

        let completes = Scripted {
            responds: vec![false, true, false],
        };
        match run_batch(&completes, &conduit).await {
            BatchEnd::Completed(responses) => {
                assert!(responses.iter().any(|responded| *responded));
                assert!(
                    responses.len() <= 3,
                    "the batch stops at the completing request"
                );
            }
            other => panic!("a completing operation ended as {other:?}"),
        }

        let never = Scripted {
            responds: vec![false, false],
        };
        match run_batch(&never, &conduit).await {
            BatchEnd::Exhausted(responses) => assert_eq!(responses.len(), 2),
            other => panic!("an incomplete operation ended as {other:?}"),
        }
    }
}
