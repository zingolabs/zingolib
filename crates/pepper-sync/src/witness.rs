//! Module for structs and types associated with witness construction

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;

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
use zl_netutils::lightwallet_protocol::TreeState;

use crate::sync::SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE;

use {
    crate::error::ServerError, shardtree::store::ShardStore, subtle::CtOption,
    zl_netutils::lightwallet_protocol::SubtreeRoot,
};

pub(crate) use zl_common::engine::SHARD_HEIGHT;

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

#[cfg(test)]
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
        let address = incrementalmerkletree::Address::from_parts(
            incrementalmerkletree::Level::new(SHARD_HEIGHT),
            (subtree_root_start_index + index) as u64,
        );
        let node = <S::H as FromBytes>::from_bytes(
            tree_root
                .root_hash
                .try_into()
                .map_err(|_| ServerError::InvalidSubtreeRoot)?,
        )
        .into_option()
        .ok_or(ServerError::InvalidSubtreeRoot)?;
        let stored = shard_tree
            .store()
            .get_shard(address)
            .expect("infallible")
            .filter(|shard| !shard.root().is_empty() && !shard.root().is_leaf());
        let shard = match stored {
            // A shard whose root the wallet's own leaves already compute is kept as it is.
            Some(shard) if shard.root_hash(address.position_range_end()).is_ok() => continue,
            // A shard that scanning populated but left with a gap keeps its leaves and takes the fetched root as
            // the cached annotation of its root node. shardtree returns the annotation for any query spanning
            // the whole shard, so the root is computable before the gap is scanned, and checks the leaves
            // scanned into the shard afterwards against it.
            Some(shard) => shard.reannotate_root(Some(Arc::new(node))),
            // A bare root or an empty shard is replaced, which heals a root that a re-org moved after it was
            // fetched.
            None => LocatedPrunableTree::with_root_value(
                address,
                (node, shardtree::RetentionFlags::EPHEMERAL),
            ),
        };
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
    /// [`Self::block_height`]. Empty pre-activation.
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
/// tree state. Servers omit the field below the ironwood activation height,
/// so an empty field means the tree is empty. See [`crate::client::get_frontiers`]
/// for the rejection of an omitted field at or above the activation height.
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
/// grid from the Ironwood pool's activation, on the chain's block spacing — or `None` when that
/// upgrade never activates. From NU7 activation the grid's boundaries sit three times as many
/// blocks apart, and the retained checkpoints follow them.
pub(crate) fn anchor_retention_policy(
    consensus_parameters: &impl zcash_protocol::consensus::Parameters,
) -> Option<AnchorRetention> {
    crate::wallet::PoolActivation::of(consensus_parameters, zcash_protocol::ShieldedPool::Ironwood)
        .map(|activation| {
            AnchorRetention::new(
                activation.height(),
                zcash_protocol::zip318::BlockSpacing::for_network(consensus_parameters),
                AnchorRetentionInterval::default(),
            )
        })
}

/// The height range whose grid-boundary checkpoints must currently be pinned, given `as_of`,
/// the wallet's newest scanned block: the last [`ANCHOR_RETENTION_INTERVALS`] grid intervals,
/// measured in reference blocks and converted to blocks by the policy's spacing, so the window
/// keeps its length in time across NU7 activation.
pub(crate) fn anchor_retention_window(
    policy: &AnchorRetention,
    as_of: BlockHeight,
) -> std::ops::RangeInclusive<BlockHeight> {
    let reference_blocks = policy
        .intervals()
        .iter()
        .map(|interval| interval.block_count().get())
        .max()
        .unwrap_or(0)
        .saturating_mul(ANCHOR_RETENTION_INTERVALS);
    retreat(policy.spacing(), as_of, reference_blocks)..=as_of
}

/// The greatest height from which `reference_blocks` reference blocks advance no further than
/// `to`: the inverse of [`BlockSpacing::advance`], which is monotone in its start, found by
/// bisection. Zero when `to` is less than that many reference blocks from genesis.
///
/// [`BlockSpacing::advance`]: zcash_protocol::zip318::BlockSpacing::advance
fn retreat(
    spacing: zcash_protocol::zip318::BlockSpacing,
    to: BlockHeight,
    reference_blocks: u32,
) -> BlockHeight {
    let (mut low, mut high) = (0u32, u32::from(to));
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if spacing.advance(BlockHeight::from_u32(mid), reference_blocks) <= to {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    BlockHeight::from_u32(low)
}

// checkpoints are held for the top SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE blocks of the chain only, the
// non-retained budget of a shard tree: a re-org is measured from the chain height and a spend anchors close to
// it, so no anchor or truncation needs a checkpoint below this window once the chain tip is scanned. the pinned
// anchor boundaries are added separately.
fn checkpoint_range(
    block_range: &Range<BlockHeight>,
    chain_height: BlockHeight,
) -> Range<BlockHeight> {
    let window_start = chain_height.saturating_sub(SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE - 1);

    std::cmp::max(block_range.start, window_start)..block_range.end
}

// the blocks of `block_range`, one load of a scan task, the shard trees hold a checkpoint for: the blocks inside
// the rolling window hanging from `chain_height`, the load's last block, so a session stopped after this load
// anchors a spend there, and the anchor retention boundaries of `policy` inside its window as of the load's
// last block, or `highest_scanned_height` when that is higher, so such a session anchors a pool crossing too.
// the scan places a leaf checkpoint on these blocks and on no other, and the shard tree update adds a
// checkpoint to every tree the block leaves unchanged.
pub(crate) fn checkpoint_heights(
    block_range: &Range<BlockHeight>,
    chain_height: BlockHeight,
    highest_scanned_height: BlockHeight,
    policy: Option<&AnchorRetention>,
) -> BTreeSet<BlockHeight> {
    let last_block = block_range.end - 1;
    let window = checkpoint_range(block_range, chain_height);
    let mut heights = (u32::from(window.start)..u32::from(window.end))
        .map(BlockHeight::from_u32)
        .collect::<BTreeSet<_>>();
    heights.insert(last_block);
    if let Some(policy) = policy {
        let retention_window =
            anchor_retention_window(policy, std::cmp::max(highest_scanned_height, last_block));
        let start = std::cmp::max(*retention_window.start(), block_range.start);
        let end = std::cmp::min(*retention_window.end(), last_block);
        heights.extend(policy.retained_in_range(start..=end));
    }

    heights
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
mod tests;
