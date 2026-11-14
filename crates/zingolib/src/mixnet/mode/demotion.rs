use super::{Indicator, MixnetSlot, StandingClient};

fn attached(born_probed: bool) -> MixnetSlot {
    let proxy = crate::mixnet::MixnetProxy::ready_for_slot_tests(
        "127.0.0.1:1080".parse().expect("the test address parses"),
        vec![crate::mixnet::ExitNodeId::from("exit-under-trial")],
    );
    MixnetSlot::Attached(StandingClient::new(proxy, None, born_probed))
}

/// HYPOTHESIS: convicting the exit under a live client dips the mode to
/// Bootstrapping — the ruled state while the failover's replacement
/// births — and never to Died, which names only the transport's own
/// loss. Falsified if a condemned client still claims readiness.
#[tokio::test]
async fn a_condemned_client_dips_to_bootstrapping() {
    let slot = attached(true);
    assert_eq!(slot.mode(), Indicator::Ready);
    if let MixnetSlot::Attached(client) = &slot {
        assert!(client.condemn(), "the first conviction reports itself");
        assert!(!client.condemn(), "a second conviction is idempotent");
    }
    assert_eq!(
        slot.mode(),
        Indicator::Bootstrapping,
        "a convicted exit means the session cannot truthfully claim readiness"
    );
}

/// HYPOTHESIS: a failover that exhausts every birth latches Died — the
/// unconsented loss — which only an explicit re-enable leaves.
/// Falsified if exhaustion reads as anything softer.
#[tokio::test]
async fn an_exhausted_failover_latches_died() {
    let slot = attached(true);
    if let MixnetSlot::Attached(client) = &slot {
        client.condemn();
        client.forsake(zl_netutils::diag::NetOpFailure::message(
            zl_netutils::diag::NetOpStage::RouteResolution,
            "the session's exit census",
            "every failover birth failed its proof",
        ));
    }
    assert_eq!(slot.mode(), Indicator::Died);
    assert!(
        slot.death_report()
            .is_some_and(|death| death.detail.is_some()),
        "the latched death holds its typed story"
    );
}
