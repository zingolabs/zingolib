use tempfile::TempDir;
use zingolib::{
    config::{construct_lightwalletd_uri, load_clientconfig, DEFAULT_LIGHTWALLETD_SERVER},
    lightclient::LightClient,
    testvectors::seeds::HOSPITAL_MUSEUM_SEED,
    wallet::WalletBase,
};

#[tokio::test]
async fn sync_test() {
    let uri = construct_lightwalletd_uri(Some(DEFAULT_LIGHTWALLETD_SERVER.to_string()));
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();
    let config = load_clientconfig(
        uri,
        Some(temp_path),
        zingolib::config::ChainType::Mainnet,
        true,
    )
    .unwrap();
    let lightclient = LightClient::create_from_wallet_base_async(
        WalletBase::from_string(HOSPITAL_MUSEUM_SEED.to_string()),
        &config,
        2_590_000,
        true,
    )
    .await
    .unwrap();

    lightclient.do_sync(true).await.unwrap();
}
