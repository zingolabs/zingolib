//! The acquisition seam: where a mixnet transport comes from.
#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;

use crate::mixnet::driver::StatusPublisher;
use crate::mixnet::pools::exit_pool::ExitPoolError;
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
        detail: Option<zl_netutils::diag::NetOpFailure>,
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

    /// Whether now is a moment to spend a rotation's bootstrap (ADR 0048).
    // The wallet cannot weigh this: the battery level, the foreground
    // state, and the radio are the inputs, and none are observable here.
    // A platform that holds its client indefinitely defers by a long
    // interval rather than refusing in a shape of its own.
    fn rotation_verdict(&self) -> RotationVerdict;
}

/// What a session acquires transports from, over the known implementors.
pub(crate) enum Acquirer {
    /// The bundled binary a desktop spawns.
    Spawned(SpawnedBinary),
    /// The host that owns the proxy where a sandbox forbids subprocesses.
    Hosted(HostedAcquirer),
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
    /// What this acquirer's platform answers about rotating now, so a test
    /// drives the hand-off without a clock.
    pub(crate) rotation: RotationVerdict,
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
        let proxy = MixnetProxy::attach(
            addr,
            &clutch,
            crate::mixnet::test_indexer(),
            std::sync::Arc::clone(&publisher),
        )?;
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

    fn rotation_verdict(&self) -> RotationVerdict {
        self.rotation
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

    fn rotation_verdict(&self) -> RotationVerdict {
        match self {
            Acquirer::Spawned(spawned) => spawned.rotation_verdict(),
            Acquirer::Hosted(hosted) => hosted.rotation_verdict(),
            #[cfg(test)]
            Acquirer::Announcing(announcing) => announcing.rotation_verdict(),
        }
    }
}

/// The provider seam, defined below the seam (ADR 0046).
pub use zl_netutils::provider::{
    HostRefusal, HostedProvider, HostedTransport, ProxyHosting, RotationVerdict, rotation_interval,
};

pub(crate) struct HostedAcquirer {
    pub(crate) provider: HostedProvider,
    pub(crate) indexer: http::Uri,
}

impl TransportAcquirable for HostedAcquirer {
    async fn discover(&self) -> Result<HashSet<crate::mixnet::ExitNodeId>, TransportError> {
        let provider = self.provider.clone();
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
        let provider = self.provider.clone();
        let clutch = clutch.to_vec();
        let hosted = tokio::task::spawn_blocking(move || provider.start_transport(&clutch))
            .await
            .map_err(|join| TransportError::HostUnavailable(join.to_string()))?
            .map_err(TransportError::HostRefused)?;
        MixnetProxy::attach(
            hosted.socks5_addr,
            &[hosted.exit_node],
            self.indexer.clone(),
            publisher,
        )
        .map_err(TransportError::from)
    }

    fn rotation_verdict(&self) -> RotationVerdict {
        self.provider.rotation_verdict()
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

    fn rotation_verdict(&self) -> RotationVerdict {
        // A desktop keeps the four role-bound clients ADR 0045 gives it, so
        // it never spends a rotation; ADR 0048 leaves it unchanged.
        RotationVerdict::Never
    }
}

#[cfg(test)]
mod tests;
