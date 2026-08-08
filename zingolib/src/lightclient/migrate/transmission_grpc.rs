//! The production [`TransmissionClient`]: a dedicated gRPC connection for part
//! submission.
//!
//! Lives outside `wallet::migration` on purpose: the migration modules must
//! not depend on the network stack. Each submit builds a fresh connection to
//! its own URI, so parts never travel over the synchronization channel.

use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zingo_netutils::Indexer as _;
use zingo_netutils::lightwallet_protocol::RawTransaction;

use crate::wallet::migration::transmission::{PartTransmissionError, TransmissionClient};

pub(super) use zingo_netutils::time::MIGRATION_SUBMIT_TIMEOUT;

/// Submits parts over gRPC and can do nothing else.
pub struct GrpcTransmissionClient {
    uri: http::Uri,
}

impl GrpcTransmissionClient {
    /// A client submitting to `uri`, ideally the dedicated
    /// `migration_transmission_uri` rather than the synchronization endpoint.
    pub fn new(uri: http::Uri) -> Self {
        GrpcTransmissionClient { uri }
    }
}

impl TransmissionClient for GrpcTransmissionClient {
    async fn submit(
        &self,
        raw_tx: Vec<u8>,
        expiry_height: BlockHeight,
    ) -> Result<TxId, PartTransmissionError> {
        let mut indexer = zingo_netutils::GrpcIndexer::new(self.uri.clone())
            .await
            .map_err(|e| PartTransmissionError::Transport(e.to_string()))?;
        let txid_hex = indexer
            .send_transaction(
                RawTransaction {
                    data: raw_tx,
                    height: u64::from(u32::from(expiry_height)),
                },
                MIGRATION_SUBMIT_TIMEOUT,
            )
            .await
            .map_err(|status| PartTransmissionError::Rejected(status.to_string()))?;
        crate::utils::conversion::txid_from_hex_encoded_str(&txid_hex).map_err(|e| {
            PartTransmissionError::Rejected(format!("endpoint returned an invalid txid: {e}"))
        })
    }
}
