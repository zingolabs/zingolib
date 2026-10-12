//! Demonstrates ADR 0029's boot phase: probe every mixnet-eligible census
//! endpoint through its own uniformly-sampled exit, then hold the
//! successes as maintained connections and prove they still answer with a
//! second `GetLightdInfo` round over the same transports.
//!
//! ```text
//! pool-discovery [--budget N]
//! ```
//!
//! `--budget N` truncates the eligible endpoint list to N, the Exit
//! Budget of a platform that cannot afford full width.
#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use zingo_netutils::Socks5Indexer;
use zingo_netutils::ensure_default_crypto_provider;
use zingo_netutils::indexers::IndexerChain;
use zingo_netutils::live_indexer_discovery::{DiscoveryFailureKind, discover_live_indexers};
use zingo_netutils::time::MIXNET_ROUND_TRIP_BOUND;

#[tokio::main]
async fn main() -> ExitCode {
    ensure_default_crypto_provider();

    let mut budget = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--budget" => match args.next().and_then(|n| n.parse::<usize>().ok()) {
                Some(n) => budget = Some(n),
                None => {
                    eprintln!("--budget takes a positive integer");
                    return ExitCode::FAILURE;
                }
            },
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock sits after the epoch")
        .as_nanos() as u64;

    let report = match discover_live_indexers(IndexerChain::Main, budget, seed, |line| {
        println!("  … {line}");
    })
    .await
    {
        Ok(report) => report,
        Err(refused) => {
            eprintln!("discovery could not start: {refused}");
            return ExitCode::FAILURE;
        }
    };

    println!();
    println!(
        "pool seeded: {} live, {} failed",
        report.live.len(),
        report.failed.len()
    );
    for found in &report.live {
        println!(
            "  LIVE {} operator={} height={} ({:.1?})",
            found.indexer.uri,
            found.indexer.operator(),
            found.tip.height,
            found.elapsed
        );
    }
    for refused in &report.failed {
        let phase = match &refused.kind {
            DiscoveryFailureKind::Bootstrap(source) => format!("bootstrap: {source}"),
            DiscoveryFailureKind::Probe(source) => format!("probe: {source}"),
        };
        println!("  DEAD {} ({phase})", refused.indexer.uri);
    }

    if report.live.is_empty() {
        return ExitCode::FAILURE;
    }

    println!();
    println!("heartbeat over the maintained transports:");
    for found in &report.live {
        let uri: http::Uri = found
            .indexer
            .uri
            .parse()
            .expect("the census tests pin every entry parseable");
        let socks5_addr = found.transport.socks5_addr();
        let heartbeat = Socks5Indexer::new(socks5_addr, uri, MIXNET_ROUND_TRIP_BOUND);
        match heartbeat.get_latest_block().await {
            Ok(tip) => println!(
                "  OK {} height={} (same transport, same exit)",
                found.indexer.uri, tip.height
            ),
            Err(source) => println!("  LOST {} ({source})", found.indexer.uri),
        }
    }

    for found in report.live {
        found.transport.disconnect().await;
    }

    ExitCode::SUCCESS
}
