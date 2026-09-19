//! The transmit-only client transfers are submitted through.
//!
//! This module deliberately contains a trait, its route-evidence types, and
//! an error type and nothing else. A background transmission task holds only
//! a [`BroadcastClient`],
//! which has no way to fetch blocks, tree state or any other chain data, so
//! the ZIP 318 requirement that a transfer-transmission session never performs a
//! synchronization holds structurally rather than by convention. The
//! production gRPC implementation lives with the LightClient, outside
//! `wallet::migration`.

use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

/// The route one transfer's submission traveled, carried back with the txid so
/// every transfer holds evidence of its own wire rather than trusting the
/// session's policy after the fact. Mirrors the send path's `TransmitRoute`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BroadcastRoute {
    /// Clearnet submission to this endpoint's host: reachable only by the
    /// deliberate mixnet opt-out, or a build without the `nym` feature.
    Clearnet {
        /// The endpoint's host.
        endpoint: String,
    },
    /// Mixnet submission to this Destination's host, through the local
    /// SOCKS5 tunnel endpoint.
    Mixnet {
        /// The drawn Destination's host.
        destination: String,
        /// The local SOCKS5 endpoint of the mixnet tunnel.
        via_socks5: String,
    },
}

impl BroadcastRoute {
    /// Whether this route traveled the mixnet, the predicate a validation
    /// pass asserts over every transfer of a migration.
    pub fn is_mixnet(&self) -> bool {
        matches!(self, BroadcastRoute::Mixnet { .. })
    }
}

/// One accepted submission: the endpoint's txid and the route it traveled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroadcastReceipt {
    /// The txid the endpoint reported.
    pub txid: TxId,
    /// The wire this submission actually used.
    pub route: BroadcastRoute,
}

/// Submits raw transactions and does nothing else.
pub trait BroadcastClient: Send + Sync {
    /// Submits a raw transaction, returning the endpoint's txid together
    /// with the route the submission traveled.
    fn submit(
        &self,
        raw_tx: Vec<u8>,
        expiry_height: BlockHeight,
    ) -> impl std::future::Future<Output = Result<BroadcastReceipt, TransferBroadcastError>> + Send;
}

/// Why a submission failed. Every failure that reached an endpoint carries
/// the route it traveled.
#[derive(Debug, thiserror::Error)]
pub enum TransferBroadcastError {
    /// No endpoint is available to submit to.
    #[error("transmission transport failure: no transmission candidates")]
    NoCandidates,
    /// The endpoint could not be reached. The transaction was not consumed
    /// and the attempt can be retried.
    #[error("transmission transport failure: {message}")]
    Transport {
        /// The wire this submission used.
        route: BroadcastRoute,
        /// The transport's failure text.
        message: String,
    },
    /// The endpoint rejected the transaction.
    #[error("transmission rejected: {message}")]
    Rejected {
        /// The wire this submission used.
        route: BroadcastRoute,
        /// The endpoint's rejection text.
        message: String,
    },
    /// The endpoint already holds the transaction, in its mempool or in a
    /// block. An earlier submission arrived, so the transfer is broadcast.
    #[error("transmission duplicate: the endpoint already holds the transaction")]
    AlreadyKnown {
        /// The wire this submission used.
        route: BroadcastRoute,
    },
}

impl TransferBroadcastError {
    /// The route the failed submission traveled, when it reached one.
    pub fn route(&self) -> Option<&BroadcastRoute> {
        match self {
            TransferBroadcastError::NoCandidates => None,
            TransferBroadcastError::Transport { route, .. }
            | TransferBroadcastError::Rejected { route, .. }
            | TransferBroadcastError::AlreadyKnown { route } => Some(route),
        }
    }
}
