//! Pepper sync error module

use std::array::TryFromSliceError;

use zcash_client_backend::PoolType;
use zcash_primitives::{block::BlockHash, consensus::BlockHeight, transaction::TxId};

/// Top level error enumerating any error that may occur during sync
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// Mempool error.
    #[error("Mempool error. {0}")]
    MempoolError(#[from] MempoolError),
    /// Scan error.
    #[error("Scan error. {0}")]
    ScanError(#[from] ScanError),
    /// Server error.
    #[error("Server error. {0}")]
    ServerError(#[from] ClientError),
}

/// Mempool errors.
#[derive(Debug, thiserror::Error)]
pub enum MempoolError {
    /// Client error.
    #[error("Client error. {0}")]
    ClientError(#[from] ClientError),
    /// Timed out fetching mempool stream during shutdown.
    #[error("Timed out fetching mempool stream during shutdown.\nNON-CRITICAL: Sync completed successfully but may not have scanned transactions in the mempool.")]
    ShutdownWithoutStream,
}

/// Scan errors.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    /// Client error.
    #[error("Client error. {0}")]
    ClientError(#[from] ClientError),
    /// Continuity error.
    #[error("Continuity error. {0}")]
    ContinuityError(#[from] ContinuityError),
    /// Zcash client backend scan error
    #[error("{0}")]
    ZcbScanError(zcash_client_backend::scanning::ScanError),
    /// Invalid sapling nullifier
    #[error("Invalid sapling nullifier. {0}")]
    InvalidSaplingNullifier(#[from] TryFromSliceError),
    /// Invalid orchard nullifier length
    #[error("Invalid orchard nullifier length. Should be 32 bytes, found {0}")]
    InvalidOrchardNullifierLength(usize),
    /// Invalid orchard nullifier
    #[error("Invalid orchard nullifier.")]
    InvalidOrchardNullifier,
    /// Invalid sapling output
    // TODO: add output data
    #[error("Invalid sapling output.")]
    InvalidSaplingOutput,
    /// Invalid orchard action
    // TODO: add output data
    #[error("Invalid orchard action.")]
    InvalidOrchardAction,
    /// Incorrect tree size
    #[error("Incorrect tree size. {shielded_protocol} tree size recorded in block metadata {block_metadata_size} does not match calculated size {calculated_size}")]
    IncorrectTreeSize {
        /// Shielded protocol
        shielded_protocol: PoolType,
        /// Block metadata size
        block_metadata_size: u32,
        /// Calculated size
        calculated_size: u32,
    },
    /// Txid of transaction returned by the server does not match requested txid.
    #[error("Txid of transaction returned by the server does not match requested txid.\nTxid requested: {txid_requested}\nTxid returned: {txid_returned}")]
    IncorrectTxid {
        /// Txid requested
        txid_requested: TxId,
        /// Txid returned
        txid_returned: TxId,
    },
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

/// Client errors.
///
/// Errors associated with connecting to the server and parsing or converting retrieved data.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Failed to read raw transaction
    #[error("Failed to read frontiers. {0}")]
    FrontierReadError(std::io::Error),
    /// Failed to read raw transaction
    #[error("Failed to read raw transaction. {0}")]
    TransactionReadError(std::io::Error),
    /// Request from server failed
    #[error("Server error. {0}")]
    RequestFailed(#[from] tonic::Status),
}
