//! The acquisition seam: where a mixnet transport comes from.
#![forbid(unsafe_code)]

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use zingo_netutils::responsiveness::ResponsivenessClass;

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
    #[error("could not run the discover mode: {0}")]
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
    /// The transport died during bootstrap.
    #[error(
        "the pool transport died during bootstrap: {}",
        detail.as_ref().map_or_else(|| "no detail reported".to_string(), ToString::to_string)
    )]
    DiedDuringBootstrap {
        /// The death detail the status channel carried, when it carried one.
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
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<crate::mixnet::ExitNodeId>, TransportError>> + Send + '_,
        >,
    >;

    /// Acquires one transport that races `clutch` under `class`.
    fn acquire<'a>(
        &'a self,
        class: ResponsivenessClass,
        clutch: &'a [crate::mixnet::ExitNodeId],
        publisher: StatusPublisher,
    ) -> Pin<Box<dyn Future<Output = Result<MixnetProxy, TransportError>> + Send + 'a>>;
}

/// One transport a mobile platform host started on the wallet's behalf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedTransport {
    /// The local SOCKS5 address the host's proxy listens on.
    pub socks5_addr: std::net::SocketAddr,
    /// The Exit Node that proxy bound.
    pub exit_node: crate::mixnet::ExitNodeId,
}

/// Why the mobile platform host declined or failed a request, with the host's own
/// detail carried verbatim as the payload.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HostRefusal {
    /// The host tried to satisfy the request and could not.
    #[error("the host failed: {detail}")]
    Failed {
        /// The host's own account of the failure.
        detail: String,
    },
    /// The host declined the request as a matter of mobile platform policy.
    #[error("the host declined: {detail}")]
    Declined {
        /// The host's own account of the refusal.
        detail: String,
    },
}

/// A mobile platform host that owns the mixnet proxy, for a mobile platform whose sandbox
/// forbids the wallet from spawning one.
pub trait ProxyHost: Send + Sync + 'static {
    /// The Exit Nodes the host's directory query reports.
    fn discover_exit_nodes(&self) -> Result<Vec<crate::mixnet::ExitNodeId>, HostRefusal>;

    /// Starts one proxy racing `clutch` under `class`, returning where it
    /// listens and which exit it bound.
    fn start_transport(
        &self,
        class: ResponsivenessClass,
        clutch: &[crate::mixnet::ExitNodeId],
    ) -> Result<HostedTransport, HostRefusal>;
}

/// The mobile acquirer: a mobile platform host that owns the proxy library.
pub(crate) struct HostedProxy {
    host: std::sync::Arc<dyn ProxyHost>,
}

impl HostedProxy {
    /// An acquirer that asks `host` for every transport.
    pub(crate) fn owned_by(host: std::sync::Arc<dyn ProxyHost>) -> Self {
        HostedProxy { host }
    }
}

impl TransportAcquirable for HostedProxy {
    fn discover(
        &self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<crate::mixnet::ExitNodeId>, TransportError>> + Send + '_,
        >,
    > {
        let host = std::sync::Arc::clone(&self.host);
        Box::pin(async move {
            // The host's directory query blocks, so it runs off the runtime's
            // worker threads.
            tokio::task::spawn_blocking(move || host.discover_exit_nodes())
                .await
                .map_err(|join| TransportError::HostUnavailable(join.to_string()))?
                .map_err(TransportError::HostRefused)
        })
    }

    fn acquire<'a>(
        &'a self,
        class: ResponsivenessClass,
        clutch: &'a [crate::mixnet::ExitNodeId],
        publisher: StatusPublisher,
    ) -> Pin<Box<dyn Future<Output = Result<MixnetProxy, TransportError>> + Send + 'a>> {
        let host = std::sync::Arc::clone(&self.host);
        let clutch = clutch.to_vec();
        Box::pin(async move {
            let hosted = tokio::task::spawn_blocking(move || host.start_transport(class, &clutch))
                .await
                .map_err(|join| TransportError::HostUnavailable(join.to_string()))?
                .map_err(TransportError::HostRefused)?;
            MixnetProxy::attach(hosted.socks5_addr, &[hosted.exit_node], publisher)
                .map_err(TransportError::from)
        })
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
    fn discover(
        &self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<crate::mixnet::ExitNodeId>, TransportError>> + Send + '_,
        >,
    > {
        Box::pin(crate::mixnet::supervisor::discover_exit_nodes(&self.path))
    }

    fn acquire<'a>(
        &'a self,
        class: ResponsivenessClass,
        clutch: &'a [crate::mixnet::ExitNodeId],
        publisher: StatusPublisher,
    ) -> Pin<Box<dyn Future<Output = Result<MixnetProxy, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            MixnetProxy::spawn(&self.path, class, publisher, clutch).map_err(TransportError::from)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host that answers both calls from a script, standing in for the
    /// mobile platform library a phone loads.
    struct ScriptedHost {
        directory: Result<Vec<crate::mixnet::ExitNodeId>, HostRefusal>,
        transport: Result<HostedTransport, HostRefusal>,
    }

    impl ProxyHost for ScriptedHost {
        fn discover_exit_nodes(&self) -> Result<Vec<crate::mixnet::ExitNodeId>, HostRefusal> {
            self.directory.clone()
        }

        fn start_transport(
            &self,
            _class: ResponsivenessClass,
            _clutch: &[crate::mixnet::ExitNodeId],
        ) -> Result<HostedTransport, HostRefusal> {
            self.transport.clone()
        }
    }

    fn hosted(host: ScriptedHost) -> HostedProxy {
        HostedProxy::owned_by(std::sync::Arc::new(host))
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
            vec![
                crate::mixnet::ExitNodeId::from("exit-a"),
                crate::mixnet::ExitNodeId::from("exit-b")
            ]
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
            .acquire(
                ResponsivenessClass::PrioritisePrivacy,
                &["exit-a".into()],
                crate::mixnet::status_publisher(),
            )
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
            .acquire(
                ResponsivenessClass::PrioritisePrivacy,
                &["exit-a".into()],
                crate::mixnet::status_publisher(),
            )
            .await
            .expect("a typed endpoint always constructs the attached transport");
        proxy.stop().await;
    }
}
