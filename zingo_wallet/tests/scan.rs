pub(crate) use core::time::Duration;

use libtonode_tests::chain_generics::RegtestEnvironment;
use zcash_wallet_interface::{BlockHeight, Wallet as _};
use zingo_wallet::ZingoWallet;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::TestnetEnvironment;
use zingolib::wallet::disk::testing::examples::{NetworkSeedVersion, TestnetSeedVersion};

async fn sleepy_scan(single_server_url: String, duration: u64) {
    let mut wallet = ZingoWallet::new_wallet().await;
    wallet
        .add_key(zingo_test_vectors::seeds::ABANDON_ART_SEED.to_string())
        .await
        .unwrap();
    wallet
        .begin_scanning_server_range(single_server_url.to_string(), None, None)
        .await
        .unwrap();
    let initial_scan_height = wallet
        .get_max_scanned_height_for_server(single_server_url.to_string())
        .await
        .unwrap();
    tracing::debug!("{initial_scan_height:#?}");
    tokio::time::sleep(Duration::from_secs(duration)).await;
    let later_scan_height = wallet
        .get_max_scanned_height_for_server(single_server_url.to_string())
        .await
        .unwrap();
    tracing::debug!("{later_scan_height:#?}");
    assert!(initial_scan_height <= later_scan_height);
}

async fn sleepy_scan_fixture<CC>(duration: u64)
where
    CC: ConductChain,
{
    let chain = CC::setup().await;
    sleepy_scan(chain.lightserver_uri().unwrap().to_string(), duration).await;
}

#[tokio::test]
#[test_log::test]
async fn regtest_10_seconds() {
    sleepy_scan_fixture::<RegtestEnvironment>(10).await;
}

#[tokio::test]
#[test_log::test]
async fn testnet_10_seconds() {
    sleepy_scan_fixture::<TestnetEnvironment>(10).await;
}

// NOTE: zingolib doesnt stop scanning, this test just ends when the specified height is reached.
async fn scan_wallet_range(single_server_url: String, seed: String, start: u32, end: u32) {
    let mut wallet = ZingoWallet::new_wallet().await;
    wallet.add_key(seed).await.unwrap();
    wallet
        .begin_scanning_server_range(
            single_server_url.to_string(),
            Some(BlockHeight(start)),
            None,
        )
        .await
        .unwrap();
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let later_scan_height = wallet
            .get_max_scanned_height_for_server(single_server_url.to_string())
            .await
            .unwrap()
            .0;
        tracing::info!("Scanning at block {later_scan_height}");
        if later_scan_height >= end {
            dbg!(
                wallet
                    .lightclient
                    .unwrap()
                    .wallet
                    .read()
                    .await
                    .birthday
                    .to_string()
            );
            break;
        }
    }
}

#[tokio::test]
#[test_log::test]
async fn testnet_gg() {
    let chain = TestnetEnvironment::setup().await;
    // a Testnet wallet with 25 transactions over 2000 blocks.
    scan_wallet_range(
        chain.lightserver_uri().unwrap().to_string(),
        NetworkSeedVersion::Testnet(TestnetSeedVersion::GloryGoddess).example_wallet_base(),
        3_070_800u32,
        3_072_799u32,
    )
    .await;
}
