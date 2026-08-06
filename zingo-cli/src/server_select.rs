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
use zingolib::netutils::{GrpcIndexer, Indexer as _};

/// A server that responded successfully to `get_info()`, with its measured latency.
#[derive(Debug)]
pub(crate) struct RankedServer {
    pub uri: http::Uri,
    pub latency: Duration,
}

/// Calls `get_info()` on all curated indexer URIs concurrently and
/// returns those that responded, sorted fastest to slowest.
///
/// Uses a per-server timeout so one slow server doesn't block the rest.
/// The probe narration goes to stderr (ADR 0031).
pub(crate) async fn select_servers() -> Vec<RankedServer> {
    use zingolib::netutils::time::SERVER_RANKING_TIMEOUT;

    let uris: Vec<http::Uri> = MOST_UP_INDEXER_URIS
        .iter()
        .filter_map(|s| s.parse::<http::Uri>().ok())
        .collect();

    eprintln!("No --server specified. Probing {} indexers...", uris.len());

    let mut handles = Vec::new();

    for uri in uris {
        handles.push(tokio::spawn(async move {
            let start = Instant::now();
            let mut indexer = match GrpcIndexer::new(uri.clone()).await {
                Ok(i) => i,
                Err(_) => return None,
            };
            match indexer.get_lightd_info(SERVER_RANKING_TIMEOUT).await {
                Ok(_info) => Some(RankedServer {
                    uri,
                    latency: start.elapsed(),
                }),
                _ => None,
            }
        }));
    }

    let mut ranked: Vec<RankedServer> = Vec::new();
    for handle in handles {
        if let Ok(Some(server)) = handle.await {
            ranked.push(server);
        }
    }

    ranked.sort_by_key(|r| r.latency);

    if ranked.is_empty() {
        eprintln!("Warning: no indexers responded. Falling back to default.");
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
) -> Result<(http::Uri, Vec<RankedServer>), http::uri::InvalidUri> {
    if let Some(explicit) = matches.get_one::<http::Uri>("server") {
        Ok((
            zingolib::config::construct_indexer_uri(Some(explicit.to_string()))?,
            vec![],
        ))
    } else {
        let ranked = RT.block_on(select_servers());
        let server = if let Some(best) = ranked.first() {
            best.uri.clone()
        } else {
            zingolib::config::construct_indexer_uri(None)?
        };
        Ok((server, ranked))
    }
}
