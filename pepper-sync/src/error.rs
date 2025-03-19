//! Top level error module for the crate

use zcash_primitives::{block::BlockHash, consensus::BlockHeight};

/// Top level error enumerating any error that may occur during sync
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// Scan error.
    #[error("Scan error. {0}")]
    ScanError(#[from] ScanError),
    /// Mempool error.
    #[error("Mempool error. {0}")]
    MempoolError(#[from] MempoolError),
}

/// Mempool errors.
#[derive(Debug, thiserror::Error)]
pub enum MempoolError {
    /// gRPC request failed to fetch mempool stream.
    #[error("gRPC request failed to fetch mempool stream.")]
    RequestFailed(tonic::Status),
    /// Timed out fetching mempool stream during shutdown.
    #[error("Timed out fetching mempool stream during shutdown.\nNON-CRITICAL: Sync completed successfully but may not have scanned transactions in the mempool.")]
    ShutdownWithoutStream,
}

/// Scan errors.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    /// Continuity error.
    #[error("Continuity error. {0}")]
    ContinuityError(#[from] ContinuityError),
}

/// Block continuity errors.
#[derive(Debug, thiserror::Error)]
pub enum ContinuityError {
    /// Height discontinuity.
    #[error("Height discontinuity. Block with height {height} is not continuous with previous block height {previous_block_height}")]
    HeightDiscontinuity {
        /// Block height
        height: BlockHeight,
        /// Previous block height
        previous_block_height: BlockHeight,
    },
    /// Hash discontinuity.
    #[error("Hash discontinuity. Block prev_hash {prev_hash} with height {height} does not match previous block hash {previous_block_hash}")]
    HashDiscontinuity {
        /// Block height
        height: BlockHeight,
        /// Block's previous block hash data
        prev_hash: BlockHash,
        /// Actual previous block hash
        previous_block_hash: BlockHash,
    },
}
