//! The acquisition seam: where a mixnet transport comes from.
#![forbid(unsafe_code)]

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use zingo_netutils::responsiveness::ResponsivenessClass;

use crate::correspondent::pool::exit_pool::ExitPoolError;
use crate::nym::driver::StatusPublisher;
use crate::nym::supervisor::{MixnetProxy, MixnetProxyError};

/// Why the session could acquire no ready transport.
#[derive(Debug, thiserror::Error)]
pub enum TransportAcquisitionError {
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
    /// The transport missed the readiness budget.
    #[error("the pool transport did not become ready within {}s", budget.as_secs())]
    NotReady {
        /// The lifecycle budget the transport missed.
        budget: std::time::Duration,
    },
}

impl From<ExitPoolError> for TransportAcquisitionError {
    fn from(refusal: ExitPoolError) -> Self {
        match refusal {
            ExitPoolError::NotSeeded => TransportAcquisitionError::ExitPoolNotSeeded,
            ExitPoolError::Exhausted { held, population } => {
                TransportAcquisitionError::ExitPoolExhausted { held, population }
            }
        }
    }
}

/// Something a mixnet transport is acquirable from: the bundled binary a
/// desktop spawns, or the host that owns the proxy where a platform forbids
/// subprocesses.
pub(crate) trait TransportAcquirable: Send + Sync + 'static {
    /// The Exit Nodes this acquirer can reach, for seeding the Exit Pool.
    fn discover(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, TransportAcquisitionError>> + Send + '_>>;

    /// Acquires one transport that races `clutch` under `class`.
    fn acquire(
        &self,
        class: ResponsivenessClass,
        clutch: &[String],
        publisher: StatusPublisher,
    ) -> Result<MixnetProxy, MixnetProxyError>;
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
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, TransportAcquisitionError>> + Send + '_>>
    {
        Box::pin(crate::nym::supervisor::discover_exit_nodes(&self.path))
    }

    fn acquire(
        &self,
        class: ResponsivenessClass,
        clutch: &[String],
        publisher: StatusPublisher,
    ) -> Result<MixnetProxy, MixnetProxyError> {
        MixnetProxy::spawn(&self.path, class, publisher, clutch)
    }
}
