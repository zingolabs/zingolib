//! TODO: Add Mod Description Here!
use zcash_client_backend::proto::service::TreeState;

use zcash_primitives::consensus::BlockHeight;

use super::LightWallet;

impl LightWallet {
    /// TODO: Add Doc Comment Here!
    pub(crate) async fn initiate_witness_trees(&self, trees: TreeState) {
        let (legacy_sapling_frontier, legacy_orchard_frontier) =
            crate::data::witness_trees::get_legacy_frontiers(trees);
        if let Some(ref mut trees) = self
            .transaction_context
            .transaction_metadata_set
            .write()
            .await
            .witness_trees_mut()
        {
            trees.insert_all_frontier_nodes(legacy_sapling_frontier, legacy_orchard_frontier)
        };
    }

    /// TODO: Add Doc Comment Here!
    pub async fn ensure_witness_tree_not_above_wallet_blocks(&self) {
        println!("Got to line {}", line!());
        let last_synced_height = self.last_synced_height().await;
        println!("Got to line {}", line!());
        let mut txmds_writelock = self
            .transaction_context
            .transaction_metadata_set
            .write()
            .await;
        println!("Got to line {}", line!());
        if let Some(ref mut trees) = txmds_writelock.witness_trees_mut() {
            trees
                .witness_tree_sapling
                .truncate_removing_checkpoint(&BlockHeight::from(last_synced_height as u32))
                .expect("Infallible");
            trees
                .witness_tree_orchard
                .truncate_removing_checkpoint(&BlockHeight::from(last_synced_height as u32))
                .expect("Infallible");
            trees.add_checkpoint(BlockHeight::from(last_synced_height as u32));
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
                    .max_leaf_position(0)
                    .unwrap()
                    .is_none()
                    || trees
                        .witness_tree_sapling
                        .max_leaf_position(0)
                        .unwrap()
                        .is_none()
            })
    }
}
