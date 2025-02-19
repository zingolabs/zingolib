//! Errors associated with sync module.

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
