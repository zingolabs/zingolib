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

    const CHAIN_HEIGHT: u32 = 20_000;
    const WINDOW_START: u32 = CHAIN_HEIGHT - SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE + 1;
    const HIGHEST_SCANNED: u32 = 10_000;
    const INTERVAL: u32 = 50;

    fn heights(
        block_range: Range<u32>,
        highest_scanned_height: u32,
        policy: Option<&AnchorRetention>,
    ) -> Vec<u32> {
        checkpoint_heights(
            &(BlockHeight::from_u32(block_range.start)..BlockHeight::from_u32(block_range.end)),
            BlockHeight::from_u32(CHAIN_HEIGHT),
            BlockHeight::from_u32(highest_scanned_height),
            policy,
        )
        .into_iter()
        .map(u32::from)
        .collect()
    }

    fn policy() -> AnchorRetention {
        AnchorRetention::new(
            BlockHeight::from_u32(1),
            zcash_protocol::zip318::BlockSpacing::UNIFORM,
            AnchorRetentionInterval::custom(NonZeroU32::new(INTERVAL).expect("nonzero")),
        )
    }

    /// A load inside the top window of the chain holds a checkpoint on every block from the window start.
    #[test]
    fn a_load_inside_the_window_checkpoints_every_block_from_the_window_start() {
        let range = WINDOW_START - 10..WINDOW_START + 10;

        assert_eq!(
            heights(range.clone(), HIGHEST_SCANNED, None),
            (WINDOW_START..range.end).collect::<Vec<_>>()
        );
    }

    /// A load below the window holds a checkpoint on its last block and on no other, whether it lies above
    /// or below the highest scanned height.
    #[test]
    fn a_load_below_the_window_checkpoints_its_last_block_only() {
        let above = HIGHEST_SCANNED + 1_000..HIGHEST_SCANNED + 2_000;
        let below = HIGHEST_SCANNED - 2_000..HIGHEST_SCANNED - 1_000;

        assert_eq!(
            heights(above.clone(), HIGHEST_SCANNED, None),
            vec![above.end - 1]
        );
        assert_eq!(
            heights(below.clone(), HIGHEST_SCANNED, None),
            vec![below.end - 1]
        );
    }

    /// A load above the highest scanned height holds the anchor retention boundaries inside the retention
    /// window as of its last block, beside the last block.
    #[test]
    fn anchor_boundaries_as_of_the_last_block_are_held() {
        let policy = policy();
        let range = HIGHEST_SCANNED + 1_000..HIGHEST_SCANNED + 2_000;
        let retention_window =
            anchor_retention_window(&policy, BlockHeight::from_u32(range.end - 1));
        let boundaries = range
            .clone()
            .filter(|height| height % INTERVAL == 0)
            .filter(|height| retention_window.contains(&BlockHeight::from_u32(*height)))
            .collect::<Vec<_>>();
        assert!(!boundaries.is_empty(), "the test range holds a boundary");

        assert_eq!(
            heights(range.clone(), HIGHEST_SCANNED, Some(&policy)),
            boundaries
                .into_iter()
                .chain(std::iter::once(range.end - 1))
                .collect::<Vec<_>>()
        );
    }

    /// A load below the highest scanned height holds the boundaries as of that height, so a load below
    /// that retention window holds none.
    #[test]
    fn anchor_boundaries_below_the_highest_scanned_height_are_as_of_it() {
        let policy = policy();
        let retention_start = u32::from(
            *anchor_retention_window(&policy, BlockHeight::from_u32(HIGHEST_SCANNED)).start(),
        );
        let outside = HIGHEST_SCANNED - 5_000..HIGHEST_SCANNED - 4_000;
        let inside = retention_start..retention_start + 2 * INTERVAL;
        let boundaries = inside
            .clone()
            .filter(|height| height % INTERVAL == 0)
            .collect::<Vec<_>>();

        assert_eq!(
            heights(outside.clone(), HIGHEST_SCANNED, Some(&policy)),
            vec![outside.end - 1]
        );
        assert_eq!(
            heights(inside.clone(), HIGHEST_SCANNED, Some(&policy)),
            boundaries
                .into_iter()
                .chain(std::iter::once(inside.end - 1))
                .collect::<Vec<_>>()
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

    let policy = AnchorRetention::new(
        BlockHeight::from_u32(1),
        zcash_protocol::zip318::BlockSpacing::UNIFORM,
        AnchorRetentionInterval::default(),
    );
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

    let checkpoint_exists =
        |store: &shardtree::store::memory::MemoryShardStore<sapling_crypto::Node, BlockHeight>,
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

/// The window retreats by reference blocks: across the NU7 activation a
/// reference block is three blocks, so the same window reaches less far
/// below the tip, and the start is exactly the height that advances to
/// the tip.
#[test]
fn the_retention_window_retreats_by_reference_blocks_across_the_activation() {
    use zcash_protocol::zip318::BlockSpacing;
    let spacing = BlockSpacing::with_nu7_activation(BlockHeight::from_u32(1_000));
    let policy = AnchorRetention::new(
        BlockHeight::from_u32(1),
        spacing,
        AnchorRetentionInterval::default(),
    );
    let reference_blocks = 144 * ANCHOR_RETENTION_INTERVALS;
    for tip in [999u32, 1_000, 1_001, 1_300, 2_000, 5_000] {
        let tip = BlockHeight::from_u32(tip);
        let start = *anchor_retention_window(&policy, tip).start();
        assert!(spacing.advance(start, reference_blocks) <= tip, "{tip:?}");
        assert!(
            spacing.advance(start + 1, reference_blocks) > tip,
            "{tip:?}: {start:?} is not the greatest start"
        );
    }
    // Entirely above the activation the window is three times as long in blocks.
    let start = *anchor_retention_window(&policy, BlockHeight::from_u32(5_000)).start();
    assert_eq!(start, BlockHeight::from_u32(5_000 - 3 * reference_blocks));
    // Entirely below it the window is the uniform one.
    let start = *anchor_retention_window(&policy, BlockHeight::from_u32(999)).start();
    assert_eq!(start, BlockHeight::from_u32(999 - reference_blocks));
}

/// The checkpoint set of a synced wallet decomposes into exactly two parts:
/// [`SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE`] rolling checkpoints, which serve ordinary reorg handling and
/// near-tip spends, plus [`ANCHOR_RETENTION_INTERVALS`] pinned ZIP 318 grid boundaries,
/// which serve pool crossings.
///
/// The two parts are disjoint and independently bounded: pinning a boundary must not
/// consume a rolling slot (that would shrink the reorg window), and the rolling budget must
/// not displace a boundary (that would break crossings).
#[cfg(feature = "wallet_essentials")]
#[test]
fn checkpoint_set_is_reorg_window_plus_pinned_boundaries() {
    use crate::shardtree_ext::ShardTreeExt as _;
    use crate::sync::{MAX_SHARDTREE_CHECKPOINTS, SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE};
    use crate::wallet::ShardTrees;
    use crate::witness::{anchor_retention_window, repin_anchor_checkpoints};
    use zcash_client_backend::data_api::anchor_retention::{
        AnchorRetention, AnchorRetentionInterval,
    };

    const TIP: u32 = 100_000;
    const INTERVAL: u32 = 144;
    let policy = AnchorRetention::new(
        BlockHeight::from_u32(90_000),
        zcash_protocol::zip318::BlockSpacing::UNIFORM,
        AnchorRetentionInterval::default(),
    );
    let mut shard_trees = ShardTrees::new();

    for height in (TIP - 2000)..=TIP {
        let height = BlockHeight::from_u32(height);
        let window = anchor_retention_window(&policy, height);
        repin_anchor_checkpoints(&policy, &window, shard_trees.orchard.store_mut());
        shard_trees
            .orchard
            .append_checkpoint(height)
            .expect("infallible");
    }

    let store = shard_trees.orchard.store();
    let total = store.checkpoint_count().expect("infallible");
    let pinned_ids = store.retained_checkpoints().expect("infallible");
    let mut pinned = Vec::new();
    let mut rolling = Vec::new();
    store
        .for_each_checkpoint(total, |id, _| {
            if pinned_ids.contains(id) {
                pinned.push(u32::from(*id));
            } else {
                rolling.push(u32::from(*id));
            }
            Ok(())
        })
        .expect("infallible");

    assert!(
        pinned.iter().all(|height| height % INTERVAL == 0),
        "every pinned checkpoint must be a grid boundary, got {pinned:?}"
    );
    assert_eq!(rolling.last().copied(), Some(TIP));
    assert_eq!(
        (rolling.len(), pinned.len(), total),
        (
            SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE as usize,
            ANCHOR_RETENTION_INTERVALS as usize,
            MAX_SHARDTREE_CHECKPOINTS as usize,
        ),
        "(rolling, pinned, total): the pinned boundaries must not be part of the \
             SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE total"
    );

    for height in (TIP - SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE + 1)..=TIP {
        assert!(
            store
                .get_checkpoint(&BlockHeight::from_u32(height))
                .expect("infallible")
                .is_some(),
            "reorg window is missing height {height}"
        );
    }

    // a pinned boundary inside the rolling window holds no rolling slot, so the rolling set reaches at
    // most one height per boundary below the window
    assert!(
        rolling
            .iter()
            .all(|height| *height >= TIP - MAX_SHARDTREE_CHECKPOINTS),
        "a rolling checkpoint survived below the reorg window: {rolling:?}"
    );

    let mut bytes = Vec::new();
    shard_trees.write(&mut bytes).expect("write should succeed");
    let reloaded = ShardTrees::read(bytes.as_slice()).expect("read should succeed");
    let reloaded_store = reloaded.orchard.store();
    for boundary in &pinned {
        assert!(
            reloaded_store
                .get_checkpoint(&BlockHeight::from_u32(*boundary))
                .expect("infallible")
                .is_some(),
            "boundary {boundary} lost on reload; a crossing anchored there cannot be built"
        );
    }
    assert_eq!(
        reloaded_store.checkpoint_count().expect("infallible"),
        MAX_SHARDTREE_CHECKPOINTS as usize
    );
}
