//! Traits for interfacing a wallet with the sync engine

use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;

use zcash_client_backend::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::zip32::AccountId;

use crate::primitives::{NullifierMap, WalletBlock};
use crate::witness::{ShardTreeData, ShardTrees};

/// Temporary dump for all neccessary wallet functionality for PoC
pub trait SyncWallet {
    /// Errors associated with interfacing the sync engine with wallet data
    type Error: Debug;

    /// Returns all unified full viewing keys known to this wallet.
    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, UnifiedFullViewingKey>, Self::Error>;
}

/// Trait for interfacing sync engine [`crate::primitives::WalletBlock`] with wallet data
pub trait SyncBlocks: SyncWallet {
    // TODO: add method to get wallet data for writing defualt implementations on other methods

    /// Get a stored wallet compact block from wallet data by block height
    /// Must return error if block is not found
    fn get_wallet_block(&self, block_height: BlockHeight) -> Result<WalletBlock, Self::Error>;

    /// Append wallet compact blocks to wallet data
    fn append_wallet_blocks(
        &mut self,
        wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    ) -> Result<(), Self::Error>;
}

/// Trait for interfacing nullifiers with wallet data
pub trait SyncNullifiers: SyncWallet {
    // TODO: add method to get wallet data for writing defualt implementations on other methods

    /// Append nullifiers to wallet data
    fn append_nullifiers(&mut self, nullifier_map: NullifierMap) -> Result<(), Self::Error>;
}

/// Trait for interfacing shard tree data with wallet data
pub trait SyncShardTrees: SyncWallet {
    /// Get mutable reference to shard trees
    fn get_shard_trees_mut(&mut self) -> Result<&mut ShardTrees, Self::Error>;

    /// Update wallet data with shard tree data
    fn update_shard_trees(&mut self, shard_tree_data: ShardTreeData) -> Result<(), Self::Error> {
        let ShardTreeData {
            sapling_initial_position,
            orchard_initial_position,
            sapling_leaves_and_retentions,
            orchard_leaves_and_retentions,
        } = shard_tree_data;

        self.get_shard_trees_mut()
            .unwrap()
            .sapling_mut()
            .batch_insert(
                sapling_initial_position,
                sapling_leaves_and_retentions.into_iter(),
            )
            .unwrap();
        self.get_shard_trees_mut()
            .unwrap()
            .orchard_mut()
            .batch_insert(
                orchard_initial_position,
                orchard_leaves_and_retentions.into_iter(),
            )
            .unwrap();

        Ok(())
    }
}
