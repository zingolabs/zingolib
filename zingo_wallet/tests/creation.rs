use std::time::Duration;

use zcash_wallet_interface::Wallet as _;
use zingo_wallet::ZingoWallet;
use zingolib::config::DEFAULT_TESTNET_LIGHTWALLETD_SERVER;

#[test_group::group(live)]
#[test_group::group(live_testnet)]
#[tokio::test]
async fn create_default() {
    let mut wallet = ZingoWallet::new_wallet().await;
    let single_server_url = DEFAULT_TESTNET_LIGHTWALLETD_SERVER;
    wallet
        .add_key(zingo_test_vectors::seeds::ABANDON_ART_SEED.to_string())
        .await
        .unwrap();
    wallet
        .add_server(single_server_url.to_string())
        .await
        .unwrap();
}
#[test_group::group(live)]
#[test_group::group(live_testnet)]
#[tokio::test]
async fn create_scan() {
    let mut wallet = ZingoWallet::new_wallet().await;
    let single_server_url = DEFAULT_TESTNET_LIGHTWALLETD_SERVER;
    wallet
        .add_key(zingo_test_vectors::seeds::ABANDON_ART_SEED.to_string())
        .await
        .unwrap();
    wallet
        .add_server(single_server_url.to_string())
        .await
        .unwrap();
    let initial_scan_height = wallet
        .get_max_scanned_height_for_server(single_server_url.to_string())
        .await
        .unwrap();
    tracing::debug!("{initial_scan_height:#?}");
    tokio::time::sleep(Duration::from_secs(10));
    let later_scan_height = wallet
        .get_max_scanned_height_for_server(single_server_url.to_string())
        .await
        .unwrap();
    tracing::debug!("{later_scan_height:#?}");
    assert!(initial_scan_height < later_scan_height);
}
