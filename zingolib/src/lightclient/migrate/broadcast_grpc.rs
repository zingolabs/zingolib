//! The production [`BroadcastClient`]: a dedicated gRPC connection for part
//! submission.
//!
//! Lives outside `wallet::migration` on purpose: the migration modules must
//! not depend on the network stack. Each submit builds a fresh connection to
//! its own URI, so parts never travel over the synchronization channel.

use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zingo_netutils::Indexer as _;
use zingo_netutils::lightwallet_protocol::RawTransaction;

use crate::wallet::migration::broadcast::{BroadcastClient, BroadcastError};

pub(super) use zingo_netutils::time::MIGRATION_SUBMIT_TIMEOUT;

/// Submits parts over gRPC and can do nothing else.
pub struct GrpcBroadcastClient {
    uri: http::Uri,
}

impl GrpcBroadcastClient {
    /// A client submitting to `uri`, ideally the dedicated
    /// `migration_broadcast_uri` rather than the synchronization endpoint.
    pub fn new(uri: http::Uri) -> Self {
        GrpcBroadcastClient { uri }
    }
}

impl BroadcastClient for GrpcBroadcastClient {
    async fn submit(
        &self,
        raw_tx: Vec<u8>,
        expiry_height: BlockHeight,
    ) -> Result<TxId, BroadcastError> {
        let mut indexer = zingo_netutils::GrpcIndexer::new(self.uri.clone())
            .await
            .map_err(|e| BroadcastError::Transport(e.to_string()))?;
        let txid_hex = indexer
            .send_transaction(
                RawTransaction {
                    data: raw_tx,
                    height: u64::from(u32::from(expiry_height)),
                },
                MIGRATION_SUBMIT_TIMEOUT,
            )
            .await
            .map_err(|status| BroadcastError::Rejected(status.to_string()))?;
        crate::utils::conversion::txid_from_hex_encoded_str(&txid_hex).map_err(|e| {
            BroadcastError::Rejected(format!("endpoint returned an invalid txid: {e}"))
        })
    }
}
