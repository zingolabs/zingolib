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
    failure: Option<(String, String)>,
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
        .enable_mixnet(std::path::Path::new(&proxy))
        .await
        .expect("the mixnet attaches");
    // The enable blocks until the standing client is born proven, so Ready
    // is expected at once; the wait survives as a cheap guard so a status
    // lag is never reported as a network outcome.
    let ready_by = Instant::now() + READY_BUDGET;
    while client.read_mixnet_indicator() != zingolib::mixnet::Indicator::Ready {
        assert!(
            Instant::now() < ready_by,
            "the session never reached Ready within {READY_BUDGET:?}: {:?}",
            client.read_mixnet_indicator()
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
            Err(e) => (
                None,
                Some((mode_tally(&e), zingo_net_diag::chain_texts(&e).join(" <- "))),
            ),
        };
        println!(
            "PRICE_HARNESS round {round}: {:.1}s -> {}",
            elapsed.as_secs_f64(),
            quote.clone().unwrap_or_else(|| failure
                .clone()
                .map(|(mode, report)| format!("[{mode}] {report}"))
                .unwrap_or_default())
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
    for round in results
        .iter()
        .filter_map(|r| r.failure.as_ref().map(|f| (r, f)))
    {
        let (round, (mode, report)) = round;
        println!(
            "PRICE_HARNESS FAILURE: {:.1}s | {mode} | {}",
            round.elapsed.as_secs_f64(),
            report.chars().take(400).collect::<String>()
        );
    }
    let modes: Vec<String> = results
        .iter()
        .filter_map(|r| r.failure.as_ref().map(|(mode, _)| mode.clone()))
        .collect();
    println!("PRICE_HARNESS MODES: {}", modes.join(" ;; "));
}

/// The failure mode one round ended in, read from the typed error rather
/// than from its rendering, so a cause that changes its prose keeps its
/// classification and a cause that gains a variant fails to compile here
/// instead of silently reporting nothing.
fn mode_tally(error: &zingolib::lightclient::error::LightClientError) -> String {
    use zingolib::wallet::error::PriceError as WalletPriceError;

    let zingolib::lightclient::error::LightClientError::PriceError(price) = error else {
        return format!("not a price failure: {error}");
    };
    match price {
        WalletPriceError::Speed(speed) => speed_mode(speed),
        WalletPriceError::NotInitialised => "price list not initialised".to_string(),
        WalletPriceError::PriceError(one) => format!("one source: {}", source_mode(one)),
        WalletPriceError::RaceFailed(report) => {
            let mut counts: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for (_, failure) in &report.failures {
                *counts.entry(source_mode(failure)).or_default() += 1;
            }
            counts
                .into_iter()
                .map(|(mode, count)| format!("{count} {mode}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
}

/// A failure of the wave itself, which belongs to the transport rather than
/// to any price source.
fn speed_mode(failure: &zingolib::mixnet::speed::SpeedError) -> String {
    match failure {
        zingolib::mixnet::speed::SpeedError::Transport(_) => "no transport acquired".to_string(),
        zingolib::mixnet::speed::SpeedError::NoLiveExit { draws, budget } => {
            format!("no live exit in {draws} draws of {}ms", budget.as_millis())
        }
        zingolib::mixnet::speed::SpeedError::DeadlineExhausted { draws, budget } => {
            format!(
                "deadline exhausted after {draws} draws of a {}s budget",
                budget.as_secs()
            )
        }
    }
}

/// One source's failure named by its own kind, and for a transport failure
/// by the stage it reached, which is what separates a tunnel that carried
/// nothing from an operator that refused.
fn source_mode(failure: &zingo_price::PriceError) -> String {
    match failure {
        zingo_price::PriceError::RequestFailed { failure, .. } => {
            format!("at {}", failure.stage)
        }
        zingo_price::PriceError::InsufficientTrades { .. } => "too few trades".to_string(),
        zingo_price::PriceError::DeserializationFailed(_) => "undecodable body".to_string(),
        zingo_price::PriceError::ParseError(_) => "unparsable number".to_string(),
        zingo_price::PriceError::PriceListNotInitialized => "no price list".to_string(),
        zingo_price::PriceError::InvalidPrice => "invalid price".to_string(),
        zingo_price::PriceError::SourceReportedError(_) => "source refused".to_string(),
        zingo_price::PriceError::UnexpectedShape(_) => "unexpected shape".to_string(),
    }
}
