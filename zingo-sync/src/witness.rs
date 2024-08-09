use std::sync::Arc;

use getset::Getters;
use incrementalmerkletree::{Position, Retention};
use orchard::tree::MerkleHashOrchard;
use sapling_crypto::Node;
use shardtree::{store::memory::MemoryShardStore, ShardTree};
use tokio::sync::RwLock;
use zcash_primitives::consensus::BlockHeight;

const NOTE_COMMITMENT_TREE_DEPTH: u8 = 32;
const SHARD_HEIGHT: u8 = 16;
const MAX_CHECKPOINTS: usize = 100;

type SaplingShardStore = MemoryShardStore<Node, BlockHeight>;
type OrchardShardStore = MemoryShardStore<MerkleHashOrchard, BlockHeight>;

#[derive(Getters)]
#[getset(get = "pub")]
pub struct ShardTrees {
    sapling: Arc<RwLock<ShardTree<SaplingShardStore, NOTE_COMMITMENT_TREE_DEPTH, SHARD_HEIGHT>>>,
    orchard: Arc<RwLock<ShardTree<OrchardShardStore, NOTE_COMMITMENT_TREE_DEPTH, SHARD_HEIGHT>>>,
}

impl ShardTrees {
    pub fn new() -> Self {
        Self {
            sapling: Arc::new(RwLock::new(ShardTree::new(
                MemoryShardStore::empty(),
                MAX_CHECKPOINTS,
            ))),
            orchard: Arc::new(RwLock::new(ShardTree::new(
                MemoryShardStore::empty(),
                MAX_CHECKPOINTS,
            ))),
        }
    }
}

impl Default for ShardTrees {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) struct ShardTreeData {
    pub(crate) sapling_initial_position: Position,
    pub(crate) orchard_initial_position: Position,
    pub(crate) sapling_leaves_and_retentions: Vec<(Node, Retention<BlockHeight>)>,
    pub(crate) orchard_leaves_and_retentions: Vec<(MerkleHashOrchard, Retention<BlockHeight>)>,
}

impl ShardTreeData {
    pub(crate) fn new(
        sapling_initial_position: Position,
        orchard_initial_position: Position,
    ) -> Self {
        ShardTreeData {
            sapling_initial_position,
            orchard_initial_position,
            sapling_leaves_and_retentions: Vec::new(),
            orchard_leaves_and_retentions: Vec::new(),
        }
    }
}

pub(crate) async fn update_shardtrees(
    shardtrees: &ShardTrees,
    shardtree_data: ShardTreeData,
) -> Result<(), ()> {
    let ShardTreeData {
        sapling_initial_position,
        orchard_initial_position,
        sapling_leaves_and_retentions,
        orchard_leaves_and_retentions,
    } = shardtree_data;

    shardtrees
        .sapling()
        .write()
        .await
        .batch_insert(
            sapling_initial_position,
            sapling_leaves_and_retentions.into_iter(),
        )
        .unwrap();
    shardtrees
        .orchard()
        .write()
        .await
        .batch_insert(
            orchard_initial_position,
            orchard_leaves_and_retentions.into_iter(),
        )
        .unwrap();

    Ok(())
}
