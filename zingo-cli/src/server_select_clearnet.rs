//! Dynamic server selection via `get_info()` against a curated list of indexers.
//!
//! When no `--server` is specified, we call `get_info()` on each URI in
//! `zingolib::netutils::indexers::MOST_UP_INDEXER_URIS` (the census) concurrently,
//! measure response time, and return the responsive servers sorted
//! from fastest to slowest.
//!
//! Probing is async: `select_servers` awaits its fan-out, and `resolve_server`
//! is the module's only sync-to-async seam (ADR 0030). Every line this module
//! prints is narration and goes to stderr, so stdout carries only command
//! results (ADR 0031).

use std::time::{Duration, Instant};

use crate::commands::RT;
use zingolib::netutils::indexers::MOST_UP_INDEXER_URIS;
use zingolib::netutils::{GetClientError, GrpcIndexer, Indexer as _};

#[cfg(test)]
mod tests;

/// A server that responded successfully to `get_info()`, with its measured latency.
#[derive(Debug)]
pub(crate) struct RankedServer {
    pub uri: http::Uri,
    pub latency: Duration,
}

/// One probed indexer that did not rank, and the stage that refused it.
#[derive(Debug)]
pub(crate) struct ProbeFailure {
    pub uri: http::Uri,
    pub stage: ProbeStage,
}

/// The probe stage that failed, carrying that stage's own error.
#[derive(Debug)]
pub(crate) enum ProbeStage {
    /// Establishing the transport (DNS, TCP, TLS, HTTP/2) failed.
    Connect(GetClientError),
    /// The transport stood, but the `get_info` call itself failed.
    Rpc(zingolib::netutils::Status),
    /// Nothing failed and nothing answered within the probe budget.
    TimedOut(Duration),
}

impl std::fmt::Display for ProbeStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeStage::Connect(error) => write!(f, "connect failed: {}", error_chain(error)),
            ProbeStage::Rpc(error) => write!(f, "get_info failed: {}", error_chain(error)),
            ProbeStage::TimedOut(budget) => write!(f, "no answer within {budget:?}"),
        }
    }
}

/// Separates one link of a rendered cause chain from the next in a probe
/// report, which keeps every link on the one line.
const PROBE_CHAIN_SEPARATOR: &str = ": ";

/// Renders an error and its full source chain as one line, outermost cause first.
fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    zingo_net_diag::chain_texts(error).join(PROBE_CHAIN_SEPARATOR)
}

/// Probes every URI concurrently within one shared budget and reports each
/// outcome, the responders sorted fastest first.
pub(crate) async fn probe_servers(
    uris: Vec<http::Uri>,
    budget: Duration,
) -> (Vec<RankedServer>, Vec<ProbeFailure>) {
    let mut handles = Vec::new();
    for uri in uris {
        handles.push(tokio::spawn(async move {
            let start = Instant::now();
            let outcome = tokio::time::timeout(budget, async {
                let mut indexer = GrpcIndexer::new(uri.clone())
                    .await
                    .map_err(ProbeStage::Connect)?;
                indexer
                    .get_lightd_info(budget)
                    .await
                    .map_err(ProbeStage::Rpc)?;
                Ok(start.elapsed())
            })
            .await
            .unwrap_or(Err(ProbeStage::TimedOut(budget)));
            match outcome {
                Ok(latency) => Ok(RankedServer { uri, latency }),
                Err(stage) => Err(ProbeFailure { uri, stage }),
            }
        }));
    }

    let mut ranked = Vec::new();
    let mut failures = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(Ok(server)) => ranked.push(server),
            Ok(Err(failure)) => failures.push(failure),
            Err(_join_error) => {}
        }
    }
    ranked.sort_by_key(|r| r.latency);
    (ranked, failures)
}

/// Calls `get_info()` on all curated indexer URIs concurrently and
/// returns those that responded, sorted fastest to slowest.
///
/// Uses a per-server timeout so one slow server doesn't block the rest.
/// The probe narration goes to stderr (ADR 0031), and every failed probe
/// is narrated with its stage and full error chain, so an empty result
/// carries its own root cause.
pub(crate) async fn select_servers() -> Vec<RankedServer> {
    use zingolib::netutils::time::SERVER_RANKING_TIMEOUT;

    let uris: Vec<http::Uri> = MOST_UP_INDEXER_URIS
        .iter()
        .filter_map(|s| s.parse::<http::Uri>().ok())
        .collect();

    eprintln!("No --server specified. Probing {} indexers...", uris.len());

    let (ranked, failures) = probe_servers(uris, SERVER_RANKING_TIMEOUT).await;

    for failure in &failures {
        eprintln!("  {}: {}", failure.uri, failure.stage);
    }
    if ranked.is_empty() {
        eprintln!(
            "Warning: none of the {} probed indexers responded; every failure is \
             listed above. There is no default server to fall back to.",
            failures.len()
        );
    } else {
        eprintln!(
            "Selected server: {} ({:?})",
            ranked[0].uri, ranked[0].latency
        );
        for r in &ranked[1..] {
            eprintln!("  also available: {} ({:?})", r.uri, r.latency);
        }
    }

    ranked
}

/// Why the clearnet resolution produced no server.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ResolveServerError {
    /// The explicit `--server` value failed to parse as a URI.
    #[error(transparent)]
    InvalidUri(#[from] http::uri::InvalidUri),
    /// No probed indexer answered, and no default server exists.
    #[error("no probed server responded and there is no default server; pass --server")]
    NoResponder,
}

/// Resolves the indexer server from CLI arguments.
///
/// If `--server` was provided explicitly, uses that URI and returns an
/// empty ranked list. Otherwise, probes curated indexers with `get_info()`
/// and returns the fastest responder along with the full ranked list.
///
/// This function is the module's audited sync-to-async seam: startup calls it
/// synchronously, so it holds the module's only `block_on` (ADR 0030).
#[allow(clippy::disallowed_methods)]
pub(crate) fn resolve_server(
    matches: &clap::ArgMatches,
) -> Result<(http::Uri, Vec<RankedServer>), ResolveServerError> {
    if let Some(explicit) = matches.get_one::<http::Uri>("server") {
        Ok((
            zingolib::config::construct_indexer_uri(explicit.to_string())?,
            vec![],
        ))
    } else {
        RT.block_on(resolve_ranked_server())
    }
}

/// Resolves the indexer for a session going online without an explicit
/// `--server`: the fastest probed responder, refused typed when nothing
/// answered, since no default server exists.
pub(crate) async fn resolve_ranked_server()
-> Result<(http::Uri, Vec<RankedServer>), ResolveServerError> {
    let ranked = select_servers().await;
    match ranked.first() {
        Some(best) => Ok((best.uri.clone(), ranked)),
        None => Err(ResolveServerError::NoResponder),
    }
}
