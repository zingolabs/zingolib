use super::*;

#[cfg(feature = "nym")]
#[test]
fn the_route_resolver_fails_closed_in_every_state() {
    use crate::mixnet::{MixnetNotReady, MixnetRoute};

    let socks5 = crate::mocks::transmission::MOCK_SOCKS5_ADDR;
    let ready = || {
        Ok(MixnetRoute::Mixnet(crate::mixnet::MixnetConduit::over(
            socks5,
        )))
    };
    let dial = resolve_transmit_route(true, ready())
        .expect("a ready conduit carries the send")
        .expect("the mixnet route dials the conduit");
    assert_eq!(dial.socks5(), socks5);
    assert!(matches!(
        resolve_transmit_route(false, ready()),
        Err(LightClientError::Offline)
    ));
    assert!(matches!(
        resolve_transmit_route(true, Ok(MixnetRoute::Nakednet)),
        Ok(None)
    ));
    assert!(matches!(
        resolve_transmit_route(false, Ok(MixnetRoute::Nakednet)),
        Err(LightClientError::Offline)
    ));
    assert!(matches!(
        resolve_transmit_route(false, Err(MixnetNotReady::Unattached)),
        Err(LightClientError::Offline)
    ));
    assert!(matches!(
        resolve_transmit_route(true, Err(MixnetNotReady::Unattached)),
        Err(LightClientError::MixnetNotReady(MixnetNotReady::Unattached))
    ));
    for has_indexer in [true, false] {
        for not_ready in [MixnetNotReady::Bootstrapping, MixnetNotReady::Died] {
            assert!(
                matches!(
                    resolve_transmit_route(has_indexer, Err(not_ready)),
                    Err(LightClientError::MixnetNotReady(refusal)) if refusal == not_ready
                ),
                "{not_ready:?} with an indexer: {has_indexer}"
            );
        }
    }
}

#[test]
fn each_wire_records_and_reports_its_own_transport() {
    let host = crate::destination::Host::of_host_str("node.example");
    assert_eq!(Wire::Nakednet.attempt_route(), AttemptRoute::Nakednet);
    assert!(!Wire::Nakednet.is_mixnet());
    assert_eq!(
        Wire::Nakednet.route_to(&host),
        TransmitRoute::Nakednet {
            destination: "node.example".to_string()
        }
    );
    #[cfg(feature = "nym")]
    {
        let shared_socks5 = crate::mocks::transmission::MOCK_SOCKS5_ADDR;
        let mixnet = Wire::Mixnet { shared_socks5 };
        assert_eq!(mixnet.attempt_route(), AttemptRoute::Mixnet);
        assert!(mixnet.is_mixnet());
        let mock = Wire::MixnetOverMock { shared_socks5 };
        assert_eq!(mock.attempt_route(), AttemptRoute::Mixnet);
        for wire in [mixnet, mock] {
            assert_eq!(
                wire.route_to(&host),
                TransmitRoute::Mixnet {
                    destination: "node.example".to_string(),
                    via_socks5: shared_socks5.to_string(),
                }
            );
        }
    }
}

/// HYPOTHESIS: a send attempt's failure reaches the history as the typed
/// taxonomy record, classified whole rather than from hand-rendered
/// prose. Falsified if the recorded category drifts.
#[test]
fn send_attempt_failure_is_classified_from_the_record() {
    let history = IndexerHistoryHandle::default();
    let failure = zl_netutils::diag::NetOpFailure::message(
        zl_netutils::diag::NetOpStage::RemoteConnect,
        "indexer.example",
        "connection refused",
    );
    record_send_attempt(
        &history,
        &crate::destination::Host::of_host_str("indexer.example"),
        AttemptRoute::Nakednet,
        std::time::Instant::now(),
        &Err(failure),
        None,
    );
    let recorded = history.load();
    assert_eq!(recorded.len(), 1, "one attempt is recorded");
    assert_eq!(
        recorded[0].outcome,
        Err(crate::lightclient::indexer_history::FailureKind::Unreachable),
        "the category comes from the typed record"
    );
}
