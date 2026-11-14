use super::*;

/// An operation whose acquisition never completes, so only a shared
/// deadline can end the run.
struct HangingAcquire;

impl SpeedPrioritized for HangingAcquire {
    type Target = usize;
    type Outcome = bool;

    fn targets(&self) -> Vec<usize> {
        Vec::new()
    }

    fn probe(
        &self,
        _dial: zl_netutils::conduit::ConduitDial,
        _target: usize,
    ) -> impl std::future::Future<Output = bool> + Send {
        std::future::ready(false)
    }

    fn settled(&self, _outcomes: &[bool]) -> bool {
        false
    }

    fn answered(&self, _outcomes: &[bool]) -> bool {
        false
    }

    fn acquire(
        &self,
    ) -> impl std::future::Future<
        Output = Result<(Member, SocketAddr), crate::mixnet::acquire::TransportError>,
    > + Send {
        std::future::pending()
    }

    fn dispose(&self, _spent: Member) {}

    fn abandon(&self, _dead: Member) {}

    fn narrate(&self, _phase: SpeedProgress) {}
}

/// HYPOTHESIS: one shared deadline bounds the whole run, so an
/// acquisition that never completes refuses at the ruled ten-proof
/// budget instead of holding the go-online moment open. Falsified if
/// the run outlives the deadline.
#[tokio::test(start_paused = true)]
async fn the_shared_deadline_ends_a_hanging_acquisition() {
    let outcome = tokio::time::timeout(
        zl_netutils::time::SPEED_ACQUISITION_DEADLINE + zl_netutils::time::SENTINEL_BUDGET,
        run_speed_prioritized(&HangingAcquire),
    )
    .await;
    let Ok(Err(refusal)) = outcome else {
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
        _dial: zl_netutils::conduit::ConduitDial,
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
    assert_eq!(WAVE_WIDTH, 4);
}

/// HYPOTHESIS: a wave ends the moment its operation settles, carrying
/// the settling outcome and no more, and reports exhaustion when
/// nothing settles it. Falsified if a settled wave keeps racing or an
/// unsettled one claims to have settled.
#[tokio::test(flavor = "multi_thread")]
async fn a_wave_ends_when_its_operation_settles() {
    let conduit =
        zl_netutils::conduit::MixnetConduit::over("127.0.0.1:1".parse().expect("a static address"));

    let settles = Scripted {
        answers: vec![false, true, false],
    };
    match run_wave(&settles, &conduit).await {
        WaveEnd::Settled(outcomes) => {
            assert!(outcomes.iter().any(|answered| *answered));
            assert!(outcomes.len() <= 3, "the wave stops at the settling leg");
        }
        other => panic!("a settling operation ended as {other:?}"),
    }

    let never = Scripted {
        answers: vec![false, false],
    };
    match run_wave(&never, &conduit).await {
        WaveEnd::Exhausted(outcomes) => assert_eq!(outcomes.len(), 2),
        other => panic!("an unsettled operation ended as {other:?}"),
    }
}
