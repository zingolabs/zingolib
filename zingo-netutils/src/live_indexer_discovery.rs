//! One-exit-per-probe liveness discovery over the maintained indexer
//! census (ADR 0029).
//!
//! Every mixnet-eligible census endpoint is probed through its own exit,
//! uniformly sampled without replacement from the directory's Exit
//! Nodes, so no exit observes more than one probe and no operator
//! learns anything at boot beyond its own endpoint having been probed. A
//! probe that succeeds keeps its transport — the discovery dial is the
//! pool connection's first dial — and a probe that fails tears its
//! transport down.
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use lightwallet_protocol::BlockId;

use crate::NymProxy;
use crate::error::NymProxyError;
use crate::indexers::{Indexer, IndexerChain, mixnet_eligible};
use crate::mixnet_connect::seeded_shuffle;
use crate::socks5_transmit::{Socks5Indexer, Socks5TransmitError};
use crate::time::{
    ATTACH_LISTENER_RETRY_PAUSE, MIXNET_ROUND_TRIP_BOUND, PER_ATTEMPT_CONNECT_TIMEOUT,
};

/// A census endpoint that answered `GetLatestBlock` through its own exit.
/// Holding the value holds the transport: dropping a `DiscoveredIndexer`
/// tears the maintained connection down.
pub struct DiscoveredIndexer {
    /// The census entry that answered.
    pub indexer: &'static Indexer,
    /// The Exit Node (Nym recipient address) assigned to this endpoint.
    pub exit_node: String,
    /// The transport the probe rode, kept alive as the pool connection.
    pub transport: NymProxy,
    /// The endpoint's chain tip, as it reported it.
    pub tip: BlockId,
    /// Bootstrap plus probe, wall clock.
    pub elapsed: Duration,
}

/// A census endpoint that did not make it into the pool, with the phase
/// that refused it.
pub struct DiscoveryFailure {
    /// The census entry that failed.
    pub indexer: &'static Indexer,
    /// The Exit Node the endpoint had been assigned.
    pub exit_node: String,
    /// Which phase failed, carrying the typed cause.
    pub kind: DiscoveryFailureKind,
}

/// The phase of a per-endpoint discovery attempt that failed.
#[derive(Debug)]
pub enum DiscoveryFailureKind {
    /// The assigned exit never became a working transport.
    Bootstrap(NymProxyError),
    /// The transport came up but the endpoint's `GetLatestBlock` did not.
    Probe(Socks5TransmitError),
}

/// The outcome of a discovery sweep: the pool seed and the refusals.
pub struct DiscoveryReport {
    /// Endpoints now held as maintained pool connections.
    pub live: Vec<DiscoveredIndexer>,
    /// Endpoints that failed, each with its typed cause.
    pub failed: Vec<DiscoveryFailure>,
}

/// A discovery sweep that could not start.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// The Nym directory could not be queried for Exit Nodes.
    #[error(transparent)]
    ExitDirectory(NymProxyError),
    /// The directory offered fewer exits than eligible endpoints, so
    /// per-endpoint exit uniqueness cannot hold.
    #[error("the directory offered {have} exits for {need} eligible endpoints")]
    InsufficientExits {
        /// Eligible endpoints to probe.
        need: usize,
        /// Exit Nodes the directory offered.
        have: usize,
    },
}

/// Probe every mixnet-eligible census endpoint of `chain`, each through
/// its own uniformly-sampled exit, and return the live set as held pool
/// connections. `budget` truncates the eligible list to a platform's Exit
/// Budget; `None` is full width. `seed` fixes the exit sampling, so a
/// caller supplies entropy (production hashes the clock, tests pass a
/// constant).
pub async fn discover_live_indexers<F>(
    chain: IndexerChain,
    budget: Option<usize>,
    seed: u64,
    on_progress: F,
) -> Result<DiscoveryReport, DiscoveryError>
where
    F: Fn(String) + Send + Sync + 'static,
{
    // Shared by reference count so every probe task narrates through the
    // one callback, and by its own type so no trait object is minted.
    let on_progress = Arc::new(on_progress);
    let mut eligible: Vec<&'static Indexer> = mixnet_eligible(chain).collect();
    if let Some(budget) = budget {
        eligible.truncate(budget);
    }

    on_progress(format!(
        "discovering exit nodes for {} eligible endpoints",
        eligible.len()
    ));
    let mut exit_nodes = NymProxy::discover_exit_nodes()
        .await
        .map_err(DiscoveryError::ExitDirectory)?;
    if exit_nodes.len() < eligible.len() {
        return Err(DiscoveryError::InsufficientExits {
            need: eligible.len(),
            have: exit_nodes.len(),
        });
    }
    seeded_shuffle(&mut exit_nodes, seed);
    exit_nodes.truncate(eligible.len());

    let mut tasks = tokio::task::JoinSet::new();
    for (indexer, exit_node) in eligible.into_iter().zip(exit_nodes) {
        let progress = Arc::clone(&on_progress);
        tasks.spawn(probe_via_unique_exit(indexer, exit_node, progress));
    }

    let mut live = Vec::new();
    let mut failed = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined.expect("a probe task never panics") {
            Ok(found) => live.push(found),
            Err(refused) => failed.push(refused),
        }
    }
    live.sort_by(|a, b| a.indexer.uri.cmp(b.indexer.uri));
    failed.sort_by(|a, b| a.indexer.uri.cmp(b.indexer.uri));
    Ok(DiscoveryReport { live, failed })
}

async fn probe_via_unique_exit<F>(
    indexer: &'static Indexer,
    exit_node: String,
    on_progress: Arc<F>,
) -> Result<DiscoveredIndexer, DiscoveryFailure>
where
    F: Fn(String) + Send + Sync + 'static,
{
    let started = Instant::now();
    on_progress(format!(
        "{}: bootstrapping exit {}",
        indexer.uri,
        short_exit(&exit_node)
    ));
    let bootstrap = tokio::time::timeout(
        PER_ATTEMPT_CONNECT_TIMEOUT,
        NymProxy::start_with_exit_node(&exit_node),
    )
    .await
    .unwrap_or_else(|_elapsed| {
        Err(NymProxyError::AttemptTimeout(
            PER_ATTEMPT_CONNECT_TIMEOUT.as_secs(),
        ))
    });
    let transport = match bootstrap {
        Ok(transport) => transport,
        Err(source) => {
            on_progress(format!(
                "{}: exit {} refused ({source})",
                indexer.uri,
                short_exit(&exit_node)
            ));
            return Err(DiscoveryFailure {
                indexer,
                exit_node,
                kind: DiscoveryFailureKind::Bootstrap(source),
            });
        }
    };

    let uri: http::Uri = indexer
        .uri
        .parse()
        .expect("the census tests pin every entry parseable");
    match probe_once_listening(&transport, &uri).await {
        Ok(tip) => {
            on_progress(format!(
                "{}: live at height {} via exit {} ({:.1?})",
                indexer.uri,
                tip.height,
                short_exit(&exit_node),
                started.elapsed()
            ));
            Ok(DiscoveredIndexer {
                indexer,
                exit_node,
                transport,
                tip,
                elapsed: started.elapsed(),
            })
        }
        Err(source) => {
            on_progress(format!("{}: probe failed ({source})", indexer.uri));
            transport.disconnect().await;
            Err(DiscoveryFailure {
                indexer,
                exit_node,
                kind: DiscoveryFailureKind::Probe(source),
            })
        }
    }
}

/// The transport's SOCKS5 listener binds asynchronously after
/// [`NymProxy::start_with_exit_node`] returns, so a dial refused at the
/// loopback is warmup, not death: retry it until the round-trip budget
/// elapses. Every other failure classifies immediately.
async fn probe_once_listening(
    transport: &NymProxy,
    uri: &http::Uri,
) -> Result<BlockId, Socks5TransmitError> {
    let socks5_addr = transport.socks5_addr();
    let probe = Socks5Indexer::new(socks5_addr, uri.clone(), MIXNET_ROUND_TRIP_BOUND);
    let deadline = Instant::now() + MIXNET_ROUND_TRIP_BOUND;
    loop {
        match probe.get_latest_block().await {
            Err(Socks5TransmitError::ProxyUnreachable { .. }) if Instant::now() < deadline => {
                tokio::time::sleep(ATTACH_LISTENER_RETRY_PAUSE).await;
            }
            outcome => return outcome,
        }
    }
}

fn short_exit(exit_node: &str) -> &str {
    let head = exit_node.split('.').next().unwrap_or(exit_node);
    &head[..head.len().min(8)]
}
