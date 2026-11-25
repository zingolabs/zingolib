//! Pepper sync error module

use std::{array::TryFromSliceError, convert::Infallible};

use shardtree::error::ShardTreeError;
use zcash_primitives::{block::BlockHash, transaction::TxId};
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::{PoolType, ShieldedPool};
use zl_common::chain;

use crate::{protocol, wallet::OutputId};

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
    /// Sync status error.
    #[error("sync status error")]
    SyncStatusError(#[from] SyncStatusError<E>),
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

// Whether a failed gRPC request hit network weather: a tonic transport error, a client-side timeout or an I/O
// error in the status's source chain, or one of the codes gRPC names as transient. Every other status is an answer
// from a server that cannot serve the request.
pub(crate) fn status_is_network_weather(status: &protocol::Status) -> bool {
    let mut link = std::error::Error::source(status);
    while let Some(cause) = link {
        if cause.is::<protocol::transport::Error>()
            || cause.is::<protocol::TimeoutExpired>()
            || cause.is::<std::io::Error>()
        {
            return true;
        }
        link = cause.source();
    }
    matches!(
        status.code(),
        protocol::Code::Unavailable
            | protocol::Code::DeadlineExceeded
            | protocol::Code::Cancelled
            | protocol::Code::ResourceExhausted
            | protocol::Code::Aborted
    )
}

const UNEXPECTED_STREAM_EOF: &str = "Unexpected EOF decoding stream.";

pub(crate) fn stream_error_is_retryable(status: &protocol::Status) -> bool {
    status.code() == protocol::Code::DeadlineExceeded
        || status.message().contains(UNEXPECTED_STREAM_EOF)
}

/// Recommended action when sync fails.
///
/// Returned by [`SyncError::recovery_recommendation`] to give callers (zingolib-cli,
/// zingo-mobile, etc.) a concrete decision without needing to match on
/// error internals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRecoveryObservables {
    /// The error is transient (e.g. timeout, connection drop).
    /// Retrying sync with the same server may succeed.
    ///
    /// Callers should retry a bounded number of times, then treat the
    /// server as [`Self::ServerUnavailable`]. A single error cannot tell
    /// passing network weather from a failure the server repeats on every
    /// attempt, such as a proxy that always cuts a long stream at the same
    /// point, so only repeated failures reveal the latter.
    MaybeRecoverableServer,
    /// The server returned invalid or unverifiable data, or answered that
    /// it cannot serve the request.
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
            // The server does not report the tree size of a pool, so a
            // different server is required.
            SyncError::ScanError(ScanError::TreeSizeNotReported { .. }) => {
                SyncRecoveryObservables::ServerUnavailable
            }
            // The server's own data placed a transaction it does not serve, so
            // its data is inconsistent and a different server is required.
            SyncError::ScanError(ScanError::TransactionNotServed { .. }) => {
                SyncRecoveryObservables::ServerUnavailable
            }
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
            | SyncError::WalletError(_)
            | SyncError::SyncStatusError(_) => SyncRecoveryObservables::Abort,
        }
    }
}

impl ServerError {
    /// Returns the recommended recovery action for this server error.
    pub fn recovery_recommendation(&self) -> SyncRecoveryObservables {
        match self {
            // Internal channel issue. The same server may work after restart.
            ServerError::FetcherDropped => SyncRecoveryObservables::MaybeRecoverableServer,
            // A re-org lowered the chain height or the server is behind the
            // chain tip. Syncing again starts from the server's chain height
            // and verifies the wallet against its chain.
            ServerError::ChainHeightBelowScanRange { .. } => {
                SyncRecoveryObservables::MaybeRecoverableServer
            }
            // Network weather is transient. The same server may answer the
            // next request on a fresh connection.
            ServerError::RequestFailed(status) if status_is_network_weather(status) => {
                SyncRecoveryObservables::MaybeRecoverableServer
            }
            // The server answered but cannot serve the request, or returned
            // bad data. Try a different server.
            ServerError::RequestFailed(_)
            | ServerError::InvalidFrontier(_)
            | ServerError::InvalidTransaction(_)
            | ServerError::InvalidTransactionHeight(_)
            | ServerError::TransactionNotMined(_)
            | ServerError::InvalidSubtreeRoot
            | ServerError::ChainVerificationError
            | ServerError::TreeStateNotServed { .. }
            | ServerError::UnsupportedProtocolVersion { .. } => {
                SyncRecoveryObservables::ServerUnavailable
            }
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
    /// Sync progress channel is closed. Receiver has been dropped.
    #[error("Sync progress channel is closed. Receiver has been dropped.")]
    SyncProgressChannelClosed,
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
    /// Invalid encoding.
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
    /// Block metadata reports a tree size of zero where the wallet has calculated a non-zero tree size.
    #[error(
        "tree size not reported. at height {height}, {shielded_protocol} tree size recorded in block metadata is zero where the calculated size is {calculated_size}. connect to a server that serves {shielded_protocol}."
    )]
    TreeSizeNotReported {
        /// Shielded protocol
        shielded_protocol: PoolType,
        /// The block height whose sizes disagreed.
        height: BlockHeight,
        /// Calculated size
        calculated_size: u32,
    },
    /// The server does not serve a transaction that its own data placed in the chain.
    #[error(
        "the server does not serve transaction {txid}, which its own data placed at block {block_height}. connect to a different server and sync again. {status}"
    )]
    TransactionNotServed {
        /// The transaction the server does not serve.
        txid: TxId,
        /// The block the server's own data placed the transaction in.
        block_height: BlockHeight,
        /// The server's answer.
        status: protocol::Status,
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
    /// Compact transaction contained transparent output with value outside the valid zatoshi range.
    #[error(
        "compact transaction contained transparent output with value {0} which is outside the valid zatoshi range"
    )]
    TransparentOutputInvalidValue(u64),
    /// All transparent addresses are already in use.
    #[error("all transparent addresses are already in use")]
    AllAddressesInUse,
    /// Transparent address derivation error.
    #[error("transparent address derivation error. {0}")]
    TransparentAddressDerivationError(bip32::Error),
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
    RequestFailed(#[from] protocol::Status),
    /// Server returned invalid frontier.
    #[error("server returned invalid frontier. {0}")]
    InvalidFrontier(std::io::Error),
    /// Server returned invalid transaction.
    #[error("server returned invalid transaction. {0}")]
    InvalidTransaction(std::io::Error),
    #[error("server returned a transaction with an invalid height. {0}")]
    InvalidTransactionHeight(chain::InvalidRawTransaction),
    #[error("server returned a transaction that is not mined on the main chain. {0:?}")]
    TransactionNotMined(chain::RawTransactionStatus),
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
    /// Server did not return a shielded pool's note commitment tree state for a height at or above the pool's
    /// activation height.
    #[error(
        "server does not serve the {pool} note commitment tree state at height {height}. connect to a server that serves {pool}."
    )]
    TreeStateNotServed {
        /// The pool whose tree state was omitted.
        pool: PoolType,
        /// The requested block height.
        height: BlockHeight,
    },
    /// Server's lightwallet protocol version is too old to serve the transparent and Ironwood data in compact blocks
    /// required for sync.
    #[error(
        "server lightwallet protocol version '{version}' does not serve transparent and ironwood data in compact blocks. v0.5.0 or later is required. change to a server that supports lightwallet protocol v0.5.0 or later and sync again."
    )]
    UnsupportedProtocolVersion {
        /// The lightwallet protocol version reported by the server. Empty if the server does not report one.
        version: String,
    },
    /// Fetching a scan range failed and the server's chain height is below the last block of the scan range. A
    /// re-org lowered the chain height after the scan range was selected, or the server is behind the chain tip.
    #[error(
        "server chain height {chain_height} is below block {scan_range_end}, the end of the scan range being fetched. sync again to verify the wallet against the server's chain."
    )]
    ChainHeightBelowScanRange {
        /// The chain height reported by the server.
        chain_height: BlockHeight,
        /// The height of the last block in the scan range.
        scan_range_end: BlockHeight,
    },
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
            ServerError::RequestFailed(protocol::Status::unavailable(DETAIL)).into();
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

    /// Stands in for `tonic::transport::Error`, which cannot be built
    /// outside tonic, wrapping the client-side timeout as its cause.
    #[derive(Debug)]
    struct TransportError(protocol::TimeoutExpired);

    impl std::fmt::Display for TransportError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "transport error")
        }
    }

    impl std::error::Error for TransportError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    /// A request cut off by the client-side timeout, as tonic reports it:
    /// `Cancelled` with the transport timeout in the source chain.
    fn client_timeout() -> ServerError {
        let status =
            protocol::Status::from_error(Box::new(TransportError(protocol::TimeoutExpired(()))));
        assert_eq!(status.code(), protocol::Code::Cancelled);
        ServerError::RequestFailed(status)
    }

    /// A connection the peer reset. tonic finds no code in the cause chain
    /// and reports `Unknown`, keeping the I/O error as the source.
    fn connection_reset() -> ServerError {
        let status = protocol::Status::from_error(Box::new(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset,
        )));
        assert_eq!(status.code(), protocol::Code::Unknown);
        ServerError::RequestFailed(status)
    }

    /// The codes gRPC names as transient, as a server or the transport
    /// reports them.
    fn network_weather_statuses() -> [protocol::Status; 5] {
        [
            protocol::Status::unavailable("down"),
            protocol::Status::deadline_exceeded("timeout"),
            protocol::Status::cancelled("cancelled"),
            protocol::Status::resource_exhausted("overloaded"),
            protocol::Status::aborted("aborted"),
        ]
    }

    /// Codes a server answers with when it cannot serve the request.
    fn server_refusal_statuses() -> [protocol::Status; 5] {
        [
            protocol::Status::unimplemented("no such method"),
            protocol::Status::invalid_argument("bad request"),
            protocol::Status::not_found("no such block"),
            protocol::Status::internal("indexer bug"),
            protocol::Status::unknown("unknown"),
        ]
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
            fn request_network_weather() {
                for status in network_weather_statuses() {
                    let code = status.code();
                    let e = ServerError::RequestFailed(status);
                    assert_eq!(
                        e.recovery_recommendation(),
                        SyncRecoveryObservables::MaybeRecoverableServer,
                        "{code:?}"
                    );
                }
            }

            #[test]
            fn sync_error_from_unavailable() {
                let e: TestSyncError =
                    ServerError::RequestFailed(protocol::Status::unavailable("down")).into();
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::MaybeRecoverableServer
                );
            }

            /// A reset connection carries no transient code, only the I/O
            /// error behind it, and is still network weather.
            #[test]
            fn request_connection_reset() {
                assert_eq!(
                    connection_reset().recovery_recommendation(),
                    SyncRecoveryObservables::MaybeRecoverableServer
                );
            }

            /// A connection left idle until the client-side timeout expires
            /// is replaced by the next request, so the same server is kept.
            #[test]
            fn sync_error_from_client_timeout() {
                let e: TestSyncError = client_timeout().into();
                assert_eq!(
                    e.recovery_recommendation(),
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

            #[test]
            fn chain_height_below_scan_range() {
                let e: TestSyncError = ServerError::ChainHeightBelowScanRange {
                    chain_height: BlockHeight::from_u32(99),
                    scan_range_end: BlockHeight::from_u32(100),
                }
                .into();
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
            fn request_refused_by_server() {
                for status in server_refusal_statuses() {
                    let code = status.code();
                    let e = ServerError::RequestFailed(status);
                    assert_eq!(
                        e.recovery_recommendation(),
                        SyncRecoveryObservables::ServerUnavailable,
                        "{code:?}"
                    );
                }
            }

            #[test]
            fn sync_error_from_request_refused() {
                let e: TestSyncError =
                    ServerError::RequestFailed(protocol::Status::unimplemented("no such method"))
                        .into();
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
            fn invalid_transaction_height() {
                let e = ServerError::InvalidTransactionHeight(chain::InvalidRawTransaction(
                    u64::from(u32::MAX) + 1,
                ));
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::ServerUnavailable
                );
            }

            #[test]
            fn transaction_not_mined() {
                let e = ServerError::TransactionNotMined(chain::RawTransactionStatus::Mempool);
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

            #[test]
            fn scan_error_transaction_not_served() {
                let e: TestSyncError = ScanError::TransactionNotServed {
                    txid: TxId::from_bytes([1; 32]),
                    block_height: BlockHeight::from_u32(100),
                    status: protocol::Status::not_found("no such transaction"),
                }
                .into();
                assert_eq!(
                    e.recovery_recommendation(),
                    SyncRecoveryObservables::ServerUnavailable
                );
            }
        }

        mod abort {
            use super::*;

            /// A hash discontinuity at the first block of a scan range is
            /// handled within the sync session. One that ends the session is
            /// between blocks the server served together, or with a scanned
            /// block above the scan range, which syncing again repeats.
            #[test]
            fn continuity_error() {
                for continuity_error in [
                    ContinuityError::HashDiscontinuity {
                        height: BlockHeight::from_u32(100),
                        prev_hash: BlockHash([1; 32]),
                        previous_block_hash: BlockHash([2; 32]),
                    },
                    ContinuityError::HeightDiscontinuity {
                        height: BlockHeight::from_u32(102),
                        previous_block_height: BlockHeight::from_u32(100),
                    },
                ] {
                    let e: TestSyncError = ScanError::ContinuityError(continuity_error).into();
                    assert_eq!(e.recovery_recommendation(), SyncRecoveryObservables::Abort);
                }
            }

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
            fn birthday_below_sapling() {
                let e: TestSyncError = SyncError::BirthdayBelowSapling(100, 419200);
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
