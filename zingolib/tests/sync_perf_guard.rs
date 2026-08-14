#![forbid(unsafe_code)]

use std::io::Write as _;
use std::num::NonZeroU32;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use zingo_netutils::Indexer as _;
use zingolib::config::{ClientConfig, WalletConfig};
use zingolib::lightclient::LightClient;
use zingolib::wallet::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery, WalletSettings};

/// The number of blocks below the live tip the guarded sync scans.
const GUARD_WINDOW: u32 = 5_000;

/// The indexer every guarded run pins, so runs compare like with like.
const GUARD_INDEXER: &str = "https://zec.rocks:443";

/// The seconds a guarded run may spend before it fails outright.
const GUARD_BUDGET_SECS: u64 = 900;

/// The seconds a tip query may take before the run refuses to guess.
const TIP_QUERY_SECS: u64 = 30;

/// The factor over this machine's best recorded sync that counts as a
/// regression, sitting above the 1.13 run-to-run spread and below the 1.24
/// step the 2026-08-13 bisect caught.
const REGRESSION_TOLERANCE: f64 = 1.15;

/// A BIP-39 mnemonic holding no funds, so the sync measures pure scanning.
const GUARD_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon art";

/// Syncs the top window against the pinned indexer and fails when the time
/// regresses past tolerance over this development environment's own best.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "network-bound performance guard; run explicitly"]
async fn syncing_the_top_window_holds_this_machines_baseline() {
    let indexer: http::Uri = std::env::var("SYNC_PERF_INDEXER")
        .unwrap_or_else(|_| GUARD_INDEXER.to_string())
        .parse()
        .expect("the indexer URI parses");
    let tolerance: f64 = std::env::var("SYNC_PERF_TOLERANCE")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(REGRESSION_TOLERANCE);

    zingo_netutils::ensure_default_crypto_provider();
    let tip = tokio::time::timeout(
        Duration::from_secs(TIP_QUERY_SECS),
        query_tip(indexer.clone()),
    )
    .await
    .expect("the tip query returns inside its budget");
    let birthday = tip - GUARD_WINDOW;

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
    let outcome = tokio::time::timeout(
        Duration::from_secs(GUARD_BUDGET_SECS),
        client.sync_and_await(),
    )
    .await;
    let elapsed = started.elapsed().as_secs_f64();
    match outcome {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => panic!("SYNC_PERF: sync failed after {elapsed:.1}s: {e:?}"),
        Err(_) => panic!("SYNC_PERF: budget of {GUARD_BUDGET_SECS}s exceeded from {birthday}"),
    }

    let commit = commit_stamp();
    let baseline = best_recorded(&indexer);
    record(&indexer, &commit, birthday, elapsed);
    match baseline {
        None => println!(
            "SYNC_PERF: baseline established at {elapsed:.1}s ({GUARD_WINDOW} blocks, {commit})"
        ),
        Some(best) => {
            let factor = elapsed / best;
            println!("SYNC_PERF: {elapsed:.1}s vs best {best:.1}s = {factor:.2} ({commit})");
            assert!(
                factor <= tolerance,
                "SYNC_PERF: REGRESSION {elapsed:.1}s is {factor:.2} of this machine's \
                 best {best:.1}s (tolerance {tolerance:.2}); the stats file is {}",
                stats_path().display()
            );
        }
    }
}

/// The pinned indexer's current chain height over clearnet.
async fn query_tip(indexer: http::Uri) -> u32 {
    let mut grpc = zingo_netutils::GrpcIndexer::new(indexer)
        .await
        .expect("the indexer client builds");
    let block = grpc
        .get_latest_block(Duration::from_secs(TIP_QUERY_SECS))
        .await
        .expect("the indexer reports its tip");
    u32::try_from(block.height).expect("the chain height fits a u32")
}

/// This development environment's stats file, under the XDG cache root.
fn stats_path() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("SYNC_PERF_STATS_DIR") {
        return std::path::PathBuf::from(dir).join("sync-perf.tsv");
    }
    let cache_root = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME is set")).join(".cache")
        });
    cache_root.join("zingolib").join("sync-perf.tsv")
}

/// The current commit, short-hashed, suffixed `-dirty` for an unclean tree.
fn commit_stamp() -> String {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let hash = output(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    match output(&["status", "--porcelain"]) {
        Some(status) if status.is_empty() => hash,
        _ => format!("{hash}-dirty"),
    }
}

/// The fastest sync this machine has recorded for the same window and indexer.
fn best_recorded(indexer: &http::Uri) -> Option<f64> {
    let raw = std::fs::read_to_string(stats_path()).ok()?;
    raw.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let (_ts, _commit, window, host, seconds) = (
                fields.next()?,
                fields.next()?,
                fields.next()?,
                fields.next()?,
                fields.next()?,
            );
            (window.parse::<u32>().ok()? == GUARD_WINDOW && host == *indexer)
                .then(|| seconds.parse::<f64>().ok())?
        })
        .fold(None, |best, seconds| match best {
            Some(current) if current <= seconds => Some(current),
            _ => Some(seconds),
        })
}

/// Appends this run to the stats file, creating its directory on first use.
fn record(indexer: &http::Uri, commit: &str, birthday: u32, seconds: f64) {
    let path = stats_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the stats directory creates");
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .expect("the stats file opens");
    writeln!(
        file,
        "{stamp}\t{commit}\t{GUARD_WINDOW}\t{indexer}\t{seconds:.1}\t{birthday}"
    )
    .expect("the stats file appends");
}
