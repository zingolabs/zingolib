use zingoconfig::load_clientconfig;
use zingoconfig::ChainType::Mainnet;
use zingolib::lightclient::LightClient;
use zingolib::lightclient::PoolBalances;
use zingolib::wallet::WalletBase;

#[tokio::test]
async fn balance() {
    let dir = tempdir::TempDir::new("zingo_live_test")
        .unwrap()
        .into_path();
    let uri = "https://mainnet.lightwalletd.com:9067"
        .to_string()
        .parse()
        .unwrap();
    let config = load_clientconfig(uri, Some(dir), Mainnet, false).unwrap();
    let client = LightClient::create_from_wallet_base_async(
        WalletBase::MnemonicPhrase("daughter safe tonight pull clarify discover gesture sting carry shine cup tourist say six ignore benefit wise argue issue above invest milk holiday source".to_string()),
        &config,
        2202688,
        true,
    )
    .await
    .unwrap();
    client.do_sync(true).await.unwrap();
    let balance = client.do_balance().await;
    assert_eq!(
        balance,
        PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(340000),
            verified_orchard_balance: Some(340000),
            spendable_orchard_balance: Some(340000),
            unverified_orchard_balance: Some(0),
            transparent_balance: Some(0)
        }
    );
}
