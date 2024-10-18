//! Module for stucts and types associated with witness construction

use getset::{Getters, MutGetters};
use incrementalmerkletree::{Position, Retention};
use orchard::tree::MerkleHashOrchard;
use sapling_crypto::Node;
use shardtree::{store::memory::MemoryShardStore, ShardTree};
use zcash_primitives::consensus::BlockHeight;

const NOTE_COMMITMENT_TREE_DEPTH: u8 = 32;
const SHARD_HEIGHT: u8 = 16;
const MAX_CHECKPOINTS: usize = 100;

type SaplingShardStore = MemoryShardStore<Node, BlockHeight>;
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
}

impl Default for ShardTrees {
    fn default() -> Self {
        Self::new()
    }
}

/// Required data for updating [`shardtree::ShardTree`]
pub struct ShardTreeData {
    pub(crate) sapling_initial_position: Position,
    pub(crate) orchard_initial_position: Position,
    pub(crate) sapling_leaves_and_retentions: Vec<(Node, Retention<BlockHeight>)>,
    pub(crate) orchard_leaves_and_retentions: Vec<(MerkleHashOrchard, Retention<BlockHeight>)>,
}

impl ShardTreeData {
    /// Creates new ShardTreeData
    pub fn new(sapling_initial_position: Position, orchard_initial_position: Position) -> Self {
        ShardTreeData {
            sapling_initial_position,
            orchard_initial_position,
            sapling_leaves_and_retentions: Vec::new(),
            orchard_leaves_and_retentions: Vec::new(),
        }
    }
}
