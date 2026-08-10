//! The acquisition seam: where a mixnet transport comes from.
#![forbid(unsafe_code)]

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use zingo_netutils::responsiveness::ResponsivenessClass;

use crate::nym::driver::StatusPublisher;
use crate::nym::supervisor::{MixnetProxy, MixnetProxyError};

/// Something a mixnet transport is acquirable from: the bundled binary a
/// desktop spawns, or the host that owns the proxy where a platform forbids
/// subprocesses.
pub(crate) trait TransportAcquirable: Send + Sync + 'static {
    /// The Exit Nodes this acquirer can reach, for seeding the Exit Pool.
    fn discover(&self) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>>;

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
    fn discover(&self) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send + '_>> {
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
