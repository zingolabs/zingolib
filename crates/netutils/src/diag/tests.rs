use super::*;

#[test]
fn stage_rendering_is_the_pinned_kebab_case_contract() {
    let table = [
        (NetOpStage::ProxyLaunch, "proxy-launch"),
        (NetOpStage::RouteResolution, "route-resolution"),
        (NetOpStage::RemoteConnect, "remote-connect"),
        (NetOpStage::LocalProxyConnect, "local-proxy-connect"),
        (NetOpStage::SocksHandshake, "socks-handshake"),
        (NetOpStage::TunnelTransport, "tunnel-transport"),
        (NetOpStage::RemoteTls, "remote-tls"),
        (NetOpStage::RemoteHttp, "remote-http"),
        (NetOpStage::PayloadDecode, "payload-decode"),
        (
            NetOpStage::TimedOut { after_ms: 25000 },
            "timed-out(25000ms)",
        ),
    ];
    for (stage, rendered) in table {
        assert_eq!(stage.to_string(), rendered);
    }
}

#[test]
fn failure_rendering_is_the_pinned_shape() {
    let failure = NetOpFailure {
        stage: NetOpStage::RemoteTls,
        target: "zec.rocks:443".to_string(),
        cause_chain: vec!["transport error".to_string(), "handshake eof".to_string()],
    };
    assert_eq!(
        failure.to_string(),
        "failed at remote-tls to zec.rocks:443: transport error: handshake eof"
    );
}

/// A three-layer fabricated chain captures one text per layer, outermost
/// first, with no layer's text repeated — the property the mobile
/// `source()` walk depends on.
#[test]
fn from_error_captures_every_layer_once() {
    #[derive(Debug)]
    struct Layer {
        text: &'static str,
        below: Option<Box<Layer>>,
    }
    impl fmt::Display for Layer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.text)
        }
    }
    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.below
                .as_deref()
                .map(|layer| layer as &(dyn std::error::Error + 'static))
        }
    }

    let chain = Layer {
        text: "request failed",
        below: Some(Box::new(Layer {
            text: "connect failed",
            below: Some(Box::new(Layer {
                text: "connection refused",
                below: None,
            })),
        })),
    };

    let failure = NetOpFailure::from_error(NetOpStage::RemoteConnect, "zec.rocks:443", &chain);
    assert_eq!(
        failure.cause_chain,
        vec!["request failed", "connect failed", "connection refused"]
    );
    let distinct: std::collections::HashSet<_> = failure.cause_chain.iter().collect();
    assert_eq!(
        distinct.len(),
        failure.cause_chain.len(),
        "no layer text repeats"
    );
}
