//! How migration parts choose their wire (ADR 0011, amendment 2026-07-23).
//!
//! Migration-part transmissions obey the Mixnet Mode policy like every other
//! transmitting surface: while the mode is on they travel ONLY over the
//! mixnet (failing closed while it bootstraps or after the proxy dies,
//! never falling back to nakednet), and nakednet carries them only when the
//! user deliberately toggled the mode off for the session, or in a build
//! compiled without the `nym` feature.

use crate::wallet::migration::transmission::{
    PartTransmissionError, TransmissionClient, TransmissionReceipt,
};
use zcash_protocol::consensus::BlockHeight;

#[cfg(feature = "nym")]
use crate::wallet::migration::transmission::TransmissionRoute;
#[cfg(feature = "nym")]
use zcash_primitives::transaction::TxId;

use super::transmission_grpc::GrpcTransmissionClient;

/// The wire migration parts travel.
pub enum MigrationWire {
    /// Direct submission.
    Nakednet,
    /// Submission through the local SOCKS5 proxy.
    #[cfg(feature = "nym")]
    Mixnet(zingo_netutils::conduit::ConduitDial),
}

/// The [`TransmissionClient`] the Mixnet Mode policy resolved for this session.
pub struct RoutedTransmissionClient {
    wire: MigrationWire,
    indexer: http::Uri,
}

impl RoutedTransmissionClient {
    pub(crate) fn new(wire: MigrationWire, indexer: http::Uri) -> Self {
        RoutedTransmissionClient { wire, indexer }
    }

    #[cfg(all(test, feature = "nym"))]
    pub(crate) fn is_mixnet(&self) -> bool {
        !matches!(self.wire, MigrationWire::Nakednet)
    }
}

impl TransmissionClient for RoutedTransmissionClient {
    async fn submit(
        &self,
        raw_tx: Vec<u8>,
        expiry_height: BlockHeight,
    ) -> Result<TransmissionReceipt, PartTransmissionError> {
        let indexer = &self.indexer;
        match &self.wire {
            MigrationWire::Nakednet => {
                GrpcTransmissionClient::new(indexer.clone())
                    .submit(raw_tx, expiry_height)
                    .await
            }
            #[cfg(feature = "nym")]
            MigrationWire::Mixnet(dial) => {
                submit_over_socks5(dial, indexer, raw_tx, expiry_height).await
            }
        }
    }
}

#[cfg(feature = "nym")]
async fn submit_over_socks5(
    dial: &zingo_netutils::conduit::ConduitDial,
    indexer: &http::Uri,
    raw_tx: Vec<u8>,
    expiry_height: BlockHeight,
) -> Result<TransmissionReceipt, PartTransmissionError> {
    let txid_hex = zingo_netutils::Socks5Indexer::new(
        dial.socks5(),
        indexer.clone(),
        super::transmission_grpc::MIGRATION_SUBMIT_TIMEOUT,
    )
    .send_transaction(&raw_tx, u64::from(u32::from(expiry_height)))
    .await
    .map_err(|error| {
        // The taxonomy's own failover reading maps onto PartTransmissionError's
        // contract: a failover candidate was not consumed (Transport,
        // retryable: the part falls to reconciliation), a verdict was.
        let rendered = error.to_string();
        if error.is_failover_candidate() {
            PartTransmissionError::Transport(rendered)
        } else {
            PartTransmissionError::Rejected(rendered)
        }
    })?;
    let txid: TxId =
        crate::utils::conversion::txid_from_hex_encoded_str(&txid_hex).map_err(|e| {
            PartTransmissionError::Rejected(format!("endpoint returned an invalid txid: {e}"))
        })?;
    Ok(TransmissionReceipt {
        txid,
        route: TransmissionRoute::Mixnet {
            destination: super::transmission_grpc::host_of(indexer),
            via_socks5: dial.socks5().to_string(),
        },
    })
}
