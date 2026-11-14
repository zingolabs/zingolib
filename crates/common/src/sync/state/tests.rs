use super::*;
use zcash_protocol::local_consensus::LocalNetwork;

fn range(start: u32, end: u32) -> Range<BlockHeight> {
    BlockHeight::from_u32(start)..BlockHeight::from_u32(end)
}

fn add_orchard_roots(sync_state: &mut SyncState, start_index: u32, completing_heights: &[u32]) {
    let completing_heights = completing_heights
        .iter()
        .copied()
        .map(BlockHeight::from_u32)
        .collect::<Vec<_>>();
    sync_state.add_shard_ranges(
        &BASE_NETWORK,
        ShieldedPool::Orchard,
        start_index,
        &completing_heights,
    );
}

fn orchard_state_from_roots(completing_heights: &[u32]) -> SyncState {
    let mut sync_state = SyncState::new();
    add_orchard_roots(&mut sync_state, 0, completing_heights);
    sync_state
}

// The first range starts at the pool's activation height and every range after it starts at the
// completing block of the range before, the one block whose note commitments belong to both shards.
#[test]
fn fetched_roots_build_contiguous_ranges_from_the_activation_height() {
    let sync_state = orchard_state_from_roots(&[100, 200, 300]);

    assert_eq!(
        sync_state.orchard_shard_ranges,
        vec![range(1, 101), range(100, 201), range(200, 301)]
    );
    assert!(shard_ranges_are_contiguous(
        &sync_state.orchard_shard_ranges
    ));
}

#[test]
fn appended_roots_continue_from_the_newest_range() {
    let mut sync_state = orchard_state_from_roots(&[100, 200]);

    add_orchard_roots(&mut sync_state, 2, &[300, 400]);

    assert_eq!(
        sync_state.orchard_shard_ranges,
        vec![
            range(1, 101),
            range(100, 201),
            range(200, 301),
            range(300, 401)
        ]
    );
    assert!(shard_ranges_are_contiguous(
        &sync_state.orchard_shard_ranges
    ));
}

// The newest recorded root is fetched again every session (see `crate::sync::subtree_root_fetch_index`):
// unchanged when its completing block is unchanged, corrected when a re-org moved it, kept when the server
// serves nothing.
#[test]
fn refetched_newest_root_rebuilds_its_shard_range() {
    let mut sync_state = orchard_state_from_roots(&[100, 200]);
    let recorded = sync_state.orchard_shard_ranges.clone();

    add_orchard_roots(&mut sync_state, 1, &[200]);
    assert_eq!(sync_state.orchard_shard_ranges, recorded);

    add_orchard_roots(&mut sync_state, 1, &[205]);
    assert_eq!(
        sync_state.orchard_shard_ranges,
        vec![range(1, 101), range(100, 206)]
    );

    add_orchard_roots(&mut sync_state, 1, &[]);
    assert_eq!(
        sync_state.orchard_shard_ranges,
        vec![range(1, 101), range(100, 206)]
    );
    assert!(shard_ranges_are_contiguous(
        &sync_state.orchard_shard_ranges
    ));
}

// A shard tree rolled back below its bare roots fetches from below the recorded count (see
// `crate::sync::rollback_unscanned_tree_states`). The ranges from there are rebuilt, never duplicated.
#[test]
fn a_fetch_below_the_recorded_count_rebuilds_the_ranges_from_there() {
    let mut sync_state = orchard_state_from_roots(&[100, 200, 300]);

    add_orchard_roots(&mut sync_state, 1, &[200, 300, 400]);

    assert_eq!(
        sync_state.orchard_shard_ranges,
        vec![
            range(1, 101),
            range(100, 201),
            range(200, 301),
            range(300, 401)
        ]
    );
    assert!(shard_ranges_are_contiguous(
        &sync_state.orchard_shard_ranges
    ));
}

#[test]
fn a_root_completing_at_or_below_the_kept_ranges_is_skipped() {
    let mut sync_state = orchard_state_from_roots(&[100, 200, 300]);

    add_orchard_roots(&mut sync_state, 2, &[200, 400]);

    assert_eq!(
        sync_state.orchard_shard_ranges,
        vec![range(1, 101), range(100, 201), range(200, 401)]
    );
    assert!(shard_ranges_are_contiguous(
        &sync_state.orchard_shard_ranges
    ));
}

#[test]
fn ranges_with_a_gap_an_overlap_or_a_duplicate_are_not_contiguous() {
    assert!(shard_ranges_are_contiguous(&[]));
    assert!(shard_ranges_are_contiguous(&[range(1, 101)]));
    // a gap
    assert!(!shard_ranges_are_contiguous(&[
        range(1, 101),
        range(101, 201)
    ]));
    // an overlap of two blocks
    assert!(!shard_ranges_are_contiguous(&[
        range(1, 101),
        range(99, 201)
    ]));
    // a duplicate
    assert!(!shard_ranges_are_contiguous(&[
        range(1, 101),
        range(1, 101)
    ]));
    // a degenerate one-block range at the boundary
    assert!(!shard_ranges_are_contiguous(&[
        range(1, 101),
        range(100, 101)
    ]));
    // a range ending below the one before
    assert!(!shard_ranges_are_contiguous(&[
        range(1, 101),
        range(100, 51)
    ]));
}

mod scan_targets {
    use sapling_crypto::value::NoteValue;
    use zcash_primitives::transaction::TxId;
    use zcash_protocol::memo::Memo;

    use super::*;
    use crate::{
        status::ConfirmationStatus,
        wallet::{OutputId, WalletNote, WalletTransaction},
    };

    fn height(height: u32) -> BlockHeight {
        BlockHeight::from_u32(height)
    }

    fn txid(byte: u8) -> TxId {
        TxId::from_bytes([byte; 32])
    }

    fn scan_range(start: u32, end: u32, priority: ScanPriority) -> ScanRange {
        ScanRange::from_parts(range(start, end), priority)
    }

    fn sapling_transaction(txid: TxId, block_height: u32) -> WalletTransaction {
        let (_, recipient) = sapling_crypto::zip32::ExtendedSpendingKey::master(&[0; 32])
            .expect("a 32 byte seed derives a key")
            .default_address();
        let note = WalletNote::new_for_test(
            OutputId::new(txid, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            sapling_crypto::Note::from_parts(
                recipient,
                NoteValue::from_raw(1),
                sapling_crypto::Rseed::AfterZip212([0; 32]),
            ),
            Memo::Empty,
            None,
        );
        WalletTransaction::new_for_test(txid, ConfirmationStatus::Confirmed(height(block_height)))
            .with_sapling_notes_for_test(vec![note])
    }

    // A target whose transaction holds no known note prioritises the narrow range of blocks around it, and a
    // target whose transaction holds a note of a pool prioritises that pool's whole shard around it.
    #[test]
    fn a_target_prioritises_the_narrow_range_without_a_pool_and_the_shard_with_one() {
        let mut sync_state = sync_state_with_ranges(1, 5000);
        sync_state.orchard_shard_ranges = vec![range(1, 1501), range(1500, 3001)];
        sync_state.set_found_note_scan_ranges(
            &BASE_NETWORK,
            [ScanTarget::new(height(2200), None)].into_iter(),
        );
        assert_eq!(
            sync_state.scan_ranges(),
            [
                scan_range(1, 2000, ScanPriority::Historic),
                scan_range(2000, 3000, ScanPriority::FoundNote),
                scan_range(3000, 5001, ScanPriority::Historic),
            ]
        );

        let mut sync_state = sync_state_with_ranges(1, 5000);
        sync_state.orchard_shard_ranges = vec![range(1, 1501), range(1500, 3001)];
        sync_state.set_found_note_scan_ranges(
            &BASE_NETWORK,
            [ScanTarget::internal(height(2200), txid(1)).with_pool(ShieldedPool::Orchard)]
                .into_iter(),
        );
        assert_eq!(
            sync_state.scan_ranges(),
            [
                scan_range(1, 1500, ScanPriority::Historic),
                scan_range(1500, 3001, ScanPriority::FoundNote),
                scan_range(3001, 5001, ScanPriority::Historic),
            ]
        );
    }

    #[test]
    fn a_found_note_records_its_pool_on_the_transaction_scan_target() {
        let mut sync_state = sync_state_with_ranges(1, 5000);
        sync_state.sapling_shard_ranges = vec![range(1, 1501), range(1500, 3001)];
        let transaction = sapling_transaction(txid(7), 2200);

        for pool in [ShieldedPool::Sapling, ShieldedPool::Orchard] {
            sync_state.update_found_note_shard_priority(&BASE_NETWORK, pool, &transaction);
        }

        let targets = sync_state.scan_targets.iter().copied().collect::<Vec<_>>();
        assert_eq!(targets, [ScanTarget::internal(height(2200), txid(7))]);
        assert_eq!(
            targets[0].pools().iter().collect::<Vec<_>>(),
            [ShieldedPool::Sapling]
        );
        assert_eq!(targets[0].origin(), ScanTargetOrigin::Internal);
        assert_eq!(
            sync_state.scan_ranges(),
            [
                scan_range(1, 1500, ScanPriority::Historic),
                scan_range(1500, 3001, ScanPriority::FoundNote),
                scan_range(3001, 5001, ScanPriority::Historic),
            ]
        );
    }

    // Targets naming the same block height and txid are one target: it holds the pools of every insertion,
    // and it is internal once any insertion was.
    #[test]
    fn inserting_the_same_target_merges_its_pools_and_keeps_the_internal_origin() {
        let mut sync_state = SyncState::new();
        sync_state.insert_scan_target(ScanTarget::new(height(50), Some(txid(1))));
        sync_state.insert_scan_target(
            ScanTarget::internal(height(50), txid(1)).with_pool(ShieldedPool::Orchard),
        );
        sync_state.insert_scan_target(ScanTarget::new(height(50), Some(txid(1))));
        sync_state.insert_scan_target(
            ScanTarget::internal(height(50), txid(1)).with_pool(ShieldedPool::Ironwood),
        );
        sync_state.insert_scan_target(ScanTarget::new(height(50), None));

        let targets = sync_state.scan_targets.iter().copied().collect::<Vec<_>>();
        assert_eq!(
            targets,
            [
                ScanTarget::new(height(50), None),
                ScanTarget::new(height(50), Some(txid(1))),
            ]
        );
        assert!(targets[0].pools().is_empty());
        assert_eq!(targets[0].origin(), ScanTargetOrigin::External);
        assert_eq!(
            targets[1].pools().iter().collect::<Vec<_>>(),
            [ShieldedPool::Orchard, ShieldedPool::Ironwood]
        );
        assert_eq!(targets[1].origin(), ScanTargetOrigin::Internal);
    }

    #[test]
    fn external_targets_outside_the_chain_are_dropped_and_internal_ones_kept() {
        let mut sync_state = sync_state_with_ranges(100, 200);
        for scan_target in [
            ScanTarget::new(height(50), None),
            ScanTarget::new(height(150), None),
            ScanTarget::new(height(250), None),
            ScanTarget::internal(height(250), txid(1)),
        ] {
            sync_state.insert_scan_target(scan_target);
        }

        sync_state.drop_external_scan_targets_outside_the_chain(height(200));

        assert_eq!(
            sync_state.scan_targets.iter().copied().collect::<Vec<_>>(),
            [
                ScanTarget::new(height(150), None),
                ScanTarget::internal(height(250), txid(1)),
            ]
        );
    }

    fn settled_targets_dropped(
        orchard_shard_ranges: Vec<Range<BlockHeight>>,
        scan_targets: Vec<ScanTarget>,
        fully_scanned_height: u32,
    ) -> Vec<ScanTarget> {
        let mut sync_state = SyncState::new();
        sync_state.orchard_shard_ranges = orchard_shard_ranges;
        sync_state.sapling_shard_ranges = vec![range(1, 101)];
        for scan_target in scan_targets {
            sync_state.insert_scan_target(scan_target);
        }
        sync_state.drop_settled_scan_targets(height(fully_scanned_height));
        sync_state.scan_targets.into_iter().collect()
    }

    // A target that recorded a pool outlives the fully scanned height while the pool's shard holding it is
    // incomplete, so the rest of the shard is prioritised when it is mined. A target without a pool does not.
    #[test]
    fn a_pool_target_is_kept_until_its_shard_is_complete_and_scanned() {
        let orchard = ShieldedPool::Orchard;
        let in_the_incomplete_shard = ScanTarget::internal(height(250), txid(1)).with_pool(orchard);
        let in_a_complete_shard = ScanTarget::internal(height(150), txid(2)).with_pool(orchard);
        let without_a_pool = ScanTarget::new(height(250), None);

        assert_eq!(
            settled_targets_dropped(
                vec![range(1, 101), range(100, 201)],
                vec![in_the_incomplete_shard, in_a_complete_shard, without_a_pool],
                300,
            ),
            [in_the_incomplete_shard]
        );
        assert_eq!(
            settled_targets_dropped(
                vec![range(1, 101), range(100, 201)],
                vec![in_a_complete_shard],
                180,
            ),
            [in_a_complete_shard],
            "the shard is not scanned through its completing block"
        );
    }

    // The completing block of the newest recorded shard also holds the first commitments of the incomplete
    // shard, so a target there is kept until the next shard is recorded and scanned.
    #[test]
    fn a_pool_target_on_the_newest_completing_block_is_kept() {
        let on_the_boundary =
            ScanTarget::internal(height(200), txid(1)).with_pool(ShieldedPool::Orchard);

        assert_eq!(
            settled_targets_dropped(
                vec![range(1, 101), range(100, 201)],
                vec![on_the_boundary],
                300
            ),
            [on_the_boundary]
        );
        assert_eq!(
            settled_targets_dropped(
                vec![range(1, 101), range(100, 201), range(200, 301)],
                vec![on_the_boundary],
                300
            ),
            []
        );
        assert_eq!(
            settled_targets_dropped(
                vec![range(1, 101), range(100, 201), range(200, 301)],
                vec![on_the_boundary],
                250
            ),
            [on_the_boundary],
            "the next shard is not scanned through its completing block"
        );
    }

    #[test]
    fn a_target_recording_two_pools_is_kept_while_either_shard_is_incomplete() {
        // the orchard shard holding block 150 is complete and scanned, the sapling one is not yet recorded
        let in_both_pools = ScanTarget::internal(height(150), txid(1))
            .with_pool(ShieldedPool::Orchard)
            .with_pool(ShieldedPool::Sapling);

        assert_eq!(
            settled_targets_dropped(
                vec![range(1, 101), range(100, 201)],
                vec![in_both_pools],
                300
            ),
            [in_both_pools]
        );
    }
}

const BASE_NETWORK: LocalNetwork = LocalNetwork {
    overwinter: Some(BlockHeight::from_u32(1)),
    sapling: Some(BlockHeight::from_u32(1)),
    blossom: Some(BlockHeight::from_u32(1)),
    heartwood: Some(BlockHeight::from_u32(1)),
    canopy: Some(BlockHeight::from_u32(1)),
    nu5: Some(BlockHeight::from_u32(1)),
    nu6: Some(BlockHeight::from_u32(1)),
    nu6_1: Some(BlockHeight::from_u32(1)),
    nu6_2: Some(BlockHeight::from_u32(1)),
    nu6_3: Some(BlockHeight::from_u32(100)),
    nu7: Some(BlockHeight::from_u32(100)),
};

const NO_NU6_3_NETWORK: LocalNetwork = LocalNetwork {
    overwinter: Some(BlockHeight::from_u32(1)),
    sapling: Some(BlockHeight::from_u32(1)),
    blossom: Some(BlockHeight::from_u32(1)),
    heartwood: Some(BlockHeight::from_u32(1)),
    canopy: Some(BlockHeight::from_u32(1)),
    nu5: Some(BlockHeight::from_u32(1)),
    nu6: Some(BlockHeight::from_u32(1)),
    nu6_1: Some(BlockHeight::from_u32(1)),
    nu6_2: Some(BlockHeight::from_u32(1)),
    nu6_3: None,
    nu7: None,
};

fn sync_state_with_ranges(birthday: u32, tip: u32) -> SyncState {
    let mut s = SyncState::new();
    s.scan_ranges = vec![ScanRange::from_parts(
        BlockHeight::from_u32(birthday)..BlockHeight::from_u32(tip + 1),
        ScanPriority::Historic,
    )];
    s
}

#[test]
fn ironwood_before_nu6_3_downgrades_to_orchard() {
    let sync_state = sync_state_with_ranges(1, 200);
    let range = sync_state.determine_block_range(
        &BASE_NETWORK,
        BlockHeight::from_u32(50),
        Some(ShieldedPool::Ironwood),
    );
    assert!(
        range.contains(&BlockHeight::from_u32(50)),
        "range should cover the requested height even after downgrade"
    );
}

#[test]
fn ironwood_without_nu6_3_always_downgrades_to_orchard() {
    let sync_state = sync_state_with_ranges(1, 200);
    let range = sync_state.determine_block_range(
        &NO_NU6_3_NETWORK,
        BlockHeight::from_u32(150),
        Some(ShieldedPool::Ironwood),
    );
    assert!(range.contains(&BlockHeight::from_u32(150)));
}

#[test]
fn ironwood_at_nu6_3_activation_uses_ironwood_shard_ranges() {
    let mut sync_state = sync_state_with_ranges(1, 200);
    sync_state.ironwood_shard_ranges = vec![BlockHeight::from_u32(100)..BlockHeight::from_u32(150)];
    let range = sync_state.determine_block_range(
        &BASE_NETWORK,
        BlockHeight::from_u32(120),
        Some(ShieldedPool::Ironwood),
    );
    assert_eq!(range.start, BlockHeight::from_u32(100));
    assert_eq!(range.end, BlockHeight::from_u32(150));
}

/// Mainnet-shaped network: NU6.3 activates recently, near the chain tip.
const TIP_ACTIVATION_NETWORK: LocalNetwork = LocalNetwork {
    overwinter: Some(BlockHeight::from_u32(1)),
    sapling: Some(BlockHeight::from_u32(1)),
    blossom: Some(BlockHeight::from_u32(1)),
    heartwood: Some(BlockHeight::from_u32(1)),
    canopy: Some(BlockHeight::from_u32(1)),
    nu5: Some(BlockHeight::from_u32(1)),
    nu6: Some(BlockHeight::from_u32(1)),
    nu6_1: Some(BlockHeight::from_u32(1)),
    nu6_2: Some(BlockHeight::from_u32(1)),
    nu6_3: Some(BlockHeight::from_u32(19_000)),
    nu7: Some(BlockHeight::from_u32(19_000)),
};

#[test]
fn chain_tip_priority_confined_to_tip_when_ironwood_shards_are_empty() {
    // A wallet with a long history: birthday (1_000) far below the chain tip (20_000).
    let mut sync_state = sync_state_with_ranges(1_000, 20_000);

    // Sapling and orchard have complete shards reaching near the tip, so their
    // incomplete tip shards start close to the chain tip (17_999 and 18_499).
    sync_state.sapling_shard_ranges =
        vec![BlockHeight::from_u32(1_000)..BlockHeight::from_u32(18_000)];
    sync_state.orchard_shard_ranges =
        vec![BlockHeight::from_u32(1_000)..BlockHeight::from_u32(18_500)];
    // Ironwood shard ranges are empty: the universal state on every network
    // immediately after NU6.3 activation.
    assert!(sync_state.ironwood_shard_ranges.is_empty());

    sync_state.set_chain_tip_scan_range(&TIP_ACTIVATION_NETWORK, BlockHeight::from_u32(20_000));

    let chain_tip_start = sync_state
        .scan_ranges()
        .iter()
        .filter(|range| range.priority() == ScanPriority::ChainTip)
        .map(|range| range.block_range().start)
        .min()
        .expect("chain tip punch should mark at least one range");

    // The chain-tip region must stay confined near the tip. The lowest
    // legitimate anchor is the sapling incomplete tip shard start (17_999);
    // an ironwood fallback may reach no lower than the NU6.3 activation
    // height (19_000). It must never flood down to the wallet birthday.
    assert!(
        chain_tip_start >= BlockHeight::from_u32(17_999),
        "ChainTip priority flooded down to {chain_tip_start} (wallet birthday is 1_000); \
             expected the chain-tip region confined to the tip shards (start >= 17_999)"
    );
}

#[test]
fn truncate_scan_ranges() {
    let mut sync_state = SyncState::new();
    sync_state.scan_ranges = vec![
        ScanRange::from_parts(1.into()..99.into(), ScanPriority::Historic),
        ScanRange::from_parts(100.into()..199.into(), ScanPriority::Historic),
        ScanRange::from_parts(200.into()..299.into(), ScanPriority::Historic),
        ScanRange::from_parts(300.into()..399.into(), ScanPriority::Historic),
    ];

    sync_state.truncate_scan_ranges(250.into());

    assert_eq!(
        sync_state.scan_ranges,
        vec![
            ScanRange::from_parts(1.into()..99.into(), ScanPriority::Historic),
            ScanRange::from_parts(100.into()..199.into(), ScanPriority::Historic),
            ScanRange::from_parts(200.into()..251.into(), ScanPriority::Historic),
        ]
    );
}

/// A re-org lowers the transparent scan floor to the height the wallet was truncated to. A truncation above the
/// floor leaves it where it is, as transparent address discovery has only covered the blocks at or below it.
#[test]
fn lower_transparent_scan_floor() {
    let mut sync_state = SyncState::new();
    sync_state.transparent_scan_floor = Some(100.into());

    sync_state.lower_transparent_scan_floor(110.into());
    assert_eq!(sync_state.transparent_scan_floor, Some(100.into()));

    sync_state.lower_transparent_scan_floor(90.into());
    assert_eq!(sync_state.transparent_scan_floor, Some(90.into()));
}

/// Reopening splits the range straddling the height and returns every
/// scanned range above it to `Historic`. History below is left scanned,
/// since a pool's record below its own first commitment is not in
/// question, and a range being scanned right now is left alone because
/// its results are already in flight against the bounds it was
/// dispatched with.
#[test]
fn reopen_scan_ranges_from() {
    let mut sync_state = SyncState::new();
    sync_state.scan_ranges = vec![
        ScanRange::from_parts(1.into()..100.into(), ScanPriority::Scanned),
        ScanRange::from_parts(100.into()..200.into(), ScanPriority::Scanned),
        ScanRange::from_parts(200.into()..300.into(), ScanPriority::ScannedWithoutMapping),
        ScanRange::from_parts(300.into()..400.into(), ScanPriority::Scanning),
    ];

    sync_state.reopen_scan_ranges_inner(150.into());

    assert_eq!(
        sync_state.scan_ranges,
        vec![
            ScanRange::from_parts(1.into()..100.into(), ScanPriority::Scanned),
            ScanRange::from_parts(100.into()..150.into(), ScanPriority::Scanned),
            ScanRange::from_parts(150.into()..200.into(), ScanPriority::Historic),
            ScanRange::from_parts(200.into()..300.into(), ScanPriority::Historic),
            ScanRange::from_parts(300.into()..400.into(), ScanPriority::Scanning),
        ]
    );
}

/// A `ScannedWithoutMapping` range is not selected while a lower range is still scanning, as its re-fetched
/// nullifiers would be discarded. It is selected once it is the first unscanned range.
#[test]
fn scanned_without_mapping_range_waits_for_lower_ranges() {
    let mut sync_state = SyncState::new();
    sync_state.scan_ranges = vec![
        ScanRange::from_parts(1.into()..21.into(), ScanPriority::Scanned),
        ScanRange::from_parts(21.into()..41.into(), ScanPriority::Scanning),
        ScanRange::from_parts(41.into()..46.into(), ScanPriority::ScannedWithoutMapping),
    ];
    assert_eq!(sync_state.select_scan_range(&BASE_NETWORK, true), None);

    sync_state.scan_ranges[1] = ScanRange::from_parts(21.into()..41.into(), ScanPriority::Scanned);
    assert_eq!(
        sync_state.select_scan_range(&BASE_NETWORK, true),
        Some(ScanRange::from_parts(
            41.into()..46.into(),
            ScanPriority::ScannedWithoutMapping
        ))
    );
    assert_eq!(
        sync_state.scan_ranges[2].priority(),
        ScanPriority::RefetchingNullifiers
    );
}

/// One pure split over the wallet's scan ranges serves the stale reset and the priority punch alike.
mod split_out_overlapping {
    use super::*;

    fn blocks(start: u32, end: u32) -> Range<BlockHeight> {
        BlockHeight::from_u32(start)..BlockHeight::from_u32(end)
    }

    #[test]
    fn splits_every_range_of_the_given_priority_that_overlaps_and_passes_the_rest_through() {
        let ranges = vec![
            ScanRange::from_parts(blocks(1, 21), ScanPriority::Scanned),
            ScanRange::from_parts(blocks(21, 31), ScanPriority::Scanning),
            ScanRange::from_parts(blocks(31, 41), ScanPriority::Scanning),
            ScanRange::from_parts(blocks(41, 51), ScanPriority::Historic),
        ];

        assert_eq!(
            split_out_overlapping(
                ranges,
                &blocks(28, 45),
                ScanPriority::Scanning,
                ScanPriority::ChainTip,
            ),
            vec![
                ScanRange::from_parts(blocks(1, 21), ScanPriority::Scanned),
                ScanRange::from_parts(blocks(21, 28), ScanPriority::Scanning),
                ScanRange::from_parts(blocks(28, 31), ScanPriority::ChainTip),
                ScanRange::from_parts(blocks(31, 41), ScanPriority::ChainTip),
                ScanRange::from_parts(blocks(41, 51), ScanPriority::Historic),
            ],
        );
    }
}
