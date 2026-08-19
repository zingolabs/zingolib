//! The acquisition seam: where a mixnet transport comes from.
#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;

use crate::correspondent::pool::exit_pool::ExitPoolError;
use crate::mixnet::driver::StatusPublisher;
use crate::mixnet::supervisor::{MixnetProxy, MixnetProxyError};

/// Why the session could acquire no ready transport.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The session has no acquirer to draw transports from.
    #[error("this session acquires no transports")]
    NoAcquirer,
    /// The proxy binary's discover mode could not be run.
    #[error("could not run the discover mode")]
    DiscoverySpawn(#[source] std::io::Error),
    /// The proxy binary's discover mode ran and reported failure.
    #[error("the discover mode exited {status}: {stderr}")]
    DiscoveryFailed {
        /// The discover child's exit status.
        status: std::process::ExitStatus,
        /// The discover child's trimmed stderr.
        stderr: String,
    },
    /// The Exit Pool has not yet learned the exit population.
    #[error("the exit pool has no population yet")]
    ExitPoolNotSeeded,
    /// Every Exit Node Reservation is issued to some holder.
    #[error("every exit is held: {held} held, {population} known")]
    ExitPoolExhausted {
        /// Reservations currently issued to some holder.
        held: usize,
        /// The whole discovered population.
        population: usize,
    },
    /// The transport process could not be created.
    #[error(transparent)]
    Proxy(#[from] MixnetProxyError),
    /// The mobile platform host could not be reached to serve the acquisition.
    #[error("the proxy host did not answer: {0}")]
    HostUnavailable(String),
    /// The mobile platform host answered and refused the acquisition.
    #[error("the proxy host refused")]
    HostRefused(#[source] HostRefusal),
    /// The ready transport reported no bound exit from the drawn Clutch, a
    /// defective report the wallet refuses instead of trusting.
    #[error(
        "the ready transport reported no bound exit from the drawn clutch ({} reported)",
        reported.len()
    )]
    ExitOutsideClutch {
        /// The exit identities the transport reported as bound.
        reported: Vec<crate::mixnet::ExitNodeId>,
    },
    /// The transport died during bootstrap.
    #[error("the transport died before it became ready")]
    DiedDuringBootstrap {
        /// The death detail the status channel carried, when it carried one,
        /// kept as a source so the typed failure reaches a caller whole
        /// rather than flattened into this message.
        #[source]
        detail: Option<zingo_net_diag::NetOpFailure>,
    },
    /// The transport's status channel closed before readiness.
    #[error("the pool transport's status channel closed")]
    StatusChannelClosed,
    /// A pooled transport that had reached readiness died before its
    /// consumer could use it.
    #[error("the pooled transport died before its run could use it")]
    DiedBeforeUse,
    /// The transport missed the readiness budget.
    #[error("the pool transport did not become ready within {}s", budget.as_secs())]
    NotReady {
        /// The lifecycle budget the transport missed.
        budget: std::time::Duration,
    },
    /// Every birth this acquisition attempted failed its proof.
    #[error(
        "no proven exit: {probed} births each proved nothing within {}ms",
        budget.as_millis()
    )]
    NoProvenExit {
        /// How many proving births were attempted.
        probed: usize,
        /// The Sentinel budget each birth's proof was given.
        budget: std::time::Duration,
    },
}

impl From<ExitPoolError> for TransportError {
    fn from(refusal: ExitPoolError) -> Self {
        match refusal {
            ExitPoolError::NotSeeded => TransportError::ExitPoolNotSeeded,
            ExitPoolError::Exhausted { held, population } => {
                TransportError::ExitPoolExhausted { held, population }
            }
        }
    }
}

/// Something a mixnet transport is acquirable from: the bundled binary a
/// desktop spawns, or the host that owns the proxy where a mobile platform
/// forbids subprocesses.
pub(crate) trait TransportAcquirable: Send + Sync + 'static {
    /// The Exit Nodes this acquirer can reach, for seeding the Exit Pool.
    fn discover(
        &self,
    ) -> impl Future<Output = Result<HashSet<crate::mixnet::ExitNodeId>, TransportError>> + Send;

    /// Acquires one transport that races `clutch` under the one hedged
    /// launch policy.
    fn acquire(
        &self,
        clutch: &[crate::mixnet::ExitNodeId],
        publisher: StatusPublisher,
    ) -> impl Future<Output = Result<MixnetProxy, TransportError>> + Send;
}

/// What a session acquires transports from, over the known implementors.
pub(crate) enum Acquirer {
    /// The bundled binary a desktop spawns.
    Spawned(SpawnedBinary),
    /// The host that owns the proxy where a sandbox forbids subprocesses.
    Hosted(HostedProvider),
    /// A double that announces readiness itself, so a test can script the
    /// transition order the real readiness probe produces on its own clock.
    #[cfg(test)]
    Announcing(AnnouncingAcquirer),
}

/// An acquirer whose transport announces its lifecycle into the publisher it
/// is handed, exactly as a spawned child would.
#[cfg(test)]
pub(crate) struct AnnouncingAcquirer {
    /// Where the already-listening endpoint accepts.
    pub(crate) socks5_addr: std::net::SocketAddr,
    /// The census this acquirer's directory reports.
    pub(crate) census: Vec<crate::mixnet::ExitNodeId>,
    /// How many times the announcement yields, letting subscribers observe
    /// each transition before the next lands.
    pub(crate) yields: usize,
}

#[cfg(test)]
impl TransportAcquirable for AnnouncingAcquirer {
    async fn discover(&self) -> Result<HashSet<crate::mixnet::ExitNodeId>, TransportError> {
        Ok(self.census.iter().cloned().collect())
    }

    async fn acquire(
        &self,
        clutch: &[crate::mixnet::ExitNodeId],
        publisher: StatusPublisher,
    ) -> Result<MixnetProxy, TransportError> {
        let addr = self.socks5_addr;
        let clutch = clutch.to_vec();
        let proxy = MixnetProxy::attach(addr, &clutch, std::sync::Arc::clone(&publisher))?;
        // The candidate announces readiness where a child would: into the
        // publisher its acquisition was handed.
        publisher.send_replace(crate::mixnet::MixnetStatus {
            mode: crate::mixnet::Indicator::Ready,
            socks5_addr: Some(addr),
            exits: clutch,
            bootstrap_detail: None,
            death: None,
        });
        for _ in 0..self.yields {
            tokio::task::yield_now().await;
        }
        Ok(proxy)
    }
}

impl TransportAcquirable for Acquirer {
    async fn discover(&self) -> Result<HashSet<crate::mixnet::ExitNodeId>, TransportError> {
        match self {
            Acquirer::Spawned(spawned) => spawned.discover().await,
            Acquirer::Hosted(hosted) => hosted.discover().await,
            #[cfg(test)]
            Acquirer::Announcing(announcing) => announcing.discover().await,
        }
    }

    async fn acquire(
        &self,
        clutch: &[crate::mixnet::ExitNodeId],
        publisher: StatusPublisher,
    ) -> Result<MixnetProxy, TransportError> {
        match self {
            Acquirer::Spawned(spawned) => spawned.acquire(clutch, publisher).await,
            Acquirer::Hosted(hosted) => hosted.acquire(clutch, publisher).await,
            #[cfg(test)]
            Acquirer::Announcing(announcing) => announcing.acquire(clutch, publisher).await,
        }
    }
}

/// The provider seam, defined below the seam (ADR 0046).
pub use zingo_netutils::provider::{HostRefusal, HostedProvider, HostedTransport, ProxyHosting};

impl TransportAcquirable for HostedProvider {
    async fn discover(&self) -> Result<HashSet<crate::mixnet::ExitNodeId>, TransportError> {
        let provider = self.clone();
        // The host's directory query blocks, so it runs off the runtime's
        // worker threads; the provider itself names no runtime.
        let reported = tokio::task::spawn_blocking(move || provider.discover_exit_nodes())
            .await
            .map_err(|join| TransportError::HostUnavailable(join.to_string()))?
            .map_err(TransportError::HostRefused)?;
        Ok(reported.into_iter().collect())
    }

    async fn acquire(
        &self,
        clutch: &[crate::mixnet::ExitNodeId],
        publisher: StatusPublisher,
    ) -> Result<MixnetProxy, TransportError> {
        let provider = self.clone();
        let clutch = clutch.to_vec();
        let hosted = tokio::task::spawn_blocking(move || provider.start_transport(&clutch))
            .await
            .map_err(|join| TransportError::HostUnavailable(join.to_string()))?
            .map_err(TransportError::HostRefused)?;
        MixnetProxy::attach(hosted.socks5_addr, &[hosted.exit_node], publisher)
            .map_err(TransportError::from)
    }
}

/// The desktop acquirer: the bundled `nym-proxy` binary, spawned as a child.
pub(crate) struct SpawnedBinary {
    path: PathBuf,
}

impl SpawnedBinary {
    /// An acquirer that spawns the binary at `path`.
    pub(crate) fn at(path: PathBuf) -> Self {
        SpawnedBinary { path }
    }
}

impl TransportAcquirable for SpawnedBinary {
    async fn discover(&self) -> Result<HashSet<crate::mixnet::ExitNodeId>, TransportError> {
        crate::mixnet::supervisor::discover_exit_nodes(&self.path).await
    }

    async fn acquire(
        &self,
        clutch: &[crate::mixnet::ExitNodeId],
        publisher: StatusPublisher,
    ) -> Result<MixnetProxy, TransportError> {
        MixnetProxy::spawn(&self.path, publisher, clutch).map_err(TransportError::from)
    }
}

#[cfg(test)]
mod tests {
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
        let chain = zingo_net_diag::chain_texts(&failure);
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
    }

    fn hosted(host: ScriptedHost) -> HostedProvider {
        HostedProvider::owned_by(host)
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
}
