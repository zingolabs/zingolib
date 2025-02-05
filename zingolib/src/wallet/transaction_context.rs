//! TODO: Add Mod Description Here!

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::ZingoConfig;
use zcash_client_backend::ShieldedProtocol;
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

use crate::wallet::{keys::unified::WalletCapability, tx_map::TxMap};

/// All the data you need to propose, build, and sign a transaction.
#[derive(Clone)]
// FIXME: zingo2
#[allow(dead_code)]
pub struct TransactionContext {
    /// TODO: Add Doc Comment Here!
    pub config: ZingoConfig,
    /// TODO: Add Doc Comment Here!
    pub(crate) key: Arc<WalletCapability>,
    /// TODO: Add Doc Comment Here!
    pub transaction_metadata_set: Arc<RwLock<TxMap>>,
}

impl TransactionContext {
    /// TODO: Add Doc Comment Here!
    pub fn new(
        config: &ZingoConfig,
        key: Arc<WalletCapability>,
        transaction_metadata_set: Arc<RwLock<TxMap>>,
    ) -> Self {
        Self {
            config: config.clone(),
            key,
            transaction_metadata_set,
        }
    }

    /// returns any outdated records that need to be rescanned for completeness..
    /// checks that each record contains output indexes for its notes
    pub async fn unindexed_records(
        &self,
        wallet_height: BlockHeight,
    ) -> Result<(), Vec<(TxId, BlockHeight)>> {
        let tmdsrl = &self
            .transaction_metadata_set
            .read()
            .await
            .transaction_records_by_id;
        tmdsrl
            .get_spendable_note_ids_and_values(
                &[ShieldedProtocol::Sapling, ShieldedProtocol::Orchard],
                wallet_height,
                &[],
            )
            .map(|_| ())
            .map_err(|mut vec| {
                vec.extend_from_slice(&tmdsrl.missing_outgoing_output_indexes());
                vec
            })
    }
}
