//! The production [`BroadcastClient`]: a dedicated gRPC connection for transfer
//! submission.
//!
//! Lives outside `wallet::migration` on purpose: the migration modules must
//! not depend on the network stack. Each submit builds a fresh connection to
//! its own URI, so transfers never travel over the synchronization channel.

use zcash_protocol::consensus::BlockHeight;
use zingo_netutils::Indexer as _;
use zingo_netutils::lightwallet_protocol::RawTransaction;

use crate::wallet::migration::broadcast::{
    BroadcastClient, BroadcastReceipt, BroadcastRoute, TransferBroadcastError,
};

pub(super) use zingo_netutils::time::MIGRATION_SUBMIT_TIMEOUT;

/// Submits transfers over gRPC and can do nothing else.
pub struct GrpcBroadcastClient {
    uri: http::Uri,
}

impl GrpcBroadcastClient {
    /// A client submitting to `uri`, ideally the dedicated
    /// `migration_transmission_uri` rather than the synchronization endpoint.
    pub fn new(uri: http::Uri) -> Self {
        GrpcBroadcastClient { uri }
    }
}

impl BroadcastClient for GrpcBroadcastClient {
    async fn submit(
        &self,
        raw_tx: Vec<u8>,
        expiry_height: BlockHeight,
    ) -> Result<BroadcastReceipt, TransferBroadcastError> {
        let route = BroadcastRoute::Clearnet {
            endpoint: host_of(&self.uri),
        };
        let mut indexer = zingo_netutils::GrpcIndexer::new(self.uri.clone())
            .await
            .map_err(|e| TransferBroadcastError::Transport {
                route: route.clone(),
                message: e.to_string(),
            })?;
        let txid_hex = indexer
            .send_transaction(
                RawTransaction {
                    data: raw_tx,
                    height: u64::from(u32::from(expiry_height)),
                },
                MIGRATION_SUBMIT_TIMEOUT,
            )
            .await
            .map_err(|status| rejection(status.to_string(), route.clone()))?;
        receipt_of(&txid_hex, route)
    }
}

pub(super) fn receipt_of(
    txid_hex: &str,
    route: BroadcastRoute,
) -> Result<BroadcastReceipt, TransferBroadcastError> {
    match crate::utils::conversion::txid_from_hex_encoded_str(txid_hex) {
        Ok(txid) => Ok(BroadcastReceipt { txid, route }),
        Err(e) => Err(TransferBroadcastError::Rejected {
            route,
            message: format!("endpoint returned an invalid txid: {e}"),
        }),
    }
}

pub(super) fn rejection(message: String, route: BroadcastRoute) -> TransferBroadcastError {
    use crate::lightclient::transmit::{RejectionClass, classify_rejection};

    match classify_rejection(&message) {
        RejectionClass::StorageBackedDuplicate => TransferBroadcastError::AlreadyKnown { route },
        _ => TransferBroadcastError::Rejected { route, message },
    }
}

/// An endpoint's host, the identity a route record names; the whole URI
/// when it carries no host.
pub(super) fn host_of(uri: &http::Uri) -> String {
    uri.host().map_or_else(|| uri.to_string(), str::to_string)
}
