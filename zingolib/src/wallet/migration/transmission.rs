//! The transmit-only client parts are submitted through.
//!
//! This module deliberately contains a trait, its route-evidence types, and
//! an error type and nothing else. A background transmission task holds only
//! a [`TransmissionClient`],
//! which has no way to fetch blocks, tree state or any other chain data, so
//! the ZIP 318 requirement that a part-transmission session never performs a
//! synchronization holds structurally rather than by convention. The
//! production gRPC implementation lives with the LightClient, outside
//! `wallet::migration`.

use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

/// The route one part's submission traveled, carried back with the txid so
/// every part holds evidence of its own wire rather than trusting the
/// session's policy after the fact. Mirrors the send path's `TransmitRoute`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransmissionRoute {
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

impl TransmissionRoute {
    /// Whether this route traveled the mixnet, the predicate a validation
    /// pass asserts over every part of a migration.
    pub fn is_mixnet(&self) -> bool {
        matches!(self, TransmissionRoute::Mixnet { .. })
    }
}

/// One accepted submission: the endpoint's txid and the route it traveled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransmissionReceipt {
    /// The txid the endpoint reported.
    pub txid: TxId,
    /// The wire this submission actually used.
    pub route: TransmissionRoute,
}

/// Submits raw transactions and does nothing else.
pub trait TransmissionClient: Send + Sync {
    /// Submits a raw transaction, returning the endpoint's txid together
    /// with the route the submission traveled.
    fn submit(
        &self,
        raw_tx: Vec<u8>,
        expiry_height: BlockHeight,
    ) -> impl std::future::Future<Output = Result<TransmissionReceipt, PartTransmissionError>> + Send;
}

/// Why a submission failed.
#[derive(Debug, thiserror::Error)]
pub enum PartTransmissionError {
    /// The endpoint could not be reached. The transaction was not consumed
    /// and the attempt can be retried.
    #[error("transmission transport failure: {0}")]
    Transport(String),
    /// The endpoint rejected the transaction.
    #[error("transmission rejected: {0}")]
    Rejected(String),
}
