//! Traits for interfacing a wallet with the sync engine

use std::collections::{BTreeMap, BTreeSet, HashMap};

use tokio::sync::mpsc;
use zip32::DiversifierIndex;

use incrementalmerkletree::Hashable;
use orchard::tree::MerkleHashOrchard;
use shardtree::ShardTree;
use shardtree::store::memory::MemoryShardStore;
use shardtree::store::{Checkpoint, ShardStore, TreeState};
use zcash_client_backend::data_api::anchor_retention::AnchorRetention;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::{self, BlockHeight};
use zcash_protocol::{PoolType, ShieldedPool};
use zip32::AccountId;

use crate::error::{ServerError, SyncError};
use crate::keys::transparent::TransparentAddressId;
use crate::reset_spends;
use crate::shardtree_ext::{RollbackOutcome, ShardTreeExt};
use crate::sync::truncate::{PoolTruncation, plan_pool_truncation, tree_facts};
use crate::sync::{MAX_SHARDTREE_CHECKPOINTS, ScanRange};
use crate::wallet::{
    Ironwood, NullifierMap, Orchard, OutputId, Sapling, ShardTrees, SyncState, WalletBlock,
    WalletTransaction, empty_shard_tree,
};
use crate::witness::LocatedTreeData;
use crate::{SyncDomain, client, sync::set_transactions_failed_unchecked};

use super::{FetchRequest, ScanTarget, witness};

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

    /// Add orchard address to wallet's unified address list.
    fn add_orchard_address(
        &mut self,
        account_id: zip32::AccountId,
        address: orchard::Address,
        diversifier_index: DiversifierIndex,
    ) -> Result<(), Self::Error>;

    /// Add sapling address to wallet's unified address list.
    fn add_sapling_address(
        &mut self,
        account_id: zip32::AccountId,
        address: sapling_crypto::PaymentAddress,
        diversifier_index: DiversifierIndex,
    ) -> Result<(), Self::Error>;

    /// Returns a reference to all transparent addresses known to this wallet.
    ///
    /// These addresses must be in-use, they do not include gap addresses.
    fn get_transparent_addresses(
        &self,
    ) -> Result<&BTreeMap<TransparentAddressId, String>, Self::Error>;

    /// Returns a mutable reference to all transparent addresses known to this wallet.
    ///
    /// These addresses must be in-use, they do not include gap addresses.
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

    /// Extend wallet transaction map with new wallet transactions.
    ///
    /// A transaction in `wallet_transactions` must replace the wallet's record of the transaction with the same txid.
    fn extend_wallet_transactions(
        &mut self,
        wallet_transactions: HashMap<TxId, WalletTransaction>,
    ) -> Result<(), Self::Error> {
        self.get_wallet_transactions_mut()?
            .extend(wallet_transactions);

        Ok(())
    }

    /// Sets all confirmed wallet transactions above the given `block_height` to `Failed` status.
    /// Also sets any output's `spending_transaction` field to `None` if it's spending transaction was set to `Failed`
    /// status.
    fn truncate_wallet_transactions(
        &mut self,
        truncate_height: BlockHeight,
        set_truncated_transactions_failed: bool,
    ) -> Result<(), Self::Error> {
        let invalid_txids: Vec<TxId> = self
            .get_wallet_transactions()?
            .values()
            .filter(|tx| tx.status().is_confirmed_after(&truncate_height))
            .map(|tx| tx.transaction().txid())
            .collect();

        if set_truncated_transactions_failed {
            set_transactions_failed_unchecked(self.get_wallet_transactions_mut()?, invalid_txids);
        } else {
            let wallet_transactions = self.get_wallet_transactions_mut()?;
            wallet_transactions.retain(|txid, _| !invalid_txids.contains(txid));
            reset_spends(wallet_transactions, invalid_txids);
        }

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
    fn append_nullifiers(&mut self, nullifiers: &mut NullifierMap) -> Result<(), Self::Error> {
        self.get_nullifiers_mut()?
            .sapling
            .append(&mut nullifiers.sapling);
        self.get_nullifiers_mut()?
            .orchard
            .append(&mut nullifiers.orchard);
        self.get_nullifiers_mut()?
            .ironwood
            .append(&mut nullifiers.ironwood);

        Ok(())
    }

    /// Removes all mapped nullifiers above the given `block_height`.
    fn truncate_nullifiers(&mut self, truncate_height: BlockHeight) -> Result<(), Self::Error> {
        let nullifier_map = self.get_nullifiers_mut()?;
        nullifier_map
            .sapling
            .retain(|_, scan_target| scan_target.block_height <= truncate_height);
        nullifier_map
            .orchard
            .retain(|_, scan_target| scan_target.block_height <= truncate_height);
        nullifier_map
            .ironwood
            .retain(|_, scan_target| scan_target.block_height <= truncate_height);

        Ok(())
    }
}

/// Trait for interfacing outpoints with wallet data
pub trait SyncOutPoints: SyncWallet {
    /// Get wallet outpoint map
    fn get_outpoints(&self) -> Result<&BTreeMap<OutputId, ScanTarget>, Self::Error>;

    /// Get mutable reference to wallet outpoint map
    fn get_outpoints_mut(&mut self) -> Result<&mut BTreeMap<OutputId, ScanTarget>, Self::Error>;

    /// Append outpoints to wallet outpoint map
    fn append_outpoints(
        &mut self,
        outpoints: &mut BTreeMap<OutputId, ScanTarget>,
    ) -> Result<(), Self::Error> {
        self.get_outpoints_mut()?.append(outpoints);

        Ok(())
    }

    /// Removes all mapped outpoints above the given `block_height`.
    fn truncate_outpoints(&mut self, truncate_height: BlockHeight) -> Result<(), Self::Error> {
        self.get_outpoints_mut()?
            .retain(|_, scan_target| scan_target.block_height <= truncate_height);

        Ok(())
    }
}

/// Trait for interfacing shard tree data with wallet data
pub trait SyncShardTrees: SyncWallet {
    /// Get reference to shard trees
    fn get_shard_trees(&self) -> Result<&ShardTrees, Self::Error>;

    /// Get mutable reference to shard trees
    fn get_shard_trees_mut(&mut self) -> Result<&mut ShardTrees, Self::Error>;

    /// Update wallet shard trees with new shard tree data.
    ///
    /// `highest_scanned_height` is the height of the highest scanned block in the wallet not including the `scan_range` we are updating.
    #[allow(clippy::too_many_arguments)]
    fn update_shard_trees(
        &mut self,
        consensus_parameters: &(impl consensus::Parameters + Sync),
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        scan_range: &ScanRange,
        highest_scanned_height: BlockHeight,
        anchor_retention: Option<AnchorRetention>,
        sapling_located_trees: Vec<LocatedTreeData<sapling_crypto::Node>>,
        orchard_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
        ironwood_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
    ) -> impl std::future::Future<Output = Result<(), SyncError<Self::Error>>> + Send
    where
        Self: std::marker::Send,
    {
        async move {
            let shard_trees = self.get_shard_trees_mut().map_err(SyncError::WalletError)?;

            // limit the range that checkpoints are manually added to the top MAX_SHARDTREE_CHECKPOINTS scanned blocks for efficiency.
            // As we sync the chain tip first and have spend-before-sync, we will always choose anchors very close to chain
            // height and we will also never need to truncate to checkpoints lower than this height.
            let checkpoint_range = if scan_range.block_range().start > highest_scanned_height {
                let verification_window_start = scan_range
                    .block_range()
                    .end
                    .saturating_sub(MAX_SHARDTREE_CHECKPOINTS);

                std::cmp::max(scan_range.block_range().start, verification_window_start)
                    ..scan_range.block_range().end
            } else if scan_range.block_range().end
                > highest_scanned_height.saturating_sub(MAX_SHARDTREE_CHECKPOINTS) + 1
            {
                let verification_window_start =
                    highest_scanned_height.saturating_sub(MAX_SHARDTREE_CHECKPOINTS) + 1;

                std::cmp::max(scan_range.block_range().start, verification_window_start)
                    ..scan_range.block_range().end
            } else {
                BlockHeight::from_u32(0)..BlockHeight::from_u32(0)
            };

            // in the case that sapling and/or orchard and/or ironwood note commitments are not in an entire block there will be no retention
            // at that height. Therefore, to prevent anchor and truncate errors, checkpoints are manually added first and
            // copy the tree state from the previous checkpoint where the commitment tree has not changed as of that block.
            let mut checkpoint_heights = (u32::from(checkpoint_range.start)
                ..u32::from(checkpoint_range.end))
                .map(BlockHeight::from_u32)
                .collect::<Vec<_>>();
            let anchor_retention_window = anchor_retention.as_ref().map(|retention| {
                let as_of = std::cmp::max(highest_scanned_height, scan_range.block_range().end - 1);
                (
                    retention,
                    witness::anchor_retention_window(retention, as_of),
                )
            });
            if let Some((retention, window)) = &anchor_retention_window {
                let start = std::cmp::max(*window.start(), scan_range.block_range().start);
                let end = std::cmp::min(*window.end(), scan_range.block_range().end - 1);
                checkpoint_heights.extend(
                    retention
                        .retained_in_range(start..=end)
                        .into_iter()
                        .filter(|boundary| !checkpoint_range.contains(boundary)),
                );
            }

            // every checkpoint is determined before the shard trees are updated, as a checkpoint may be fetched from
            // the server. a failed request then leaves the shard trees as they are.
            let mut sapling_checkpoints = BTreeMap::new();
            let mut orchard_checkpoints = BTreeMap::new();
            let mut ironwood_checkpoints = BTreeMap::new();
            for checkpoint_height in checkpoint_heights {
                let sapling_checkpoint = determine_checkpoint::<
                    Sapling,
                    sapling_crypto::Node,
                    { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH },
                    { witness::SHARD_HEIGHT },
                >(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    checkpoint_height,
                    &sapling_located_trees,
                    &shard_trees.sapling,
                    &sapling_checkpoints,
                )
                .await?;
                sapling_checkpoints.insert(checkpoint_height, sapling_checkpoint);
                let orchard_checkpoint = determine_checkpoint::<
                    Orchard,
                    MerkleHashOrchard,
                    { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
                    { witness::SHARD_HEIGHT },
                >(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    checkpoint_height,
                    &orchard_located_trees,
                    &shard_trees.orchard,
                    &orchard_checkpoints,
                )
                .await?;
                orchard_checkpoints.insert(checkpoint_height, orchard_checkpoint);
                let ironwood_checkpoint = determine_checkpoint::<
                    Ironwood,
                    MerkleHashOrchard,
                    { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
                    { witness::SHARD_HEIGHT },
                >(
                    consensus_parameters,
                    fetch_request_sender.clone(),
                    checkpoint_height,
                    &ironwood_located_trees,
                    &shard_trees.ironwood,
                    &ironwood_checkpoints,
                )
                .await?;
                ironwood_checkpoints.insert(checkpoint_height, ironwood_checkpoint);
            }

            if let Some((retention, window)) = &anchor_retention_window {
                witness::repin_anchor_checkpoints(
                    retention,
                    window,
                    shard_trees.sapling.store_mut(),
                );
                witness::repin_anchor_checkpoints(
                    retention,
                    window,
                    shard_trees.orchard.store_mut(),
                );
                witness::repin_anchor_checkpoints(
                    retention,
                    window,
                    shard_trees.ironwood.store_mut(),
                );
            }
            for (checkpoint_height, checkpoint) in sapling_checkpoints {
                shard_trees
                    .sapling
                    .store_mut()
                    .add_checkpoint(checkpoint_height, checkpoint)
                    .expect("infallible");
            }
            for (checkpoint_height, checkpoint) in orchard_checkpoints {
                shard_trees
                    .orchard
                    .store_mut()
                    .add_checkpoint(checkpoint_height, checkpoint)
                    .expect("infallible");
            }
            for (checkpoint_height, checkpoint) in ironwood_checkpoints {
                shard_trees
                    .ironwood
                    .store_mut()
                    .add_checkpoint(checkpoint_height, checkpoint)
                    .expect("infallible");
            }

            // TODO: use `batch_insert_trees`
            for tree in sapling_located_trees {
                shard_trees
                    .sapling
                    .insert_tree(tree.subtree, tree.checkpoints)?;
            }
            for tree in orchard_located_trees {
                shard_trees
                    .orchard
                    .insert_tree(tree.subtree, tree.checkpoints)?;
            }
            for tree in ironwood_located_trees {
                shard_trees
                    .ironwood
                    .insert_tree(tree.subtree, tree.checkpoints)?;
            }

            Ok(())
        }
    }

    /// Replaces all three shard trees with empty trees.
    fn clear_shard_trees(&mut self) -> Result<(), SyncError<Self::Error>> {
        let shard_trees = self.get_shard_trees_mut().map_err(SyncError::WalletError)?;
        tracing::info!("Clearing shard trees.");
        shard_trees.sapling = empty_shard_tree();
        shard_trees.orchard = empty_shard_tree();
        shard_trees.ironwood = empty_shard_tree();

        Ok(())
    }

    /// Removes all shard tree data above the given `truncate_height`:
    /// each tree rolls back to its checkpoint at that height, stays
    /// untouched because it records nothing above it, or, holding state
    /// it cannot roll back, aborts with
    /// [`SyncError::TruncationError`] so the caller can fall back to the
    /// clear-and-rescan recovery. Each tree's outcome is decided by the
    /// pure per-pool rule `plan_pool_truncation` over facts read here
    /// at the point of application (see [`crate::sync::truncate`]).
    fn truncate_shard_trees(
        &mut self,
        truncate_height: BlockHeight,
    ) -> Result<(), SyncError<Self::Error>> {
        let shard_trees = self.get_shard_trees_mut().map_err(SyncError::WalletError)?;
        truncate_pool_tree(&mut shard_trees.sapling, truncate_height, PoolType::SAPLING)?;
        truncate_pool_tree(&mut shard_trees.orchard, truncate_height, PoolType::ORCHARD)?;
        truncate_pool_tree(
            &mut shard_trees.ironwood,
            truncate_height,
            PoolType::IRONWOOD,
        )?;

        Ok(())
    }
}

/// Truncates one pool's shard tree: reads the tree's facts, decides its
/// outcome through the pure per-pool rule ([`plan_pool_truncation`]),
/// and applies it.
///
/// [`PoolTruncation::ToCheckpoint`] rolls the tree back to its
/// checkpoint at `truncate_height`. [`PoolTruncation::Untouched`] leaves
/// the tree alone. [`PoolTruncation::RequiresRescan`], and a planned
/// rollback the tree store unexpectedly refuses, becomes
/// [`SyncError::TruncationError`] naming the pool, so the caller can
/// fall back to the clear-and-rescan recovery.
fn truncate_pool_tree<H, E, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    tree: &mut ShardTree<MemoryShardStore<H, BlockHeight>, DEPTH, SHARD_HEIGHT>,
    truncate_height: BlockHeight,
    pool: PoolType,
) -> Result<(), SyncError<E>>
where
    H: Hashable + Clone + PartialEq,
    E: std::fmt::Debug + std::fmt::Display,
{
    match plan_pool_truncation(tree_facts(tree, truncate_height), truncate_height) {
        PoolTruncation::Untouched => Ok(()),
        PoolTruncation::ToCheckpoint { checkpoint } => {
            match tree.rollback_to_checkpoint(checkpoint)? {
                RollbackOutcome::RolledBack => Ok(()),
                RollbackOutcome::NoSuchCheckpoint => {
                    tracing::error!(
                        "{pool} shard tree refused the planned rollback to its checkpoint at \
                         {checkpoint}! Beginning rescan."
                    );
                    Err(SyncError::TruncationError(truncate_height, pool))
                }
            }
        }
        PoolTruncation::RequiresRescan { newest_checkpoint } => {
            tracing::error!(
                "{pool} shard tree holds state up to checkpoint {newest_checkpoint}, above the \
                 truncation target {truncate_height}, with no checkpoint at the target! \
                 Beginning rescan."
            );
            Err(SyncError::TruncationError(truncate_height, pool))
        }
    }
}

/// Determines the checkpoint of `shard_tree` at `checkpoint_height`. The shard tree is only read.
///
/// The checkpoint is taken from the `located_trees` where the block holds note commitments of the pool. Otherwise
/// the pool's commitment tree is unchanged as of the block, so the tree state is copied from the checkpoint of the
/// block below, looked up in `determined_checkpoints` and then in the shard tree, or fetched from the server where
/// neither holds it.
// TODO: move into `update_shard_trees` trait method
async fn determine_checkpoint<D, L, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    checkpoint_height: BlockHeight,
    located_trees: &[LocatedTreeData<L>],
    shard_tree: &shardtree::ShardTree<
        shardtree::store::memory::MemoryShardStore<L, BlockHeight>,
        DEPTH,
        SHARD_HEIGHT,
    >,
    determined_checkpoints: &BTreeMap<BlockHeight, Checkpoint>,
) -> Result<Checkpoint, ServerError>
where
    L: Clone + PartialEq + incrementalmerkletree::Hashable,
    D: SyncDomain,
{
    if let Some((_, position)) = located_trees
        .iter()
        .flat_map(|tree| tree.checkpoints.iter())
        .find(|(height, _)| **height == checkpoint_height)
    {
        return Ok(Checkpoint::at_position(*position));
    }

    let mut previous_checkpoint = determined_checkpoints
        .get(&(checkpoint_height - 1))
        .cloned();
    if previous_checkpoint.is_none() {
        shard_tree
            .store()
            .for_each_checkpoint(
                MAX_SHARDTREE_CHECKPOINTS as usize + 100,
                |height, checkpoint| {
                    if *height == checkpoint_height - 1 {
                        previous_checkpoint = Some(checkpoint.clone());
                    }
                    Ok(())
                },
            )
            .expect("infallible");
    }

    let tree_state = if let Some(checkpoint) = previous_checkpoint {
        checkpoint.tree_state()
    } else {
        let frontiers = client::get_frontiers(
            fetch_request_sender.clone(),
            consensus_parameters,
            checkpoint_height,
        )
        .await?;
        let tree_size = match D::SHIELDED_PROTOCOL {
            ShieldedPool::Sapling => frontiers.final_sapling_tree().tree_size(),
            ShieldedPool::Orchard => frontiers.final_orchard_tree().tree_size(),
            ShieldedPool::Ironwood => frontiers.final_ironwood_tree().tree_size(),
        };
        if tree_size == 0 {
            TreeState::Empty
        } else {
            TreeState::AtPosition(incrementalmerkletree::Position::from(tree_size - 1))
        }
    };

    Ok(Checkpoint::from_parts(tree_state, BTreeSet::new()))
}
