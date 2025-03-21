//! Traits for interfacing a wallet with the sync engine

use std::collections::{BTreeMap, BTreeSet, HashMap};

use orchard::tree::MerkleHashOrchard;
use shardtree::store::{Checkpoint, ShardStore};
use zcash_client_backend::data_api::scanning::ScanRange;
use zcash_client_backend::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::transaction::TxId;
use zcash_primitives::zip32::AccountId;

use crate::error::SyncError;
use crate::keys::transparent::TransparentAddressId;
use crate::sync::MAX_VERIFICATION_WINDOW;
use crate::wallet::{
    Locator, NullifierMap, OutputId, ShardTrees, SyncState, WalletBlock, WalletTransaction,
};
use crate::witness::LocatedTreeData;

// TODO: clean up interface and move many default impls out of traits. consider merging to a simplified SyncWallet interface.

/// Trait for interfacing wallet with the sync engine.
pub trait SyncWallet {
    /// Errors associated with interfacing the sync engine with wallet data
    type Error: std::fmt::Debug + std::fmt::Display + std::error::Error;

    /// Returns the block height wallet was created.
    fn get_birthday(&self) -> Result<BlockHeight, Self::Error>;

    /// Returns a reference to wallet sync state.
    fn get_sync_state(&self) -> Result<&SyncState, Self::Error>;

    /// Returns a mutable reference to wallet sync state.
    fn get_sync_state_mut(&mut self) -> Result<&mut SyncState, Self::Error>;

    /// Returns all unified full viewing keys known to this wallet.
    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, UnifiedFullViewingKey>, Self::Error>;

    /// Returns a reference to all the transparent addresses known to this wallet.
    fn get_transparent_addresses(
        &self,
    ) -> Result<&BTreeMap<TransparentAddressId, String>, Self::Error>;

    /// Returns a mutable reference to all the transparent addresses known to this wallet.
    fn get_transparent_addresses_mut(
        &mut self,
    ) -> Result<&mut BTreeMap<TransparentAddressId, String>, Self::Error>;

    /// Aids in-memory wallets to only save when the wallet state has changed by setting a flag to mark that save is
    /// required.
    /// Persitance wallets may use the default implementation.
    fn set_save_flag(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// Trait for interfacing [`crate::wallet::WalletBlock`]s with wallet data
pub trait SyncBlocks: SyncWallet {
    /// Get a stored wallet compact block from wallet data by block height
    ///
    /// Must return error if block is not found
    fn get_wallet_block(&self, block_height: BlockHeight) -> Result<WalletBlock, Self::Error>;

    /// Get mutable reference to wallet blocks
    fn get_wallet_blocks_mut(
        &mut self,
    ) -> Result<&mut BTreeMap<BlockHeight, WalletBlock>, Self::Error>;

    /// Append wallet compact blocks to wallet data
    fn append_wallet_blocks(
        &mut self,
        mut wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    ) -> Result<(), Self::Error> {
        self.get_wallet_blocks_mut()?.append(&mut wallet_blocks);

        Ok(())
    }

    /// Removes all wallet blocks above the given `block_height`.
    fn truncate_wallet_blocks(&mut self, truncate_height: BlockHeight) -> Result<(), Self::Error> {
        self.get_wallet_blocks_mut()?
            .retain(|block_height, _| *block_height <= truncate_height);

        Ok(())
    }
}

/// Trait for interfacing [`crate::wallet::WalletTransaction`]s with wallet data
pub trait SyncTransactions: SyncWallet {
    /// Get reference to wallet transactions
    fn get_wallet_transactions(&self) -> Result<&HashMap<TxId, WalletTransaction>, Self::Error>;

    /// Get mutable reference to wallet transactions
    fn get_wallet_transactions_mut(
        &mut self,
    ) -> Result<&mut HashMap<TxId, WalletTransaction>, Self::Error>;

    /// Insert wallet transaction
    fn insert_wallet_transaction(
        &mut self,
        wallet_transaction: WalletTransaction,
    ) -> Result<(), Self::Error> {
        self.get_wallet_transactions_mut()?
            .insert(wallet_transaction.txid(), wallet_transaction);

        Ok(())
    }

    /// Extend wallet transaction map with new wallet transactions
    fn extend_wallet_transactions(
        &mut self,
        wallet_transactions: HashMap<TxId, WalletTransaction>,
    ) -> Result<(), Self::Error> {
        self.get_wallet_transactions_mut()?
            .extend(wallet_transactions);

        Ok(())
    }

    /// Removes all confirmed wallet transactions above the given `block_height`.
    /// Also sets any output's spending_transaction field to `None` if it's spending transaction was removed.
    fn truncate_wallet_transactions(
        &mut self,
        truncate_height: BlockHeight,
    ) -> Result<(), Self::Error> {
        // TODO: Replace with `extract_if()` when it's in stable rust
        let invalid_txids: Vec<TxId> = self
            .get_wallet_transactions()?
            .values()
            .filter(|tx| tx.status().is_confirmed_after(&truncate_height))
            .map(|tx| tx.transaction().txid())
            .collect();

        let wallet_transactions = self.get_wallet_transactions_mut()?;
        wallet_transactions
            .values_mut()
            .flat_map(|tx| tx.sapling_notes_mut())
            .filter(|note| {
                note.spending_transaction.map_or_else(
                    || false,
                    |spending_txid| invalid_txids.contains(&spending_txid),
                )
            })
            .for_each(|note| {
                note.spending_transaction = None;
            });
        wallet_transactions
            .values_mut()
            .flat_map(|tx| tx.orchard_notes_mut())
            .filter(|note| {
                note.spending_transaction.map_or_else(
                    || false,
                    |spending_txid| invalid_txids.contains(&spending_txid),
                )
            })
            .for_each(|note| {
                note.spending_transaction = None;
            });

        invalid_txids.iter().for_each(|invalid_txid| {
            wallet_transactions.remove(invalid_txid);
        });

        Ok(())
    }
}

/// Trait for interfacing nullifiers with wallet data
pub trait SyncNullifiers: SyncWallet {
    /// Get wallet nullifier map
    fn get_nullifiers(&self) -> Result<&NullifierMap, Self::Error>;

    /// Get mutable reference to wallet nullifier map
    fn get_nullifiers_mut(&mut self) -> Result<&mut NullifierMap, Self::Error>;

    /// Append nullifiers to wallet nullifier map
    fn append_nullifiers(&mut self, mut nullifiers: NullifierMap) -> Result<(), Self::Error> {
        self.get_nullifiers_mut()?
            .sapling
            .append(&mut nullifiers.sapling);
        self.get_nullifiers_mut()?
            .orchard
            .append(&mut nullifiers.orchard);

        Ok(())
    }

    /// Removes all mapped nullifiers above the given `block_height`.
    fn truncate_nullifiers(&mut self, truncate_height: BlockHeight) -> Result<(), Self::Error> {
        let nullifier_map = self.get_nullifiers_mut()?;
        nullifier_map
            .sapling
            .retain(|_, (block_height, _)| *block_height <= truncate_height);
        nullifier_map
            .orchard
            .retain(|_, (block_height, _)| *block_height <= truncate_height);

        Ok(())
    }
}

/// Trait for interfacing outpoints with wallet data
pub trait SyncOutPoints: SyncWallet {
    /// Get wallet outpoint map
    fn get_outpoints(&self) -> Result<&BTreeMap<OutputId, Locator>, Self::Error>;

    /// Get mutable reference to wallet outpoint map
    fn get_outpoints_mut(&mut self) -> Result<&mut BTreeMap<OutputId, Locator>, Self::Error>;

    /// Append outpoints to wallet outpoint map
    fn append_outpoints(
        &mut self,
        outpoints: &mut BTreeMap<OutputId, Locator>,
    ) -> Result<(), Self::Error> {
        self.get_outpoints_mut()?.append(outpoints);

        Ok(())
    }

    /// Removes all mapped outpoints above the given `block_height`.
    fn truncate_outpoints(&mut self, truncate_height: BlockHeight) -> Result<(), Self::Error> {
        self.get_outpoints_mut()?
            .retain(|_, (block_height, _)| *block_height <= truncate_height);

        Ok(())
    }
}

/// Trait for interfacing shard tree data with wallet data
pub trait SyncShardTrees: SyncWallet {
    /// Get reference to shard trees
    fn get_shard_trees(&self) -> Result<&ShardTrees, Self::Error>;

    /// Get mutable reference to shard trees
    fn get_shard_trees_mut(&mut self) -> Result<&mut ShardTrees, Self::Error>;

    /// Update wallet shard trees with new shard tree data
    fn update_shard_trees(
        &mut self,
        scan_range: &ScanRange,
        wallet_height: BlockHeight,
        sapling_located_trees: Vec<LocatedTreeData<sapling_crypto::Node>>,
        orchard_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
    ) -> Result<(), SyncError<Self::Error>> {
        let shard_trees = self.get_shard_trees_mut().map_err(SyncError::WalletError)?;

        // limit the range that checkpoints are manually added to the top MAX_VERIFICATION_WINDOW blocks for efficiency.
        // As we sync the chain tip first and have spend-before-sync, we will always choose anchors very close to chain
        // height and we will also never need to truncate to checkpoints lower than this height.
        let checkpoint_range = match (
            scan_range.block_range().start > wallet_height - MAX_VERIFICATION_WINDOW,
            scan_range.block_range().end - 1 > wallet_height - MAX_VERIFICATION_WINDOW,
        ) {
            (true, _) => scan_range.block_range().clone(),
            (false, true) => {
                (wallet_height - MAX_VERIFICATION_WINDOW)..scan_range.block_range().end
            }
            (false, false) => BlockHeight::from_u32(0)..BlockHeight::from_u32(0),
        };

        // in the case that sapling and/or orchard note commitments are not in an entire block there will be no retention
        // at that height. Therefore, to prevent anchor and truncate errors, checkpoints are manually added first and
        // copy the tree state from the previous checkpoint where the commitment tree has not changed as of that block.
        for checkpoint_height in u32::from(checkpoint_range.start)..u32::from(checkpoint_range.end)
        {
            let checkpoint_height = BlockHeight::from_u32(checkpoint_height);
            let checkpoint = sapling_located_trees
                .iter()
                .flat_map(|tree| tree.checkpoints.iter())
                .find(|(height, _)| **height == checkpoint_height)
                .map_or_else(
                    || {
                        let mut next_checkpoint_below = None;
                        shard_trees
                            .sapling
                            .store()
                            .for_each_checkpoint(100, |height, checkpoint| {
                                if *height < checkpoint_height {
                                    next_checkpoint_below = Some(checkpoint.clone());
                                }
                                Ok(())
                            })
                            .expect("infallible");

                        Checkpoint::from_parts(
                            next_checkpoint_below
                                .expect("should always have a checkpoint below")
                                .tree_state(),
                            BTreeSet::new(),
                        )
                    },
                    |(_, position)| Checkpoint::at_position(*position),
                );

            shard_trees
                .sapling
                .store_mut()
                .add_checkpoint(checkpoint_height, checkpoint)
                .expect("infallible");

            let checkpoint = orchard_located_trees
                .iter()
                .flat_map(|tree| tree.checkpoints.iter())
                .find(|(height, _)| **height == checkpoint_height)
                .map_or_else(
                    || {
                        let mut next_checkpoint_below = None;
                        shard_trees
                            .orchard
                            .store()
                            .for_each_checkpoint(100, |height, checkpoint| {
                                if *height < checkpoint_height {
                                    next_checkpoint_below = Some(checkpoint.clone());
                                }
                                Ok(())
                            })
                            .expect("infallible");

                        Checkpoint::from_parts(
                            next_checkpoint_below
                                .expect("should always have a checkpoint below")
                                .tree_state(),
                            BTreeSet::new(),
                        )
                    },
                    |(_, position)| Checkpoint::at_position(*position),
                );

            shard_trees
                .orchard
                .store_mut()
                .add_checkpoint(checkpoint_height, checkpoint)
                .expect("infallible");
        }

        for tree in sapling_located_trees.into_iter() {
            shard_trees
                .sapling
                .insert_tree(tree.subtree, tree.checkpoints)?;
        }
        for tree in orchard_located_trees.into_iter() {
            shard_trees
                .orchard
                .insert_tree(tree.subtree, tree.checkpoints)?;
        }

        Ok(())
    }

    /// Removes all shard tree data above the given `block_height`.
    fn truncate_shard_trees(
        &mut self,
        truncate_height: BlockHeight,
    ) -> Result<(), SyncError<Self::Error>> {
        if !self
            .get_shard_trees_mut()
            .map_err(SyncError::WalletError)?
            .sapling
            .truncate_to_checkpoint(&truncate_height)?
        {
            panic!("max checkpoints should always be higher or equal to max verification window!");
        }
        if !self
            .get_shard_trees_mut()
            .map_err(SyncError::WalletError)?
            .orchard
            .truncate_to_checkpoint(&truncate_height)?
        {
            panic!("max checkpoints should always be higher or equal to max verification window!");
        }

        Ok(())
    }
}
