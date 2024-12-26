//! Module for structs and types associated with witness construction

use std::collections::BTreeMap;

use getset::{Getters, MutGetters};
use incrementalmerkletree::{Position, Retention};
use orchard::tree::MerkleHashOrchard;
use shardtree::{store::memory::MemoryShardStore, LocatedPrunableTree, ShardTree};
use zcash_primitives::consensus::BlockHeight;

const NOTE_COMMITMENT_TREE_DEPTH: u8 = 32;
const SHARD_HEIGHT: u8 = 16;
const MAX_CHECKPOINTS: usize = 100;
const LOCATED_TREE_SIZE: usize = 128;

type SaplingShardStore = MemoryShardStore<sapling_crypto::Node, BlockHeight>;
type OrchardShardStore = MemoryShardStore<MerkleHashOrchard, BlockHeight>;

/// Shard tree wallet data struct
#[derive(Debug, Getters, MutGetters)]
#[getset(get = "pub", get_mut = "pub")]
pub struct ShardTrees {
    /// Sapling shard tree
    sapling: ShardTree<SaplingShardStore, NOTE_COMMITMENT_TREE_DEPTH, SHARD_HEIGHT>,
    /// Orchard shard tree
    orchard: ShardTree<OrchardShardStore, NOTE_COMMITMENT_TREE_DEPTH, SHARD_HEIGHT>,
}

impl ShardTrees {
    /// Create new ShardTrees
    pub fn new() -> Self {
        Self {
            sapling: ShardTree::new(MemoryShardStore::empty(), MAX_CHECKPOINTS),
            orchard: ShardTree::new(MemoryShardStore::empty(), MAX_CHECKPOINTS),
        }
    }

    // This lets us 'split the borrow' to operate
    // on both trees concurrently.
    pub(crate) fn both_mut(
        &mut self,
    ) -> (
        &mut ShardTree<SaplingShardStore, NOTE_COMMITMENT_TREE_DEPTH, SHARD_HEIGHT>,
        &mut ShardTree<OrchardShardStore, NOTE_COMMITMENT_TREE_DEPTH, SHARD_HEIGHT>,
    ) {
        (&mut self.sapling, &mut self.orchard)
    }
}

impl Default for ShardTrees {
    fn default() -> Self {
        Self::new()
    }
}

/// Required data for updating [`shardtree::ShardTree`]
pub(crate) struct WitnessData {
    pub(crate) sapling_initial_position: Position,
    pub(crate) orchard_initial_position: Position,
    pub(crate) sapling_leaves_and_retentions: Vec<(sapling_crypto::Node, Retention<BlockHeight>)>,
    pub(crate) orchard_leaves_and_retentions: Vec<(MerkleHashOrchard, Retention<BlockHeight>)>,
}

impl WitnessData {
    /// Creates new ShardTreeData
    pub fn new(sapling_initial_position: Position, orchard_initial_position: Position) -> Self {
        WitnessData {
            sapling_initial_position,
            orchard_initial_position,
            sapling_leaves_and_retentions: Vec::new(),
            orchard_leaves_and_retentions: Vec::new(),
        }
    }
}

/// Located prunable tree data built from nodes and retentions during scanning for insertion into the shard store.
pub struct LocatedTreeData<H> {
    /// Located prunable tree
    pub subtree: LocatedPrunableTree<H>,
    /// Checkpoints
    pub checkpoints: BTreeMap<BlockHeight, Position>,
}

pub(crate) fn build_located_trees<H>(
    initial_position: Position,
    leaves_and_retentions: Vec<(H, Retention<BlockHeight>)>,
) -> Result<Vec<LocatedTreeData<H>>, ()>
where
    H: Copy + PartialEq + incrementalmerkletree::Hashable + Sync + Send,
{
    //TODO: Play with numbers. Is it more efficient to
    // build larger trees to allow for more pruning to
    // happen in parallel before insertion?
    // Is it better to build smaller trees so that more
    // trees can be built in parallel at the same time?
    // Is inserting trees more efficient if trees are
    // a power of 2 size? Is it more efficient if they
    // are 'aligned' so that the initial_position is
    // a multiple of tree size? All unanswered questions
    // that want to be benchmarked.

    let (sender, receiver) = crossbeam_channel::unbounded();
    rayon::scope_fifo(|scope| {
        for (i, chunk) in leaves_and_retentions.chunks(LOCATED_TREE_SIZE).enumerate() {
            let sender = sender.clone();
            scope.spawn_fifo(move |_scope| {
                let start_position = initial_position + ((i * LOCATED_TREE_SIZE) as u64);
                let tree = LocatedPrunableTree::from_iter(
                    start_position..(start_position + chunk.len() as u64),
                    incrementalmerkletree::Level::from(SHARD_HEIGHT),
                    chunk.iter().copied(),
                );
                sender.send(tree).unwrap();
            })
        }
    });
    drop(sender);

    let mut located_tree_data = Vec::new();
    for tree in receiver.iter() {
        let tree = tree.unwrap();
        located_tree_data.push(LocatedTreeData {
            subtree: tree.subtree,
            checkpoints: tree.checkpoints,
        });
    }

    Ok(located_tree_data)
}
