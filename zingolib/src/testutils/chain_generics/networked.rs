//! - Dials the public testnet indexer.
//! - Spends from the shared GloryGoddess testnet wallet, so only one test may use this environment at a time.
//! - Creates temporary wallet directories that live as long as the environment.
//! - Logs every phase, every sync's scan state, and every awaited block through `tracing`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use http::Uri;

use zcash_protocol::consensus::BlockHeight;

use zingo_netutils::Indexer as _;

use super::conduct_chain::ConductChain;
use crate::config::{ChainType, ClientConfig, WalletConfig};
use crate::lightclient::{DEFAULT_REQUEST_TIMEOUT, LightClient};
use crate::testutils::lightclient::from_inputs;
use crate::testutils::{default_test_wallet_settings, tempfile, timed, timestamped_test_log};
use crate::wallet::LightWallet;
use crate::wallet::disk::testing::examples;

pub const TESTNET_INDEXER: &str = "https://testnet.zec.rocks:443";
const FAUCET: examples::NetworkSeedVersion =
    examples::NetworkSeedVersion::Testnet(examples::TestnetSeedVersion::GloryGoddess);
const BLOCK_POLL: Duration = Duration::from_secs(5);
const PROGRESS_INTERVAL: Duration = Duration::from_secs(10);
const CONFIRMATION_PATIENCE_BLOCKS: usize = 10;
const SYNC_ATTEMPTS: usize = 5;
const SYNC_RETRY_PAUSE: Duration = Duration::from_secs(5);
const FAUCET_CACHE_DIR: &str = "target/testnet/faucet";
const ACCOUNTS: std::num::NonZeroU32 = std::num::NonZeroU32::MIN;

/// - Removes its temporary wallet directories when dropped.
pub struct NetworkedTestEnvironment {
    indexer_uri: Uri,
    latest_known_server_height: Option<BlockHeight>,
    wallet_dirs: Vec<tempfile::TempDir>,
}

fn height(value: Option<BlockHeight>) -> String {
    value.map_or_else(
        || "none".to_string(),
        |height| u32::from(height).to_string(),
    )
}

fn scan_state_line(wallet: &LightWallet) -> String {
    let state = &wallet.sync_state;
    format!(
        "birthday {}, fully scanned {}, highest scanned {}, last known chain height {}, {} scan ranges",
        height(state.wallet_birthday()),
        height(state.fully_scanned_height()),
        height(state.highest_scanned_height()),
        height(state.last_known_chain_height()),
        state.scan_ranges().len()
    )
}

/// - Spawns a task that logs the wallet's scan state every ten seconds until the sync returns, then aborts it.
/// - Runs the client's sync to the tip and awaits it, overriding a stored continuous-sync setting for that
///   session, and retries up to five times after a sync error, with a pause between.
/// - Panics when every attempt fails.
async fn sync_with_progress(label: &str, client: &mut LightClient) {
    let wallet = client.wallet().clone();
    timestamped_test_log(
        format!(
            "{label}: before sync, {}",
            scan_state_line(&*wallet.read().await)
        )
        .as_str(),
    );
    let progress = tokio::spawn({
        let wallet = wallet.clone();
        let label = label.to_string();
        async move {
            let started = Instant::now();
            loop {
                tokio::time::sleep(PROGRESS_INTERVAL).await;
                let line = scan_state_line(&*wallet.read().await);
                timestamped_test_log(
                    format!(
                        "{label}: syncing for {:.0}s, {line}",
                        started.elapsed().as_secs_f64()
                    )
                    .as_str(),
                );
            }
        }
    });
    let mut attempt = 1;
    loop {
        let result = timed(
            format!("{label}::sync_to_tip_and_await[{attempt}]").as_str(),
            client.sync_to_tip_and_await(),
        )
        .await;
        match result {
            Ok(_) => break,
            Err(error) if attempt < SYNC_ATTEMPTS => {
                timestamped_test_log(
                    format!(
                        "{label}: sync attempt {attempt} of {SYNC_ATTEMPTS} ended in {error}; the wallet keeps its scan progress, retrying after {}s",
                        SYNC_RETRY_PAUSE.as_secs()
                    )
                    .as_str(),
                );
                attempt += 1;
                tokio::time::sleep(SYNC_RETRY_PAUSE).await;
            }
            Err(error) => {
                progress.abort();
                panic!("{label}: sync failed {SYNC_ATTEMPTS} times, last: {error}");
            }
        }
    }
    progress.abort();
    timestamped_test_log(
        format!(
            "{label}: after sync, {}",
            scan_state_line(&*wallet.read().await)
        )
        .as_str(),
    );
}

/// - Sends one `account_balance` query to the client's wallet.
async fn confirmed_shielded(client: &LightClient) -> u64 {
    let balance = client
        .account_balance(zip32::AccountId::ZERO)
        .await
        .expect("a synced client reports a balance");
    [
        balance.confirmed_orchard_balance,
        balance.confirmed_sapling_balance,
        balance.confirmed_ironwood_balance,
    ]
    .into_iter()
    .map(|value| value.map_or(0, |value| value.into_u64()))
    .sum()
}

impl NetworkedTestEnvironment {
    /// - Sends one `get_latest_block` request to the indexer.
    async fn update_server_height(&mut self) {
        let uri = self
            .lightserver_uri()
            .expect("the networked environment always holds its indexer URI");
        let mut indexer = zingo_netutils::GrpcIndexer::new(uri)
            .await
            .unwrap_or_else(|error| panic!("{TESTNET_INDEXER} refused a connection: {error}"));
        let latest = indexer
            .get_latest_block(DEFAULT_REQUEST_TIMEOUT)
            .await
            .unwrap_or_else(|error| panic!("{TESTNET_INDEXER} served no latest block: {error}"))
            .height as u32;
        self.latest_known_server_height = Some(BlockHeight::from(latest));
    }

    /// - Creates a temporary directory and keeps it until the environment drops.
    fn fresh_wallet_dir(&mut self) -> PathBuf {
        let dir = tempfile::TempDir::new().unwrap_or_else(|error| {
            panic!("the host refused a temporary wallet directory: {error}")
        });
        let path = dir.path().to_path_buf();
        self.wallet_dirs.push(dir);
        path
    }

    fn config_for(&self, wallet_dir: PathBuf) -> crate::config::ClientConfigBuilder {
        ClientConfig::builder()
            .set_indexer_uri(self.indexer_uri.clone())
            .set_chain_type(ChainType::Testnet)
            .set_wallet_dir(wallet_dir)
    }

    fn birthday(&self) -> u32 {
        self.latest_known_server_height
            .map(u32::from)
            .expect("setup read the server height")
    }

    /// - Sends one `account_balance` query to the client's wallet.
    async fn log_balance(label: &str, client: &LightClient) {
        let balance = client
            .account_balance(zip32::AccountId::ZERO)
            .await
            .expect("a synced client reports a balance");
        let zatoshis = |value: Option<zcash_protocol::value::Zatoshis>| {
            value.map_or(0, |value| value.into_u64())
        };
        timestamped_test_log(
            format!(
                "{label}: confirmed balance orchard {} sapling {} transparent {} ironwood {}",
                zatoshis(balance.confirmed_orchard_balance),
                zatoshis(balance.confirmed_sapling_balance),
                zatoshis(balance.confirmed_transparent_balance),
                zatoshis(balance.confirmed_ironwood_balance),
            )
            .as_str(),
        );
    }
}

impl ConductChain for NetworkedTestEnvironment {
    async fn setup() -> Self {
        crate::ensure_default_crypto_provider();
        let mut environment = Self {
            indexer_uri: <Uri as std::str::FromStr>::from_str(TESTNET_INDEXER)
                .expect("the hard-coded testnet indexer constant is a valid URI"),
            latest_known_server_height: None,
            wallet_dirs: Vec::new(),
        };
        timed(
            "networked_setup::update_server_height",
            environment.update_server_height(),
        )
        .await;
        timestamped_test_log(
            format!(
                "networked_setup: indexer {TESTNET_INDEXER}, chain tip {}",
                height(environment.latest_known_server_height)
            )
            .as_str(),
        );
        environment
    }

    /// - On first use, seeds `target/testnet/faucet` under the repository root with a copy of the committed
    ///   GloryGoddess wallet file, so the committed file is never written.
    /// - Opens the cached copy, syncs it to the testnet tip with progress logging, logs its balance, and writes
    ///   the synced wallet back to the cache, so later tests and runs start from the tip instead of the snapshot.
    async fn create_faucet(&mut self) -> LightClient {
        let source = FAUCET.example_wallet_path();
        let file_name = source
            .file_name()
            .expect("every example wallet path ends in the wallet file's name")
            .to_string_lossy()
            .to_string();
        let cache_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("zingolib sits under the repository root")
            .join(FAUCET_CACHE_DIR);
        let cached = cache_dir.join(&file_name);
        if cached.exists() {
            timestamped_test_log(
                format!(
                    "create_faucet: reusing the cached wallet at {}",
                    cached.display()
                )
                .as_str(),
            );
        } else {
            std::fs::create_dir_all(&cache_dir)
                .unwrap_or_else(|error| panic!("cannot create {}: {error}", cache_dir.display()));
            std::fs::copy(&source, &cached).unwrap_or_else(|error| {
                panic!(
                    "cannot seed {} from {}: {error}",
                    cached.display(),
                    source.display()
                )
            });
            timestamped_test_log(
                format!(
                    "create_faucet: seeded {} from the committed snapshot {}",
                    cached.display(),
                    source.display()
                )
                .as_str(),
            );
        }
        let config = self
            .config_for(cache_dir)
            .set_wallet_name(file_name)
            .set_wallet_config(WalletConfig::Read)
            .build()
            .expect("every required field of the faucet config is set above");
        let mut faucet = timed("create_faucet::open", LightClient::new(config, false))
            .await
            .unwrap_or_else(|error| panic!("the cached faucet wallet does not open: {error}"));
        sync_with_progress("create_faucet", &mut faucet).await;
        timed("create_faucet::flush", faucet.flush())
            .await
            .unwrap_or_else(|error| panic!("the synced faucet wallet does not flush: {error}"));
        Self::log_balance("create_faucet", &faucet).await;
        faucet
    }

    async fn zingo_config(&mut self) -> ClientConfig {
        let wallet_dir = self.fresh_wallet_dir();
        timestamped_test_log(
            format!(
                "zingo_config: new wallet in {} born at {}",
                wallet_dir.display(),
                self.birthday()
            )
            .as_str(),
        );
        self.config_for(wallet_dir)
            .set_wallet_config(WalletConfig::NewSeed {
                no_of_accounts: ACCOUNTS,
                chain_height: self.birthday(),
                wallet_settings: default_test_wallet_settings(),
            })
            .build()
            .expect("every required field of the client config is set above")
    }

    /// - Reads the chain height, then polls the indexer every five seconds until it moves, logging the wait.
    async fn increase_chain_height(&mut self) {
        self.update_server_height().await;
        let before_height = self.latest_known_server_height;
        let started = Instant::now();
        loop {
            tokio::time::sleep(BLOCK_POLL).await;
            self.update_server_height().await;
            if self.latest_known_server_height != before_height {
                break;
            }
        }
        timestamped_test_log(
            format!(
                "increase_chain_height: {} -> {} after {:.0}s",
                height(before_height),
                height(self.latest_known_server_height),
                started.elapsed().as_secs_f64()
            )
            .as_str(),
        );
    }

    /// - Opens the faucet and a new client, sends `value` to the client's unified address, then awaits blocks,
    ///   syncing the client after each, until its confirmed shielded balance covers `value`.
    /// - Panics when the confirmation patience runs out first.
    async fn fund_client_orchard(&mut self, value: u64) -> LightClient {
        let mut faucet = timed("fund_client_orchard::create_faucet", self.create_faucet()).await;
        let mut recipient = timed("fund_client_orchard::create_client", self.create_client()).await;
        let address = crate::get_base_address_macro!(recipient, "unified");
        timed(
            "fund_client_orchard::quick_send",
            from_inputs::quick_send(&mut faucet, vec![(address.as_str(), value, None)]),
        )
        .await
        .unwrap_or_else(|error| panic!("the faucet's funding send of {value} failed: {error}"));
        for awaited in 1..=CONFIRMATION_PATIENCE_BLOCKS {
            self.increase_chain_height().await;
            self.sync_client_to_tip(&mut recipient).await;
            let confirmed = confirmed_shielded(&recipient).await;
            timestamped_test_log(
                format!(
                    "fund_client_orchard: block {awaited} of {CONFIRMATION_PATIENCE_BLOCKS}, recipient holds {confirmed} of {value}"
                )
                .as_str(),
            );
            if confirmed >= value {
                return recipient;
            }
        }
        panic!(
            "fund_client_orchard: {value} did not confirm within {CONFIRMATION_PATIENCE_BLOCKS} blocks"
        );
    }

    async fn sync_client_to_tip(&self, client: &mut LightClient) {
        sync_with_progress("sync_client_to_tip", client).await;
    }

    fn lightserver_uri(&self) -> Option<Uri> {
        Some(self.indexer_uri.clone())
    }

    fn confirmation_patience_blocks(&self) -> usize {
        CONFIRMATION_PATIENCE_BLOCKS
    }
}
