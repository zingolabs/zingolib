use super::*;

/// The client-side bound every fabricated case shares, so a timeout
/// verdict names a value the case can assert.
const BOUND: Duration = Duration::from_millis(300);

/// The local SOCKS5 address the proxied cases name in their chains.
const PROXY: &str = "127.0.0.1:43210";

/// The cause chain as the classifier reads it, one text per layer.
fn chain(layers: &[&str]) -> Vec<String> {
    layers.iter().map(|layer| (*layer).to_string()).collect()
}

/// The signals for an error the transport reported as connect-related.
fn connect() -> RequestSignals {
    RequestSignals {
        is_connect: true,
        ..Default::default()
    }
}

/// One fabricated input and the stage the table must reach from it.
struct Case {
    why: &'static str,
    signals: RequestSignals,
    chain: Vec<String>,
    proxy: Option<&'static str>,
    expected: NetOpStage,
}

/// Every stage this classifier can reach is reached by one fabricated
/// input, so a new arm must earn a case here before it can hide.
#[test]
fn every_reachable_stage_has_a_fabricated_input() {
    let cases = vec![
        Case {
            why: "an elapsed bound outranks every other signal",
            signals: RequestSignals {
                is_timeout: true,
                is_connect: true,
                ..Default::default()
            },
            chain: chain(&["operation timed out"]),
            proxy: Some(PROXY),
            expected: NetOpStage::TimedOut { after_ms: 300 },
        },
        Case {
            why: "a connect failure naming the local proxy is the proxy's",
            signals: connect(),
            chain: chain(&[&format!("tcp connect error to {PROXY}")]),
            proxy: Some(PROXY),
            expected: NetOpStage::LocalProxyConnect,
        },
        Case {
            why: "a chain naming socks negotiation is the tunnel's handshake",
            signals: RequestSignals::default(),
            chain: chain(&["socks connect error: host unreachable"]),
            proxy: None,
            expected: NetOpStage::SocksHandshake,
        },
        Case {
            why: "a certificate rejection reached the target",
            signals: RequestSignals::default(),
            chain: chain(&["invalid peer certificate: Expired"]),
            proxy: None,
            expected: NetOpStage::RemoteTls,
        },
        Case {
            why: "a mid-handshake eof broke underneath the handshake",
            signals: RequestSignals::default(),
            chain: chain(&["tls handshake eof"]),
            proxy: None,
            expected: NetOpStage::TunnelTransport,
        },
        Case {
            why: "a status the server chose is the server's",
            signals: RequestSignals {
                is_status: true,
                ..Default::default()
            },
            chain: chain(&["HTTP status server error (502) for url"]),
            proxy: None,
            expected: NetOpStage::RemoteHttp,
        },
        Case {
            why: "a body that arrived and would not decode is the payload's",
            signals: RequestSignals {
                is_decode_or_body: true,
                ..Default::default()
            },
            chain: chain(&["error decoding response body"]),
            proxy: None,
            expected: NetOpStage::PayloadDecode,
        },
        Case {
            why: "a connect failure naming no proxy is the remote's",
            signals: connect(),
            chain: chain(&["tcp connect error: connection refused"]),
            proxy: None,
            expected: NetOpStage::RemoteConnect,
        },
        Case {
            why: "an unrevealing failure is the tunnel's",
            signals: RequestSignals::default(),
            chain: chain(&["connection reset by peer"]),
            proxy: None,
            expected: NetOpStage::TunnelTransport,
        },
    ];

    let mut reached = Vec::new();
    for case in cases {
        let verdict = classify_stage(case.signals, &case.chain, case.proxy, BOUND);
        assert_eq!(verdict, case.expected, "{}", case.why);
        reached.push(verdict);
    }

    for stage in [
        NetOpStage::TimedOut { after_ms: 300 },
        NetOpStage::LocalProxyConnect,
        NetOpStage::SocksHandshake,
        NetOpStage::RemoteTls,
        NetOpStage::RemoteHttp,
        NetOpStage::PayloadDecode,
        NetOpStage::RemoteConnect,
        NetOpStage::TunnelTransport,
    ] {
        assert!(
            reached.contains(&stage),
            "no fabricated input reaches {stage}"
        );
    }
}

/// The TLS arm fires on chain text alone, where the design's table at
/// `docs/agents/net-diag-design.md` conjoins it with `is_connect()`.
#[test]
fn the_tls_arm_needs_no_connect_signal() {
    assert_eq!(
        classify_stage(
            RequestSignals::default(),
            &chain(&["invalid peer certificate: UnknownIssuer"]),
            None,
            BOUND,
        ),
        NetOpStage::RemoteTls,
        "chain text alone reaches the TLS arm"
    );
}

/// The TLS arm outranks `is_status()`, so a status error whose chain
/// happens to name a certificate never reaches `RemoteHttp`.
#[test]
fn a_status_error_naming_a_certificate_classifies_as_tls() {
    assert_eq!(
        classify_stage(
            RequestSignals {
                is_status: true,
                ..Default::default()
            },
            &chain(&["HTTP status client error (400)", "certificate required"]),
            None,
            BOUND,
        ),
        NetOpStage::RemoteTls,
        "the earlier arm wins the error the later arm was written for"
    );
}
