//! Runs the mixnet price race repeatedly and tallies its outcomes.
//!
//! Each round draws its own Exit Node, races every price source through
//! that one tunnel, and records whether a quote arrived, which source won,
//! and how long the round took. A round that fails names how many sources
//! failed and how many of those were timeouts, which is what distinguishes
//! an exit that carries nothing from sources that would not answer.
//! Levers: `PRICE_HARNESS_ROUNDS` (default 20) and `ZINGO_NYM_PROXY`.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use zingolib::config::{ClientConfig, WalletConfig};
use zingolib::lightclient::LightClient;

/// The rounds run when `PRICE_HARNESS_ROUNDS` says nothing.
const DEFAULT_ROUNDS: usize = 20;

/// How long the harness waits for the session to reach `Ready` before it
/// gives up, which is a harness failure rather than a round's outcome.
const READY_BUDGET: Duration = Duration::from_secs(90);

/// How often the harness checks whether the session has reached `Ready`.
const READY_POLL: Duration = Duration::from_millis(500);

/// A BIP-39 mnemonic holding no funds: the harness never syncs.
const HARNESS_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon art";

/// One round's outcome.
struct Round {
    elapsed: Duration,
    quote: Option<String>,
    failure: Option<String>,
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "network-bound price harness; run explicitly"]
async fn price_rounds_report_their_outcomes() {
    let rounds: usize = std::env::var("PRICE_HARNESS_ROUNDS")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(DEFAULT_ROUNDS);
    let proxy = std::env::var("ZINGO_NYM_PROXY").expect("ZINGO_NYM_PROXY names the proxy binary");
    println!("PRICE_HARNESS: {rounds} rounds, each over its own exit");

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
    client
        .enable_mixnet::<zingolib::mixnet::PrioritiseSpeed>(std::path::Path::new(&proxy))
        .await
        .expect("the mixnet attaches");
    // Attaching returns before the session's status reaches Ready, and the
    // price fetch refuses anything less, so the harness waits it out rather
    // than reporting the guard as a network outcome.
    let ready_by = Instant::now() + READY_BUDGET;
    while client.mixnet_mode() != zingolib::mixnet::MixnetMode::Ready {
        assert!(
            Instant::now() < ready_by,
            "the session never reached Ready within {READY_BUDGET:?}: {:?}",
            client.mixnet_mode()
        );
        tokio::time::sleep(READY_POLL).await;
    }
    println!("PRICE_HARNESS: the session is Ready; rounds begin");

    let mut results: Vec<Round> = Vec::new();
    for round in 1..=rounds {
        let started = Instant::now();
        let outcome = client.update_current_price().await;
        let elapsed = started.elapsed();
        let (quote, failure) = match outcome {
            Ok(fetch) => (Some(format!("{:?} {}", fetch.source, fetch.usd)), None),
            // The top layer renders only its own link, so the per-source
            // failures live further down the chain: walk every layer.
            Err(e) => (None, Some(zingo_net_diag::chain_texts(&e).join(" <- "))),
        };
        println!(
            "PRICE_HARNESS round {round}: {:.1}s -> {}",
            elapsed.as_secs_f64(),
            quote
                .clone()
                .unwrap_or_else(|| failure.clone().unwrap_or_default())
        );
        results.push(Round {
            elapsed,
            quote,
            failure,
        });
    }

    let quoted = results.iter().filter(|r| r.quote.is_some()).count();
    let mean = |select: fn(&Round) -> bool| -> f64 {
        let picked: Vec<&Round> = results.iter().filter(|r| select(r)).collect();
        if picked.is_empty() {
            return 0.0;
        }
        picked.iter().map(|r| r.elapsed.as_secs_f64()).sum::<f64>() / picked.len() as f64
    };
    println!(
        "PRICE_HARNESS SUMMARY: {quoted} quotes, {} failures of {rounds} rounds",
        rounds - quoted
    );
    println!(
        "PRICE_HARNESS SUMMARY: mean {:.1}s to a quote, mean {:.1}s to a failure",
        mean(|r| r.quote.is_some()),
        mean(|r| r.quote.is_none())
    );
    for round in results.iter().filter(|r| r.failure.is_some()) {
        let report = round.failure.clone().unwrap_or_default();
        println!(
            "PRICE_HARNESS FAILURE: {:.1}s | {} | {}",
            round.elapsed.as_secs_f64(),
            mode_tally(&report),
            report.chars().take(400).collect::<String>()
        );
    }
    let modes = results
        .iter()
        .filter_map(|r| r.failure.as_ref())
        .map(|report| mode_tally(report))
        .collect::<Vec<_>>();
    println!("PRICE_HARNESS MODES: {}", modes.join(" ;; "));
}

/// Counts the failure modes a race report names, so a round says whether
/// its tunnel died, its sources refused, or its answers would not parse.
fn mode_tally(report: &str) -> String {
    let lowered = report.to_ascii_lowercase();
    let count = |needle: &str| lowered.matches(needle).count();
    let modes = [
        (
            "tunnel",
            count("tunneltransport") + count("tunnel transport"),
        ),
        ("connect", count("remoteconnect") + count("remote connect")),
        (
            "timeout",
            count("timed out") + count("timeout") + count("deadline"),
        ),
        (
            "tls",
            count("tls") + count("handshake") + count("certificate"),
        ),
        ("http", count("status") + count("http")),
        ("decode", count("deserial") + count("parse")),
        ("trades", count("insufficient")),
    ];
    let named: Vec<String> = modes
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(name, n)| format!("{n} {name}"))
        .collect();
    if named.is_empty() {
        "no mode named".to_string()
    } else {
        named.join(", ")
    }
}
