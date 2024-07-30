use tempfile::TempDir;
use zingo_netutils::GrpcConnector;
use zingo_sync::sync::sync;
use zingo_testvectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingoconfig::{construct_lightwalletd_uri, load_clientconfig, DEFAULT_LIGHTWALLETD_SERVER};
use zingolib::{lightclient::LightClient, wallet::WalletBase};

#[tokio::test]
async fn sync_test() {
    tracing_subscriber::fmt().init();

    let uri = construct_lightwalletd_uri(Some(DEFAULT_LIGHTWALLETD_SERVER.to_string()));
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();
    let config = load_clientconfig(
        uri.clone(),
        Some(temp_path),
        zingoconfig::ChainType::Mainnet,
        true,
    )
    .unwrap();
    let mut lightclient = LightClient::create_from_wallet_base_async(
        WalletBase::from_string(HOSPITAL_MUSEUM_SEED.to_string()),
        &config,
        2_590_000,
        true,
    )
    .await
    .unwrap();

    let client = GrpcConnector::new(uri).get_client().await.unwrap();

    sync(client, &config.chain, &mut lightclient.wallet)
        .await
        .unwrap();
}
