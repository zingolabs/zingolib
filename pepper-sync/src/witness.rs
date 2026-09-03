//! Module for structs and types associated with witness construction

use std::collections::BTreeMap;

use incrementalmerkletree::{
    Position, Retention,
    frontier::{CommitmentTree, Frontier},
};
use orchard::tree::MerkleHashOrchard;
use sapling_crypto::Node;
use shardtree::LocatedPrunableTree;
use zcash_client_backend::data_api::anchor_retention::{AnchorRetention, AnchorRetentionInterval};
use zcash_primitives::{block::BlockHash, merkle_tree::read_commitment_tree};
use zcash_protocol::consensus::BlockHeight;
use zingo_netutils::lightwallet_protocol::TreeState;

use {
    crate::error::ServerError, shardtree::store::ShardStore, subtle::CtOption,
    zingo_netutils::lightwallet_protocol::SubtreeRoot,
};

pub(crate) const SHARD_HEIGHT: u8 = 16;

/// Required data for updating [`shardtree::ShardTree`]
#[derive(Debug)]
pub(crate) struct WitnessData {
    pub(crate) sapling_initial_position: Position,
    pub(crate) orchard_initial_position: Position,
    pub(crate) ironwood_initial_position: Position,
    pub(crate) sapling_leaves_and_retentions: Vec<(sapling_crypto::Node, Retention<BlockHeight>)>,
    pub(crate) orchard_leaves_and_retentions: Vec<(MerkleHashOrchard, Retention<BlockHeight>)>,
    pub(crate) ironwood_leaves_and_retentions: Vec<(MerkleHashOrchard, Retention<BlockHeight>)>,
}

impl WitnessData {
    /// Creates new `ShardTreeData`
    pub(crate) fn new(
        sapling_initial_position: Position,
        orchard_initial_position: Position,
        ironwood_initial_position: Position,
    ) -> Self {
        WitnessData {
            sapling_initial_position,
            orchard_initial_position,
            ironwood_initial_position,
            sapling_leaves_and_retentions: Vec::new(),
            orchard_leaves_and_retentions: Vec::new(),
            ironwood_leaves_and_retentions: Vec::new(),
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

/// The number of subtree roots the shard store currently holds.
pub(crate) fn stored_subtree_root_count<S, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    shard_tree: &shardtree::ShardTree<S, DEPTH, SHARD_HEIGHT>,
) -> usize
where
    S: ShardStore<
            H: incrementalmerkletree::Hashable + Clone + PartialEq,
            CheckpointId: Clone + Ord + std::fmt::Debug,
            Error = std::convert::Infallible,
        >,
{
    shard_tree
        .store()
        .get_shard_roots()
        .expect("infallible")
        .len()
}

/// Returns the index to resume subtree-root fetching from for `shard_tree`:
/// its stored-root count, less one when its newest stored root is bare
pub(crate) fn subtree_fetch_start_index<S, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    shard_tree: &shardtree::ShardTree<S, DEPTH, SHARD_HEIGHT>,
) -> u32
where
    S: ShardStore<
            H: incrementalmerkletree::Hashable + Clone + PartialEq,
            CheckpointId: Clone + Ord + std::fmt::Debug,
            Error = std::convert::Infallible,
        >,
{
    let roots = shard_tree.store().get_shard_roots().expect("infallible");
    let Some(newest) = roots.last() else {
        return 0;
    };
    let bare = shard_tree
        .store()
        .get_shard(*newest)
        .expect("infallible")
        .is_some_and(|shard| shard.root().is_leaf());
    (roots.len() - usize::from(bare)) as u32
}

pub(crate) fn add_subtree_roots<S, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    subtree_root_start_index: usize,
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
                (subtree_root_start_index + index) as u64,
            ),
            (node, shardtree::RetentionFlags::EPHEMERAL),
        );
        shard_tree.store_mut().put_shard(shard).expect("infallible");
    }

    Ok(())
}

/// Allows generic construction of a shardtree node from raw byte representation
pub(crate) trait FromBytes
where
    Self: Sized,
{
    fn from_bytes(array: [u8; 32]) -> CtOption<Self>;
}

impl FromBytes for orchard::tree::MerkleHashOrchard {
    fn from_bytes(array: [u8; 32]) -> CtOption<Self> {
        Self::from_bytes(&array)
    }
}

impl FromBytes for sapling_crypto::Node {
    fn from_bytes(array: [u8; 32]) -> CtOption<Self> {
        Self::from_bytes(array)
    }
}

/// The final note commitment tree state for each shielded pool, as of a particular block height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frontiers {
    block_height: BlockHeight,
    block_hash: BlockHash,
    final_sapling_tree:
        Frontier<sapling_crypto::Node, { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH }>,
    final_orchard_tree:
        Frontier<orchard::tree::MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>,
    final_ironwood_tree:
        Frontier<orchard::tree::MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>,
}

#[allow(dead_code)]
impl Frontiers {
    /// Construct a new empty chain state.
    pub(crate) fn empty(block_height: BlockHeight, block_hash: BlockHash) -> Self {
        Self {
            block_height,
            block_hash,
            final_sapling_tree: Frontier::empty(),
            final_orchard_tree: Frontier::empty(),
            final_ironwood_tree: Frontier::empty(),
        }
    }

    /// Construct a new [`Frontiers`] from its constituent parts.
    pub(crate) fn new(
        block_height: BlockHeight,
        block_hash: BlockHash,
        final_sapling_tree: Frontier<
            sapling_crypto::Node,
            { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH },
        >,
        final_orchard_tree: Frontier<
            orchard::tree::MerkleHashOrchard,
            { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
        >,
        final_ironwood_tree: Frontier<
            orchard::tree::MerkleHashOrchard,
            { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
        >,
    ) -> Self {
        Self {
            block_height,
            block_hash,
            final_sapling_tree,
            final_orchard_tree,
            final_ironwood_tree,
        }
    }

    /// Returns the block height to which this chain state applies.
    pub(crate) fn block_height(&self) -> BlockHeight {
        self.block_height
    }

    /// Return the hash of the block.
    pub(crate) fn block_hash(&self) -> BlockHash {
        self.block_hash
    }

    /// Returns the frontier of the Sapling note commitment tree as of the end of the block at
    /// [`Self::block_height`].
    pub(crate) fn final_sapling_tree(
        &self,
    ) -> &Frontier<sapling_crypto::Node, { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH }> {
        &self.final_sapling_tree
    }

    /// Returns the frontier of the Orchard note commitment tree as of the end of the block at
    /// [`Self::block_height`].
    pub(crate) fn final_orchard_tree(
        &self,
    ) -> &Frontier<orchard::tree::MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>
    {
        &self.final_orchard_tree
    }

    /// Returns the frontier of the Ironwood note commitment tree as of the end of the block at
    /// [`Self::block_height`]. Empty pre-activation, and also when the server
    /// does not serve the ironwood tree state yet.
    pub(crate) fn final_ironwood_tree(
        &self,
    ) -> &Frontier<orchard::tree::MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>
    {
        &self.final_ironwood_tree
    }
}

impl TryFrom<TreeState> for Frontiers {
    type Error = std::io::Error;

    fn try_from(value: TreeState) -> Result<Self, Self::Error> {
        let mut hash_bytes = hex::decode(&value.hash).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Block hash is not valid hex: {e:?}"),
            )
        })?;
        // Block-hash hex strings are byte-reversed on the wire.
        hash_bytes.reverse();

        Ok(Frontiers::new(
            value.height.try_into().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid block height")
            })?,
            BlockHash::try_from_slice(&hash_bytes).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Invalid block hash length.",
                )
            })?,
            get_sapling_tree(&value)?.to_frontier(),
            get_orchard_tree(&value)?.to_frontier(),
            get_ironwood_tree(&value)?.to_frontier(),
        ))
    }
}

/// Deserializes and returns the Sapling note commitment tree field of the tree state.
pub(crate) fn get_sapling_tree(
    tree_state: &TreeState,
) -> std::io::Result<CommitmentTree<Node, { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH }>> {
    if tree_state.sapling_tree.is_empty() {
        Ok(CommitmentTree::empty())
    } else {
        let sapling_tree_bytes = hex::decode(&tree_state.sapling_tree).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Hex decoding of Sapling tree bytes failed: {e:?}"),
            )
        })?;
        read_commitment_tree::<Node, _, { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH }>(
            &sapling_tree_bytes[..],
        )
    }
}

/// Deserializes and returns the Orchard note commitment tree field of the tree state.
pub(crate) fn get_orchard_tree(
    tree_state: &TreeState,
) -> std::io::Result<CommitmentTree<MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>>
{
    if tree_state.orchard_tree.is_empty() {
        Ok(CommitmentTree::empty())
    } else {
        let orchard_tree_bytes = hex::decode(&tree_state.orchard_tree).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Hex decoding of Orchard tree bytes failed: {e:?}"),
            )
        })?;
        read_commitment_tree::<MerkleHashOrchard, _, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>(
            &orchard_tree_bytes[..],
        )
    }
}

/// Deserializes and returns the Ironwood note commitment tree field of the
/// tree state. An empty field means the tree is empty, which covers both
/// pre-activation heights and servers that do not serve the ironwood tree
/// state yet.
pub(crate) fn get_ironwood_tree(
    tree_state: &TreeState,
) -> std::io::Result<CommitmentTree<MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>>
{
    if tree_state.ironwood_tree.is_empty() {
        Ok(CommitmentTree::empty())
    } else {
        let ironwood_tree_bytes = hex::decode(&tree_state.ironwood_tree).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Hex decoding of Ironwood tree bytes failed: {e:?}"),
            )
        })?;
        read_commitment_tree::<MerkleHashOrchard, _, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>(
            &ironwood_tree_bytes[..],
        )
    }
}

/// How many grid intervals behind the wallet's newest scanned block anchor-boundary checkpoints
/// stay pinned: the ZIP 318 anchor age cap plus two intervals of slack.
pub(crate) const ANCHOR_RETENTION_INTERVALS: u32 = zcash_protocol::zip318::ANCHOR_AGE_CAP + 2;

/// The anchor-checkpoint retention policy in force for `consensus_parameters` — the ZIP 318
/// grid from the Ironwood pool's activation — or `None` when that upgrade never activates.
pub(crate) fn anchor_retention_policy(
    consensus_parameters: &impl zcash_protocol::consensus::Parameters,
) -> Option<AnchorRetention> {
    crate::wallet::PoolActivation::of(consensus_parameters, zcash_protocol::ShieldedPool::Ironwood)
        .map(|activation| {
            AnchorRetention::new(activation.height(), AnchorRetentionInterval::default())
        })
}

/// The height range whose grid-boundary checkpoints must currently be pinned, given `as_of`,
/// the wallet's newest scanned block.
pub(crate) fn anchor_retention_window(
    policy: &AnchorRetention,
    as_of: BlockHeight,
) -> std::ops::RangeInclusive<BlockHeight> {
    let span = policy
        .intervals()
        .iter()
        .map(|interval| interval.block_count().get())
        .max()
        .unwrap_or(0)
        .saturating_mul(ANCHOR_RETENTION_INTERVALS);
    as_of.saturating_sub(span)..=as_of
}

/// Re-derives one shard store's pinned-checkpoint set from `policy`, pinning every grid
/// boundary in `window` and releasing every pin outside it.
pub(crate) fn repin_anchor_checkpoints<S>(
    policy: &AnchorRetention,
    window: &std::ops::RangeInclusive<BlockHeight>,
    store: &mut S,
) where
    S: ShardStore<CheckpointId = BlockHeight, Error = std::convert::Infallible>,
{
    for id in store.retained_checkpoints().expect("infallible") {
        if !window.contains(&id) || !policy.retains(id) {
            store.remove_retained_checkpoint(&id).expect("infallible");
        }
    }
    for boundary in policy.retained_in_range(window.clone()) {
        store.add_retained_checkpoint(boundary).expect("infallible");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shardtree_ext::ShardTreeExt;
    use crate::wallet::empty_shard_tree;

    /// Pinned grid boundaries survive checkpoint pruning until re-deriving the pins releases
    /// them.
    #[test]
    fn pinned_boundaries_survive_pruning_until_released() {
        let policy =
            AnchorRetention::new(BlockHeight::from_u32(1), AnchorRetentionInterval::default());
        let mut tree = empty_shard_tree::<
            sapling_crypto::Node,
            { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH },
            SHARD_HEIGHT,
        >();

        for height in 1..=600u32 {
            let height = BlockHeight::from_u32(height);
            if policy.retains(height) {
                tree.store_mut()
                    .add_retained_checkpoint(height)
                    .expect("infallible");
            }
            tree.append_checkpoint(height).expect("infallible");
        }

        let checkpoint_exists = |store: &shardtree::store::memory::MemoryShardStore<
            sapling_crypto::Node,
            BlockHeight,
        >,
                                 height: u32| {
            store
                .get_checkpoint(&BlockHeight::from_u32(height))
                .expect("infallible")
                .is_some()
        };
        let count = tree.store().checkpoint_count().expect("infallible");
        tree.store()
            .for_each_checkpoint(count, |id, _| {
                if u32::from(*id) < 494 {
                    assert_eq!(
                        u32::from(*id) % 144,
                        0,
                        "old checkpoint {id} is not a boundary"
                    );
                }
                Ok(())
            })
            .expect("infallible");
        for boundary in [144, 288, 432, 576] {
            assert!(
                checkpoint_exists(tree.store(), boundary),
                "boundary {boundary}"
            );
        }
        for pruned in [143, 300, 431] {
            assert!(
                !checkpoint_exists(tree.store(), pruned),
                "non-boundary {pruned}"
            );
        }

        let window = anchor_retention_window(&policy, BlockHeight::from_u32(1200));
        assert_eq!(*window.start(), BlockHeight::from_u32(336));
        repin_anchor_checkpoints(&policy, &window, tree.store_mut());
        tree.append_checkpoint(BlockHeight::from_u32(601))
            .expect("infallible");
        assert!(!checkpoint_exists(tree.store(), 144));
        assert!(!checkpoint_exists(tree.store(), 288));
        assert!(checkpoint_exists(tree.store(), 432));
        assert!(checkpoint_exists(tree.store(), 576));
    }
}
