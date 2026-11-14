use super::*;

#[test]
fn transport_from_conversion() {
    // Verify the From impl exists at compile time.
    let _: fn(tonic::transport::Error) -> GetClientError = GetClientError::from;
}

/// The prose account of an exhausted race is a rendering of the typed
/// records, pinned here. The records are the storage form; a consumer
/// dispatches on their fields, never on this sentence.
#[cfg(feature = "nym")]
#[test]
fn attempts_exhausted_renders_each_typed_attempt() {
    use crate::diag::{NetOpFailure, NetOpStage};

    let error = NymProxyError::AttemptsExhausted {
        attempts: 2,
        failures: vec![
            NetOpFailure::message(NetOpStage::RemoteConnect, "Emq7Gc3PLdp…", "gateway refused"),
            NetOpFailure::message(
                NetOpStage::TimedOut { after_ms: 20_000 },
                "9f2kQvR8sWx…",
                "exit node connect attempt timed out after 20s",
            ),
        ],
    };
    assert_eq!(
        error.to_string(),
        "no exit node connected after contacting 2 exit nodes: \
             failed at remote-connect to Emq7Gc3PLdp…: gateway refused; \
             failed at timed-out(20000ms) to 9f2kQvR8sWx…: \
             exit node connect attempt timed out after 20s"
    );
}
