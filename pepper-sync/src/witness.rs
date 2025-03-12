//! Module for structs and types associated with witness construction

use std::collections::BTreeMap;

use incrementalmerkletree::{Position, Retention};
use orchard::tree::MerkleHashOrchard;
use shardtree::LocatedPrunableTree;
use zcash_primitives::consensus::BlockHeight;

use crate::MAX_BATCH_OUTPUTS;

#[cfg(not(feature = "darkside_test"))]
use {shardtree::store::ShardStore, zcash_client_backend::proto::service::SubtreeRoot};

pub(crate) const SHARD_HEIGHT: u8 = 16;
const LOCATED_TREE_SIZE: usize = MAX_BATCH_OUTPUTS / 16;

/// Required data for updating [`shardtree::ShardTree`]
pub(crate) struct WitnessData {
    pub(crate) sapling_initial_position: Position,
    pub(crate) orchard_initial_position: Position,
    pub(crate) sapling_leaves_and_retentions: Vec<(sapling_crypto::Node, Retention<BlockHeight>)>,
    pub(crate) orchard_leaves_and_retentions: Vec<(MerkleHashOrchard, Retention<BlockHeight>)>,
}

impl WitnessData {
    /// Creates new ShardTreeData
    pub(crate) fn new(
        sapling_initial_position: Position,
        orchard_initial_position: Position,
    ) -> Self {
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
    pub(crate) subtree: LocatedPrunableTree<H>,
    /// Checkpoints
    pub(crate) checkpoints: BTreeMap<BlockHeight, Position>,
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

#[cfg(not(feature = "darkside_test"))]
pub(crate) fn add_subtree_roots<S, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    subtree_roots: Vec<SubtreeRoot>,
    shard_tree: &mut shardtree::ShardTree<S, DEPTH, SHARD_HEIGHT>,
) where
    S: ShardStore<
        H: incrementalmerkletree::Hashable + Clone + PartialEq + FromBytes<32>,
        CheckpointId: Clone + Ord + std::fmt::Debug,
        Error = std::convert::Infallible,
    >,
{
    subtree_roots
        .into_iter()
        .enumerate()
        .for_each(|(index, tree_root)| {
            let node = <S::H as FromBytes<32>>::from_bytes(tree_root.root_hash.try_into().unwrap());
            let shard = LocatedPrunableTree::with_root_value(
                incrementalmerkletree::Address::from_parts(
                    incrementalmerkletree::Level::new(SHARD_HEIGHT),
                    index as u64,
                ),
                (node, shardtree::RetentionFlags::EPHEMERAL),
            );
            shard_tree.store_mut().put_shard(shard).unwrap();
        });
}

/// Allows generic construction of a shardtree node from raw byte representation
#[cfg(not(feature = "darkside_test"))]
pub(crate) trait FromBytes<const N: usize> {
    fn from_bytes(array: [u8; N]) -> Self;
}

#[cfg(not(feature = "darkside_test"))]
impl FromBytes<32> for orchard::tree::MerkleHashOrchard {
    fn from_bytes(array: [u8; 32]) -> Self {
        Self::from_bytes(&array).unwrap()
    }
}

#[cfg(not(feature = "darkside_test"))]
impl FromBytes<32> for sapling_crypto::Node {
    fn from_bytes(array: [u8; 32]) -> Self {
        Self::from_bytes(array).unwrap()
    }
}
