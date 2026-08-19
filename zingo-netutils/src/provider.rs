//! The mixnet provider a platform host supplies (ADR 0046).
#![forbid(unsafe_code)]

use std::sync::Arc;

use crate::exit::ExitNodeId;

/// One transport a platform host started on the wallet's behalf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedTransport {
    /// Where the host's proxy listens.
    pub socks5_addr: std::net::SocketAddr,
    /// The Exit Node it bound.
    pub exit_node: ExitNodeId,
}

/// Why a host declined or failed a request, carrying the host's own detail.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HostRefusal {
    /// The host tried and could not.
    #[error("the host failed: {detail}")]
    Failed {
        /// The host's account of the failure.
        detail: String,
    },
    /// The host declined as a matter of platform policy.
    #[error("the host declined: {detail}")]
    Declined {
        /// The host's account of the refusal.
        detail: String,
    },
}

/// A platform host that owns the mixnet proxy, where a sandbox forbids the
/// wallet from spawning one.
pub trait ProxyHosting: Send + Sync + 'static {
    /// The Exit Nodes the host's directory query reports.
    fn discover_exit_nodes(&self) -> Result<Vec<ExitNodeId>, HostRefusal>;

    /// Starts one proxy racing `clutch`, reporting where it listens and
    /// which exit it bound.
    fn start_transport(&self, clutch: &[ExitNodeId]) -> Result<HostedTransport, HostRefusal>;
}

/// The provider a hosted platform supplies, holding the host it asks.
///
/// The wallet names this type and never the host behind it, so the dynamic
/// dispatch a supplied host requires stays below the seam where the
/// implementation lives (ADR 0046).
#[derive(Clone)]
pub struct HostedProvider {
    host: Arc<dyn ProxyHosting>,
}

impl HostedProvider {
    /// A provider that asks `host` for every transport.
    pub fn owned_by<H: ProxyHosting>(host: H) -> Self {
        HostedProvider {
            host: Arc::new(host),
        }
    }

    /// The host's reported census.
    ///
    /// This blocks, so a caller on an async runtime hands it to a blocking
    /// thread; the provider names no runtime of its own.
    pub fn discover_exit_nodes(&self) -> Result<Vec<ExitNodeId>, HostRefusal> {
        self.host.discover_exit_nodes()
    }

    /// One started transport over `clutch`, blocking on the same terms.
    pub fn start_transport(&self, clutch: &[ExitNodeId]) -> Result<HostedTransport, HostRefusal> {
        self.host.start_transport(clutch)
    }
}
