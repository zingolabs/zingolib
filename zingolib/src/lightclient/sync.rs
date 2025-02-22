//! AWISOTT
//! LightClient sync stuff.
//! the difference between this and wallet/sync.rs is that these can interact with the network layer.

use std::sync::atomic;
use std::sync::atomic::AtomicU8;
use std::sync::Arc;
use std::time::Duration;

use log::debug;
use pepper_sync::wallet::SyncMode;
use zingo_netutils::GetClientError;

use super::LightClient;
use super::SyncResult;

impl LightClient {
    /// Calls [`self::sync`] and awaits the handle.
    // TODO: error handling, concrete error types
    pub async fn sync_and_await(&mut self, print_updates: bool) -> Result<SyncResult, String> {
        self.sync(print_updates).await.map_err(|e| e.to_string())?;

        Ok(self
            .sync_handle
            .take()
            .expect("handle should always exist after calling `sync`")
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?)
    }

    /// Launches a task for syncing the wallet to the latest state of the block chain, storing the handle in the
    /// `sync_handle` field.
    pub async fn sync(&mut self, print_updates: bool) -> Result<(), GetClientError> {
        let client = zingo_netutils::GrpcConnector::new(self.config.get_lightwalletd_uri())
            .get_client()
            .await?;
        let network = self.wallet.lock().await.network;
        let wallet = self.wallet.clone();
        let sync_mode = self.sync_mode.clone();
        let sync_handle =
            tokio::spawn(
                async move { pepper_sync::sync(client, &network, wallet, sync_mode).await },
            );
        self.sync_handle = Some(sync_handle);

        // TODO: replace this with calls to `sync status` in zingo-cli
        if print_updates {
            let sync_mode = self.sync_mode.clone();
            let wallet = self.wallet.clone();
            tokio::spawn(async move {
                loop {
                    if LightClient::sync_mode(sync_mode.clone()) == SyncMode::NotRunning {
                        break;
                    };

                    let sync_status = pepper_sync::sync_status(&*wallet.lock().await).await;
                    println!("{}", sync_status);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            });
        }

        Ok(())
    }

    /// Clear the wallet state and rescan from wallet birthday.
    pub async fn do_rescan(&mut self) -> Result<SyncResult, String> {
        debug!("Rescan starting");

        self.wallet.lock().await.clear_all();

        let response = self.sync_and_await(false).await;

        debug!("Rescan finished");

        response
    }

    /// Creates [`pepper_sync::wallet::SyncMode`] from an atomic u8.
    pub fn sync_mode(atomic_sync_mode: Arc<AtomicU8>) -> SyncMode {
        SyncMode::from_u8(atomic_sync_mode.load(atomic::Ordering::Acquire))
            .expect("this library does not allow setting of non-valid sync mode variants")
    }

    // TODO: split into separate functions with checks
    pub fn set_sync_mode(&self, sync_mode: SyncMode) {
        self.sync_mode
            .store(sync_mode as u8, atomic::Ordering::Release);
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

        let mut lc = wallet_case.load_example_wallet_with_client().await;

        let sync_result = lc.sync_and_await(false).await.unwrap();
        println!("{}", sync_result);
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
