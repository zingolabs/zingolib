//! Sync implementations for [`crate::lightclient::LightClient`] and related types.

use std::borrow::BorrowMut;
use std::sync::Arc;
use std::sync::atomic;
use std::time::Duration;

use futures::FutureExt;
use pepper_sync::error::{SyncError, SyncModeError, SyncRecoveryObservables};
use pepper_sync::wallet::SyncMode;
use tokio::time::MissedTickBehavior;
use tokio::time::interval;
use tokio::time::timeout;

use crate::data::PollReport;
use crate::wallet::error::WalletError;

use super::LightClient;
use super::SyncResult;
use super::error::LightClientError;

use zingo_netutils::time::SYNC_START_TIMEOUT;

impl LightClient {
    /// Launches a task for syncing the wallet to the latest state of the block chain, storing the handle in the
    /// `sync_handle` field.
    // TODO: add realtime sync updates to zingo-cli when it can handle printing during user input
    pub async fn sync(&mut self) -> Result<(), LightClientError> {
        if self.sync_mode() != SyncMode::NotRunning {
            return Err(LightClientError::SyncModeError(
                SyncModeError::SyncAlreadyRunning,
            ));
        }

        let client = self.require_indexer()?.clone();
        let chain_type = self.chain_type();
        let sync_config = self
            .wallet()
            .read()
            .await
            .wallet_settings
            .sync_config
            .clone();
        let wallet = self.wallet().clone();
        let sync_mode = self.sync_mode.clone();
        let (progress_sender, progress_receiver) = tokio::sync::watch::channel(None);
        self.sync_progress = progress_receiver;
        let sync_handle = tokio::spawn(async move {
            pepper_sync::sync(
                client,
                &chain_type,
                wallet,
                sync_mode,
                progress_sender,
                sync_config,
            )
            .await
        });
        self.sync_handle = Some(sync_handle);

        if timeout(SYNC_START_TIMEOUT, async {
            let mut interval = interval(Duration::from_millis(50));
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            interval.tick().await;
            while self.sync_mode() == SyncMode::NotRunning {
                interval.tick().await;
            }
        })
        .await
        .is_err()
        {
            match self.poll_sync() {
                PollReport::Ready(sync_result) => {
                    // sync can only return an error in this context so ignore success and return errror.
                    let _ignore_sync_success = sync_result?;
                }
                PollReport::NotReady => {
                    // this error should never be reached.
                    // timeout only occurs when sync has returned an error so handle will always be ready.
                    // cleans up sync handle and sets sync mode back to 'not running'.
                    if let Some(sync_handle) = self.sync_handle.take() {
                        sync_handle.abort();
                        let _ignore_cancelled = sync_handle.await;
                    }
                    self.sync_mode
                        .store(SyncMode::NotRunning as u8, atomic::Ordering::Release);
                    return Err(LightClientError::SyncLaunchError);
                }
                PollReport::NoHandle => {
                    // this error should never be reached.
                    // sync handle must exist in this scope.
                    return Err(LightClientError::SyncLaunchError);
                }
            }
        }

        Ok(())
    }

    /// Clear the wallet data obtained from the blockchain and launch sync from wallet birthday.
    ///
    /// If sync is already running, stops sync and waits for it to shutdown before clearing the wallet and rescanning.
    pub async fn rescan(&mut self) -> Result<(), LightClientError> {
        if self.sync_mode() != SyncMode::NotRunning {
            self.stop_sync().expect("infallible in this scope");

            let mut interval = tokio::time::interval(Duration::from_millis(500));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            while matches!(self.poll_sync(), PollReport::NotReady) {
                interval.tick().await;
            }
        }
        self.wallet().write().await.clear_all();
        self.sync().await
    }

    /// Aborts any in-flight sync task and awaits its cancellation.
    pub(crate) async fn abort_sync(&mut self) {
        if let Some(sync_handle) = self.sync_handle.take() {
            sync_handle.abort();
            let _cancelled = sync_handle.await;
        }
        self.sync_mode
            .store(SyncMode::NotRunning as u8, atomic::Ordering::Release);
    }

    /// Returns the sync engine's most recently published status without touching the wallet lock.
    // TODO: return result with status error
    pub fn latest_sync_status(&self) -> Option<pepper_sync::sync::SyncStatus> {
        self.sync_progress.borrow().clone()
    }

    /// Returns the lightclient's sync mode in non-atomic (enum) form.
    pub fn sync_mode(&self) -> SyncMode {
        SyncMode::from_atomic_u8(self.sync_mode.clone())
            .expect("this library does not allow setting of non-valid sync mode variants")
    }

    /// Pause the sync engine, releasing the wallet lock until [`crate::lightclient::LightClient::resume_sync`] is called.
    ///
    /// Returns an error if sync is not running or paused.
    pub fn pause_sync(&self) -> Result<(), SyncModeError> {
        if self.sync_mode() != SyncMode::Running {
            return Err(SyncModeError::SyncNotRunning);
        }
        self.sync_mode
            .store(SyncMode::Paused as u8, atomic::Ordering::Release);

        Ok(())
    }

    /// Stop the sync engine after the next batch is scanned.
    ///
    /// Returns an error if sync is not running.
    pub fn stop_sync(&self) -> Result<(), SyncModeError> {
        if self.sync_mode() == SyncMode::NotRunning {
            return Err(SyncModeError::SyncNotRunning);
        }
        self.sync_mode
            .store(SyncMode::Shutdown as u8, atomic::Ordering::Release);

        Ok(())
    }

    /// Resume scanning after [`crate::lightclient::LightClient::pause_sync`] has been called.
    ///
    /// Returns an error if sync is not paused.
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
                self.sync_mode
                    .store(SyncMode::NotRunning as u8, atomic::Ordering::Release);
                PollReport::Ready(sync_result.expect("task panicked"))
            } else {
                self.sync_handle = Some(sync_handle);
                PollReport::NotReady
            }
        } else {
            self.sync_mode
                .store(SyncMode::NotRunning as u8, atomic::Ordering::Release);
            PollReport::NoHandle
        }
    }

    /// Awaits until sync has successfully completed or failed.
    /// Returns [`pepper_sync::sync::SyncResult`] if successful.
    /// Returns [`crate::lightclient::error::LightClientError`] on failure.
    pub async fn await_sync(&mut self) -> Result<SyncResult, LightClientError> {
        // Completion-detection quantum: at 500ms this added up to half a
        // second of pure quantization to every sync_and_await; the sync
        // engine's own session on a regtest chain runs ~1.4s, so the poll
        // interval was a visible fraction of every test's sync cost.
        let mut interval = tokio::time::interval(Duration::from_millis(50));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match self.poll_sync() {
                PollReport::NoHandle => return Err(LightClientError::SyncNotRunning),
                PollReport::NotReady => (),
                PollReport::Ready(result) => {
                    let result = result.map_err(LightClientError::SyncError);
                    if result.is_ok() {
                        // Boundary checkpoints are only retained for a finite
                        // window; capture migration part witnesses while they
                        // are available.
                        self.wallet().write().await.refresh_part_witnesses()?;
                    }
                    return result;
                }
            }
        }
    }

    /// Calls [`crate::lightclient::LightClient::sync`] and then [`crate::lightclient::LightClient::await_sync`].
    pub async fn sync_and_await(&mut self) -> Result<SyncResult, LightClientError> {
        self.sync().await?;
        self.await_sync().await
    }

    /// Calls [`crate::lightclient::LightClient::rescan`] and then [`crate::lightclient::LightClient::await_sync`].
    pub async fn rescan_and_await(&mut self) -> Result<SyncResult, LightClientError> {
        self.rescan().await?;
        self.await_sync().await
    }

    /// Pauses the sync engine and returns the guard that proves it.
    ///
    /// A running engine is paused and resumes when the guard drops. A
    /// paused or not-running engine needs no pause, so the guard
    /// changes nothing. In particular, dropping it never resumes a pause
    /// somebody else established. A shutting-down engine still scans its
    /// final batch, so it is *not* paused. The call fails with
    /// [`SyncModeError::SyncAlreadyRunning`] and the caller should await
    /// shutdown first.
    pub fn pause_sync_scoped(&self) -> Result<SyncPauseGuard, SyncModeError> {
        loop {
            let resume_on_drop = match self.sync_mode() {
                SyncMode::Running => {
                    if self
                        .sync_mode
                        .compare_exchange(
                            SyncMode::Running as u8,
                            SyncMode::Paused as u8,
                            atomic::Ordering::AcqRel,
                            atomic::Ordering::Acquire,
                        )
                        .is_err()
                    {
                        // The engine changed state between the read and the
                        // exchange; reclassify from the fresh mode.
                        continue;
                    }
                    true
                }
                SyncMode::Paused | SyncMode::NotRunning => false,
                SyncMode::Shutdown => return Err(SyncModeError::SyncAlreadyRunning),
            };
            return Ok(SyncPauseGuard {
                sync_mode: self.sync_mode.clone(),
                resume_on_drop,
            });
        }
    }

    /// Pauses the engine for the lifetime of a stored proposal, keeping
    /// the guard in the client until the proposal is consumed (sent or
    /// calculated), cleared, or fails to come into existence. Returns
    /// whether this call minted the guard, so a proposing call's error
    /// path can release exactly what it took while an already-held guard
    /// keeps guarding the proposal it was minted for. A shutting-down
    /// engine cannot be paused. The proposal then proceeds unguarded,
    /// exactly as the imperative pause discipline did.
    pub(crate) fn hold_proposal_pause(&mut self) -> bool {
        if self.proposal_pause_guard.is_none()
            && let Ok(guard) = self.pause_sync_scoped()
        {
            self.proposal_pause_guard = Some(guard);
            return true;
        }
        false
    }

    /// Releases the stored proposal's pause guard, if one is held.
    /// `restore: true` drops the guard, returning the engine to the mode
    /// it held before the proposal was created. `restore: false` disarms
    /// it, leaving the engine paused for the caller to resume, the
    /// shipped `resume_sync: false` semantics.
    pub(crate) fn release_proposal_pause(&mut self, restore: bool) {
        if let Some(guard) = self.proposal_pause_guard.take()
            && !restore
        {
            guard.disarm();
        }
    }

    /// Polls the sync task and, if it failed, returns the recommended
    /// recovery action alongside the error description.
    ///
    /// This is the primary entry point for consumers (CLI, mobile, PC)
    /// that need to decide whether to retry, switch servers, or give up
    /// after a sync failure.
    ///
    /// Returns `None` if sync has not been launched, is still running,
    /// or completed successfully.
    pub fn poll_sync_recovery(&mut self) -> Option<(SyncRecoveryObservables, String)> {
        match self.poll_sync() {
            PollReport::Ready(Err(e)) => {
                let action = e.recovery_recommendation();
                let description =
                    zingo_net_diag::chain_texts(&e).join(super::migrate::CAUSE_CHAIN_SEPARATOR);
                Some((action, description))
            }
            _ => None,
        }
    }
}

/// A guard proving the sync engine is paused (actively paused or not running)
/// for as long as this value lives.
///
/// [`LightClient::pause_sync_scoped`] is the only constructor: it performs the
/// one side effect (pausing a running engine) up front and undoes it on drop. A
/// function that takes `&SyncPauseGuard` performs no sync-mode transitions of
/// its own. The parameter is pure proof that the caller already paused
/// sync, so wallet state cannot shift under the callee between its reads and
/// its writes. That turns the requirement into a compile-time contract: the
/// racy call shape (planning against a wallet a running sync is still
/// mutating) has no well-typed spelling.
pub struct SyncPauseGuard {
    sync_mode: Arc<atomic::AtomicU8>,
    resume_on_drop: bool,
}

impl SyncPauseGuard {
    /// Cancels the restore-on-drop: the engine stays exactly as the guard
    /// held it, so a pause taken by [`LightClient::pause_sync_scoped`] persists
    /// past the guard. Crate-internal on purpose, since the public contract
    /// remains "drop restores the prior mode". Only the shipped
    /// `resume_sync: false` protocol needs to keep an engine paused for the
    /// caller to resume later.
    pub(crate) fn disarm(mut self) {
        self.resume_on_drop = false;
    }
}

impl Drop for SyncPauseGuard {
    fn drop(&mut self) {
        if self.resume_on_drop {
            // Resume only if the engine is still paused: a stop_sync issued
            // while the guard was held must win over the resume.
            let _ignore_raced_transition = self.sync_mode.compare_exchange(
                SyncMode::Paused as u8,
                SyncMode::Running as u8,
                atomic::Ordering::AcqRel,
                atomic::Ordering::Acquire,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use pepper_sync::wallet::SyncMode;

    use super::atomic;
    use crate::lightclient::LightClient;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

    /// An offline client whose sync-mode atomic the tests drive directly.
    /// No engine runs, so every observed transition is the guard's own.
    async fn offline_client(mode: SyncMode) -> LightClient {
        let wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
        let client = LightClient::new_for_test(wallet).await;
        client
            .sync_mode
            .store(mode as u8, atomic::Ordering::Release);
        client
    }

    #[tokio::test]
    async fn running_pauses_and_drop_resumes() {
        let client = offline_client(SyncMode::Running).await;
        let guard = client.pause_sync_scoped().expect("a running engine pauses");
        assert_eq!(client.sync_mode(), SyncMode::Paused);
        drop(guard);
        assert_eq!(client.sync_mode(), SyncMode::Running);
    }

    #[tokio::test]
    async fn not_running_is_a_no_op_guard() {
        let client = offline_client(SyncMode::NotRunning).await;
        let guard = client.pause_sync_scoped().expect("no engine needs a pause");
        assert_eq!(client.sync_mode(), SyncMode::NotRunning);
        drop(guard);
        assert_eq!(client.sync_mode(), SyncMode::NotRunning);
    }

    #[tokio::test]
    async fn paused_guard_does_not_steal_the_resume() {
        let client = offline_client(SyncMode::Paused).await;
        let guard = client
            .pause_sync_scoped()
            .expect("a paused engine needs no pause");
        drop(guard);
        assert_eq!(
            client.sync_mode(),
            SyncMode::Paused,
            "whoever paused the engine owns its resume"
        );
    }

    #[tokio::test]
    async fn shutdown_cannot_be_paused() {
        let client = offline_client(SyncMode::Shutdown).await;
        assert!(
            client.pause_sync_scoped().is_err(),
            "a shutting-down engine still scans its final batch"
        );
    }

    #[tokio::test]
    async fn stop_while_held_wins_over_the_resume() {
        let client = offline_client(SyncMode::Running).await;
        let guard = client.pause_sync_scoped().expect("a running engine pauses");
        client.stop_sync().expect("a paused engine can be stopped");
        drop(guard);
        assert_eq!(
            client.sync_mode(),
            SyncMode::Shutdown,
            "the drop must not overwrite a shutdown request"
        );
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
        zingo_netutils::ensure_default_crypto_provider();

        let mut lc = wallet_case.load_example_wallet().await;

        let sync_result = lc.sync_and_await().await.unwrap();
        tracing::info!("{sync_result}");
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
}
