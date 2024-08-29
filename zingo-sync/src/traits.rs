//! Traits for interfacing a wallet with the sync engine

use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::ops::Range;

use zcash_client_backend::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::transaction::TxId;
use zcash_primitives::zip32::AccountId;

use crate::primitives::{NullifierMap, SyncState, WalletBlock, WalletTransaction};
use crate::witness::{ShardTreeData, ShardTrees};

/// Temporary dump for all neccessary wallet functionality for PoC
pub trait SyncWallet {
    /// Errors associated with interfacing the sync engine with wallet data
    type Error: Debug;

    /// Returns block height wallet was created
    fn get_birthday(&self) -> Result<BlockHeight, Self::Error>;

    /// Returns reference to wallet sync state
    fn get_sync_state(&self) -> Result<&SyncState, Self::Error>;

    /// Returns mutable reference to wallet sync state
    fn get_sync_state_mut(&mut self) -> Result<&mut SyncState, Self::Error>;

    /// Returns all unified full viewing keys known to this wallet.
    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, UnifiedFullViewingKey>, Self::Error>;
}

/// Trait for interfacing [`crate::primitives::WalletBlock`]s with wallet data
pub trait SyncBlocks: SyncWallet {
    /// Get a stored wallet compact block from wallet data by block height
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

    /// Removes all wallet blocks with block height's within the given [invalid_range] (end exclusive)
    fn remove_wallet_blocks(
        &mut self,
        invalid_range: &Range<BlockHeight>,
    ) -> Result<(), Self::Error> {
        self.get_wallet_blocks_mut()?
            .retain(|_, block| !invalid_range.contains(&block.block_height()));

        Ok(())
    }
}

/// Trait for interfacing [`crate::primitives::WalletTransaction`]s with wallet data
pub trait SyncTransactions: SyncWallet {
    /// Get wallet transactions
    fn get_wallet_transactions(&self) -> Result<&HashMap<TxId, WalletTransaction>, Self::Error>;

    /// Get mutable reference to wallet transactions
    fn get_wallet_transactions_mut(
        &mut self,
    ) -> Result<&mut HashMap<TxId, WalletTransaction>, Self::Error>;

    /// Extend wallet transaction map with new wallet transactions
    fn extend_wallet_transactions(
        &mut self,
        wallet_transactions: HashMap<TxId, WalletTransaction>,
    ) -> Result<(), Self::Error> {
        self.get_wallet_transactions_mut()?
            .extend(wallet_transactions);

        Ok(())
    }

    /// Removes all wallet transactions with block height's within the given [invalid_range] (end exclusive)
    /// Also sets any output's spending_transaction field to `None` if it's spending transaction was removed.
    fn remove_wallet_transactions(
        &mut self,
        invalid_range: &Range<BlockHeight>,
    ) -> Result<(), Self::Error> {
        // Replace with `extract_if()` when it's in stable rust
        let invalid_txids: Vec<TxId> = self
            .get_wallet_transactions()?
            .values()
            .filter(|tx| invalid_range.contains(&tx.block_height()))
            .map(|tx| tx.transaction().txid())
            .collect();

        let wallet_transactions = self.get_wallet_transactions_mut()?;
        wallet_transactions
            .values_mut()
            .flat_map(|tx| tx.sapling_notes_mut())
            .filter(|note| {
                note.spending_transaction().map_or_else(
                    || false,
                    |spending_txid| invalid_txids.contains(&spending_txid),
                )
            })
            .for_each(|note| {
                note.set_spending_transaction(None);
            });
        wallet_transactions
            .values_mut()
            .flat_map(|tx| tx.orchard_notes_mut())
            .filter(|note| {
                note.spending_transaction().map_or_else(
                    || false,
                    |spending_txid| invalid_txids.contains(&spending_txid),
                )
            })
            .for_each(|note| {
                note.set_spending_transaction(None);
            });

        invalid_txids.iter().for_each(|invalid_txid| {
            wallet_transactions.remove(invalid_txid);
        });

        Ok(())
    }
}

/// Trait for interfacing nullifiers with wallet data
pub trait SyncNullifiers: SyncWallet {
    // /// Get wallet nullifier map
    // fn get_nullifiers(&self) -> Result<&NullifierMap, Self::Error>;

    /// Get mutable reference to wallet nullifier map
    fn get_nullifiers_mut(&mut self) -> Result<&mut NullifierMap, Self::Error>;

    /// Append nullifiers to wallet nullifier map
    fn append_nullifiers(&mut self, mut nullifier_map: NullifierMap) -> Result<(), Self::Error> {
        self.get_nullifiers_mut()?
            .sapling_mut()
            .append(nullifier_map.sapling_mut());
        self.get_nullifiers_mut()?
            .orchard_mut()
            .append(nullifier_map.orchard_mut());

        Ok(())
    }

    /// Removes all mapped nullifiers with block height's within the given [invalid_range] (end exclusive)
    fn remove_nullifiers(&mut self, invalid_range: &Range<BlockHeight>) -> Result<(), Self::Error> {
        let nullifier_map = self.get_nullifiers_mut()?;
        nullifier_map
            .sapling_mut()
            .retain(|_, (block_height, _)| !invalid_range.contains(block_height));
        nullifier_map
            .orchard_mut()
            .retain(|_, (block_height, _)| !invalid_range.contains(block_height));

        Ok(())
    }
}

/// Trait for interfacing shard tree data with wallet data
pub trait SyncShardTrees: SyncWallet {
    /// Get mutable reference to shard trees
    fn get_shard_trees_mut(&mut self) -> Result<&mut ShardTrees, Self::Error>;

    /// Update wallet shard trees with new shard tree data
    fn update_shard_trees(&mut self, shard_tree_data: ShardTreeData) -> Result<(), Self::Error> {
        let ShardTreeData {
            sapling_initial_position,
            orchard_initial_position,
            sapling_leaves_and_retentions,
            orchard_leaves_and_retentions,
        } = shard_tree_data;

        self.get_shard_trees_mut()?
            .sapling_mut()
            .batch_insert(
                sapling_initial_position,
                sapling_leaves_and_retentions.into_iter(),
            )
            .unwrap();
        self.get_shard_trees_mut()?
            .orchard_mut()
            .batch_insert(
                orchard_initial_position,
                orchard_leaves_and_retentions.into_iter(),
            )
            .unwrap();

        Ok(())
    }

    // TODO: check if shard tree needs to be invalidated due to re-org or leaves can be inserted to replace invalid parts of commitment tree
}
