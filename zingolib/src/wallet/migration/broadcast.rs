//! The broadcast-only client parts are submitted through.
//!
//! This module deliberately contains a trait and an error type and nothing
//! else. A background broadcast task holds only a [`BroadcastClient`], which
//! has no way to fetch blocks, tree state or any other chain data, so the
//! ZIP 318 requirement that a broadcast session never performs a
//! synchronization holds structurally rather than by convention. The
//! production gRPC implementation lives with the LightClient, outside
//! `wallet::migration`.

use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

/// Submits raw transactions and does nothing else.
pub(crate) trait BroadcastClient: Send + Sync {
    /// Submits a raw transaction, returning its txid as reported by the
    /// endpoint.
    fn submit(
        &self,
        raw_tx: Vec<u8>,
        expiry_height: BlockHeight,
    ) -> impl std::future::Future<Output = Result<TxId, BroadcastError>> + Send;
}

/// Why a submission failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum BroadcastError {
    /// The endpoint could not be reached. The transaction was not consumed
    /// and the attempt can be retried.
    #[error("broadcast transport failure: {0}")]
    Transport(String),
    /// The endpoint rejected the transaction.
    #[error("broadcast rejected: {0}")]
    Rejected(String),
}
