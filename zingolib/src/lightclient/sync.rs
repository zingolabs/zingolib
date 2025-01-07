//! AWISOTT
//! LightClient sync stuff.
//! the difference between this and wallet/sync.rs is that these can interact with the network layer.

use log::{debug, error};

use super::LightClient;
use super::SyncResult;

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
/// likely no errors here. but this makes clippy (and fv) happier
pub enum StartMempoolMonitorError {
    #[error("Mempool Monitor is disabled.")]
    Disabled,
    #[error("could not read mempool monitor: {0}")]
    CouldNotRead(String),
    #[error("could not write mempool monitor: {0}")]
    CouldNotWrite(String),
    #[error("Mempool Monitor does not exist.")]
    DoesNotExist,
}

impl LightClient {
    /// TODO: Add Doc Comment Here!
    pub async fn do_sync(&self, _print_updates: bool) -> Result<SyncResult, String> {
        todo!();
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_rescan(&self) -> Result<SyncResult, String> {
        debug!("Rescan starting");

        self.clear_state().await;

        // Then, do a sync, which will force a full rescan from the initial state
        let response = self.do_sync(true).await;

        if response.is_ok() {
            // self.save_internal_rust().await?;
        }

        debug!("Rescan finished");

        response
    }
}

#[cfg(test)]
pub mod test {
    use crate::{lightclient::LightClient, wallet::disk::testing::examples};

    /// loads a wallet from example data
    /// turns on the internet tube
    /// and syncs to the present blockchain moment
    pub(crate) async fn sync_example_wallet(
        wallet_case: examples::NetworkSeedVersion,
    ) -> LightClient {
        // install default crypto provider (ring)
        if let Err(e) = rustls::crypto::ring::default_provider().install_default() {
            log::error!("Error installing crypto provider: {:?}", e)
        };

        let wallet = wallet_case.load_example_wallet().await;
        let lc = LightClient::create_from_wallet_async(wallet).await.unwrap();
        lc.do_sync(true).await.unwrap();
        println!("{:?}", lc.do_balance().await);
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
}
