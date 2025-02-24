//! Errors assoicated with [`crate::lightclient::LightClient`].

#[derive(Debug, thiserror::Error)]
pub enum LightClientError {
    /// Sync not running.
    #[error("No sync handle. Sync is not running.")]
    SyncNotRunning,
    /// Sync failed.
    #[error("Sync failed. {0}")]
    SyncFailed(#[from] pepper_sync::error::SyncError),
    /// gPRC client error
    #[error("gRPC client error. {0}")]
    ClientError(#[from] zingo_netutils::GetClientError),
}
