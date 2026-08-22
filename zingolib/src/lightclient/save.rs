//! Wallet persistence for [`crate::lightclient::LightClient`].

use futures::FutureExt as _;
use log::error;

use std::{borrow::BorrowMut as _, fs::remove_file, sync::atomic};

use super::LightClient;
use crate::data::PollReport;

/// The outcome of a save-task shutdown request, distinguishing a stopped task from an absent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveShutdown {
    /// A running save task was stopped after its final save completed.
    ShutDown,
    /// No save task was running.
    NotRunning,
}

/// Writes `bytes` to file at `wallet_path` using a blocking thread.
///
/// The write is atomic: bytes go to a `.tmp` sibling file first, then renamed into place.
async fn write_to_path(wallet_path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let wallet_path = wallet_path.to_path_buf();
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || {
        let temp_wallet_path: std::path::PathBuf = wallet_path.with_extension(
            wallet_path
                .extension()
                .map(|e| format!("{}.tmp", e.to_string_lossy()))
                .unwrap_or_else(|| "tmp".to_string()),
        );
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp_wallet_path)?;
        let mut writer = std::io::BufWriter::new(file);
        std::io::Write::write_all(&mut writer, &bytes)?;

        let file = writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        std::fs::rename(&temp_wallet_path, &wallet_path)?;

        // NOTE: in windows no need to sync the folder, only for linux & macOS.
        #[cfg(unix)]
        {
            if let Some(parent) = wallet_path.parent() {
                let wallet_dir = std::fs::File::open(parent)?;
                let _ignore_error = wallet_dir.sync_all(); // NOTE: error is ignored as syncing dirs on windows OS may return an error
            }
        }

        Ok(())
    })
    .await
    .expect("write_to_path task panicked")
}
impl LightClient {
    /// Launches a task for saving the wallet data to persistance when the wallet's `save_required` flag is set.
    pub async fn save_task(&mut self) {
        if self.save_active.load(atomic::Ordering::Acquire) {
            return;
        }

        self.save_active.store(true, atomic::Ordering::Release);
        let save_active = self.save_active.clone();
        let wallet = self.wallet().clone();
        let wallet_path = self.wallet_path();
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let save_handle = tokio::spawn(async move {
            loop {
                interval.tick().await;
                let mut wallet_guard = wallet.write().await;
                if let Some(wallet_bytes) = wallet_guard.save()? {
                    write_to_path(&wallet_path, &wallet_bytes).await?;
                    wallet_guard.save_required = false;
                }
                if !save_active.load(atomic::Ordering::Acquire) {
                    return Ok(());
                }
            }
        });
        self.save_handle = Some(save_handle);
    }

    /// Wait until the wallet's `save_required` flag is not set.
    pub async fn wait_for_save(&self) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if !self.wallet().read().await.save_required {
                return;
            }
        }
    }

    /// Polls the save task, returning [`self::PollReport`].
    fn poll_save_task(&mut self) -> PollReport<(), std::io::Error> {
        if let Some(mut save_handle) = self.save_handle.take() {
            if let Some(save_result) = save_handle.borrow_mut().now_or_never() {
                PollReport::Ready(save_result.expect("task panicked"))
            } else {
                self.save_handle = Some(save_handle);
                PollReport::NotReady
            }
        } else {
            PollReport::NoHandle
        }
    }

    /// Checks the save task handle in case of failure.
    /// On save task failure, restarts the save task and returns the error.
    pub async fn check_save_error(&mut self) -> std::io::Result<()> {
        match self.poll_save_task() {
            PollReport::Ready(save_result) => {
                if save_result.is_err() {
                    self.save_task().await;
                }
                save_result
            }
            _ => Ok(()),
        }
    }

    /// Stops the save task after its in-flight save completes, reporting whether a task was running at all.
    pub async fn shutdown_save_task(&mut self) -> std::io::Result<SaveShutdown> {
        self.save_active.store(false, atomic::Ordering::Release);
        if let Some(save_handle) = self.save_handle.take() {
            save_handle.await.expect("task panicked")?;
            Ok(SaveShutdown::ShutDown)
        } else {
            Ok(SaveShutdown::NotRunning)
        }
    }

    /// Immediately serialises and writes the wallet to disk if `save_required` is set.
    ///
    /// Unlike [`Self::save_task`], this is synchronous and does not start a background loop.
    /// Use it when an immediate, one-shot persist is needed (e.g. before process exit or in tests).
    pub async fn flush(&mut self) -> std::io::Result<()> {
        if let Some(bytes) = self.wallet().write().await.save()? {
            write_to_path(&self.wallet_path(), &bytes).await?;
        }
        Ok(())
    }

    /// Only relevant in non-mobile, this function removes the save file.
    // TodO: can we shred it?
    pub async fn delete_wallet_file(&self) -> std::io::Result<()> {
        // Check if the file exists before attempting to delete
        if self.wallet_path().exists() {
            match remove_file(self.wallet_path()) {
                Ok(()) => {
                    log::debug!("File deleted successfully!");
                    Ok(())
                }
                Err(e) => {
                    error!("ERR: {e}");
                    log::debug!("DELETE FAIL ON FILE!");
                    Err(e)
                }
            }
        } else {
            let err = std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "File does not exist, nothing to delete.",
            );
            error!("{err}");
            log::debug!("File does not exist, nothing to delete.");
            Err(err)
        }
    }
}
