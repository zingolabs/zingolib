//! Runs the Server-Selection Sweep repeatedly and tallies its outcomes.
//!
//! BENCHMARK-LOCAL, UNCOMMITTED. Each round drives the real
//! `run_server_selection_sweep` against the live mainnet census, records
//! whether it reached a verdict or collapsed, and prints a summary: the
//! collapse rate, the causes an empty cohort reported, and the wall time
//! each round took. Levers: `SWEEP_HARNESS_ROUNDS` (default 10),
//! `SWEEP_HARNESS_PIN` (a pinned URI, default none),
//! `ZINGO_NYM_PROXY` (the proxy binary, required).

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use zingolib::config::{ClientConfig, WalletConfig};
use zingolib::indexers::IndexerChain;
use zingolib::lightclient::LightClient;
use zingolib::lightclient::select::SweepProgress;

/// The rounds run when `SWEEP_HARNESS_ROUNDS` says nothing.
const DEFAULT_ROUNDS: usize = 10;

/// A BIP-39 mnemonic holding no funds: the harness never syncs.
const HARNESS_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon art";

/// One round's outcome.
struct Round {
    elapsed: Duration,
    answered: usize,
    surveyed: usize,
    verdict: Option<String>,
    refusal: Option<String>,
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "network-bound sweep harness; run explicitly"]
async fn sweep_rounds_report_their_outcomes() {
    let rounds: usize = std::env::var("SWEEP_HARNESS_ROUNDS")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(DEFAULT_ROUNDS);
    let pin: Option<http::Uri> = std::env::var("SWEEP_HARNESS_PIN")
        .ok()
        .filter(|raw| !raw.is_empty())
        .map(|raw| raw.parse().expect("the pinned URI parses"));
    let proxy = std::env::var("ZINGO_NYM_PROXY").expect("ZINGO_NYM_PROXY names the proxy binary");

    let mut candidates: Vec<http::Uri> = zingolib::indexers::mixnet_eligible(IndexerChain::Main)
        .map(|indexer| indexer.uri.parse().expect("census URIs parse"))
        .collect();
    if let Some(pinned) = &pin
        && !candidates.contains(pinned)
    {
        candidates.push(pinned.clone());
    }
    println!(
        "SWEEP_HARNESS: {rounds} rounds over {} candidates, pin {}",
        candidates.len(),
        pin.as_ref().map_or("none".to_string(), |p| p.to_string())
    );

    let wallet_dir = tempfile::tempdir().expect("a wallet tempdir opens");
    let config = ClientConfig::builder()
        .set_wallet_dir(wallet_dir.path().to_path_buf())
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: HARNESS_MNEMONIC.to_string(),
            no_of_accounts: std::num::NonZeroU32::new(1).unwrap(),
            birthday: u32::MAX,
            wallet_settings: Default::default(),
        })
        .build()
        .expect("the config builds");
    let mut client = LightClient::new(config, true)
        .await
        .expect("the wallet creates");

    let mut results: Vec<Round> = Vec::new();
    for round in 1..=rounds {
        let seen = std::cell::Cell::new((0usize, 0usize));
        let started = Instant::now();
        let outcome = client
            .run_server_selection_sweep(
                std::path::Path::new(&proxy),
                &candidates,
                pin.as_ref(),
                |phase| {
                    if let SweepProgress::Judging { answered, surveyed } = phase {
                        seen.set((answered, surveyed));
                    }
                },
            )
            .await;
        let elapsed = started.elapsed();
        let (answered, surveyed) = seen.get();
        let (verdict, refusal) = match outcome {
            Ok(selection) => (Some(selection.sync_indexer.to_string()), None),
            Err(e) => (None, Some(format!("{e}"))),
        };
        println!(
            "SWEEP_HARNESS round {round}: {answered}/{surveyed} answered in {:.1}s -> {}",
            elapsed.as_secs_f64(),
            verdict
                .clone()
                .unwrap_or_else(|| refusal.clone().unwrap_or_default())
        );
        results.push(Round {
            elapsed,
            answered,
            surveyed,
            verdict,
            refusal,
        });
        client.go_offline().await;
    }

    let collapses = results.iter().filter(|r| r.verdict.is_none()).count();
    let verdicts = rounds - collapses;
    let mean = |select: fn(&Round) -> bool| -> f64 {
        let picked: Vec<&Round> = results.iter().filter(|r| select(r)).collect();
        if picked.is_empty() {
            return 0.0;
        }
        picked.iter().map(|r| r.elapsed.as_secs_f64()).sum::<f64>() / picked.len() as f64
    };
    println!(
        "SWEEP_HARNESS SUMMARY: {verdicts} verdicts, {collapses} collapses of {rounds} rounds"
    );
    println!(
        "SWEEP_HARNESS SUMMARY: mean {:.1}s to a verdict, mean {:.1}s to a collapse",
        mean(|r| r.verdict.is_some()),
        mean(|r| r.verdict.is_none())
    );
    for round in results.iter().filter(|r| r.refusal.is_some()) {
        println!(
            "SWEEP_HARNESS COLLAPSE: {}/{} answered, {:.1}s, {}",
            round.answered,
            round.surveyed,
            round.elapsed.as_secs_f64(),
            round.refusal.clone().unwrap_or_default()
        );
    }
}
