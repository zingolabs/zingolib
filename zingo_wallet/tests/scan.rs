pub(crate) use core::time::Duration;

use libtonode_tests::chain_generics::RegtestEnvironment;
use zcash_wallet_interface::Wallet as _;
use zingo_wallet::ZingoWallet;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::TestnetEnvironment;

async fn sleepy_scan_n<CC>(duration: usize)
where
    CC: ConductChain,
{
    let chain = CC::setup().await;
    let single_server_url = chain.lightserver_uri().unwrap().to_string();
    let mut wallet = ZingoWallet::new_wallet().await;
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
    tokio::time::sleep(Duration::from_secs(10)).await;
    let later_scan_height = wallet
        .get_max_scanned_height_for_server(single_server_url.to_string())
        .await
        .unwrap();
    tracing::debug!("{later_scan_height:#?}");
    assert!(initial_scan_height <= later_scan_height);
}

#[tokio::test]
#[test_log::test]
async fn regtest_10_seconds() {
    sleepy_scan_n::<RegtestEnvironment>(10).await;
}

#[tokio::test]
#[test_log::test]
async fn testnet_10_seconds() {
    sleepy_scan_n::<TestnetEnvironment>(10).await;
}
