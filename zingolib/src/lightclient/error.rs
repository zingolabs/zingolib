//! Errors assoicated with [`crate::lightclient::LightClient`].

use std::convert::Infallible;

use zcash_protocol::TxId;

use pepper_sync::error::{SyncError, SyncModeError};
use zingo_netutils::GetClientError;

use crate::wallet::{
    error::{CalculateTransactionError, ProposeSendError, ProposeShieldError, WalletError},
    output::OutputRef,
};

#[derive(Debug, thiserror::Error)]
pub enum LightClientError {
    /// Sync failed to launch..
    #[error("Sync failed to launch.")]
    SyncLaunchError,
    /// Sync not running.
    #[error("No sync handle. Sync is not running.")]
    SyncNotRunning,
    /// Sync error.
    #[error("Sync error. {0}")]
    SyncError(#[from] SyncError<WalletError>),
    /// Sync mode error.
    #[error("Sync mode error. {0}")]
    SyncModeError(#[from] SyncModeError),
    /// Send error.
    #[error("Send error. {0}")]
    SendError(#[from] SendError),
    /// gPRC client error.
    #[error("gRPC client error. {0}")]
    ClientError(#[from] GetClientError),
    /// File error.
    #[error("File error. {0}")]
    FileError(std::io::Error),
    /// Wallet error.
    #[error("Wallet error. {0}")]
    WalletError(#[from] WalletError),
    /// Ironwood migration error.
    #[error("Ironwood migration error. {0}")]
    MigrationError(String),
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// Propose send error.
    #[error("Propose send error. {0}")]
    ProposeSendError(#[from] ProposeSendError),
    /// Propose shield error.
    #[error("Propose shield error. {0}")]
    ProposeShieldError(#[from] ProposeShieldError),
    /// Failed to construct sending transaction.
    #[error("Failed to construct sending transaction. {0}")]
    CalculateSendError(CalculateTransactionError<OutputRef>),
    /// Failed to construct shielding transaction.
    #[error("Failed to construct shielding transaction. {0}")]
    CalculateShieldError(CalculateTransactionError<Infallible>),
    /// No proposal found in the wallet.
    #[error("No proposal found in the wallet.")]
    NoStoredProposal,
    /// Transmission error.
    #[error("Transmission error. {0}")]
    TransmissionError(#[from] TransmissionError),
}

#[derive(Debug, thiserror::Error)]
pub enum TransmissionError {
    /// Transmission failed.
    #[error("Transmission failed. {0}")]
    TransmissionFailed(String),
    /// Transaction to transmit does not have `Calculated` status: {0}
    #[error("Transaction to transmit does not have `Calculated` status: {0}")]
    IncorrectTransactionStatus(TxId),
    /// Txid reported by server does not match calculated txid.
    #[error(
        "Server error: txid reported by the server does not match calculated txid.\ncalculated txid:\n{0}\ntxid from server: {1}"
    )]
    IncorrectTxidFromServer(TxId, TxId),
}
