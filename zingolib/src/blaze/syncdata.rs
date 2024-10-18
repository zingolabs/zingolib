use std::sync::Arc;

use tokio::sync::RwLock;
use zcash_client_backend::proto::service::TreeState;

use super::{block_management_reorg_detection::BlockManagementData, sync_status::BatchSyncStatus};
use crate::wallet::data::BlockData;
use crate::wallet::WalletOptions;

pub struct BlazeSyncData {
    pub(crate) block_data: BlockManagementData,
    pub(crate) wallet_options: WalletOptions,
}

impl BlazeSyncData {
    pub fn new() -> Self {
        let sync_status = Arc::new(RwLock::new(BatchSyncStatus::default()));

        Self {
            block_data: BlockManagementData::new(sync_status),
            wallet_options: WalletOptions::default(),
        }
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
        self.block_data
            .sync_status
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
        self.block_data.sync_status.write().await.finish();
    }
}
