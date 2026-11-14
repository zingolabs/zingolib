use crate::{lightclient::LightClient, wallet::disk::testing::examples};

/// loads a wallet from example data
/// turns on the internet tube
/// and syncs to the present blockchain moment
pub(crate) async fn sync_example_wallet(wallet_case: examples::NetworkSeedVersion) -> LightClient {
    zl_netutils::ensure_default_crypto_provider();

    let mut lc = wallet_case.load_example_wallet_online().await;

    let sync_result = lc.sync_and_await().await.unwrap();
    tracing::info!(
        "{}",
        crate::perspective::sync_progress::SessionReport::from(sync_result)
    );
    tracing::info!("{:?}", lc.account_balance(zip32::AccountId::ZERO).await);
    lc
}

mod testnet {
    use super::{examples, sync_example_wallet};
    /// this is a live sync test. its execution time scales linearly since last updated
    #[ignore = "live chain experiment"]
    #[tokio::test]
    async fn testnet_sync_mskmgdbhotbpetcjwcspgopp() {
        sync_example_wallet(examples::NetworkSeedVersion::Testnet(
            examples::TestnetSeedVersion::MobileShuffle(examples::MobileShuffleVersion::Latest),
        ))
        .await;
    }
    /// this is a live sync test. its execution time scales linearly since last updated
    #[ignore = "live chain experiment"]
    #[tokio::test]
    async fn testnet_sync_cbbhrwiilgbrababsshsmtpr() {
        sync_example_wallet(examples::NetworkSeedVersion::Testnet(
            examples::TestnetSeedVersion::ChimneyBetter(examples::ChimneyBetterVersion::Latest),
        ))
        .await;
    }
}
/// this is a live sync test. its execution time scales linearly since last updated
#[tokio::test]
#[ignore = "testnet and mainnet tests should be ignored due to increasingly large execution times"]
async fn mainnet_sync() {
    sync_example_wallet(examples::NetworkSeedVersion::Mainnet(
        examples::MainnetSeedVersion::HotelHumor(examples::HotelHumorVersion::Gf0aaf9347),
    ))
    .await;
}
