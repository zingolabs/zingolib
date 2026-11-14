//! Finding F2's contract: a bind-stage failure spends a birth and
//! convicts the drawn exits instead of escaping the six-birth loop.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// An acquirer whose every acquisition fails at the bind stage, with a
/// census of exactly one exit per birth, so every drawn exit is
/// distinct and every one of them is convicted.
struct BindRefusingAcquirer {
    census: usize,
    acquisitions: AtomicUsize,
}

impl crate::mixnet::acquire::TransportAcquirable for BindRefusingAcquirer {
    async fn discover(
        &self,
    ) -> Result<std::collections::HashSet<crate::mixnet::ExitNodeId>, acquire::TransportError> {
        Ok((0..self.census)
            .map(|index| crate::mixnet::ExitNodeId::from(format!("exit-{index}").as_str()))
            .collect())
    }

    async fn acquire(
        &self,
        _clutch: &[crate::mixnet::ExitNodeId],
        _publisher: crate::mixnet::driver::StatusPublisher,
    ) -> Result<crate::mixnet::MixnetProxy, acquire::TransportError> {
        self.acquisitions.fetch_add(1, Ordering::AcqRel);
        Err(acquire::TransportError::NotReady {
            budget: zl_netutils::time::NYM_LIFECYCLE_TIMEOUT,
        })
    }

    fn rotation_verdict(&self) -> zl_netutils::provider::RotationVerdict {
        zl_netutils::provider::RotationVerdict::Never
    }
}

/// HYPOTHESIS: a bind-stage failure spends one of the six births and
/// convicts every drawn exit, so the loop absorbs it and the final
/// refusal is the typed NoProvenExit — never the first bind error
/// escaping with nothing learned. Falsified if the loop exits early or
/// the drawn-and-dead exits stay eligible.
#[tokio::test]
async fn a_bind_failure_spends_a_birth_and_convicts_the_drawn() {
    let pools = Pools::new();
    let acquirer = BindRefusingAcquirer {
        census: MAX_PROVING_BIRTHS,
        acquisitions: AtomicUsize::new(0),
    };
    let Err(refusal) = pools.acquire_proven(&acquirer).await else {
        panic!("every bind fails, so no birth can succeed");
    };

    assert!(
        matches!(
            refusal,
            acquire::TransportError::NoProvenExit { probed, .. }
                if probed == MAX_PROVING_BIRTHS
        ),
        "the loop absorbs bind failures into its budget, got: {refusal}"
    );
    assert_eq!(
        acquirer.acquisitions.load(Ordering::Acquire),
        MAX_PROVING_BIRTHS,
        "every birth was spent on an acquisition"
    );
    let convicted = {
        let exits = pools.exits.lock().expect("exit pool mutex");
        let now = Instant::now();
        (0..acquirer.census)
            .filter(|index| {
                exits.epoch_failed(
                    &crate::mixnet::ExitNodeId::from(format!("exit-{index}").as_str()),
                    now,
                )
            })
            .count()
    };
    assert_eq!(
        convicted, acquirer.census,
        "every drawn-and-dead exit is convicted rather than staying eligible"
    );
}
