//! Traits for interfacing a wallet with the sync engine

use std::fmt::Debug;
use std::{
    collections::{BTreeMap, HashMap},
    hash::Hash,
};

use incrementalmerkletree::{Hashable, Level, Position, Retention};
use memuse::DynamicUsage;
use shardtree::LocatedPrunableTree;
use zcash_client_backend::{data_api::ORCHARD_SHARD_HEIGHT, keys::UnifiedFullViewingKey};
use zcash_note_encryption::{BatchDomain, Domain};
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::transaction::TxId;
use zcash_primitives::zip32::AccountId;

use crate::{
    keys::transparent::TransparentAddressId,
    scan::compact_blocks::runners::{BatchRunner, TaggedShardTreeUpdateOrchardBatchRunner},
};
use crate::{
    primitives::{NullifierMap, OutPointMap, SyncState, WalletBlock, WalletTransaction},
    scan::compact_blocks::runners::ShardTreeUpdateBatchRunners,
};
use crate::{
    scan::compact_blocks::runners::TaggedShardTreeUpdateSaplingBatchRunner,
    witness::{ShardTreeData, ShardTrees},
};

// TODO: clean up interface and move many default impls out of traits. consider merging to a simplified SyncWallet interface.

/// Temporary dump for all neccessary wallet functionality for PoC
pub trait SyncWallet {
    /// Errors associated with interfacing the sync engine with wallet data
    type Error: Debug;

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

    /// Removes all wallet blocks above the given `block_height`.
    fn truncate_wallet_blocks(&mut self, truncate_height: BlockHeight) -> Result<(), Self::Error> {
        self.get_wallet_blocks_mut()?
            .retain(|block_height, _| *block_height <= truncate_height);

        Ok(())
    }
}

/// Trait for interfacing [`crate::primitives::WalletTransaction`]s with wallet data
pub trait SyncTransactions: SyncWallet {
    /// Get reference to wallet transactions
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

    /// Removes all wallet transactions above the given `block_height`.
    /// Also sets any output's spending_transaction field to `None` if it's spending transaction was removed.
    fn truncate_wallet_transactions(
        &mut self,
        truncate_height: BlockHeight,
    ) -> Result<(), Self::Error> {
        // TODO: Replace with `extract_if()` when it's in stable rust
        let invalid_txids: Vec<TxId> = self
            .get_wallet_transactions()?
            .values()
            .filter(|tx| tx.block_height() > truncate_height)
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
    /// Get wallet nullifier map
    fn get_nullifiers(&self) -> Result<&NullifierMap, Self::Error>;

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

    /// Removes all mapped nullifiers above the given `block_height`.
    fn truncate_nullifiers(&mut self, truncate_height: BlockHeight) -> Result<(), Self::Error> {
        let nullifier_map = self.get_nullifiers_mut()?;
        nullifier_map
            .sapling_mut()
            .retain(|_, (block_height, _)| *block_height <= truncate_height);
        nullifier_map
            .orchard_mut()
            .retain(|_, (block_height, _)| *block_height <= truncate_height);

        Ok(())
    }
}

/// Trait for interfacing outpoints with wallet data
pub trait SyncOutPoints: SyncWallet {
    /// Get wallet outpoint map
    fn get_outpoints(&self) -> Result<&OutPointMap, Self::Error>;

    /// Get mutable reference to wallet outpoint map
    fn get_outpoints_mut(&mut self) -> Result<&mut OutPointMap, Self::Error>;

    /// Append outpoints to wallet outpoint map
    fn append_outpoints(&mut self, mut outpoint_map: OutPointMap) -> Result<(), Self::Error> {
        self.get_outpoints_mut()?
            .inner_mut()
            .append(outpoint_map.inner_mut());

        Ok(())
    }

    /// Removes all mapped outpoints above the given `block_height`.
    fn truncate_outpoints(&mut self, truncate_height: BlockHeight) -> Result<(), Self::Error> {
        self.get_outpoints_mut()?
            .inner_mut()
            .retain(|_, (block_height, _)| *block_height <= truncate_height);

        Ok(())
    }
}

/// A batch scanning task.
pub(crate) trait Task: Send + 'static {
    fn run(self);
}

pub(crate) trait Batch<D>: Task + Sized
where
    D: BatchDomain<IncomingViewingKey: Send, Recipient: Send, Memo: Send>,
{
    /// The data needed to create the batch
    type Initial;

    /// The items to be processed, in the batch
    type Input;

    /// The result of processing the items
    type ResultVal: Send;

    /// As we may return more than one result, we want a unique
    /// identifier for each
    type ResultKey: Hash + Eq;

    /// The key used to identify a batch
    type BatchKey: Hash + Eq + DynamicUsage;

    fn new(init: Self::Initial) -> Self;

    fn inputs(&self) -> &Vec<Self::Input>;
    fn inputs_mut(&mut self) -> &mut Vec<Self::Input>;

    fn repliers_mut(
        &mut self,
    ) -> &mut Vec<(usize, crossbeam_channel::Sender<(usize, Self::ResultVal)>)>;

    fn is_empty(&self) -> bool {
        self.inputs().is_empty()
    }

    /// Adds the given inputs to this batch.
    ///
    /// `replier` will be called with the result of every output.
    fn add_widgets(
        &mut self,
        widgets: impl ExactSizeIterator<Item = Self::Input>,
        replier: crossbeam_channel::Sender<(usize, Self::ResultVal)>,
    ) {
        let widget_len = widgets.len();
        self.inputs_mut().extend(widgets);
        self.repliers_mut()
            .extend((0..widget_len).map(|output_index| (output_index, replier.clone())));
    }
    fn init_from_runner<T: Tasks<Self>>(
        runner: &crate::scan::compact_blocks::runners::BatchRunner<D, Self, T>,
    ) -> Self::Initial;

    fn reskey_from_batchkeyval(batchkey: &Self::BatchKey, reply_index: usize) -> Self::ResultKey;
}

/// A tracker for the batch scanning tasks that are currently running.
///
/// This enables a [`BatchRunner`] to be optionally configured to track heap memory usage.
pub(crate) trait Tasks<Item> {
    type Task: Task;
    fn new() -> Self;
    fn add_task(&self, item: Item) -> Self::Task;
    fn run_task(&self, item: Item) {
        let task = self.add_task(item);
        rayon::spawn_fifo(|| task.run());
    }
}

impl<Item: Task> Tasks<Item> for () {
    type Task = Item;
    fn new() -> Self {}
    fn add_task(&self, item: Item) -> Self::Task {
        // Return the item itself as the task; we aren't tracking anything about it, so
        // there is no need to wrap it in a newtype.
        item
    }
}

impl<Node> Task for ShardTreeUpdateBatch<Node>
where
    Node: PartialEq + Eq + Hashable + Send + Sync + Clone + 'static,
{
    fn run(self) {
        let tree = LocatedPrunableTree::from_iter(
            self.initial_pos..self.initial_pos + self.leaves_and_retentions.len() as u64,
            // This should be tied to domain. Currently orchard
            // and sapling use the same height.
            Level::new(ORCHARD_SHARD_HEIGHT),
            self.leaves_and_retentions.into_iter(),
        )
        .unwrap();
        match self.repliers.first() {
            Some((index, sender)) => sender.send((*index, tree.subtree)).unwrap(),
            None => panic!("no sender"),
        }
    }
}

pub(crate) struct ShardTreeUpdateBatch<Node> {
    initial_pos: Position,
    leaves_and_retentions: Vec<(Node, Retention<BlockHeight>)>,
    repliers: Vec<(
        usize,
        crossbeam_channel::Sender<(usize, LocatedPrunableTree<Node>)>,
    )>,
}

impl<Node> ShardTreeUpdateBatch<Node> {}

impl<D, Node> Batch<D> for ShardTreeUpdateBatch<Node>
where
    D: BatchDomain + 'static,
    <D as Domain>::Memo: Send,
    <D as Domain>::Recipient: Send,
    <D as Domain>::IncomingViewingKey: Send,
    Node: Send + Sync + PartialEq + Eq + Hashable + Clone + 'static,
{
    type Initial = Position;

    type Input = (Node, Retention<BlockHeight>);

    type ResultVal = LocatedPrunableTree<Node>;

    type ResultKey = ();

    type BatchKey = u64;

    fn new(init: Self::Initial) -> Self {
        Self {
            initial_pos: init,
            leaves_and_retentions: vec![],
            repliers: vec![],
        }
    }

    fn inputs(&self) -> &Vec<Self::Input> {
        &self.leaves_and_retentions
    }

    fn inputs_mut(&mut self) -> &mut Vec<Self::Input> {
        &mut self.leaves_and_retentions
    }

    fn repliers_mut(
        &mut self,
    ) -> &mut Vec<(usize, crossbeam_channel::Sender<(usize, Self::ResultVal)>)> {
        &mut self.repliers
    }

    fn init_from_runner<T: Tasks<Self>>(runner: &BatchRunner<D, Self, T>) -> Self::Initial {
        runner.accumulating_batch.initial_pos
    }

    fn reskey_from_batchkeyval(_batchkey: &Self::BatchKey, _reply_index: usize) -> Self::ResultKey {
    }
}

/// Trait for interfacing shard tree data with wallet data
pub trait SyncShardTrees: SyncWallet {
    /// Get mutable reference to shard trees
    fn get_shard_trees_mut(&mut self) -> Result<&mut ShardTrees, Self::Error>;

    /// Update wallet shard trees with new shard tree data
    // TODO: The batch interface supports streaming leaves and
    // retentions, with a new batch starting whenever full
    // Currently, we collect all shardtreedata before starting
    // the update process.
    fn update_shard_trees(&mut self, shard_tree_data: ShardTreeData) -> Result<(), Self::Error> {
        let ShardTreeData {
            sapling_initial_position,
            orchard_initial_position,
            sapling_leaves_and_retentions,
            orchard_leaves_and_retentions,
        } = shard_tree_data;
        let mut runners = ShardTreeUpdateBatchRunners {
            sapling: TaggedShardTreeUpdateSaplingBatchRunner::<()>::new(
                128,
                sapling_initial_position,
            ),
            orchard: TaggedShardTreeUpdateOrchardBatchRunner::<()>::new(
                128,
                orchard_initial_position,
            ),
        };

        for (i, sapling_chunk) in sapling_leaves_and_retentions.chunks(128).enumerate() {
            runners.sapling.add_widgets(
                (sapling_initial_position + (128 * i as u64)).into(),
                sapling_chunk.iter().cloned(),
            );
            runners.sapling.flush();
        }
        for (i, orchard_chunk) in orchard_leaves_and_retentions.chunks(128).enumerate() {
            runners.orchard.add_widgets(
                (orchard_initial_position + (128 * i as u64)).into(),
                orchard_chunk.iter().cloned(),
            );
            runners.orchard.flush();
        }

        let trees = self.get_shard_trees_mut()?;

        for i in (u64::from(sapling_initial_position)
            ..u64::from(sapling_initial_position + sapling_leaves_and_retentions.len() as u64))
            .into_iter()
            .step_by(128)
        {
            trees
                .sapling_mut()
                .insert_tree(
                    runners.sapling.collect_results(&i).remove(&()).unwrap(),
                    // TODO: This is the checkpoints to insert.
                    BTreeMap::new(),
                )
                .unwrap();
        }
        for i in (u64::from(orchard_initial_position)
            ..u64::from(orchard_initial_position + orchard_leaves_and_retentions.len() as u64))
            .into_iter()
            .step_by(128)
        {
            trees
                .orchard_mut()
                .insert_tree(
                    runners.orchard.collect_results(&i).remove(&()).unwrap(),
                    // TODO: This is the checkpoints to insert.
                    BTreeMap::new(),
                )
                .unwrap();
        }

        Ok(())
    }

    /// Removes all shard tree data above the given `block_height`.
    fn truncate_shard_trees(&mut self, truncate_height: BlockHeight) -> Result<(), Self::Error> {
        // TODO: investigate resetting the shard completely when truncate height is 0
        if !self
            .get_shard_trees_mut()?
            .sapling_mut()
            .truncate_to_checkpoint(&truncate_height)
            .unwrap()
        {
            panic!("max checkpoints should always be higher than verification window!");
        }
        if !self
            .get_shard_trees_mut()?
            .orchard_mut()
            .truncate_to_checkpoint(&truncate_height)
            .unwrap()
        {
            panic!("max checkpoints should always be higher than verification window!");
        }

        Ok(())
    }
}
