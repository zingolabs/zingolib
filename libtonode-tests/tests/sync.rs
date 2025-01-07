use std::time::Duration;

use tempfile::TempDir;
use testvectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingo_netutils::GrpcConnector;
use zingo_sync::sync::{self, sync};
use zingolib::{
    config::{construct_lightwalletd_uri, load_clientconfig, DEFAULT_LIGHTWALLETD_SERVER},
    get_base_address_macro,
    lightclient::LightClient,
    testutils::{lightclient::from_inputs, scenarios},
    wallet::WalletBase,
};

#[ignore = "temporary mainnet test for sync development"]
#[tokio::test]
async fn sync_mainnet_test() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Ring to work as a default");
    tracing_subscriber::fmt().init();

    let uri = construct_lightwalletd_uri(Some(DEFAULT_LIGHTWALLETD_SERVER.to_string()));
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();
    let config = load_clientconfig(
        uri.clone(),
        Some(temp_path),
        zingolib::config::ChainType::Mainnet,
        true,
    )
    .unwrap();
    let lightclient = LightClient::create_from_wallet_base_async(
        WalletBase::from_string(HOSPITAL_MUSEUM_SEED.to_string()),
        &config,
        2_650_318,
        true,
    )
    .await
    .unwrap();

    let client = GrpcConnector::new(uri).get_client().await.unwrap();

    sync(client, &config.chain, lightclient.wallet.clone())
        .await
        .unwrap();

    let wallet = lightclient.wallet.lock().await;
    // dbg!(&wallet.wallet_blocks);
    // dbg!(&wallet.nullifier_map);
    dbg!(&wallet.sync_state);
}

#[ignore = "mainnet test for large chain"]
#[tokio::test]
async fn sync_status() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Ring to work as a default");
    tracing_subscriber::fmt().init();

    let uri = construct_lightwalletd_uri(Some(DEFAULT_LIGHTWALLETD_SERVER.to_string()));
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();
    let config = load_clientconfig(
        uri.clone(),
        Some(temp_path),
        zingolib::config::ChainType::Mainnet,
        true,
    )
    .unwrap();
    let lightclient = LightClient::create_from_wallet_base_async(
        WalletBase::from_string(HOSPITAL_MUSEUM_SEED.to_string()),
        &config,
        2_750_000,
        // 2_670_000,
        true,
    )
    .await
    .unwrap();

    let client = GrpcConnector::new(uri).get_client().await.unwrap();

    let wallet = lightclient.wallet.clone();
    let sync_handle = tokio::spawn(async move {
        sync(client, &config.chain, wallet).await.unwrap();
    });

    let wallet = lightclient.wallet.clone();
    tokio::spawn(async move {
        loop {
            let wallet = wallet.clone();
            let sync_status = sync::sync_status(wallet).await;
            dbg!(sync_status);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    sync_handle.await.unwrap();
}

// temporary test for sync development
#[tokio::test]
async fn sync_test() {
    tracing_subscriber::fmt().init();

    let (_regtest_manager, _cph, faucet, recipient, _txid) =
        scenarios::faucet_funded_recipient_default(5_000_000).await;
    from_inputs::quick_send(
        &faucet,
        vec![(
            &get_base_address_macro!(&recipient, "transparent"),
            100_000,
            None,
        )],
    )
    .await
    .unwrap();
    // from_inputs::quick_send(
    //     &recipient,
    //     vec![(
    //         &get_base_address_macro!(&faucet, "unified"),
    //         100_000,
    //         Some("Outgoing decrypt test"),
    //     )],
    // )
    // .await
    // .unwrap();

    // increase_server_height(&regtest_manager, 1).await;
    // recipient.do_sync(false).await.unwrap();
    // recipient.quick_shield().await.unwrap();
    // increase_server_height(&regtest_manager, 1).await;

    let uri = recipient.config().lightwalletd_uri.read().unwrap().clone();
    let client = GrpcConnector::new(uri).get_client().await.unwrap();
    sync(client, &recipient.config().chain.clone(), recipient.wallet)
        .await
        .unwrap();

    // dbg!(&recipient.wallet.wallet_transactions);
    // dbg!(recipient.wallet.wallet_blocks());
    // dbg!(recipient.wallet.nullifier_map());
    // dbg!(recipient.wallet.outpoint_map());
    // dbg!(recipient.wallet.sync_state());
}
