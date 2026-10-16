use std::time::Duration;

use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingolib::config::{ChainType, ClientConfig, WalletConfig};
use zingolib::lightclient::LightClient;
use zingolib::perspective::sync_progress;
use zingolib::testutils::{default_test_wallet_settings, tempfile::TempDir};

use super::TESTNET_INDEXER;

const RELOADS: u8 = 20;
const BIRTHDAY: u32 = 2_000_000;
const PROGRESS_POLL: Duration = Duration::from_millis(100);

fn testnet_uri() -> http::Uri {
    TESTNET_INDEXER
        .parse::<http::Uri>()
        .expect("the hard-coded testnet indexer constant is a valid URI")
}

#[tokio::test]
async fn reload_wallet_after_short_sync() {
    zingolib::ensure_default_crypto_provider();

    for _ in 0..RELOADS {
        let wallet_dir = TempDir::new().expect("the host provides a temporary wallet directory");
        let config = ClientConfig::builder()
            .set_chain_type(ChainType::Testnet)
            .set_indexer_uri(testnet_uri())
            .set_wallet_config(WalletConfig::MnemonicPhrase {
                mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().expect("hard-coded non-zero"),
                birthday: BIRTHDAY,
                wallet_settings: default_test_wallet_settings(),
            })
            .set_wallet_dir(wallet_dir.path().to_path_buf())
            .build()
            .expect("every required field of the config is set above");

        let mut lightclient = LightClient::new(config, true)
            .await
            .expect("a fresh testnet client opens");
        lightclient.save_task().await;
        lightclient.sync().await.expect("the sync session launches");
        let mut interval = tokio::time::interval(PROGRESS_POLL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        while lightclient.latest_sync_status().is_none_or(|status| {
            sync_progress::SyncProgress::from(status).percentage_total_outputs_scanned < 1.0
        }) {
            interval.tick().await;
        }
        lightclient
            .stop_sync()
            .expect("a running sync session accepts a stop");
        lightclient
            .await_sync()
            .await
            .expect("the stopped session returns");
        lightclient
            .shutdown_save_task()
            .await
            .expect("the save task flushes and exits");

        // A reload fails on any write that did not flush or sync the file.
        let config = ClientConfig::builder()
            .set_chain_type(ChainType::Testnet)
            .set_indexer_uri(testnet_uri())
            .set_wallet_config(WalletConfig::Read)
            .set_wallet_dir(wallet_dir.path().to_path_buf())
            .build()
            .expect("every required field of the config is set above");
        LightClient::new(config, true)
            .await
            .expect("the saved wallet reloads");
    }
}
