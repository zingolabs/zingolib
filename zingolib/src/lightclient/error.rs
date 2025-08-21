//! Errors assoicated with [`crate::lightclient::LightClient`].

use std::convert::Infallible;

use pepper_sync::error::SyncModeError;

use crate::wallet::{
    error::{
        CalculateTransactionError, ProposeSendError, ProposeShieldError, TransmissionError,
        WalletError,
    },
    output::OutputRef,
};

#[derive(Debug, thiserror::Error)]
pub enum LightClientError {
    /// Sync not running.
    #[error("No sync handle. Sync is not running.")]
    SyncNotRunning,
    /// Sync error.
    #[error("Sync error. {0}")]
    SyncError(#[from] pepper_sync::error::SyncError<WalletError>),
    /// Sync mode error.
    #[error("sync mode error. {0}")]
    SyncModeError(#[from] SyncModeError),
    /// gPRC client error
    #[error("gRPC client error. {0}")]
    ClientError(#[from] zingo_netutils::GetClientError),
    /// File error
    #[error("File error. {0}")]
    FileError(#[from] std::io::Error),
    /// Wallet error
    #[error("Wallet error. {0}")]
    WalletError(#[from] WalletError),
    /// Tor client error.
    #[error("Tor client error. {0}")]
    TorClientError(#[from] zcash_client_backend::tor::Error),
}

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("The sending transaction could not be calculated. {0}")]
    CalculateSendError(CalculateTransactionError<OutputRef>),
    #[error("The shieldng transaction could not be calculated. {0}")]
    CalculateShieldError(CalculateTransactionError<Infallible>),
    #[error("Transmission failed. {0}")]
    TransmissionError(#[from] TransmissionError),
    #[error("No proposal found.")]
    NoStoredProposal,
}

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
pub enum QuickSendError {
    #[error("proposal failed. {0}")]
    ProposalError(#[from] ProposeSendError),
    #[error("send failed. {0}")]
    SendError(#[from] SendError),
}

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
pub enum QuickShieldError {
    #[error("proposal failed. {0}")]
    ProposalError(#[from] ProposeShieldError),
    #[error("send failed. {0}")]
    SendError(#[from] SendError),
}
