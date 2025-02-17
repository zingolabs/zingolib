//! AWISOTT
//! LightClient sync stuff.
//! the difference between this and wallet/sync.rs is that these can interact with the network layer.

use std::sync::atomic;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

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
    /// Sync the wallet to the latest state of the block chain.
    pub async fn do_sync(&self, print_updates: bool) -> Result<SyncResult, String> {
        let client = zingo_netutils::GrpcConnector::new(self.config.get_lightwalletd_uri())
            .get_client()
            .await
            .unwrap();
        let network = self.wallet.lock().await.network;
        let wallet = self.wallet.clone();
        let sync_handle = tokio::spawn(async move {
            pepper_sync::sync(client, &network, wallet).await.unwrap();
        });

        // FIXME: replace with lightclient syncing field
        let syncing = Arc::new(AtomicBool::new(true));
        if print_updates {
            let syncing = syncing.clone();
            let wallet = self.wallet.clone();
            tokio::spawn(async move {
                loop {
                    if !syncing.load(atomic::Ordering::Acquire) {
                        break;
                    };

                    let sync_status = pepper_sync::sync_status(wallet.clone()).await;
                    println!("{}", sync_status);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            });
        }

        sync_handle.await.unwrap();
        syncing.store(false, atomic::Ordering::Release);

        let final_sync_status = pepper_sync::sync_status(self.wallet.clone()).await;
        if print_updates {
            println!("{}", &final_sync_status);
        }

        Ok(SyncResult {
            success: true,
            latest_block: (final_sync_status
                .scan_ranges
                .last()
                .expect("should be non-empty after syncing")
                .block_range()
                .end
                - 1)
            .into(),
            total_blocks_synced: final_sync_status.scanned_blocks as u64,
        })
    }

    /// Clear the wallet state and rescan from wallet birthday.
    pub async fn do_rescan(&self) -> Result<SyncResult, String> {
        debug!("Rescan starting");

        self.wallet.lock().await.clear_all();

        let response = self.do_sync(false).await;

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

        let lc = wallet_case.load_example_wallet_with_client().await;

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
