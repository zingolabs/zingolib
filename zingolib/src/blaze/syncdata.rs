use std::sync::Arc;

use http::Uri;
use tokio::sync::RwLock;

use super::{block_witness_data::BlockAndWitnessData, sync_status::BatchSyncStatus};
use crate::compact_formats::TreeState;
use crate::lightclient::PerBlockTrialDecryptLog;
use crate::wallet::data::BlockData;
use crate::wallet::WalletOptions;
use zingoconfig::ZingoConfig;

pub struct BlazeSyncData {
    pub(crate) sync_status: Arc<RwLock<BatchSyncStatus>>,
    pub(crate) block_data: BlockAndWitnessData,
    uri: Arc<std::sync::RwLock<Uri>>,
    pub(crate) wallet_options: WalletOptions,
    pub(crate) per_block_trials: Vec<PerBlockTrialDecryptLog>,
}

impl BlazeSyncData {
    pub fn new(config: &ZingoConfig) -> Self {
        let sync_status = Arc::new(RwLock::new(BatchSyncStatus::default()));

        Self {
            sync_status: sync_status.clone(),
            uri: config.lightwalletd_uri.clone(),
            block_data: BlockAndWitnessData::new(config, sync_status),
            wallet_options: WalletOptions::default(),
            per_block_trials: vec![],
        }
    }

    /*
    pub(crate) fn drain_per_block_log(&mut self) -> Vec<PerBlockTrialDecryptLog> {
        self.per_block_trials.drain(..).collect()
    }*/
    pub fn uri(&self) -> Uri {
        self.uri.read().unwrap().clone()
    }

    pub async fn setup_nth_batch(
        &mut self,
        start_block: u64,
        end_block: u64,
        batch_num: usize,
        existing_blocks: Vec<BlockData>,
        verified_tree: Option<TreeState>,
        wallet_options: WalletOptions,
    ) {
        if start_block < end_block {
            panic!(
                "start_block is: {start_block}\n\
                 end_block is:   {end_block}"
            );
        }

        // Clear the status for a new sync batch
        self.sync_status
            .write()
            .await
            .new_sync_batch(start_block, end_block, batch_num);

        self.wallet_options = wallet_options;

        self.block_data
            .setup_sync(existing_blocks, verified_tree)
            .await;
    }

    // Finish up the sync
    pub async fn finish(&self) {
        self.sync_status.write().await.finish();
    }
}
