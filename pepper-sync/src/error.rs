//! Pepper sync error module

use std::{array::TryFromSliceError, convert::Infallible};

use shardtree::error::ShardTreeError;
use zcash_primitives::{block::BlockHash, transaction::TxId};
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::{PoolType, ShieldedPool};

use crate::wallet::OutputId;

/// The separator between two layers of a rendered cause chain.
pub(crate) const CAUSE_CHAIN_SEPARATOR: &str = ": ";

/// Renders `error` and every `source()` link as one separated line, so a log
/// message keeps the whole cause chain.
pub(crate) fn cause_chain_text(error: &(dyn std::error::Error + 'static)) -> String {
    let mut texts = vec![error.to_string()];
    let mut link = error.source();
    while let Some(cause) = link {
        texts.push(cause.to_string());
        link = cause.source();
    }
    texts.join(CAUSE_CHAIN_SEPARATOR)
}

/// Top level error enumerating any error that may occur during sync
#[derive(Debug, thiserror::Error)]
pub enum SyncError<E>
where
    E: std::fmt::Debug + std::fmt::Display,
{
    /// Mempool error.
    #[error("mempool error")]
    MempoolError(#[from] MempoolError),
    /// Scan error.
    #[error("scan error")]
    ScanError(#[from] ScanError),
    /// Server error.
    #[error("server error")]
    ServerError(#[from] ServerError),
    /// Sync mode error.
    #[error("sync mode error")]
    SyncModeError(#[from] SyncModeError),
    /// Chain error.
    #[error("wallet height {0} is more than {1} blocks ahead of best chain height {2}")]
    ChainError(u32, u32, u32),
    /// Birthday below sapling error.
    #[error(
        "birthday {0} below sapling activation height {1}. pre-sapling wallets are not supported!"
    )]
    BirthdayBelowSapling(u32, u32),
    /// Shard tree error.
    #[error("shard tree error")]
    ShardTreeError(#[from] ShardTreeError<Infallible>),
    /// Critical non-recoverable truncation error due to missing shard tree checkpoints.
    #[error(
        "critical non-recoverable truncation error at height {0} due to missing {1} shard tree checkpoints. wallet data cleared. rescan required."
    )]
    TruncationError(BlockHeight, PoolType),
    /// One pool's recorded history could not account for the commitment
    /// tree the chain reports, so that pool was reopened for rescanning
    /// from the given height. The session ends here; the next one rescans.
    #[error(
        "RESCAN TRIGGERED: the wallet's {pool} records were cleared back to height \
         {rescan_from} and the next sync rescans from there, because at height {disagreed_at} \
         the wallet's history accounted for a {pool} commitment tree of {calculated_size} where \
         the chain reports {block_metadata_size}"
    )]
    PoolHistoryReopened {
        /// The pool whose history was cleared and reopened.
        pool: PoolType,
        /// The height rescanning restarts from.
        rescan_from: BlockHeight,
        /// The block height whose tree sizes disagreed.
        disagreed_at: BlockHeight,
        /// The tree size the chain's block metadata reports.
        block_metadata_size: u32,
        /// The tree size the wallet's history accounts for.
        calculated_size: u32,
    },
    /// Transparent address derivation error.
    #[error("transparent address derivation error. {0}")]
    TransparentAddressDerivationError(bip32::Error),
    /// Wallet error.
    #[error("wallet error. {0}")]
    WalletError(E),
}

impl<E: std::fmt::Debug + std::fmt::Display> SyncError<E> {
    /// Returns `true` if this error is likely transient and retrying sync
    /// (possibly against a different server) may succeed.
    ///
    /// Server errors from failed gRPC requests and mempool stream failures
    /// are recommend_same_server. Configuration errors, wallet corruption, and data
    /// integrity failures are not.
    pub fn recommend_same_server(&self) -> bool {
        match self {
            // Network/server issues. Retrying may help, especially with a different server.
            SyncError::ServerError(e) => e.recommend_same_server(),
            SyncError::MempoolError(_) => true,
            // Not the server's doing, but the wallet has already reopened
            // the pool it could not account for, so the next sync against
            // this same server makes progress.
            SyncError::PoolHistoryReopened { .. } => true,

            // Local or configuration errors. Retrying won't help.
            SyncError::ScanError(_)
            | SyncError::SyncModeError(_)
            | SyncError::ChainError(..)
            | SyncError::BirthdayBelowSapling(..)
            | SyncError::ShardTreeError(_)
            | SyncError::TruncationError(..)
            | SyncError::TransparentAddressDerivationError(_)
            | SyncError::WalletError(_) => false,
        }
    }
}

impl ServerError {
    /// Returns `true` if this server error is likely transient.
    ///
    /// gRPC request failures (timeouts, connection drops) are recommend_same_server.
    /// Invalid data from the server suggests a bad server that should be
    /// avoided rather than retried.
    pub fn recommend_same_server(&self) -> bool {
        match self {
            // Internal channel issue. Retrying may help after restart.
            ServerError::FetcherDropped => true,

            // gRPC request failure. The server may be down or overloaded.
            // Switch to a different server rather than retrying the same one.
            ServerError::RequestFailed(_) => false,

            // Bad data from server. Retrying the same server won't help.
            ServerError::InvalidFrontier(_)
            | ServerError::InvalidTransaction(_)
            | ServerError::InvalidSubtreeRoot
            | ServerError::ChainVerificationError
            | ServerError::GenesisBlockOnly => false,
        }
    }
}

/// Recommended action when sync fails.
///
/// Returned by [`SyncError::recovery_recommendation`] to give callers (zingo-cli,
/// zingo-mobile, etc.) a concrete decision without needing to match on
/// error internals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRecoveryObservables {
    /// The error is transient (e.g. timeout, connection drop).
    /// Retrying sync with the same server may succeed.
    MaybeRecoverableServer,
    /// The server returned invalid or unverifiable data.
    /// A different server should be tried if available.
    ServerUnavailable,
    /// The error is not recoverable by retrying or switching servers.
    /// User intervention is required (e.g. rescan, fix config).
    Abort,
}

impl<E: std::fmt::Debug + std::fmt::Display> SyncError<E> {
    /// Returns the recommended recovery action for this error.
    ///
    /// This is the primary entry point for callers that need to decide
    /// whether to retry, switch servers, or give up.
    pub fn recovery_recommendation(&self) -> SyncRecoveryObservables {
        match self {
            SyncError::ServerError(e) => e.recovery_recommendation(),
            SyncError::MempoolError(_) => SyncRecoveryObservables::MaybeRecoverableServer,

            SyncError::ScanError(ScanError::ServerError(e)) => e.recovery_recommendation(),
            SyncError::ScanError(_) => SyncRecoveryObservables::Abort,

            // The wallet has already reopened the pool it could not account
            // for, so syncing again against the same server is exactly the
            // recovery.
            SyncError::PoolHistoryReopened { .. } => {
                SyncRecoveryObservables::MaybeRecoverableServer
            }

            SyncError::SyncModeError(_)
            | SyncError::ChainError(..)
            | SyncError::BirthdayBelowSapling(..)
            | SyncError::ShardTreeError(_)
            | SyncError::TruncationError(..)
            | SyncError::TransparentAddressDerivationError(_)
            | SyncError::WalletError(_) => SyncRecoveryObservables::Abort,
        }
    }
}

impl ServerError {
    /// Returns the recommended recovery action for this server error.
    pub fn recovery_recommendation(&self) -> SyncRecoveryObservables {
        match self {
            // Internal channel issue. The same server may work after restart.
            ServerError::FetcherDropped => SyncRecoveryObservables::MaybeRecoverableServer,
            // gRPC request failure or bad data. Try a different server.
            ServerError::RequestFailed(_)
            | ServerError::InvalidFrontier(_)
            | ServerError::InvalidTransaction(_)
            | ServerError::InvalidSubtreeRoot
            | ServerError::ChainVerificationError => SyncRecoveryObservables::ServerUnavailable,
            // Empty chain. No point retrying anywhere.
            ServerError::GenesisBlockOnly => SyncRecoveryObservables::Abort,
        }
    }
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
    #[error("server error")]
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
    #[error("server error")]
    ServerError(#[from] ServerError),
    /// Continuity error.
    #[error("continuity error")]
    ContinuityError(#[from] ContinuityError),
    /// Zcash client backend scan error
    #[error(transparent)]
    EncodingError(#[from] EncodingInvalid),
    /// Invalid sapling nullifier
    #[error("invalid sapling nullifier")]
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
        "incorrect tree size. at height {height}, {shielded_protocol} tree size recorded in block metadata {block_metadata_size} does not match calculated size {calculated_size}"
    )]
    IncorrectTreeSize {
        /// Shielded protocol
        shielded_protocol: PoolType,
        /// The block height whose sizes disagreed.
        height: BlockHeight,
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
    #[error("invalid memo bytes")]
    InvalidMemoBytes(#[from] zcash_protocol::memo::Error),
    /// Failed to parse encoded address.
    #[error("failed to parse encoded address")]
    AddressParseError(#[from] zcash_address::unified::ParseError),
}

/// The encoding of a compact Sapling output or compact Orchard action was invalid.
#[derive(Debug, thiserror::Error)]
#[error("{pool_type:?} output {index} of transaction {txid} was improperly encoded.")]
pub struct EncodingInvalid {
    pub(crate) at_height: BlockHeight,
    pub(crate) txid: TxId,
    pub(crate) pool_type: ShieldedPool,
    pub(crate) index: usize,
    pub(crate) error: CompactFormatError,
}

/// An error indicating that a field of a compact format structure could not be parsed.
#[derive(Clone, Debug)]
pub enum CompactFormatError {
    /// A byte slice had an invalid length for the expected field.
    InvalidLength(std::array::TryFromSliceError),
    /// A field value did not represent a valid protocol element.
    InvalidValue,
}

impl std::fmt::Display for CompactFormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompactFormatError::InvalidLength(e) => write!(f, "Invalid compact format field: {e}"),
            CompactFormatError::InvalidValue => {
                write!(f, "Compact format field is not a valid protocol element")
            }
        }
    }
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
    #[error("server request failed")]
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
        "server returned blocks that could not be verified against wallet block data. exceeded max verification window. wallet data has been cleared as shard tree data cannot be truncated further. wallet rescan required."
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Use `String` as the wallet error type for testing.
    type TestSyncError = SyncError<String>;

    /// The number of times one detail may appear across a whole cause chain.
    const DETAIL_RENDERINGS: usize = 1;

    /// Renders `error` and every link of its source chain, one line per link.
    fn chain_rendering(error: &(dyn std::error::Error + 'static)) -> String {
        let mut rendered = error.to_string();
        let mut cursor = error.source();
        while let Some(cause) = cursor {
            rendered.push('\n');
            rendered.push_str(&cause.to_string());
            cursor = cause.source();
        }
        rendered
    }

    /// HYPOTHESIS: every layer wrapping a failed server request states its
    /// own layer only, so the transport status text reaches the reader
    /// exactly once across the whole cause chain. Falsified if the status
    /// text appears more than once.
    #[test]
    fn a_failed_server_request_renders_its_status_once() {
        const DETAIL: &str = "the indexer refused the block range";
        let failure: TestSyncError =
            ServerError::RequestFailed(tonic::Status::unavailable(DETAIL)).into();
        let rendered = chain_rendering(&failure);
        assert_eq!(
            rendered.matches(DETAIL).count(),
            DETAIL_RENDERINGS,
            "the chain rendered was {rendered}"
        );
    }

    /// HYPOTHESIS: the rescan announcement is one self-contained sentence
    /// naming the consequence (records cleared, rescan from a height) and
    /// the cause (which height disagreed, both tree sizes), so a user
    /// reading a single log or error line knows what happened and why.
    /// Falsified if any of those five facts leaves the rendering.
    #[test]
    fn the_rescan_announcement_names_the_consequence_and_the_cause() {
        let announcement: TestSyncError = SyncError::PoolHistoryReopened {
            pool: PoolType::IRONWOOD,
            rescan_from: BlockHeight::from_u32(3_000_000),
            disagreed_at: BlockHeight::from_u32(3_100_000),
            block_metadata_size: 999,
            calculated_size: 7,
        };
        let rendered = announcement.to_string();
        assert!(rendered.starts_with("RESCAN TRIGGERED"), "{rendered}");
        for fact in [
            "cleared back to height 3000000",
            "at height 3100000",
            "commitment tree of 7",
            "the chain reports 999",
        ] {
            assert!(rendered.contains(fact), "missing '{fact}' in: {rendered}");
        }
    }

    /// HYPOTHESIS: the tree-size refusal names the block whose sizes
    /// disagreed, so the trigger is locatable from the error alone.
    /// Falsified if the height leaves the rendering.
    #[test]
    fn the_tree_size_refusal_names_the_disagreeing_block() {
        let refusal = ScanError::IncorrectTreeSize {
            shielded_protocol: PoolType::IRONWOOD,
            height: BlockHeight::from_u32(3_100_000),
            block_metadata_size: 999,
            calculated_size: 7,
        };
        assert!(
            refusal.to_string().contains("at height 3100000"),
            "{refusal}"
        );
    }

    mod recommend_same_server {
        use super::*;

        mod server_error {
            use super::*;

            #[test]
            fn fetcher_dropped() {
                assert!(ServerError::FetcherDropped.recommend_same_server());
            }
        }

        mod sync_error {
            use super::*;

            #[test]
            fn mempool_error() {
                let e: TestSyncError = MempoolError::ShutdownWithoutStream.into();
                assert!(e.recommend_same_server());
            }

            /// The wallet has already reopened the pool, so the very next
            /// sync against this same server performs the rescan. Reporting
            /// this as a reason to change server would send wallets hunting
            /// for a server that was never at fault.
            #[test]
            fn pool_history_reopened() {
                let e: TestSyncError = SyncError::PoolHistoryReopened {
                    pool: PoolType::IRONWOOD,
                    rescan_from: BlockHeight::from_u32(100),
                    disagreed_at: BlockHeight::from_u32(150),
                    block_metadata_size: 999,
                    calculated_size: 7,
                };
                assert!(e.recommend_same_server());
            }
        }
    }

    mod recommend_change_server {
        use super::*;

        mod server_error {
            use super::*;

            #[test]
            fn request_failed() {
                let e = ServerError::RequestFailed(tonic::Status::deadline_exceeded("timeout"));
                assert!(!e.recommend_same_server());
            }

            #[test]
            fn invalid_frontier() {
                let e = ServerError::InvalidFrontier(std::io::Error::other("bad frontier"));
                assert!(!e.recommend_same_server());
            }

            #[test]
            fn invalid_transaction() {
                let e = ServerError::InvalidTransaction(std::io::Error::other("bad tx"));
                assert!(!e.recommend_same_server());
            }

            #[test]
            fn invalid_subtree_root() {
                assert!(!ServerError::InvalidSubtreeRoot.recommend_same_server());
            }

            #[test]
            fn chain_verification_error() {
                assert!(!ServerError::ChainVerificationError.recommend_same_server());
            }

            #[test]
            fn genesis_block_only() {
                assert!(!ServerError::GenesisBlockOnly.recommend_same_server());
            }
        }

        mod sync_error {
            use super::*;

            #[test]
            fn server_request_failed() {
                let e: TestSyncError =
                    ServerError::RequestFailed(tonic::Status::deadline_exceeded("timeout")).into();
                assert!(!e.recommend_same_server());
            }

            #[test]
            fn sync_mode_error() {
                let e: TestSyncError = SyncModeError::SyncAlreadyRunning.into();
                assert!(!e.recommend_same_server());
            }

            #[test]
            fn chain_error() {
                let e: TestSyncError = SyncError::ChainError(100, 50, 50);
                assert!(!e.recommend_same_server());
            }

            #[test]
            fn birthday_below_sapling() {
                let e: TestSyncError = SyncError::BirthdayBelowSapling(100, 419200);
                assert!(!e.recommend_same_server());
            }

            #[test]
            fn wallet_error() {
                let e: TestSyncError = SyncError::WalletError("db locked".to_string());
                assert!(!e.recommend_same_server());
            }
        }
    }

    mod recovery_recommendation {
        use super::*;

        mod retry_same_server {
            use super::*;

            #[test]
            fn fetcher_dropped() {
                assert_eq!(
                    ServerError::FetcherDropped.recovery_recommendation(),
                    SyncRecoveryObservables::MaybeRecoverableServer
                );
            }

            #[test]
            fn mempool_error() {
                let e: TestSyncError = MempoolError::ShutdownWithoutStream.into();
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::MaybeRecoverableServer
                );
            }

            /// Syncing again is the recovery, so this must never be reported
            /// as needing the user to intervene.
            #[test]
            fn pool_history_reopened() {
                let e: TestSyncError = SyncError::PoolHistoryReopened {
                    pool: PoolType::IRONWOOD,
                    rescan_from: BlockHeight::from_u32(100),
                    disagreed_at: BlockHeight::from_u32(150),
                    block_metadata_size: 999,
                    calculated_size: 7,
                };
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::MaybeRecoverableServer
                );
            }
        }

        mod try_different_server {
            use super::*;

            #[test]
            fn request_failed() {
                let e = ServerError::RequestFailed(tonic::Status::deadline_exceeded("timeout"));
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::ServerUnavailable
                );
            }

            #[test]
            fn sync_error_from_request_failed() {
                let e: TestSyncError =
                    ServerError::RequestFailed(tonic::Status::unavailable("down")).into();
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::ServerUnavailable
                );
            }

            #[test]
            fn invalid_frontier() {
                let e = ServerError::InvalidFrontier(std::io::Error::other("bad"));
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::ServerUnavailable
                );
            }

            #[test]
            fn invalid_transaction() {
                let e = ServerError::InvalidTransaction(std::io::Error::other("bad"));
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::ServerUnavailable
                );
            }

            #[test]
            fn invalid_subtree_root() {
                assert_eq!(
                    ServerError::InvalidSubtreeRoot.recovery_recommendation(),
                    SyncRecoveryObservables::ServerUnavailable
                );
            }

            #[test]
            fn chain_verification_error() {
                assert_eq!(
                    ServerError::ChainVerificationError.recovery_recommendation(),
                    SyncRecoveryObservables::ServerUnavailable
                );
            }

            #[test]
            fn sync_error_from_invalid_frontier() {
                let e: TestSyncError =
                    ServerError::InvalidFrontier(std::io::Error::other("bad")).into();
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::ServerUnavailable
                );
            }

            #[test]
            fn scan_error_wrapping_server_error() {
                let e: TestSyncError =
                    ScanError::ServerError(ServerError::InvalidSubtreeRoot).into();
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::ServerUnavailable
                );
            }
        }

        mod abort {
            use super::*;

            #[test]
            fn genesis_block_only() {
                assert_eq!(
                    ServerError::GenesisBlockOnly.recovery_recommendation(),
                    SyncRecoveryObservables::Abort
                );
            }

            #[test]
            fn sync_mode_error() {
                let e: TestSyncError = SyncModeError::SyncAlreadyRunning.into();
                assert_eq!(e.recovery_recommendation(), SyncRecoveryObservables::Abort);
            }

            #[test]
            fn chain_error() {
                let e: TestSyncError = SyncError::ChainError(100, 50, 50);
                assert_eq!(e.recovery_recommendation(), SyncRecoveryObservables::Abort);
            }

            #[test]
            fn wallet_error() {
                let e: TestSyncError = SyncError::WalletError("db locked".to_string());
                assert_eq!(e.recovery_recommendation(), SyncRecoveryObservables::Abort);
            }
        }
    }
}
