use super::*;

/// The number of times one detail may appear across a whole cause chain.
const DETAIL_RENDERINGS: usize = 1;

/// HYPOTHESIS: a discovery spawn failure states its own layer only, so
/// the underlying detail reaches the reader exactly once across the
/// sanctioned chain walk. Falsified if the detail appears more than
/// once, which is what a Display embedding its own source produces.
#[test]
fn a_discovery_spawn_failure_renders_its_detail_once() {
    const DETAIL: &str = "the discover binary is absent";
    let failure = TransportError::DiscoverySpawn(std::io::Error::other(DETAIL));
    let chain = zl_netutils::diag::chain_texts(&failure);
    assert_eq!(
        chain.join("\n").matches(DETAIL).count(),
        DETAIL_RENDERINGS,
        "the chain rendered was {chain:?}"
    );
}

/// A host that answers both calls from a script, standing in for the
/// mobile platform library a phone loads.
struct ScriptedHost {
    directory: Result<Vec<crate::mixnet::ExitNodeId>, HostRefusal>,
    transport: Result<HostedTransport, HostRefusal>,
}

impl ProxyHosting for ScriptedHost {
    fn discover_exit_nodes(&self) -> Result<Vec<crate::mixnet::ExitNodeId>, HostRefusal> {
        self.directory.clone()
    }

    fn start_transport(
        &self,
        _clutch: &[crate::mixnet::ExitNodeId],
    ) -> Result<HostedTransport, HostRefusal> {
        self.transport.clone()
    }

    /// A script rotates only when its test asks it to, so the default
    /// holds the client for the whole of any test's run.
    fn rotation_verdict(&self) -> zl_netutils::provider::RotationVerdict {
        zl_netutils::provider::RotationVerdict::Defer(zl_netutils::time::CLIENT_ROTATION_MAX)
    }
}

fn hosted(host: ScriptedHost) -> HostedAcquirer {
    HostedAcquirer {
        provider: HostedProvider::owned_by(host),
        indexer: crate::mixnet::test_indexer(),
    }
}

/// HYPOTHESIS: a host's directory answer seeds the Exit Pool exactly as
/// the spawned binary's discover mode does.
#[tokio::test]
async fn a_host_directory_answers_discovery() {
    let acquirer = hosted(ScriptedHost {
        directory: Ok(vec!["exit-a".into(), "exit-b".into()]),
        transport: Err(HostRefusal::Declined {
            detail: "unused".to_string(),
        }),
    });
    assert_eq!(
        acquirer.discover().await.expect("the host answers"),
        HashSet::from([
            crate::mixnet::ExitNodeId::from("exit-a"),
            crate::mixnet::ExitNodeId::from("exit-b")
        ])
    );
}

/// The identities a duplicating host names, counted once each.
const DISTINCT_DISCOVERED: usize = 2;

/// HYPOTHESIS: a host naming one identity twice discovers that identity
/// once, so the discovery edge carries a population rather than the
/// host's sequence. Falsified if the repeat reaches the caller twice.
#[tokio::test]
async fn a_duplicated_host_directory_discovers_once() {
    let acquirer = hosted(ScriptedHost {
        directory: Ok(vec!["exit-a".into(), "exit-b".into(), "exit-a".into()]),
        transport: Err(HostRefusal::Declined {
            detail: "unused".to_string(),
        }),
    });
    assert_eq!(
        acquirer.discover().await.expect("the host answers").len(),
        DISTINCT_DISCOVERED,
        "the repeated identity is one Exit Node"
    );
}

/// HYPOTHESIS: a refusing host surfaces as a typed host refusal, never
/// as a spawn failure the mobile platform could not have produced.
#[tokio::test]
async fn a_refusing_host_refuses_typed() {
    let acquirer = hosted(ScriptedHost {
        directory: Err(HostRefusal::Declined {
            detail: "no directory on this mobile platform".to_string(),
        }),
        transport: Err(HostRefusal::Declined {
            detail: "the app declined".to_string(),
        }),
    });
    assert!(matches!(
        acquirer.discover().await.expect_err("the host refuses"),
        TransportError::HostRefused(_)
    ));
    let Err(refusal) = acquirer
        .acquire(&["exit-a".into()], crate::mixnet::status_publisher())
        .await
    else {
        panic!("a declining host must not yield a transport");
    };
    assert!(matches!(refusal, TransportError::HostRefused(_)));
}

/// HYPOTHESIS: a well-typed host report attaches, so the typed
/// `HostedTransport` is sufficient evidence to reach the slot seam.
#[tokio::test]
async fn a_typed_host_endpoint_reaches_the_attach_seam() {
    let acquirer = hosted(ScriptedHost {
        directory: Ok(Vec::new()),
        transport: Ok(HostedTransport {
            socks5_addr: "127.0.0.1:1080".parse().expect("the test address parses"),
            exit_node: "exit-a".into(),
        }),
    });
    let proxy = acquirer
        .acquire(&["exit-a".into()], crate::mixnet::status_publisher())
        .await
        .expect("a typed endpoint always constructs the attached transport");
    proxy.stop().await;
}
