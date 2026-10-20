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
use zingo_netutils::lightwallet_protocol::TreeState;

use crate::sync::SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE;

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

// checkpoints are held for the top SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE scanned blocks only, the non-retained
// budget of a shard tree. the chain tip is scanned first and a spend anchors close to the chain height, so no
// anchor or truncation needs a checkpoint below this window. the pinned anchor boundaries are added separately.
// the window hangs from `scan_horizon`, the highest block the wallet holds as scanned once the load's task
// completes, so the loads of a task below its top window place no checkpoint for the next load to prune.
fn checkpoint_range(
    block_range: &Range<BlockHeight>,
    scan_horizon: BlockHeight,
) -> Range<BlockHeight> {
    let window_start = scan_horizon.saturating_sub(SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE - 1);

    std::cmp::max(block_range.start, window_start)..block_range.end
}

// the blocks of `block_range` the shard trees hold a checkpoint for: the blocks inside the checkpoint window
// hanging from `scan_horizon`, and the anchor retention boundaries of `policy` inside its window as of the
// horizon. the scan places a leaf checkpoint on these blocks and on no other, and the shard tree update adds a
// checkpoint to every tree the block leaves unchanged.
pub(crate) fn checkpoint_heights(
    block_range: &Range<BlockHeight>,
    scan_horizon: BlockHeight,
    policy: Option<&AnchorRetention>,
) -> BTreeSet<BlockHeight> {
    let window = checkpoint_range(block_range, scan_horizon);
    let mut heights = (u32::from(window.start)..u32::from(window.end))
        .map(BlockHeight::from_u32)
        .collect::<BTreeSet<_>>();
    if let Some(policy) = policy {
        let retention_window = anchor_retention_window(policy, scan_horizon);
        let start = std::cmp::max(*retention_window.start(), block_range.start);
        let end = std::cmp::min(*retention_window.end(), block_range.end - 1);
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
mod tests {
    use super::*;
    use crate::mocks::{orchard_subtree_root, orchard_tree_scanned_past_the_fetched_roots};
    use crate::shardtree_ext::ShardTreeExt;
    use crate::wallet::empty_shard_tree;
    use incrementalmerkletree::{Address, Hashable as _, Level};

    #[test]
    fn fetched_roots_replace_bare_roots_and_keep_scanned_shards() {
        let mut shard_trees = orchard_tree_scanned_past_the_fetched_roots();
        let tree = &mut shard_trees.orchard;
        assert_eq!(stored_subtree_root_count(tree), 5);
        let moved_root = MerkleHashOrchard::empty_root(Level::from(SHARD_HEIGHT + 1));

        add_subtree_roots(2, vec![orchard_subtree_root(moved_root); 2], tree).unwrap();

        let shard = |index: u64| {
            let address = Address::from_parts(Level::from(SHARD_HEIGHT), index);
            tree.store().get_shard(address).unwrap().unwrap()
        };
        assert_eq!(
            shard(2)
                .root_hash(Position::from(3 << SHARD_HEIGHT))
                .unwrap(),
            moved_root,
            "the bare root is replaced"
        );
        assert!(
            !shard(3).root().is_leaf(),
            "the shard scanning populated is kept"
        );
    }

    // A shard that scanning populated but left with a gap has no computable root until the gap is scanned. The
    // fetched root fills in as the cached annotation of its root node, survives the leaves scanned into the shard
    // afterwards, and is not used for an anchor inside the shard, which still needs the leaves up to the anchor.
    #[test]
    fn fetched_root_annotates_a_populated_shard_with_a_gap() {
        let mut shard_trees = orchard_tree_scanned_past_the_fetched_roots();
        let tree = &mut shard_trees.orchard;
        let shard_3 = Address::from_parts(Level::from(SHARD_HEIGHT), 3);
        let whole_shard = shard_3.position_range_end();
        let stored_shard = |tree: &shardtree::ShardTree<
            crate::wallet::OrchardShardStore,
            { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
            SHARD_HEIGHT,
        >| tree.store().get_shard(shard_3).unwrap().unwrap();
        assert!(stored_shard(tree).root_hash(whole_shard).is_err());
        // the root the chain holds for shard 3, consistent with the cap node over shards 0 to 3 that the shard 4
        // frontier placed, as a fetched root and the frontier both come from the chain
        let chain_root = MerkleHashOrchard::empty_root(Level::from(SHARD_HEIGHT));

        add_subtree_roots(3, vec![orchard_subtree_root(chain_root)], tree).unwrap();

        assert!(
            !stored_shard(tree).root().is_leaf(),
            "the scanned leaves are kept"
        );
        assert_eq!(
            stored_shard(tree).root_hash(whole_shard).unwrap(),
            chain_root
        );

        tree.batch_insert(
            Position::from((3 << SHARD_HEIGHT) + 1),
            std::iter::repeat_n((MerkleHashOrchard::empty_leaf(), Retention::Ephemeral), 4),
        )
        .unwrap();

        assert_eq!(
            stored_shard(tree).root_hash(whole_shard).unwrap(),
            chain_root
        );
        assert!(
            stored_shard(tree)
                .root_hash(Position::from((3 << SHARD_HEIGHT) + 3))
                .is_err(),
            "an anchor inside the shard computes from the leaves, and the gap is still there"
        );
    }

    mod checkpoint_heights {
        use std::num::NonZeroU32;

        use super::*;

        const HIGHEST_SCANNED: u32 = 10_000;
        // twice the window, so a range holds blocks below its top window
        const RANGE_LENGTH: u32 = 2 * SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE;

        fn heights(
            block_range: Range<u32>,
            scan_horizon: u32,
            policy: Option<&AnchorRetention>,
        ) -> Vec<u32> {
            checkpoint_heights(
                &(BlockHeight::from_u32(block_range.start)..BlockHeight::from_u32(block_range.end)),
                BlockHeight::from_u32(scan_horizon),
                policy,
            )
            .into_iter()
            .map(u32::from)
            .collect()
        }

        /// A task above the highest scanned height, whose horizon is its last block, holds checkpoints for its
        /// top window of blocks.
        #[test]
        fn a_task_above_the_highest_scanned_height_checkpoints_its_top_window() {
            let range = HIGHEST_SCANNED + 1_000..HIGHEST_SCANNED + 1_000 + RANGE_LENGTH;

            assert_eq!(
                heights(range.clone(), range.end - 1, None),
                (range.end - SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE..range.end)
                    .collect::<Vec<_>>()
            );
        }

        /// The loads of a task hang their checkpoints from the task's last block: a load below the task's top
        /// window holds none, and a load reaching into it holds checkpoints from the window start.
        #[test]
        fn a_load_hangs_its_checkpoints_from_its_task() {
            let task = HIGHEST_SCANNED + 1_000..HIGHEST_SCANNED + 1_000 + RANGE_LENGTH;
            let horizon = task.end - 1;
            let window_start = task.end - SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE;
            let below = task.start..window_start - 10;
            let reaching = window_start - 10..window_start + 10;

            assert!(heights(below, horizon, None).is_empty());
            assert_eq!(
                heights(reaching.clone(), horizon, None),
                (window_start..reaching.end).collect::<Vec<_>>()
            );
        }

        /// A range inside the window below the highest scanned height holds checkpoints from the window start.
        #[test]
        fn a_range_below_the_highest_scanned_height_checkpoints_from_the_window_start() {
            let window_start = HIGHEST_SCANNED - SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE + 1;
            let range = window_start - 10..window_start + 10;

            assert_eq!(
                heights(range.clone(), HIGHEST_SCANNED, None),
                (window_start..range.end).collect::<Vec<_>>()
            );
        }

        /// A range below the window holds no checkpoint.
        #[test]
        fn a_range_below_the_window_holds_no_checkpoint() {
            let window_start = HIGHEST_SCANNED - SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE + 1;

            assert!(
                heights(window_start - 100..window_start - 50, HIGHEST_SCANNED, None).is_empty()
            );
        }

        /// The anchor retention boundaries inside the retention window are held below the top window, and nothing
        /// else is.
        #[test]
        fn anchor_boundaries_in_the_retention_window_are_held() {
            // the retention span of ANCHOR_RETENTION_INTERVALS intervals must reach below the top window for
            // a boundary to be held there
            const INTERVAL: u32 =
                2 * SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE.div_ceil(ANCHOR_RETENTION_INTERVALS);
            let policy = AnchorRetention::new(
                BlockHeight::from_u32(1),
                AnchorRetentionInterval::custom(NonZeroU32::new(INTERVAL).expect("nonzero")),
            );
            let range = HIGHEST_SCANNED + 1_000..HIGHEST_SCANNED + 1_000 + RANGE_LENGTH;
            let top_window = range.end - SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE..range.end;
            let retention_window =
                anchor_retention_window(&policy, BlockHeight::from_u32(range.end - 1));
            let boundaries = (range.start..top_window.start)
                .filter(|height| height % INTERVAL == 0)
                .filter(|height| retention_window.contains(&BlockHeight::from_u32(*height)))
                .collect::<Vec<_>>();
            assert!(!boundaries.is_empty(), "the test range holds a boundary");

            assert_eq!(
                heights(range.clone(), range.end - 1, Some(&policy)),
                boundaries.into_iter().chain(top_window).collect::<Vec<_>>()
            );
        }
    }

    /// Pinned grid boundaries survive checkpoint pruning until re-deriving the pins releases
    /// them.
    #[test]
    fn pinned_boundaries_survive_pruning_until_released() {
        const INTERVAL: u32 = 144;
        // holds the four boundaries the assertions name
        const HEIGHTS_WITH_BOUNDARIES: u32 = 600;
        const TOP: u32 = SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE + HEIGHTS_WITH_BOUNDARIES;
        // a pinned boundary inside the rolling window holds no rolling slot, so the window spans up to one
        // more height per boundary it crosses
        const OLDEST_ROLLING: u32 = TOP
            - SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE
            - SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE / INTERVAL
            - 1;

        let policy =
            AnchorRetention::new(BlockHeight::from_u32(1), AnchorRetentionInterval::default());
        let mut tree = empty_shard_tree::<
            sapling_crypto::Node,
            { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH },
            SHARD_HEIGHT,
        >();

        for height in 1..=TOP {
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
                if u32::from(*id) < OLDEST_ROLLING {
                    assert_eq!(
                        u32::from(*id) % INTERVAL,
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
        tree.append_checkpoint(BlockHeight::from_u32(TOP + 1))
            .expect("infallible");
        assert!(!checkpoint_exists(tree.store(), 144));
        assert!(!checkpoint_exists(tree.store(), 288));
        assert!(checkpoint_exists(tree.store(), 432));
        assert!(checkpoint_exists(tree.store(), 576));
    }
}
