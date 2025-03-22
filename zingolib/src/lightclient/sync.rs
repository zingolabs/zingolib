//! Sync implementations for [crate::lightclient::LightClient] and related types.

use std::borrow::BorrowMut;
use std::sync::atomic;

use futures::FutureExt;
use pepper_sync::error::SyncError;
use pepper_sync::error::SyncModeError;
use pepper_sync::wallet::SyncMode;
use zingo_netutils::GetClientError;

use crate::data::PollReport;
use crate::wallet::error::WalletError;

use super::error::LightClientError;
use super::LightClient;
use super::SyncResult;

impl LightClient {
    /// Launches a task for syncing the wallet to the latest state of the block chain, storing the handle in the
    /// `sync_handle` field.
    // TODO: add realtime sync updates to zingo-cli when it can handle printing during user input
    pub async fn sync(
        &mut self,
        transparent_address_discovery: bool,
    ) -> Result<(), GetClientError> {
        let client = zingo_netutils::GrpcConnector::new(self.config.get_lightwalletd_uri())
            .get_client()
            .await?;
        let network = self.wallet.lock().await.network;
        let wallet = self.wallet.clone();
        let sync_mode = self.sync_mode.clone();
        let sync_handle = tokio::spawn(async move {
            pepper_sync::sync(
                client,
                &network,
                wallet,
                sync_mode,
                transparent_address_discovery,
            )
            .await
        });
        self.sync_handle = Some(sync_handle);

        Ok(())
    }

    /// Clear the wallet data obtained from the blockchain and launch sync from wallet birthday.
    pub async fn rescan(
        &mut self,
        transparent_address_discovery: bool,
    ) -> Result<(), GetClientError> {
        self.wallet.lock().await.clear_all();
        self.sync(transparent_address_discovery).await
    }

    /// Returns the lightclient's sync mode in non-atomic (enum) form.
    pub fn sync_mode(&self) -> SyncMode {
        SyncMode::from_atomic_u8(self.sync_mode.clone())
            .expect("this library does not allow setting of non-valid sync mode variants")
    }

    /// Pause the sync engine, releasing the wallet lock until [`crate::lightclient::LightClient::resume_sync`] is called.
    pub fn pause_sync(&self) -> Result<(), SyncModeError> {
        if self.sync_mode() != SyncMode::Running {
            return Err(SyncModeError::SyncNotRunning);
        }
        self.sync_mode
            .store(SyncMode::Paused as u8, atomic::Ordering::Release);

        Ok(())
    }

    /// Resume scanning after [`crate::lightclient::LightClient::pause_sync`] has been called.
    pub fn resume_sync(&self) -> Result<(), SyncModeError> {
        if self.sync_mode() != SyncMode::Paused {
            return Err(SyncModeError::SyncNotPaused);
        }
        self.sync_mode
            .store(SyncMode::Running as u8, atomic::Ordering::Release);

        Ok(())
    }

    /// Polls the sync task, returning [`self::PollReport`].
    pub fn poll_sync(&mut self) -> PollReport<SyncResult, SyncError<WalletError>> {
        if let Some(mut sync_handle) = self.sync_handle.take() {
            if let Some(sync_result) = sync_handle.borrow_mut().now_or_never() {
                PollReport::Ready(sync_result.expect("task panicked"))
            } else {
                self.sync_handle = Some(sync_handle);
                PollReport::NotReady
            }
        } else {
            PollReport::NoHandle
        }
    }

    /// Awaits until sync has completed
    /// Returns [`pepper_sync::sync::SyncResult`] if successful.
    /// Returns [`crate::lightclient::error::LightClientError`] on failure.
    pub async fn await_sync(&mut self) -> Result<SyncResult, LightClientError> {
        Ok(self
            .sync_handle
            .take()
            .ok_or(LightClientError::SyncNotRunning)?
            .await
            .expect("task panicked")?)
    }

    /// Calls [`crate::lightclient::LightClient::sync`] and then [`crate::lightclient::LightClient::await_sync`].
    pub async fn sync_and_await(
        &mut self,
        transparent_address_discovery: bool,
    ) -> Result<SyncResult, LightClientError> {
        self.sync(transparent_address_discovery).await?;
        self.await_sync().await
    }

    /// Calls [`crate::lightclient::LightClient::rescan`] and then [`crate::lightclient::LightClient::await_sync`].
    pub async fn rescan_and_await(
        &mut self,
        transparent_address_discovery: bool,
    ) -> Result<SyncResult, LightClientError> {
        self.rescan(transparent_address_discovery).await?;
        self.await_sync().await
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

        let sync_result = lc.sync_and_await(true).await.unwrap();
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
