use super::*;

// checkpoints appended past the rolling window, so pruning has run before a round trip
const APPENDED: u32 = SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE + 49;

fn written(scan_target: &ScanTarget) -> Vec<u8> {
    let mut bytes = Vec::new();
    scan_target
        .write(&mut bytes)
        .expect("writing to a vec cannot fail");
    bytes
}

#[test]
fn scan_target_round_trips_with_its_pools_and_origin() {
    let txid = TxId::from_bytes([3; 32]);
    for scan_target in [
        ScanTarget::new(BlockHeight::from_u32(10), None),
        ScanTarget::new(BlockHeight::from_u32(10), Some(txid)),
        ScanTarget::internal(BlockHeight::from_u32(20), txid)
            .with_pool(zcash_protocol::ShieldedPool::Sapling)
            .with_pool(zcash_protocol::ShieldedPool::Ironwood),
    ] {
        let read = ScanTarget::read(written(&scan_target).as_slice())
            .expect("a written scan target reads back");
        assert_eq!(read, scan_target);
        assert_eq!(read.txid(), scan_target.txid());
        assert_eq!(read.pools(), scan_target.pools());
        assert_eq!(read.origin(), scan_target.origin());
    }
}

// A version 0 target carried a zero txid where no particular transaction was wanted, and a narrow scan area
// flag. It is read as an external target with no pool.
#[test]
fn scan_target_version_zero_is_read_as_external() {
    let v0 = |txid: [u8; 32]| {
        let mut bytes = vec![0u8];
        bytes
            .write_u32::<LittleEndian>(30)
            .expect("writing to a vec cannot fail");
        bytes.extend_from_slice(&txid);
        bytes.push(1);
        bytes
    };

    let without_transaction =
        ScanTarget::read(v0([0; 32]).as_slice()).expect("a version 0 scan target reads");
    assert_eq!(
        without_transaction,
        ScanTarget::new(BlockHeight::from_u32(30), None)
    );
    assert_eq!(without_transaction.txid(), None);
    assert_eq!(without_transaction.origin(), ScanTargetOrigin::External);
    assert!(without_transaction.pools().is_empty());

    let with_transaction =
        ScanTarget::read(v0([5; 32]).as_slice()).expect("a version 0 scan target reads");
    assert_eq!(with_transaction.txid(), Some(TxId::from_bytes([5; 32])));
}

#[test]
fn scan_target_with_an_unknown_pool_or_origin_is_refused() {
    let mut unknown_pool = written(&ScanTarget::new(BlockHeight::from_u32(10), None));
    *unknown_pool
        .last_mut()
        .expect("a written scan target is non-empty") = 0;
    let pools_index = unknown_pool.len() - 2;
    unknown_pool[pools_index] = 1 << 3;
    assert_eq!(
        ScanTarget::read(unknown_pool.as_slice())
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidData
    );

    let mut unknown_origin = written(&ScanTarget::new(BlockHeight::from_u32(10), None));
    *unknown_origin
        .last_mut()
        .expect("a written scan target is non-empty") = 2;
    assert_eq!(
        ScanTarget::read(unknown_origin.as_slice())
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidData
    );
}

mod read_version {
    use super::super::read_version;

    /// The current version of a made-up type. It follows no type's serialized version.
    const ARBITRARY_CURRENT_VERSION: u8 = 2;

    #[test]
    fn accepts_versions_up_to_the_current_version() {
        for version in [0, ARBITRARY_CURRENT_VERSION] {
            assert_eq!(
                read_version([version].as_slice(), "Type", ARBITRARY_CURRENT_VERSION)
                    .expect("a version up to the current version is accepted"),
                version
            );
        }
    }

    #[test]
    fn refuses_a_version_above_the_current_version() {
        let error = read_version(
            [ARBITRARY_CURRENT_VERSION + 1].as_slice(),
            "Type",
            ARBITRARY_CURRENT_VERSION,
        )
        .expect_err("a version above the current version is refused");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}

/// Each reader is given only a serialized version above the one its type writes. A reader that refuses the
/// version returns invalid data. A reader that read on would report the end of the input.
#[test]
fn readers_refuse_serialized_versions_above_their_own() {
    let consensus_parameters = zcash_protocol::consensus::MAIN_NETWORK;
    let newer_scan_target = [ScanTarget::serialized_version() + 1];
    let newer_sync_state = [SyncState::serialized_version() + 1];
    let newer_tree_bounds = [TreeBounds::serialized_version() + 1];
    let newer_nullifier_map = [NullifierMap::serialized_version() + 1];
    let newer_wallet_block = [WalletBlock::serialized_version() + 1];
    let newer_wallet_transaction = [WalletTransaction::serialized_version() + 1];
    let newer_transparent_coin = [TransparentCoin::serialized_version() + 1];
    let newer_wallet_note = [SaplingNote::serialized_version() + 1];
    let newer_outgoing_note = [OutgoingSaplingNote::serialized_version() + 1];
    let newer_shard_trees = [ShardTrees::serialized_version() + 1];

    for (type_name, read) in [
        (
            "ScanTarget",
            ScanTarget::read(newer_scan_target.as_slice()).map(drop),
        ),
        (
            "SyncState",
            SyncState::read(newer_sync_state.as_slice()).map(drop),
        ),
        (
            "TreeBounds",
            TreeBounds::read(newer_tree_bounds.as_slice()).map(drop),
        ),
        (
            "NullifierMap",
            NullifierMap::read(newer_nullifier_map.as_slice()).map(drop),
        ),
        (
            "WalletBlock",
            WalletBlock::read(newer_wallet_block.as_slice()).map(drop),
        ),
        (
            "WalletTransaction",
            WalletTransaction::read(newer_wallet_transaction.as_slice(), &consensus_parameters)
                .map(drop),
        ),
        (
            "TransparentCoin",
            TransparentCoin::read(newer_transparent_coin.as_slice()).map(drop),
        ),
        (
            "SaplingNote",
            SaplingNote::read(newer_wallet_note.as_slice()).map(drop),
        ),
        (
            "OrchardNote",
            OrchardNote::read(newer_wallet_note.as_slice()).map(drop),
        ),
        (
            "IronwoodNote",
            IronwoodNote::read(newer_wallet_note.as_slice()).map(drop),
        ),
        (
            "OutgoingSaplingNote",
            OutgoingSaplingNote::read(newer_outgoing_note.as_slice(), &consensus_parameters)
                .map(drop),
        ),
        (
            "OutgoingOrchardNote",
            OutgoingOrchardNote::read(newer_outgoing_note.as_slice(), &consensus_parameters)
                .map(drop),
        ),
        (
            "OutgoingIronwoodNote",
            OutgoingIronwoodNote::read(newer_outgoing_note.as_slice(), &consensus_parameters)
                .map(drop),
        ),
        (
            "ShardTrees",
            ShardTrees::read(newer_shard_trees.as_slice()).map(drop),
        ),
    ] {
        assert_eq!(
            read.expect_err(type_name).kind(),
            std::io::ErrorKind::InvalidData,
            "{type_name}"
        );
    }
}

// Helper: build a minimal v1 NullifierMap byte blob (no ironwood BTreeMap).
fn v1_nullifier_map_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    out.write_u8(1).expect("writing to a vec cannot fail");
    Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).expect("writing to a vec cannot fail");
    Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).expect("writing to a vec cannot fail");
    out
}

#[test]
fn nullifier_map_v1_reads_with_empty_ironwood_map() {
    let bytes = v1_nullifier_map_bytes();
    let map = NullifierMap::read(bytes.as_slice()).expect("v1 should read cleanly");
    assert!(map.ironwood.is_empty());
}

#[test]
fn nullifier_map_v2_roundtrip_preserves_ironwood() {
    let map = NullifierMap::new();
    let mut bytes = Vec::new();
    map.write(&mut bytes).expect("write should succeed");
    let recovered = NullifierMap::read(bytes.as_slice()).expect("read should succeed");
    assert!(recovered.ironwood.is_empty());
}

// Helper: build a v0 TreeBounds byte blob (no ironwood tree sizes).
fn v0_tree_bounds_bytes(
    sapling_initial: u32,
    sapling_final: u32,
    orchard_initial: u32,
    orchard_final: u32,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.write_u8(0).expect("writing to a vec cannot fail");
    out.write_u32::<LittleEndian>(sapling_initial)
        .expect("writing to a vec cannot fail");
    out.write_u32::<LittleEndian>(sapling_final)
        .expect("writing to a vec cannot fail");
    out.write_u32::<LittleEndian>(orchard_initial)
        .expect("writing to a vec cannot fail");
    out.write_u32::<LittleEndian>(orchard_final)
        .expect("writing to a vec cannot fail");
    out
}

#[test]
fn tree_bounds_v0_reads_with_zero_ironwood_sizes() {
    let bytes = v0_tree_bounds_bytes(10, 20, 30, 40);
    let bounds = TreeBounds::read(bytes.as_slice()).expect("v0 should read cleanly");
    assert_eq!(bounds.sapling_initial_tree_size, 10);
    assert_eq!(bounds.orchard_final_tree_size, 40);
    assert_eq!(bounds.ironwood_initial_tree_size, 0);
    assert_eq!(bounds.ironwood_final_tree_size, 0);
}

#[test]
fn tree_bounds_v1_roundtrip() {
    let bounds = TreeBounds {
        sapling_initial_tree_size: 1,
        sapling_final_tree_size: 2,
        orchard_initial_tree_size: 3,
        orchard_final_tree_size: 4,
        ironwood_initial_tree_size: 5,
        ironwood_final_tree_size: 6,
    };
    let mut bytes = Vec::new();
    bounds.write(&mut bytes).expect("write should succeed");
    let recovered = TreeBounds::read(bytes.as_slice()).expect("read should succeed");
    assert_eq!(recovered.ironwood_initial_tree_size, 5);
    assert_eq!(recovered.ironwood_final_tree_size, 6);
}

/// Serialization preserves the checkpoints pruning left instead of imposing a cap.
#[test]
fn shardtree_roundtrip_keeps_newest_checkpoints() {
    use crate::shardtree_ext::ShardTreeExt as _;

    let mut shard_trees = ShardTrees::new();

    for height in 1..=APPENDED {
        let height = BlockHeight::from_u32(height);
        shard_trees
            .sapling
            .append_checkpoint(height)
            .expect("infallible");
        shard_trees
            .orchard
            .append_checkpoint(height)
            .expect("infallible");
    }

    let mut bytes = Vec::new();
    shard_trees.write(&mut bytes).expect("write should succeed");
    let roundtripped = ShardTrees::read(bytes.as_slice()).expect("read should succeed");

    let sapling_store = roundtripped.sapling.store();
    let orchard_store = roundtripped.orchard.store();

    let oldest_kept =
        BlockHeight::from_u32(APPENDED - SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE + 1);
    fn assert_window<S>(store: &S, oldest_kept: BlockHeight)
    where
        S: ShardStore<CheckpointId = BlockHeight, Error = std::convert::Infallible>,
    {
        assert_eq!(
            store.checkpoint_count().expect("infallible"),
            SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE as usize
        );
        assert_eq!(
            store.min_checkpoint_id().expect("infallible"),
            Some(oldest_kept)
        );
        assert_eq!(
            store.max_checkpoint_id().expect("infallible"),
            Some(BlockHeight::from_u32(APPENDED))
        );
        assert!(
            store
                .get_checkpoint(&(oldest_kept - 1))
                .expect("infallible")
                .is_none()
        );
    }
    assert_window(sapling_store, oldest_kept);
    assert_window(orchard_store, oldest_kept);
}

/// A blob with no checkpoint reads back with the height-zero checkpoint.
#[test]
fn shardtree_read_adds_initialization_checkpoint_when_blob_has_none() {
    fn checkpoint_less<H, const DEPTH: u8, const SHARD_HEIGHT: u8>()
    -> ShardTree<MemoryShardStore<H, BlockHeight>, DEPTH, SHARD_HEIGHT>
    where
        H: Hashable + Clone + PartialEq,
    {
        ShardTree::new(
            MemoryShardStore::empty(),
            SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE as usize,
        )
    }

    let mut shard_trees = ShardTrees {
        sapling: checkpoint_less(),
        orchard: checkpoint_less(),
        ironwood: checkpoint_less(),
    };
    assert_eq!(
        shard_trees
            .sapling
            .store()
            .max_checkpoint_id()
            .expect("infallible"),
        None
    );

    let mut bytes = Vec::new();
    shard_trees.write(&mut bytes).expect("write should succeed");
    let roundtripped = ShardTrees::read(bytes.as_slice()).expect("read should succeed");

    fn assert_initialization_checkpoint<S>(store: &S)
    where
        S: ShardStore<CheckpointId = BlockHeight, Error = std::convert::Infallible>,
    {
        assert_eq!(store.checkpoint_count().expect("infallible"), 1);
        assert_eq!(
            store.max_checkpoint_id().expect("infallible"),
            Some(BlockHeight::from_u32(0))
        );
    }
    assert_initialization_checkpoint(roundtripped.sapling.store());
    assert_initialization_checkpoint(roundtripped.orchard.store());
    assert_initialization_checkpoint(roundtripped.ironwood.store());
}

/// A pinned anchor checkpoint survives serialization even once it has aged out of the
/// rolling window. The pinned set itself is not persisted, being re-derived at the start of
/// every sync.
#[test]
fn shardtree_roundtrip_keeps_pinned_anchor_checkpoints() {
    use crate::shardtree_ext::ShardTreeExt as _;

    let pinned = BlockHeight::from_u32(24);
    let mut shard_trees = ShardTrees::new();

    shard_trees
        .sapling
        .store_mut()
        .add_retained_checkpoint(pinned)
        .expect("infallible");
    for height in 1..=APPENDED {
        shard_trees
            .sapling
            .append_checkpoint(BlockHeight::from_u32(height))
            .expect("infallible");
    }

    assert!(
        shard_trees
            .sapling
            .store()
            .get_checkpoint(&pinned)
            .expect("infallible")
            .is_some(),
        "pruning must not evict a pinned checkpoint"
    );

    let mut bytes = Vec::new();
    shard_trees.write(&mut bytes).expect("write should succeed");
    let roundtripped = ShardTrees::read(bytes.as_slice()).expect("read should succeed");

    let sapling_store = roundtripped.sapling.store();
    assert!(
        sapling_store
            .get_checkpoint(&pinned)
            .expect("infallible")
            .is_some(),
        "serialization must not evict a pinned checkpoint"
    );
    assert_eq!(
        sapling_store.checkpoint_count().expect("infallible"),
        SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE as usize + 1
    );
    assert!(
        sapling_store
            .retained_checkpoints()
            .expect("infallible")
            .is_empty()
    );
}

// Helper: build a minimal v3 SyncState byte blob (no ironwood_shard_ranges).
// Format: version(1) | scan_ranges[0] | sapling_shard_ranges[0] |
//         orchard_shard_ranges[0] | scan_targets[0]
fn v3_sync_state_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    out.write_u8(3).expect("writing to a vec cannot fail");
    Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).expect("writing to a vec cannot fail");
    Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).expect("writing to a vec cannot fail");
    Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).expect("writing to a vec cannot fail");
    Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).expect("writing to a vec cannot fail");
    out
}

#[test]
fn sync_state_v3_reads_with_empty_ironwood_ranges() {
    let bytes = v3_sync_state_bytes();
    let sync_state = SyncState::read(bytes.as_slice()).expect("v3 should read cleanly");
    assert!(sync_state.ironwood_shard_ranges().is_empty());
}

#[test]
fn sync_state_v4_roundtrip_preserves_ironwood_ranges() {
    let mut state = SyncState::new();
    state.ironwood_shard_ranges = vec![
        BlockHeight::from_u32(100)..BlockHeight::from_u32(200),
        BlockHeight::from_u32(300)..BlockHeight::from_u32(400),
    ];
    state.scan_ranges.push(ScanRange::from_parts(
        BlockHeight::from_u32(100)..BlockHeight::from_u32(400),
        ScanPriority::Historic,
    ));
    let mut bytes = Vec::new();
    state.write(&mut bytes).expect("write should succeed");
    let recovered = SyncState::read(bytes.as_slice()).expect("read should succeed");
    assert_eq!(recovered.ironwood_shard_ranges, state.ironwood_shard_ranges);
    assert_eq!(recovered.scan_ranges, state.scan_ranges);
}

// Helper: build a minimal v4 SyncState byte blob (no transparent scan floor).
// Format: version(1) | scan_ranges[0] | sapling_shard_ranges[0] |
//         orchard_shard_ranges[0] | ironwood_shard_ranges[0] | scan_targets[0]
fn v4_sync_state_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    out.write_u8(4).expect("writing to a vec cannot fail");
    Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).expect("writing to a vec cannot fail");
    Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).expect("writing to a vec cannot fail");
    Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).expect("writing to a vec cannot fail");
    Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).expect("writing to a vec cannot fail");
    Vector::write(&mut out, &[] as &[()], |_, _| Ok(())).expect("writing to a vec cannot fail");
    out
}

#[test]
fn sync_state_v4_reads_with_unset_transparent_scan_floor() {
    let bytes = v4_sync_state_bytes();
    let sync_state = SyncState::read(bytes.as_slice()).expect("v4 should read cleanly");
    assert_eq!(sync_state.transparent_scan_floor, None);
}

#[test]
fn sync_state_v5_roundtrip_preserves_transparent_scan_floor() {
    for transparent_scan_floor in [None, Some(BlockHeight::from_u32(100))] {
        let mut state = SyncState::new();
        state.transparent_scan_floor = transparent_scan_floor;
        let mut bytes = Vec::new();
        state.write(&mut bytes).expect("write should succeed");
        let recovered = SyncState::read(bytes.as_slice()).expect("read should succeed");
        assert_eq!(recovered.transparent_scan_floor, transparent_scan_floor);
    }
}
