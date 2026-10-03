#![forbid(unsafe_code)]

use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use zingo_netutils::Indexer as _;
use zingolib::config::{ClientConfig, WalletConfig};
use zingolib::lightclient::LightClient;
use zingolib::wallet::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery, WalletSettings};

/// The number of blocks below the live tip the timed sync scans.
const GUARD_WINDOW: u32 = 5_000;

/// The indexer every timed run pins, so runs compare like with like.
const GUARD_INDEXER: &str = "https://zec.rocks:443";

/// The seconds a timed run may spend before it fails outright.
const GUARD_BUDGET_SECS: u64 = 900;

/// The seconds a tip query may take before the run refuses to guess.
const TIP_QUERY_SECS: u64 = 30;

/// A BIP-39 mnemonic holding no funds, so the sync measures pure scanning.
const GUARD_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon art";

/// Times the top-window sync at this revision, with no mixnet present.
fn main() {
    tokio::runtime::Runtime::new()
        .expect("the runtime builds")
        .block_on(async {
            let indexer: http::Uri = std::env::args()
                .nth(1)
                .unwrap_or_else(|| GUARD_INDEXER.to_string())
                .parse()
                .expect("the indexer URI parses");
            zingo_netutils::ensure_default_crypto_provider();
            let mut grpc = zingo_netutils::GrpcIndexer::new(indexer.clone())
                .await
                .expect("the indexer client builds");
            let block = grpc
                .get_latest_block(Duration::from_secs(TIP_QUERY_SECS))
                .await
                .expect("the indexer reports its tip");
            let tip = u32::try_from(block.height).expect("the chain height fits a u32");
            // An explicit birthday rescans a named window; without one the
            // run takes the guard window below the live tip.
            let birthday: u32 = std::env::args()
                .nth(2)
                .map(|raw| raw.parse().expect("the birthday parses"))
                .unwrap_or(tip - GUARD_WINDOW);

            let wallet_dir = tempfile::tempdir().expect("a wallet tempdir opens");
            let config = ClientConfig::builder()
                .set_wallet_dir(wallet_dir.path().to_path_buf())
                .set_wallet_config(WalletConfig::MnemonicPhrase {
                    mnemonic_phrase: GUARD_MNEMONIC.to_string(),
                    no_of_accounts: NonZeroU32::new(1).unwrap(),
                    birthday,
                    wallet_settings: WalletSettings {
                        sync_config: SyncConfig {
                            transparent_address_discovery: TransparentAddressDiscovery::default(),
                            performance_level: match std::env::args().nth(3).as_deref() {
                                Some("maximum") => PerformanceLevel::Maximum,
                                Some("medium") => PerformanceLevel::Medium,
                                Some("low") => PerformanceLevel::Low,
                                _ => PerformanceLevel::High,
                            },
                            shutdown_on_completion: true,
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
            let outcome = tokio::time::timeout(
                Duration::from_secs(GUARD_BUDGET_SECS),
                client.sync_and_await(),
            )
            .await;
            let elapsed = started.elapsed().as_secs_f64();
            match outcome {
                Ok(Ok(result)) => {
                    // Scan cost tracks shielded outputs, so the rate that
                    // compares across windows is outputs per second.
                    let outputs = result.sapling_outputs_scanned
                        + result.orchard_outputs_scanned
                        + result.ironwood_outputs_scanned;
                    println!(
                        "SYNC_PERF_TAG: {elapsed:.1}s for {} blocks from {birthday}, \
                         {outputs} outputs ({} sapling, {} orchard, {} ironwood) \
                         = {:.0} outputs/s, {:.0} blocks/s via {indexer} at {}",
                        result.blocks_scanned,
                        result.sapling_outputs_scanned,
                        result.orchard_outputs_scanned,
                        result.ironwood_outputs_scanned,
                        f64::from(outputs) / elapsed,
                        f64::from(result.blocks_scanned) / elapsed,
                        std::env::args().nth(3).unwrap_or("high".to_string()),
                    );
                }
                Ok(Err(e)) => panic!("SYNC_PERF_TAG: sync failed after {elapsed:.1}s: {e:?}"),
                Err(_) => {
                    panic!("SYNC_PERF_TAG: budget of {GUARD_BUDGET_SECS}s exceeded from {birthday}")
                }
            }
        });
}
