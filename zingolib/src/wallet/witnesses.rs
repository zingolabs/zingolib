//! TODO: Add Mod Description Here!
use zcash_primitives::consensus::BlockHeight;

use super::LightWallet;

impl LightWallet {
    /// TODO: Add Doc Comment Here!
    pub async fn ensure_witness_tree_not_above_wallet_blocks(&self) {
        let last_synced_height = self.last_synced_height().await;
        let mut txmds_writelock = self
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;
        if let Some(ref mut trees) = txmds_writelock.witness_trees_mut() {
            trees.truncate_to_checkpoint(BlockHeight::from(last_synced_height as u32));
        }
    }

    /// TODO: Add Doc Comment Here!
    pub async fn has_any_empty_commitment_trees(&self) -> bool {
        self.transaction_context
            .transaction_metadata_set
            .read()
            .await
            .witness_trees()
            .is_some_and(|trees| {
                trees
                    .witness_tree_orchard
                    .max_leaf_position(None)
                    .unwrap()
                    .is_none()
                    || trees
                        .witness_tree_sapling
                        .max_leaf_position(None)
                        .unwrap()
                        .is_none()
            })
    }
}
