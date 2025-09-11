//! Pepper sync error module

use std::{array::TryFromSliceError, convert::Infallible};

use shardtree::error::ShardTreeError;
use zcash_primitives::{block::BlockHash, consensus::BlockHeight, transaction::TxId};
use zcash_protocol::PoolType;

use crate::wallet::OutputId;

/// Top level error enumerating any error that may occur during sync
#[derive(Debug, thiserror::Error)]
pub enum SyncError<E>
where
    E: std::fmt::Debug + std::fmt::Display,
{
    /// Mempool error.
    #[error("mempool error. {0}")]
    MempoolError(#[from] MempoolError),
    /// Scan error.
    #[error("scan error. {0}")]
    ScanError(#[from] ScanError),
    /// Server error.
    #[error("server error. {0}")]
    ServerError(#[from] ServerError),
    /// Sync mode error.
    #[error("sync mode error. {0}")]
    SyncModeError(#[from] SyncModeError),
    /// Chain error.
    #[error("wallet height is more than {0} blocks ahead of best chain height")]
    ChainError(u32),
    /// Shard tree error.
    #[error("shard tree error. {0}")]
    ShardTreeError(#[from] ShardTreeError<Infallible>),
    /// Transparent address derivation error.
    #[error("transparent address derivation error. {0}")]
    TransparentAddressDerivationError(bip32::Error),
    /// Wallet error.
    #[error("wallet error. {0}")]
    WalletError(E),
}

/// Sync status errors.
#[derive(Debug, thiserror::Error)]
pub enum SyncStatusError<E>
where
    E: std::fmt::Debug + std::fmt::Display,
{
    /// No sync data. Wallet has never been synced with the block chain.
    #[error("No sync data. Wallet has never been synced with the block chain.")]
    NoSyncData,
    /// Wallet error.
    #[error("wallet error. {0}")]
    WalletError(E),
}

/// Mempool errors.
#[derive(Debug, thiserror::Error)]
pub enum MempoolError {
    /// Server error.
    #[error("server error. {0}")]
    ServerError(#[from] ServerError),
    /// Timed out fetching mempool stream during shutdown.
    #[error(
        "timed out fetching mempool stream during shutdown.\nNON-CRITICAL: sync completed successfully but may not have scanned transactions in the mempool."
    )]
    ShutdownWithoutStream,
}

/// Scan errors.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    /// Server error.
    #[error("server error. {0}")]
    ServerError(#[from] ServerError),
    /// Continuity error.
    #[error("continuity error. {0}")]
    ContinuityError(#[from] ContinuityError),
    /// Zcash client backend scan error
    #[error("{0}")]
    ZcbScanError(zcash_client_backend::scanning::ScanError),
    /// Invalid sapling nullifier
    #[error("invalid sapling nullifier. {0}")]
    InvalidSaplingNullifier(#[from] TryFromSliceError),
    /// Invalid orchard nullifier length
    #[error("invalid orchard nullifier length. should be 32 bytes, found {0}")]
    InvalidOrchardNullifierLength(usize),
    /// Invalid orchard nullifier
    #[error("invalid orchard nullifier")]
    InvalidOrchardNullifier,
    /// Invalid sapling output
    // TODO: add output data
    #[error("invalid sapling output")]
    InvalidSaplingOutput,
    /// Invalid orchard action
    // TODO: add output data
    #[error("invalid orchard action")]
    InvalidOrchardAction,
    /// Incorrect tree size
    #[error(
        "incorrect tree size. {shielded_protocol} tree size recorded in block metadata {block_metadata_size} does not match calculated size {calculated_size}"
    )]
    IncorrectTreeSize {
        /// Shielded protocol
        shielded_protocol: PoolType,
        /// Block metadata size
        block_metadata_size: u32,
        /// Calculated size
        calculated_size: u32,
    },
    /// Txid of transaction returned by the server does not match requested txid.
    #[error(
        "txid of transaction returned by the server does not match requested txid.\ntxid requested: {txid_requested}\ntxid returned: {txid_returned}"
    )]
    IncorrectTxid {
        /// Txid requested
        txid_requested: TxId,
        /// Txid returned
        txid_returned: TxId,
    },
    /// Decrypted note nullifier and position data not found.
    #[error("decrypted note nullifier and position data not found. output id: {0:?}")]
    DecryptedNoteDataNotFound(OutputId),
    /// Invalid memo bytes..
    #[error("invalid memo bytes. {0}")]
    InvalidMemoBytes(#[from] zcash_primitives::memo::Error),
    /// Failed to parse encoded address.
    #[error("failed to parse encoded address. {0}")]
    AddressParseError(#[from] zcash_address::unified::ParseError),
}

/// Block continuity errors.
#[derive(Debug, thiserror::Error)]
pub enum ContinuityError {
    /// Height discontinuity.
    #[error(
        "height discontinuity. block with height {height} is not continuous with previous block height {previous_block_height}"
    )]
    HeightDiscontinuity {
        /// Block height
        height: BlockHeight,
        /// Previous block height
        previous_block_height: BlockHeight,
    },
    /// Hash discontinuity.
    #[error(
        "hash discontinuity. block prev_hash {prev_hash} with height {height} does not match previous block hash {previous_block_hash}"
    )]
    HashDiscontinuity {
        /// Block height
        height: BlockHeight,
        /// Block's previous block hash data
        prev_hash: BlockHash,
        /// Actual previous block hash
        previous_block_hash: BlockHash,
    },
}

/// Server errors.
///
/// Errors associated with connecting to the server and receiving invalid data.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Server request failed.
    #[error("server request failed. {0}")]
    RequestFailed(#[from] tonic::Status),
    /// Server returned invalid frontier.
    #[error("server returned invalid frontier. {0}")]
    InvalidFrontier(std::io::Error),
    /// Server returned invalid transaction.
    #[error("server returned invalid transaction. {0}")]
    InvalidTransaction(std::io::Error),
    /// Server returned invalid subtree root.
    // TODO: add more info
    #[error("server returned invalid subtree root.")]
    InvalidSubtreeRoot,
    /// Server returned blocks that could not be verified against wallet block data. Exceeded max verification window.
    #[error(
        "server returned blocks that could not be verified against wallet block data. exceeded max verification window."
    )]
    ChainVerificationError,
    /// Fetcher task was dropped.
    #[error("fetcher task was dropped.")]
    FetcherDropped,
    /// Server reports only the genesis block exists.
    #[error("server reports only the genesis block exists.")]
    GenesisBlockOnly,
}

/// Sync mode error.
#[derive(Debug, thiserror::Error)]
pub enum SyncModeError {
    /// Invalid sync mode.
    #[error("invalid sync mode. {0}")]
    InvalidSyncMode(u8),
    /// Sync is already running.
    #[error("sync is already running")]
    SyncAlreadyRunning,
    /// Sync is not running.
    #[error("sync is not running")]
    SyncNotRunning,
    /// Sync is not paused.
    #[error("sync is not paused")]
    SyncNotPaused,
}
