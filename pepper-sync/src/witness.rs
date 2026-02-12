//! Module for structs and types associated with witness construction

use std::collections::BTreeMap;

use incrementalmerkletree::{Position, Retention};
use orchard::tree::MerkleHashOrchard;
use shardtree::LocatedPrunableTree;
use zcash_primitives::consensus::BlockHeight;

#[cfg(not(feature = "darkside_test"))]
use {
    crate::error::ServerError, shardtree::store::ShardStore, subtle::CtOption,
    zcash_client_backend::proto::service::SubtreeRoot,
};

pub(crate) const SHARD_HEIGHT: u8 = 16;

/// Required data for updating [`shardtree::ShardTree`]
#[derive(Debug)]
pub(crate) struct WitnessData {
    pub(crate) sapling_initial_position: Position,
    pub(crate) orchard_initial_position: Position,
    pub(crate) sapling_leaves_and_retentions: Vec<(sapling_crypto::Node, Retention<BlockHeight>)>,
    pub(crate) orchard_leaves_and_retentions: Vec<(MerkleHashOrchard, Retention<BlockHeight>)>,
}

impl WitnessData {
    /// Creates new `ShardTreeData`
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
#[derive(Debug)]
pub struct LocatedTreeData<H> {
    /// Located prunable tree
    pub(crate) subtree: LocatedPrunableTree<H>,
    /// Checkpoints
    pub(crate) checkpoints: BTreeMap<BlockHeight, Position>,
}

pub(crate) fn build_located_trees<H>(
    initial_position: Position,
    leaves_and_retentions: Vec<(H, Retention<BlockHeight>)>,
    located_tree_size: usize,
) -> Vec<LocatedTreeData<H>>
where
    H: Copy + PartialEq + incrementalmerkletree::Hashable + Sync + Send,
{
    let (sender, receiver) = crossbeam_channel::unbounded();
    rayon::scope_fifo(|scope| {
        for (i, chunk) in leaves_and_retentions.chunks(located_tree_size).enumerate() {
            let sender = sender.clone();
            scope.spawn_fifo(move |_scope| {
                let start_position = initial_position + ((i * located_tree_size) as u64);
                let tree = LocatedPrunableTree::from_iter(
                    start_position..(start_position + chunk.len() as u64),
                    incrementalmerkletree::Level::from(SHARD_HEIGHT),
                    chunk.iter().copied(),
                );
                let _ignore_error = sender.send(tree);
            });
        }
    });
    drop(sender);

    let mut located_tree_data = Vec::new();
    for tree in receiver.iter().flatten() {
        located_tree_data.push(LocatedTreeData {
            subtree: tree.subtree,
            checkpoints: tree.checkpoints,
        });
    }

    located_tree_data
}

#[cfg(not(feature = "darkside_test"))]
pub(crate) fn add_subtree_roots<S, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    subtree_roots: Vec<SubtreeRoot>,
    shard_tree: &mut shardtree::ShardTree<S, DEPTH, SHARD_HEIGHT>,
) -> Result<(), ServerError>
where
    S: ShardStore<
            H: incrementalmerkletree::Hashable + Clone + PartialEq + FromBytes,
            CheckpointId: Clone + Ord + std::fmt::Debug,
            Error = std::convert::Infallible,
        >,
{
    for (index, tree_root) in subtree_roots.into_iter().enumerate() {
        let node = <S::H as FromBytes>::from_bytes(
            tree_root
                .root_hash
                .try_into()
                .map_err(|_| ServerError::InvalidSubtreeRoot)?,
        )
        .into_option()
        .ok_or(ServerError::InvalidSubtreeRoot)?;
        let shard = LocatedPrunableTree::with_root_value(
            incrementalmerkletree::Address::from_parts(
                incrementalmerkletree::Level::new(SHARD_HEIGHT),
                index as u64,
            ),
            (node, shardtree::RetentionFlags::EPHEMERAL),
        );
        shard_tree.store_mut().put_shard(shard).expect("infallible");
    }

    Ok(())
}

/// Allows generic construction of a shardtree node from raw byte representation
#[cfg(not(feature = "darkside_test"))]
pub(crate) trait FromBytes
where
    Self: Sized,
{
    fn from_bytes(array: [u8; 32]) -> CtOption<Self>;
}

#[cfg(not(feature = "darkside_test"))]
impl FromBytes for orchard::tree::MerkleHashOrchard {
    fn from_bytes(array: [u8; 32]) -> CtOption<Self> {
        Self::from_bytes(&array)
    }
}

#[cfg(not(feature = "darkside_test"))]
impl FromBytes for sapling_crypto::Node {
    fn from_bytes(array: [u8; 32]) -> CtOption<Self> {
        Self::from_bytes(array)
    }
}
