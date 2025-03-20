//! Errors assoicated with [`crate::lightclient::LightClient`].

use crate::wallet::{error::WalletError, LightWallet};

#[derive(Debug, thiserror::Error)]
pub enum LightClientError {
    /// Sync not running.
    #[error("No sync handle. Sync is not running.")]
    SyncNotRunning,
    /// Sync failed.
    #[error("Sync failed. {0}")]
    SyncFailed(#[from] pepper_sync::error::SyncError<LightWallet>),
    /// gPRC client error
    #[error("gRPC client error. {0}")]
    ClientError(#[from] zingo_netutils::GetClientError),
    /// File error
    #[error("File error. {0}")]
    FileError(#[from] std::io::Error),
    /// Wallet error
    #[error("Wallet error. {0}")]
    WalletError(#[from] WalletError),
}
