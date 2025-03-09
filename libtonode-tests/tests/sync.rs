use tempfile::TempDir;
use testvectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingolib::{
    config::{construct_lightwalletd_uri, load_clientconfig, DEFAULT_LIGHTWALLETD_SERVER},
    get_base_address_macro,
    lightclient::LightClient,
    testutils::{
        increase_height_and_wait_for_client, increase_server_height,
        lightclient::from_inputs::{self, quick_send},
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

    lightclient.sync_and_await().await.unwrap();

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

    lightclient.sync_and_await().await.unwrap();
}

// temporary test for sync development
#[ignore = "sync development only"]
#[tokio::test]
async fn sync_test() {
    tracing_subscriber::fmt().init();

    let (regtest_manager, _cph, faucet, mut recipient, _txid) =
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

    increase_server_height(&regtest_manager, 1).await;
    recipient.sync_and_await().await.unwrap();
    recipient.quick_shield().await.unwrap();
    increase_server_height(&regtest_manager, 1).await;
    recipient.sync_and_await().await.unwrap();

    // let wallet = recipient.wallet.lock().await;
    // dbg!(&wallet.wallet_transactions);
    // dbg!(&wallet.wallet_blocks);
    // dbg!(&wallet.nullifier_map);
    // dbg!(&wallet.outpoint_map);
    // dbg!(&wallet.sync_state);
}

#[ignore = "sync and zingo 2.0 dev temp test"]
#[tokio::test]
async fn initial_frontier_test() {
    let (regtest_manager, _cph, mut faucet, mut recipient, _txid) =
        scenarios::faucet_funded_recipient_default(100_000).await;

    // increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 3)
    //     .await
    //     .unwrap();
    // println!("{}", recipient.do_balance().await);
    // println!("{}", recipient.transaction_summaries().await);
    // println!("{:#?}", recipient.wallet.lock().await.shard_trees.sapling);
    // println!("{:#?}", recipient.wallet.lock().await.shard_trees.orchard);
    quick_send(
        &recipient,
        vec![(&get_base_address_macro!(&faucet, "sapling"), 50_000, None)],
    )
    .await
    .unwrap();
    // increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 3)
    //     .await
    //     .unwrap();
    // quick_send(
    //     &faucet,
    //     vec![(
    //         &get_base_address_macro!(&recipient, "sapling"),
    //         100_000,
    //         None,
    //     )],
    // )
    // .await
    // .unwrap();
}
