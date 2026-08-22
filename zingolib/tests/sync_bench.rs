//! A timed mainnet sync benchmark over a fixed 20,000-block window.
//!
//! The test creates a fresh unfunded wallet whose birthday sits one sync
//! window below the mainnet tip of the authoring day, syncs it against a
//! public indexer over clearnet (the one network class clearnet serves),
//! and fails if the sync exceeds its time budget. Because the window is
//! fixed, the same test run at two commits measures the same work, which
//! makes it a `git bisect` probe for sync-throughput regressions.

#![forbid(unsafe_code)]

use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use zingolib::config::{ClientConfig, WalletConfig};
use zingolib::lightclient::LightClient;
use zingolib::wallet::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery, WalletSettings};

/// The number of mainnet blocks the benchmark syncs.
const SYNC_WINDOW: u32 = 20_000;

/// The mainnet chain height on the day the benchmark was authored.
const TIP_AT_AUTHORING: u32 = 3_445_000;

/// The fixed wallet birthday, one sync window below the authoring-day tip.
const BENCH_BIRTHDAY: u32 = TIP_AT_AUTHORING - SYNC_WINDOW;

/// The default seconds of budget, overridable via `SYNC_BENCH_BUDGET_SECS`.
const DEFAULT_BUDGET_SECS: u64 = 600;

/// The default indexer URI, overridable via `SYNC_BENCH_INDEXER`.
const DEFAULT_INDEXER: &str = "https://zec.rocks:443";

/// A BIP-39 mnemonic holding no funds, so the sync measures pure scanning.
const BENCH_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon art";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "network-bound timing benchmark; run explicitly"]
async fn sync_20k_mainnet_blocks_within_budget() {
    let budget = Duration::from_secs(
        std::env::var("SYNC_BENCH_BUDGET_SECS")
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(DEFAULT_BUDGET_SECS),
    );
    let indexer: http::Uri = std::env::var("SYNC_BENCH_INDEXER")
        .unwrap_or_else(|_| DEFAULT_INDEXER.to_string())
        .parse()
        .expect("the indexer URI parses");

    let wallet_dir = tempfile::tempdir().expect("a wallet tempdir opens");
    let config = ClientConfig::builder()
        .set_wallet_dir(wallet_dir.path().to_path_buf())
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: BENCH_MNEMONIC.to_string(),
            no_of_accounts: NonZeroU32::new(1).unwrap(),
            birthday: BENCH_BIRTHDAY,
            wallet_settings: WalletSettings {
                sync_config: SyncConfig {
                    transparent_address_discovery: TransparentAddressDiscovery::default(),
                    performance_level: PerformanceLevel::High,
                },
                min_confirmations: NonZeroU32::new(3).unwrap(),
            },
        })
        .build()
        .unwrap();

    let mut client = LightClient::new(config, true)
        .await
        .expect("the wallet creates");
    client
        .set_indexer_uri(indexer.clone())
        .await
        .expect("the indexer connects");

    let started = Instant::now();
    let outcome = tokio::time::timeout(budget, client.sync_and_await()).await;
    let elapsed = started.elapsed().as_secs_f64();
    match outcome {
        Ok(Ok(_)) => println!(
            "SYNC_BENCH: {SYNC_WINDOW} blocks from {BENCH_BIRTHDAY} via {indexer} in {elapsed:.1}s"
        ),
        Ok(Err(e)) => panic!("SYNC_BENCH: sync failed after {elapsed:.1}s: {e:?}"),
        Err(_) => panic!(
            "SYNC_BENCH: budget of {}s exceeded at {SYNC_WINDOW} blocks from {BENCH_BIRTHDAY}",
            budget.as_secs()
        ),
    }
}
