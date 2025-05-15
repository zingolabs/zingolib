use pepper_sync::sync::{SyncConfig, TransparentAddressDiscovery};
use tempfile::TempDir;
use testvectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingolib::{
    config::{DEFAULT_LIGHTWALLETD_SERVER, construct_lightwalletd_uri, load_clientconfig},
    get_base_address_macro,
    lightclient::LightClient,
    testutils::{
        lightclient::from_inputs::{self},
        scenarios,
    },
    wallet::{LightWallet, WalletBase, WalletSettings},
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
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
            },
        },
        1.try_into().unwrap(),
    )
    .unwrap();
    let mut lightclient = LightClient::create_from_wallet(
        LightWallet::new(
            config.chain,
            WalletBase::from_string(HOSPITAL_MUSEUM_SEED.to_string()),
            2_650_318.into(),
            config.wallet_settings.clone(),
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
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
            },
        },
        1.try_into().unwrap(),
    )
    .unwrap();
    let mut lightclient = LightClient::create_from_wallet(
        LightWallet::new(
            config.chain,
            WalletBase::from_string(HOSPITAL_MUSEUM_SEED.to_string()),
            2_496_152.into(),
            config.wallet_settings.clone(),
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
#[allow(unused_mut, unused_variables)]
#[tokio::test]
async fn sync_test() {
    tracing_subscriber::fmt().init();

    let (regtest_manager, _cph, mut faucet, mut recipient, _txid) =
        scenarios::faucet_funded_recipient_default(5_000_000).await;

    // let recipient_ua = get_base_address_macro!(&recipient, "unified");
    let recipient_taddr = get_base_address_macro!(&recipient, "transparent");
    from_inputs::quick_send(&mut faucet, vec![(&recipient_taddr, 100_000, None)])
        .await
        .unwrap();

    recipient.sync_and_await().await.unwrap();

    // increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
    //     .await
    //     .unwrap();

    // println!("{}", recipient.transaction_summaries().await.unwrap());
    println!("{}", recipient.value_transfers().await.unwrap());
    println!("{}", recipient.do_balance().await);
    println!("{:?}", recipient.propose_shield().await);

    // println!(
    //     "{:?}",
    //     recipient
    //         .get_spendable_shielded_balance(
    //             zcash_address::ZcashAddress::try_from_encoded(&recipient_ua).unwrap(),
    //             false
    //         )
    //         .await
    //         .unwrap()
    // );
    // let wallet = recipient.wallet.lock().await;
    // dbg!(wallet.wallet_blocks.len());
}
