//! Top level error module for the crate

use crate::scan::error::ScanError;

/// Top level error enum encapsulating any error that may occur during sync
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// Errors associated with scanning
    #[error("Scan error. {0}")]
    ScanError(#[from] ScanError),
}
