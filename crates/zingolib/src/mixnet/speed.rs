//! Runs the requests of a speed-priority operation as one batch through
//! one Proven Client. If no target responds, it draws a new client.

#![forbid(unsafe_code)]

use std::net::SocketAddr;

/// The maximum number of requests in flight at one time. With more than
/// four connections through one Nym client, all requests exceed their time
/// budget together.
pub const MAX_CONCURRENT_REQUESTS: usize = 4;

/// An operation that sends requests to several targets through one Proven
/// Client.
pub(crate) trait SpeedPrioritized {
    /// The destination of one request.
    type Target: Send;

    /// The result of one request.
    type Response: Send;

    /// The label for log lines.
    fn log_label(&self) -> &'static str;

    /// The targets, in request order.
    fn targets(&self) -> Vec<Self::Target>;

    /// Sends one request to `target` through `dial`.
    fn request(
        &self,
        dial: zl_netutils::conduit::ConduitDial,
        target: Self::Target,
    ) -> impl std::future::Future<Output = Self::Response> + Send;

    /// Returns true if the responses complete the operation.
    fn is_complete(&self, responses: &[Self::Response]) -> bool;

    /// Returns true if at least one target responded.
    fn has_response(&self, responses: &[Self::Response]) -> bool;

    /// Acquires a Proven Client and its SOCKS5 address.
    fn acquire(
        &self,
    ) -> impl std::future::Future<
        Output = Result<(Member, SocketAddr), crate::mixnet::acquire::TransportError>,
    > + Send;

    /// Closes the client after a completed operation.
    fn release(&self, used: Member);

    /// Records the exit as failed, then closes the client.
    fn mark_exit_failed(&self, failed: Member);
}

/// The client of one batch.
pub(crate) type Member = crate::mixnet::pools::Member<crate::mixnet::MixnetProxy>;

/// Runs `operation` until it completes or every target got a request. If
/// no target in a batch responds, it draws a new client and repeats. One
/// deadline limits the whole run.
pub(crate) async fn run_speed_prioritized<T: SpeedPrioritized>(
    operation: &T,
) -> Result<(Vec<T::Response>, Member), SpeedError> {
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
        let conduit = zl_netutils::conduit::MixnetConduit::over(socks5);
        let Ok(batch) = tokio::time::timeout_at(deadline, run_batch(operation, &conduit)).await
        else {
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

/// The reason a speed-priority operation got no response.
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
    /// The deadline passed before a batch completed.
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

/// The maximum number of exits that one operation draws.
pub(crate) const MAX_SPEED_EXIT_DRAWS: usize = 6;

/// How a batch ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BatchEnd<R> {
    /// The operation completed.
    Completed(Vec<R>),
    /// Every target got a request and the operation did not complete.
    Exhausted(Vec<R>),
}

/// Sends the requests of `operation` through `conduit` as one batch.
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

    /// An operation whose targets respond from a script.
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

        async fn acquire(
            &self,
        ) -> Result<(Member, SocketAddr), crate::mixnet::acquire::TransportError> {
            Err(crate::mixnet::acquire::TransportError::NoAcquirer)
        }

        fn release(&self, _used: Member) {}

        fn mark_exit_failed(&self, _failed: Member) {}
    }

    /// HYPOTHESIS: the limit of requests in flight is four.
    #[test]
    fn a_batch_keeps_four_requests_in_flight() {
        assert_eq!(MAX_CONCURRENT_REQUESTS, 4);
    }

    /// HYPOTHESIS: a batch stops when its operation completes, and reports
    /// exhaustion when it never completes.
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
