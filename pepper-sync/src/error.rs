//! Top level error module for the crate

use crate::{scan::error::ScanError, sync::error::MempoolError};

/// Top level error enum encapsulating any error that may occur during sync
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// Scan error.
    #[error("Scan error. {0}")]
    ScanError(#[from] ScanError),
    /// Mempool error.
    #[error("Mempool error. {0}")]
    MempoolError(#[from] MempoolError),
}
