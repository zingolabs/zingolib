use zcash_wallet_interface::Wallet as _;
use zingo_wallet::ZingoWallet;
use zingolib::config::DEFAULT_TESTNET_LIGHTWALLETD_SERVER;

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
    // wallet
    //     .get_max_scanned_height_for_server(single_server_url.to_string())
    //     .await
    //     .unwrap();
}
