use tempfile::TempDir;
use zingo_netutils::GrpcConnector;
use zingo_sync::sync::sync;
use zingo_testutils::scenarios;
use zingo_testvectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingoconfig::{construct_lightwalletd_uri, load_clientconfig, DEFAULT_LIGHTWALLETD_SERVER};
use zingolib::{lightclient::LightClient, wallet::WalletBase};

#[tokio::test]
async fn sync_mainnet_test() {
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
    let lightclient = LightClient::create_from_wallet_base_async(
        WalletBase::from_string(HOSPITAL_MUSEUM_SEED.to_string()),
        &config,
        2_590_000,
        true,
    )
    .await
    .unwrap();

    let client = GrpcConnector::new(uri).get_client().await.unwrap();

    sync(client, &config.chain, &lightclient.wallet)
        .await
        .unwrap();

    dbg!(lightclient.wallet.compact_blocks());
}
#[tokio::test]
async fn sync_test() {
    tracing_subscriber::fmt().init();

    let (_regtest_manager, _cph, _faucet, recipient, _txid) =
        scenarios::orchard_funded_recipient(5_000_000).await;
    let uri = recipient.config().lightwalletd_uri.read().unwrap().clone();

    let client = GrpcConnector::new(uri).get_client().await.unwrap();

    sync(client, &recipient.config().chain.clone(), &recipient.wallet)
        .await
        .unwrap();

    dbg!(recipient.wallet.compact_blocks());
}
