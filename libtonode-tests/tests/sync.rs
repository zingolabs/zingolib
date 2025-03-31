use tempfile::TempDir;
use testvectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingolib::{
    config::{construct_lightwalletd_uri, load_clientconfig, DEFAULT_LIGHTWALLETD_SERVER},
    get_base_address_macro,
    lightclient::LightClient,
    testutils::{
        increase_height_and_wait_for_client,
        lightclient::from_inputs::{self},
        scenarios,
    },
    wallet::{LightWallet, WalletBase},
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
    )
    .unwrap();
    let mut lightclient = LightClient::create_from_wallet(
        LightWallet::new(
            config.chain,
            WalletBase::from_string(HOSPITAL_MUSEUM_SEED.to_string()),
            2_650_318.into(),
        )
        .unwrap(),
        config,
        true,
    )
    .unwrap();

    lightclient.sync_and_await(false).await.unwrap();

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
    )
    .unwrap();
    let mut lightclient = LightClient::create_from_wallet(
        LightWallet::new(
            config.chain,
            WalletBase::from_string(HOSPITAL_MUSEUM_SEED.to_string()),
            2_496_152.into(),
        )
        .unwrap(),
        config,
        true,
    )
    .unwrap();

    lightclient.sync_and_await(false).await.unwrap();
}

// temporary test for sync development
#[ignore = "sync development only"]
#[tokio::test]
async fn sync_test() {
    tracing_subscriber::fmt().init();

    let (regtest_manager, _cph, _faucet, mut recipient, _txid) =
        scenarios::faucet_funded_recipient_default(5_000_000).await;

    from_inputs::quick_send(
        &recipient,
        vec![(
            &get_base_address_macro!(&recipient, "unified"),
            100_000,
            None,
            // Some("send to self test"),
        )],
    )
    .await
    .unwrap();

    increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
        .await
        .unwrap();

    println!("{}", recipient.transaction_summaries().await);
    println!("{}", recipient.value_transfers().await);
    // let wallet = recipient.wallet.lock().await;
    // dbg!(&wallet.wallet_transactions);
    // dbg!(&wallet.wallet_blocks);
    // dbg!(&wallet.nullifier_map);
    // dbg!(&wallet.outpoint_map);
    // dbg!(&wallet.sync_state);
}
