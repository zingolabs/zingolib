//! Paired clearnet/mixnet indexer probes.
//!
//! A failed mixnet send leaves an ambiguity: is the indexer down, or is the
//! mixnet path to it broken? A paired probe answers it by running the same
//! `GetLightdInfo` against the same indexer over both routes and reporting
//! the two outcomes side by side: clearnet-ok + mixnet-fail is a
//! mixnet-specific failure (dead proxy, exit-gateway policy, tunnel
//! transport), fail + fail is the indexer itself, ok + ok exonerates both.
//! Probe outcomes are appended to the cross-session indexer history like
//! send attempts, so reliability accumulates.
//!
//! Privacy note: the clearnet leg contacts the indexer directly from the
//! client's real IP. `GetLightdInfo` carries no wallet data, but the contact
//! itself is observable — this is a user-invoked diagnostic, never an
//! automatic path.
#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use http::Uri;
use zingo_netutils::{GrpcIndexer, Indexer as _};

use crate::lightclient::indexer_history::{
    AttemptKind, AttemptRoute, IndexerAttempt, IndexerHistoryHandle, now_unix_secs,
};

/// One leg of a paired probe.
#[derive(Clone, Debug)]
pub struct ProbeLeg {
    /// A one-line success summary (chain and height), or the failure detail.
    pub outcome: Result<String, String>,
    /// How long the leg took, in milliseconds.
    pub millis: u64,
}

/// The two legs of one indexer's paired probe.
#[derive(Clone, Debug)]
pub struct PairedProbe {
    /// The probed indexer's host.
    pub host: String,
    /// The direct clearnet leg.
    pub clearnet: ProbeLeg,
    /// The mixnet leg, or `None` when the proxy was not ready to carry it.
    pub mixnet: Option<ProbeLeg>,
}

/// Renders a `LightdInfo` as the probe's one-line success summary.
fn summarize(info: &zingo_netutils::lightwallet_protocol::LightdInfo) -> String {
    format!("chain {}, height {}", info.chain_name, info.block_height)
}

/// Probes `indexer` over clearnet, recording the attempt.
async fn clearnet_leg(
    indexer: &Uri,
    timeout: Duration,
    history: &IndexerHistoryHandle,
    host: &str,
) -> ProbeLeg {
    let started = Instant::now();
    // `GrpcIndexer::new` connects eagerly with no connect timeout of its own,
    // so a black-holing clearnet host would stall this leg past `timeout` and
    // (through `join_all`) hang the whole probe. Bound construction and the
    // RPC together.
    let outcome = match tokio::time::timeout(timeout, async {
        let mut grpc = GrpcIndexer::new(indexer.clone())
            .await
            .map_err(|e| e.to_string())?;
        grpc.get_lightd_info(timeout)
            .await
            .map(|info| summarize(&info))
            .map_err(|status| format!("{status:?}"))
    })
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => Err(format!("clearnet connect timed out after {timeout:.0?}")),
    };
    let leg = ProbeLeg {
        millis: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        outcome,
    };
    record_probe(history, host, AttemptRoute::Clearnet, &leg);
    leg
}

/// Probes `indexer` through the SOCKS5 proxy, recording the attempt.
async fn mixnet_leg(
    socks5_addr: &str,
    indexer: &Uri,
    timeout: Duration,
    history: &IndexerHistoryHandle,
    host: &str,
) -> ProbeLeg {
    let started = Instant::now();
    let outcome = zingo_netutils::get_lightd_info_via_socks5(socks5_addr, indexer, timeout)
        .await
        .map(|info| summarize(&info))
        .map_err(|e| e.to_string());
    let leg = ProbeLeg {
        millis: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        outcome,
    };
    record_probe(history, host, AttemptRoute::Mixnet, &leg);
    leg
}

fn record_probe(history: &IndexerHistoryHandle, host: &str, route: AttemptRoute, leg: &ProbeLeg) {
    history.record(&IndexerAttempt {
        unix_secs: now_unix_secs(),
        host: host.to_string(),
        route,
        kind: AttemptKind::Probe,
        millis: leg.millis,
        outcome: match &leg.outcome {
            Ok(_) => Ok(()),
            Err(detail) => Err(detail.clone()),
        },
    });
}

/// Runs the paired probe against one indexer: both legs concurrently, so a
/// hanging mixnet tunnel does not serialize behind the clearnet answer.
/// `socks5_addr` is `None` when the proxy is not ready — the mixnet leg is
/// then skipped and reported as absent.
pub(crate) async fn probe_indexer(
    indexer: &Uri,
    socks5_addr: Option<&str>,
    timeout: Duration,
    history: &IndexerHistoryHandle,
) -> PairedProbe {
    let host = indexer
        .host()
        .map_or_else(|| indexer.to_string(), str::to_string);
    match socks5_addr {
        Some(socks5) => {
            let (clearnet, mixnet) = tokio::join!(
                clearnet_leg(indexer, timeout, history, &host),
                mixnet_leg(socks5, indexer, timeout, history, &host),
            );
            PairedProbe {
                host,
                clearnet,
                mixnet: Some(mixnet),
            }
        }
        None => PairedProbe {
            clearnet: clearnet_leg(indexer, timeout, history, &host).await,
            mixnet: None,
            host,
        },
    }
}
