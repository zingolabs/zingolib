use super::*;
use crate::config::ChainType;
use crate::wallet::migration::parts::BoundNote;
use pepper_sync::wallet::OutputId;
use proptest::prelude::*;
use zcash_pool_migration::scheduling::{ANCHOR_AGE_CAP, EXPIRY_MODULUS, EXPIRY_WINDOW};
use zcash_primitives::transaction::TxId;

fn params() -> MigrationParams {
    MigrationParams::provisional(ChainType::Mainnet)
}

/// A chain on which NU7 never activates: the grid is uniform at every
/// height, one block per reference block, and the target spacing is 75
/// seconds throughout.
fn uniform_chain() -> ChainType {
    ChainType::Regtest(crate::testutils::mock_activation_heights_with(|era| {
        era.set_nu7(None)
    }))
}

/// The uniform grid at the provisional bucket modulus, for tests whose
/// subject is the schedule arithmetic rather than the NU7 scaling.
fn grid() -> BucketGrid {
    params().grid(uniform_chain())
}

/// The weakest activation floor available for tests whose subject is the
/// schedule arithmetic itself. Not fully inert: even a height-zero
/// activation puts the lowest legal anchor at bucket one, so the earliest
/// window is bucket two. Every test using it schedules far above that.
fn no_floor() -> PoolActivation {
    PoolActivation::new_for_test(BlockHeight::from_u32(0))
}

/// The matching weakest anchorability floor: notes confirmed at height
/// zero exist under every boundary.
fn old_notes(_part: &PartRecord) -> Option<BlockHeight> {
    Some(BlockHeight::from_u32(0))
}

/// The [`AnchorFloor`] matching [`no_floor`] and [`old_notes`], for the
/// placement operations, which take one part's floor rather than a
/// lookup.
fn weakest_floor() -> AnchorFloor {
    AnchorFloor::new(no_floor(), Some(BlockHeight::from_u32(0)))
}

fn bound_part(id: u32, denomination: u64) -> PartRecord {
    PartRecord::new(
        PartId(id),
        denomination,
        BoundNote {
            output_id: OutputId::new(TxId::from_bytes([id as u8; 32]), id),
            nullifier: [0; 32],
            commitment: [0; 32],
        },
    )
}

fn parts_with_denominations(denominations: &[u64]) -> Vec<PartRecord> {
    denominations
        .iter()
        .enumerate()
        .map(|(id, &denomination)| bound_part(id as u32, denomination))
        .collect()
}

/// Issue #2493, finding 6: consent given before the NU6.3 activation
/// must not schedule any part whose *anchor* predates the activation.
/// The window such an anchor belongs to would commit to a pre-NU6.3
/// consensus branch, in which no Ironwood bundle exists, so every
/// transmission attempt skips and the whole consented batch slides into the
/// correlation-disclosed catch-up path. Note splitting is explicitly
/// permitted before activation (module doc), so a fully split wallet at a
/// pre-activation consent height is a supported state.
#[test]
fn schedule_respects_the_era_floor() {
    let params = params();
    let mut parts = parts_with_denominations(&[1_000_000, 2_000_000]);
    // Consent at height 10; the activation lies far above it.
    let now = BlockHeight::from_u32(10);
    let activation = BlockHeight::from_u32(1_000);

    plan_schedule(
        &mut parts,
        now,
        PoolActivation::new_for_test(activation),
        old_notes,
        &params,
        &grid(),
        &mut crate::utils::system_rng(),
    )
    .unwrap();

    for part in &parts {
        let anchor = grid().boundary_of(part.anchor_bucket.unwrap());
        assert!(
            anchor > activation,
            "part {:?} anchors at {anchor:?}, not strictly above the \
                 NU6.3 activation {activation:?}",
            part.id,
        );
        // And the window is strictly above the anchor, so it clears the
        // activation without the floor being restated on it.
        assert!(
            part.bucket_index.unwrap() > part.anchor_bucket.unwrap(),
            "part {:?} must transmit above the bucket it anchors in",
            part.id,
        );
    }
}

/// Every accepted anchor sits inside the candidate set: at least one
/// bucket below the window, at or above both floors, and within the age
/// cap. The first bound is the age-never-zero rule: a part must never
/// prove against the boundary of the window it is transmitting in,
/// because that boundary is the newest tree state there is and its
/// cohort has not accumulated yet. The delegated `draw_anchor_boundary`
/// enforces every one of these bounds now that the local rejection loop
/// is retired; this test pins that contract from the caller's side.
#[test]
fn drawn_anchors_land_inside_the_candidate_set() {
    let activation = PoolActivation::new_for_test(BlockHeight::from_u32(1_000));
    let note = BlockHeight::from_u32(2_000);
    let floor = AnchorFloor::new(activation, Some(note));
    let lowest = floor.lowest_anchor_bucket(&grid());
    let window = lowest + 40;

    for _ in 0..2_000 {
        let anchor = draw_anchor_bucket(window, &floor, &mut crate::utils::system_rng(), &grid())
            .expect("the set is non-empty forty buckets above the floor");
        assert!(anchor < window, "the anchor is below the open window");
        assert!(anchor >= lowest, "the anchor clears both floors");
        assert!(
            window - anchor <= u64::from(ANCHOR_AGE_CAP),
            "the age {} exceeds the cap",
            window - anchor
        );
        let boundary = grid().boundary_of(anchor);
        assert!(boundary > activation.height(), "anchor above activation");
        assert!(boundary >= note, "anchor at or above its own note");
    }
}

/// A window with no legal anchor below it is refused, not silently given
/// an age-zero anchor. This is the state a hand-computed window can reach
/// and `first_permitted_bucket` cannot.
#[test]
fn a_window_with_no_legal_anchor_is_refused() {
    let floor = AnchorFloor::new(
        PoolActivation::new_for_test(BlockHeight::from_u32(0)),
        Some(BlockHeight::from_u32(0)),
    );
    let lowest = floor.lowest_anchor_bucket(&grid());

    assert!(
        draw_anchor_bucket(lowest, &floor, &mut crate::utils::system_rng(), &grid()).is_none(),
        "the lowest legal anchor cannot also be the window"
    );
    assert_eq!(
        floor.earliest_window(&grid()),
        lowest + 1,
        "the earliest window is one bucket above the lowest anchor"
    );

    let mut part = bound_part(0, 1_000_000);
    assert!(matches!(
        place(
            &mut part,
            lowest,
            &floor,
            &mut crate::utils::system_rng(),
            &grid()
        ),
        Err(WalletError::MigrationNoLegalAnchor { .. })
    ));
}

/// Issue #2493, finding 7 (ratified form): every move of a part
/// between buckets goes through a placement operation, and the
/// jittered one draws a fresh random target inside the new bucket's
/// window. A stale target from the old bucket (or a cleared-to-None
/// target treated as immediately due) would fire the rebuilt part at
/// its window's first block: the boundary clustering the jitter
/// exists to prevent, correlated across every wallet that rebuilds
/// after an expiry.
#[test]
fn place_draws_a_fresh_target_from_the_new_boundary() {
    let mut part = bound_part(0, 1_000_000);
    part.assign(1).unwrap();
    // The schedule's randomized target inside bucket 1's window.
    let old_target = grid().boundary_of(1) + 17;
    part.target_height = Some(old_target);
    part.mark_signed(TxId::from_bytes([9; 32]), old_target + 40, None)
        .unwrap();
    part.mark_expired().unwrap();

    place(
        &mut part,
        5,
        &weakest_floor(),
        &mut crate::utils::system_rng(),
        &grid(),
    )
    .unwrap();

    assert_eq!(part.state, PartState::Assigned);
    let boundary = u32::from(grid().boundary_of(5));
    let target = u32::from(
        part.target_height
            .expect("jittered placement draws a fresh target"),
    );
    let cap = SchedulingParams::ZIP_318.transfer_delay().cap().get();
    assert!(
        target >= boundary && target <= boundary + cap,
        "the fresh target {target} must be drawn from bucket 5's boundary \
             under the canonical delay law [{boundary}, {}]",
        boundary + cap,
    );
}

#[test]
fn schedule_fills_consecutive_buckets_largest_first() {
    let params = params();
    // 13 parts, target 6 sessions → k = 3, so 5 buckets: 3+3+3+3+1.
    let denominations: Vec<u64> = (1..=13).map(|i| i * 1_000_000).collect();
    let mut parts = parts_with_denominations(&denominations);
    let now = BlockHeight::from_u32(10_000);
    plan_schedule(
        &mut parts,
        now,
        no_floor(),
        old_notes,
        &params,
        &grid(),
        &mut crate::utils::system_rng(),
    )
    .unwrap();

    // The first batch opens in the current bucket, not the next.
    let first_bucket = grid().index_of(now);
    for part in &parts {
        assert_eq!(part.state, PartState::Assigned);
        let bucket = part.bucket_index.unwrap();
        assert!(bucket >= first_bucket, "current or future bucket");
        // Target is drawn from the part's boundary under the canonical
        // delay law (mean 66, capped at 576; it may pass the window,
        // and the target is advisory per ADR 0017).
        let boundary = u32::from(grid().boundary_of(bucket));
        let target = u32::from(part.target_height.unwrap());
        let cap = SchedulingParams::ZIP_318.transfer_delay().cap().get();
        assert!(target >= boundary && target <= boundary + cap);
    }
    // Largest denominations land in the earliest buckets.
    for a in &parts {
        for b in &parts {
            if a.denomination > b.denomination {
                assert!(a.bucket_index.unwrap() <= b.bucket_index.unwrap());
            }
        }
    }
    let distinct: std::collections::BTreeSet<u64> =
        parts.iter().filter_map(|p| p.bucket_index).collect();
    assert_eq!(distinct.len(), 5);
}

/// The timeline reports every bucket holding a part, earliest first,
/// with per-window confirmation tallies and the current-window marker.
/// Unassigned parts have no window and are not listed.
#[test]
fn window_timeline_tallies_windows_past_and_future() {
    let params = params();
    let mut done = bound_part(0, 2_000_000);
    done.assign(3).unwrap();
    done.mark_confirmed(BlockHeight::from_u32(3 * params.bucket_modulus + 5))
        .unwrap();
    let mut pending = bound_part(1, 1_000_000);
    pending.assign(3).unwrap();
    let mut ahead = bound_part(2, 5_000_000);
    ahead.assign(5).unwrap();
    let unassigned = bound_part(3, 1_000_000);

    let now = grid().boundary_of(3) + 10;
    let timeline = window_timeline(&[done, pending, ahead, unassigned], now, &grid());

    assert_eq!(
        timeline.len(),
        2,
        "assigned buckets plus the current window, which bucket 3 already is"
    );
    let current = &timeline[0];
    assert_eq!(current.bucket_index, 3);
    assert_eq!(current.boundary, grid().boundary_of(3));
    assert_eq!(current.close, grid().boundary_of(4));
    assert!(current.is_current);
    assert_eq!((current.parts_confirmed, current.parts_total), (1, 2));
    assert_eq!(current.value_migrated, 2_000_000);
    assert_eq!(current.value_total, 3_000_000);

    let future = &timeline[1];
    assert_eq!(future.bucket_index, 5);
    assert!(!future.is_current);
    assert_eq!((future.parts_confirmed, future.parts_total), (0, 1));
    assert_eq!(future.value_migrated, 0);
}

/// The calendar exists before any schedule: with no parts at all the
/// timeline still reports the window the tip is inside, zero tallies.
/// With parts elsewhere, the empty current window is still listed.
#[test]
fn window_timeline_always_reports_the_current_window() {
    let now = grid().boundary_of(7) + 100;

    let bare = window_timeline(&[], now, &grid());
    assert_eq!(bare.len(), 1);
    assert!(bare[0].is_current);
    assert_eq!(bare[0].bucket_index, 7);
    assert_eq!(bare[0].boundary, grid().boundary_of(7));
    assert_eq!(bare[0].close, grid().boundary_of(8));
    assert_eq!((bare[0].parts_total, bare[0].value_total), (0, 0));

    let mut ahead = bound_part(0, 1_000_000);
    ahead.assign(9).unwrap();
    let timeline = window_timeline(&[ahead], now, &grid());
    assert_eq!(timeline.len(), 2);
    assert!(timeline[0].is_current);
    assert_eq!(timeline[0].parts_total, 0);
    assert_eq!(timeline[1].bucket_index, 9);
}

/// The random target no longer gates sendability: a current-bucket part
/// awaiting transmission is due for the whole open window, the exact case the
/// old target-gated predicate rejected. Bucket membership plus
/// awaiting-transmission state is the whole rule.
#[test]
fn part_in_current_bucket_ignores_the_target_height() {
    let current_bucket = 40;
    let mut part = bound_part(0, 1_000_000);
    part.assign(current_bucket).unwrap();
    // A target high in the window, above where an early-window tip sits.
    part.target_height = Some(grid().boundary_of(current_bucket) + 100);

    assert!(
        part_in_current_bucket(&part, current_bucket),
        "Assigned with its target still ahead is due"
    );
    assert!(
        !part_in_current_bucket(&part, current_bucket + 1),
        "a part of another bucket is not due"
    );

    part.mark_confirmed(BlockHeight::from_u32(20_000)).unwrap();
    assert!(
        !part_in_current_bucket(&part, current_bucket),
        "a confirmed part is not due"
    );
}

/// The first batch is placed in the bucket the chain is currently in, so
/// its window is already open and the batch is immediately sendable, which
/// the anchor age permits whenever the notes have been settled a bucket.
#[test]
fn first_batch_opens_in_the_current_bucket_when_the_notes_are_settled() {
    let params = params();
    let mut parts = parts_with_denominations(&[3_000_000, 2_000_000, 1_000_000]);
    let now = BlockHeight::from_u32(10_000);
    plan_schedule(
        &mut parts,
        now,
        no_floor(),
        old_notes,
        &params,
        &grid(),
        &mut crate::utils::system_rng(),
    )
    .unwrap();

    let current_bucket = grid().index_of(now);
    let earliest = parts.iter().filter_map(|p| p.bucket_index).min().unwrap();
    assert_eq!(
        earliest, current_bucket,
        "notes settled a bucket or more ago pay nothing for the anchor age: \
             the current bucket still has legal anchors below it"
    );
    for part in &parts {
        assert!(
            part.anchor_bucket.unwrap() < part.bucket_index.unwrap(),
            "even in the open window the anchor is a closed bucket"
        );
    }
}

/// The current bucket opened up to `M - 1` blocks ago, and a split's
/// outputs confirm at the tip. Anchoring them at a boundary older than
/// they are is impossible: the note has no Merkle path under it. The
/// anchor therefore moves up to the boundary that covers the notes, and
/// the window moves a further bucket above the anchor, so a freshly split
/// wallet waits one window longer than a settled one. That is the cost
/// ADR 0018 accepted in exchange for a non-empty cohort.
#[test]
fn a_fresh_split_costs_one_window() {
    let params = params();
    let mut parts = parts_with_denominations(&[3_000_000, 2_000_000, 1_000_000]);
    let now = BlockHeight::from_u32(10_000);
    let split_confirmed = now - 3;
    plan_schedule(
        &mut parts,
        now,
        no_floor(),
        |_| Some(split_confirmed),
        &params,
        &grid(),
        &mut crate::utils::system_rng(),
    )
    .unwrap();

    let earliest = parts.iter().filter_map(|p| p.bucket_index).min().unwrap();
    assert_eq!(
        earliest,
        grid().index_of(now) + 2,
        "the anchor clears the fresh notes and the window clears the anchor"
    );
    for part in &parts {
        let anchor = grid().boundary_of(part.anchor_bucket.unwrap());
        assert!(
            anchor >= split_confirmed,
            "every part must anchor at or above its own notes"
        );
    }
}

proptest! {
    // Totality, target-in-window, cohort bound and session count for any
    // part set and parameterization. Bucket assignments are deterministic;
    // target_height is intentionally random so we only pin bucket structure.
    #[test]
    fn schedule_properties(
        denominations in proptest::collection::vec(1u64..=10_000_000_000, 1..80),
        now in 0u32..=10_000_000,
        target_sessions in 1u32..=12,
        k_max in 1u32..=16,
    ) {
        let mut params = params();
        params.target_sessions = target_sessions;
        params.k_max = k_max;
        let now = BlockHeight::from_u32(now);

        let mut parts = parts_with_denominations(&denominations);
        plan_schedule(
        &mut parts,
        now,
        no_floor(),
        old_notes,
        &params,
        &grid(),
        &mut crate::utils::system_rng(),
    ).unwrap();

        let k = (denominations.len() as u64)
            .div_ceil(u64::from(target_sessions))
            .clamp(1, u64::from(k_max));
        let mut cohort_sizes: std::collections::BTreeMap<u64, u64> = Default::default();
        let current_bucket = grid().index_of(now);
        for part in &parts {
            prop_assert_eq!(part.state, PartState::Assigned, "total");
            let bucket = part.bucket_index.unwrap();
            prop_assert!(bucket >= current_bucket, "current or future buckets");
            *cohort_sizes.entry(bucket).or_default() += 1;
            // Target is within the bucket's window.
            let boundary = u32::from(grid().boundary_of(bucket));
            let target = u32::from(part.target_height.unwrap());
            let cap = SchedulingParams::ZIP_318.transfer_delay().cap().get();
            prop_assert!(
                target >= boundary && target <= boundary + cap,
                "target drawn from the boundary under the canonical delay law"
            );
        }
        prop_assert!(cohort_sizes.values().all(|&size| size <= k), "k bound");
        prop_assert_eq!(
            cohort_sizes.len() as u64,
            (denominations.len() as u64).div_ceil(k),
            "session count"
        );
    }

    // On a uniform grid previous_boundary is the closest height ≡ 0 (mod M)
    // at or below.
    #[test]
    fn bucket_algebra(height in 0u32..=100_000_000, modulus in 1u32..=4096) {
        let grid = BucketGrid::new(modulus, uniform_chain());
        let height = BlockHeight::from_u32(height);
        let boundary = previous_boundary(height, &grid);
        prop_assert!(boundary <= height);
        prop_assert_eq!(u32::from(boundary) % modulus, 0);
        prop_assert!(u32::from(height) - u32::from(boundary) < modulus);
        prop_assert_eq!(
            grid.index_of(boundary),
            grid.index_of(height)
        );
    }
}

#[test]
fn upcoming_windows_lists_future_windows_within_the_horizon() {
    let params = params();
    let mut parts = parts_with_denominations(&[100, 200, 300, 400, 500, 600, 700]);
    let now = BlockHeight::from_u32(10_000);
    let now_unix = 1_780_000_000;
    plan_schedule(
        &mut parts,
        now,
        no_floor(),
        old_notes,
        &params,
        &grid(),
        &mut crate::utils::system_rng(),
    )
    .unwrap();

    let windows = upcoming_windows(&parts, now, now_unix, u64::MAX, &grid());
    let listed: usize = windows.iter().map(|w| w.part_ids.len()).sum();

    // upcoming_windows lists strictly future windows. The first batch opens
    // in the current bucket and is surfaced through `due_now`, not here, so
    // every future-bucket part appears and no current-bucket one does.
    let current_bucket = grid().index_of(now);
    let future_parts = parts
        .iter()
        .filter(|part| {
            part.bucket_index
                .is_some_and(|bucket| bucket > current_bucket)
        })
        .count();
    assert!(
        future_parts > 0 && future_parts < parts.len(),
        "the fixture must span current and future buckets"
    );
    assert_eq!(listed, future_parts, "every future-bucket part appears");
    assert!(
        windows
            .iter()
            .all(|window| window.bucket_index > current_bucket),
        "no window is for the current bucket"
    );
    for pair in windows.windows(2) {
        assert!(pair[0].bucket_index < pair[1].bucket_index, "soonest first");
    }
    for window in &windows {
        assert_eq!(window.boundary, grid().boundary_of(window.bucket_index));
        assert!(window.window_opens_unix_time > now_unix);

        // The user-facing wake time is the window's latest per-part
        // target, never earlier than the window opening.
        let latest_target = parts
            .iter()
            .filter(|part| part.bucket_index == Some(window.bucket_index))
            .filter_map(|part| part.target_height)
            .max()
            .expect("scheduled parts carry targets");
        assert_eq!(
            window.latest_target_unix_time,
            grid().estimated_unix_at(latest_target, now, now_unix)
        );
        assert!(window.latest_target_unix_time >= window.window_opens_unix_time);
    }

    // A confirmed part never appears in a window.
    parts[0]
        .mark_confirmed(BlockHeight::from_u32(20_000))
        .unwrap();
    let windows = upcoming_windows(&parts, now, now_unix, u64::MAX, &grid());
    assert!(
        windows
            .iter()
            .all(|window| !window.part_ids.contains(&PartId(0)))
    );

    // The horizon bounds the listing.
    assert!(upcoming_windows(&parts, now, now_unix, 0, &grid()).is_empty());
}

/// The grid numbers the canonical `AnchorGrid`'s boundaries, across the
/// NU7 activation and on chains without one.
mod the_grid_follows_the_chain {
    use super::*;

    fn chain_activating_nu7_at(height: Option<u32>) -> ChainType {
        ChainType::Regtest(crate::testutils::mock_activation_heights_with(|era| {
            era.set_nu7(height)
        }))
    }

    /// Walking the canonical grid boundary by boundary visits exactly the
    /// buckets' boundaries in order, and every height between two
    /// boundaries belongs to the lower one's bucket.
    #[test]
    fn buckets_number_the_canonical_boundaries() {
        // The mock schedule activates every earlier upgrade at height 1,
        // and NU7 may not precede them.
        for activation in [
            None,
            Some(1),
            Some(2),
            Some(100),
            Some(144),
            Some(145),
            Some(431),
            Some(432),
            Some(1_000),
        ] {
            for modulus in [1u32, 7, 144, 256] {
                let grid = BucketGrid::new(modulus, chain_activating_nu7_at(activation));
                let canonical = grid.anchor_grid();
                let mut boundary = BlockHeight::from_u32(0);
                for index in 0..40u64 {
                    assert_eq!(
                        grid.boundary_of(index),
                        boundary,
                        "{activation:?} {modulus} {index}"
                    );
                    assert_eq!(
                        grid.index_of(boundary),
                        index,
                        "{activation:?} {modulus} {index}"
                    );
                    assert!(grid.is_boundary(boundary));
                    assert!(canonical.is_boundary(boundary));
                    let next = canonical.boundary_after(boundary);
                    for height in u32::from(boundary) + 1..u32::from(next) {
                        let height = BlockHeight::from_u32(height);
                        assert_eq!(
                            grid.index_of(height),
                            index,
                            "{activation:?} {modulus} {height:?}"
                        );
                        assert!(!grid.is_boundary(height));
                        assert_eq!(grid.index_at_or_after(height), index + 1);
                    }
                    boundary = next;
                }
            }
        }
    }

    /// From activation the boundaries sit `3·M` apart and the expiry grid
    /// spans three times as many blocks, as ZIP 318's block-count section
    /// states; below it nothing changes.
    #[test]
    fn boundaries_and_expiries_scale_from_the_activation() {
        let grid = BucketGrid::new(144, chain_activating_nu7_at(Some(1_000)));
        // Boundaries 0..=864 are below the activation; the next is 1_296.
        assert_eq!(grid.boundary_of(6), BlockHeight::from_u32(864));
        assert_eq!(grid.boundary_of(7), BlockHeight::from_u32(1_296));
        assert_eq!(grid.boundary_of(8), BlockHeight::from_u32(1_728));
        assert_eq!(grid.index_of(BlockHeight::from_u32(1_000)), 6);
        assert_eq!(grid.index_of(BlockHeight::from_u32(1_295)), 6);
        assert_eq!(grid.index_of(BlockHeight::from_u32(1_296)), 7);
        assert_eq!(
            canonical_expiry_height(&grid, BlockHeight::from_u32(999)),
            BlockHeight::from_u32(69_120)
        );
        assert_eq!(
            canonical_expiry_height(&grid, BlockHeight::from_u32(1_000)),
            BlockHeight::from_u32(207_360)
        );
        assert_eq!(
            canonical_expiry_height(&grid, BlockHeight::from_u32(103_680 + 5)),
            BlockHeight::from_u32(103_680 + 207_360)
        );
    }

    /// The last bucket below the activation runs past it. A part
    /// transmitting there targets the window's boundary while the chain
    /// is below the activation, and the activation itself once the chain
    /// has crossed it; every other window targets its boundary.
    #[test]
    fn a_window_straddling_the_activation_targets_it_once_crossed() {
        let grid = BucketGrid::new(144, chain_activating_nu7_at(Some(1_000)));
        // Bucket 6 opens at 864 and closes at 1_296, across the activation.
        let h = BlockHeight::from_u32;
        assert_eq!(transmission_target(6, h(999), &grid), h(865));
        assert_eq!(transmission_target(6, h(1_000), &grid), h(1_000));
        assert_eq!(transmission_target(6, h(1_295), &grid), h(1_000));
        assert_eq!(transmission_target(5, h(1_000), &grid), h(1_000));
        assert_eq!(transmission_target(7, h(1_000), &grid), h(1_297));
        assert_eq!(transmission_target(7, h(2_000), &grid), h(1_297));
        let uniform = BucketGrid::new(144, chain_activating_nu7_at(None));
        assert_eq!(transmission_target(6, h(5_000), &uniform), h(865));
    }

    /// A uniform index between the activation and the first scaled
    /// boundary opens no bucket on the grid: it comes back as its
    /// boundary height, which the window reading moves to the next bucket.
    #[test]
    fn a_uniform_index_between_the_activation_and_the_first_scaled_boundary_is_off_grid() {
        // Activation at 145: boundaries 0 and 144 lie below it, then 432.
        let grid = BucketGrid::new(144, chain_activating_nu7_at(Some(145)));
        assert_eq!(grid.renumber_uniform_index(0), Ok(0));
        assert_eq!(grid.renumber_uniform_index(1), Ok(1));
        assert_eq!(
            grid.renumber_uniform_index(2),
            Err(BlockHeight::from_u32(288))
        );
        assert_eq!(grid.index_at_or_after(BlockHeight::from_u32(288)), 2);
        assert_eq!(grid.renumber_uniform_index(3), Ok(2));
        assert_eq!(
            grid.renumber_uniform_index(4),
            Err(BlockHeight::from_u32(576))
        );
        assert_eq!(grid.renumber_uniform_index(6), Ok(3));
    }

    /// A transfer delay is drawn in reference blocks, so from activation
    /// the same draw lands three times as many blocks past the boundary,
    /// and a window's wall-clock estimate counts 25 seconds a block.
    #[test]
    fn targets_and_times_follow_the_spacing() {
        use rand::SeedableRng;
        use rand_chacha::ChaCha12Rng;
        let uniform = BucketGrid::new(144, chain_activating_nu7_at(None));
        let scaled = BucketGrid::new(144, chain_activating_nu7_at(Some(1)));
        for seed in 0..8u64 {
            let uniform_target =
                random_target_in_bucket(5, &mut ChaCha12Rng::seed_from_u64(seed), &uniform);
            let scaled_target =
                random_target_in_bucket(5, &mut ChaCha12Rng::seed_from_u64(seed), &scaled);
            let uniform_offset = u32::from(uniform_target) - u32::from(uniform.boundary_of(5));
            let scaled_offset = u32::from(scaled_target) - u32::from(scaled.boundary_of(5));
            assert_eq!(scaled_offset, 3 * uniform_offset, "seed {seed}");
        }
        let now = BlockHeight::from_u32(2_000);
        let later = BlockHeight::from_u32(2_010);
        assert_eq!(uniform.estimated_unix_at(later, now, 100), 100 + 10 * 75);
        assert_eq!(scaled.estimated_unix_at(later, now, 100), 100 + 10 * 25);
        assert_eq!(scaled.estimated_unix_at(now, later, 100), 100);
    }
}

/// Pins the ZIP 318 values imported from `zcash_pool_migration`, the
/// reference implementation, to the ZIP's literal numbers. The values
/// feed the consent hash, so they must move only by an explicit commit:
/// a dependency bump that changes one turns this suite red, and red
/// means adopt deliberately, with a params version bump.
mod zip318_conformance_tripwires {
    use super::*;
    use zcash_protocol::value::COIN;

    /// <https://zips.z.cash/zip-0318#canonicalmigrationtransactionstructure>
    #[test]
    fn expiry_constants_match_the_zip() {
        assert_eq!(EXPIRY_MODULUS, 34_560);
        assert_eq!(EXPIRY_WINDOW, 69_120);
    }

    /// <https://zips.z.cash/zip-0318#anchor-heightbucketingandcohorts>
    #[test]
    fn anchor_age_cap_matches_the_zip() {
        assert_eq!(ANCHOR_AGE_CAP, 4);
    }

    /// Strict behavioral equivalence across the anchor-draw delegation
    /// (ADR 0020, Landing C): these vectors were captured from the
    /// retired local implementation under seeded rngs immediately before
    /// the swap to `draw_anchor_boundary`, so a divergence here means the
    /// delegated draw no longer reproduces the behavior the mirror had.
    ///
    /// The generator is pinned to [`rand_chacha::ChaCha12Rng`] rather
    /// than spelled `StdRng`, because the vectors are a property of the
    /// exact random stream: at capture time rand 0.8's `StdRng` was
    /// ChaCha12, but `StdRng` is documented as free to change algorithms
    /// across rand versions, and such a change would fail these goldens
    /// spuriously. If this test ever goes red, the delegated draw has
    /// diverged (or the pinned generator itself moved); never repair it
    /// by re-capturing the vectors from the delegated implementation,
    /// because vectors captured from the code under test pin nothing.
    #[test]
    fn anchor_draw_matches_the_retired_mirror_goldens() {
        use rand::SeedableRng;
        use rand_chacha::ChaCha12Rng;
        let activation = PoolActivation::new_for_test(BlockHeight::from_u32(1_000));
        let expected: [(Option<u64>, Option<u64>, Option<u64>); 6] = [
            (Some(99), Some(99), Some(51)),
            (Some(99), Some(98), Some(51)),
            (Some(99), Some(98), Some(52)),
            (Some(97), Some(97), Some(52)),
            (Some(97), Some(99), Some(52)),
            (Some(98), Some(99), Some(52)),
        ];
        for (seed, expected) in expected.iter().enumerate() {
            let mut rng = ChaCha12Rng::seed_from_u64(seed as u64);
            let floor_loose = AnchorFloor::new(activation, None);
            let floor_note =
                AnchorFloor::new(activation, Some(BlockHeight::from_u32(50 * 144 + 3)));
            let drawn = (
                draw_anchor_bucket(100, &floor_loose, &mut rng, &grid()),
                draw_anchor_bucket(100, &floor_note, &mut rng, &grid()),
                draw_anchor_bucket(53, &floor_note, &mut rng, &grid()),
            );
            assert_eq!(&drawn, expected, "diverged at seed {seed}");
        }
    }

    /// The `zcash_pool_migration` release this workspace has adjudicated.
    const ADJUDICATED_UPSTREAM_VERSION: &str = "0.2.0";

    /// Fails when the migration dependency moves (ADR 0020's movement
    /// tripwire). Reads the workspace lockfile rather than any crate
    /// metadata, because the lockfile is where a resolution lands first.
    #[test]
    fn upstream_version_is_the_adjudicated_one() {
        let lockfile = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("zingolib sits two levels below the workspace root")
            .join("Cargo.lock");
        let lock = std::fs::read_to_string(lockfile).expect("workspace lockfile is readable");
        let package_block = lock
            .split("[[package]]")
            .find(|block| block.contains("name = \"zcash_pool_migration\""))
            .expect("the lockfile resolves zcash_pool_migration");
        assert!(
            package_block.contains(&format!("version = \"{ADJUDICATED_UPSTREAM_VERSION}\"")),
            "zcash_pool_migration moved off the adjudicated release \
                 {ADJUDICATED_UPSTREAM_VERSION}. Re-run the zip318 tripwires against \
                 the new release and bump ADJUDICATED_UPSTREAM_VERSION in the same \
                 commit that adopts it."
        );
    }

    /// Pins the delegated expiry to the ZIP's own arithmetic (the most
    /// recent `EXPIRY_MODULUS` multiple at or below the height, plus
    /// `EXPIRY_WINDOW`), independent of the implementation now that
    /// `canonical_expiry_height` calls the canonical crate.
    /// <https://zips.z.cash/zip-0318#canonicalmigrationtransactionstructure>
    #[test]
    fn expiry_heights_match_the_zip_arithmetic() {
        let sample = [
            0,
            1,
            EXPIRY_MODULUS - 1,
            EXPIRY_MODULUS,
            EXPIRY_MODULUS + 1,
            3_428_499,
            100 * EXPIRY_MODULUS,
            101 * EXPIRY_MODULUS - 1,
        ];
        for height in sample {
            assert_eq!(
                u32::from(canonical_expiry_height(
                    &grid(),
                    BlockHeight::from_u32(height)
                )),
                height - (height % 34_560) + 69_120,
                "diverged at {height}"
            );
        }
    }

    #[test]
    fn params_match_the_zip() {
        let params = MigrationParams::provisional(ChainType::Mainnet);
        // <https://zips.z.cash/zip-0318#anchor-heightbucketingandcohorts>
        assert_eq!(params.bucket_modulus, 144);
        // <https://zips.z.cash/zip-0318#amountselectioncanonicalquantization>
        assert_eq!(params.denom_cap, 10_000 * COIN);
        assert_eq!(params.max_residual_value, COIN / 100);
        // <https://zips.z.cash/zip-0318#notepreparationtransactions>
        assert_eq!(params.max_actions_per_split_tx, 16);
    }

    /// <https://zips.z.cash/zip-0318#transferscheduling>
    #[test]
    fn transfer_delay_matches_the_zip() {
        let delay = SchedulingParams::ZIP_318.transfer_delay();
        assert_eq!(delay.mean().get(), 66);
        assert_eq!(delay.cap().get(), 576);
    }

    /// Pins the derived ladder to the ZIP 318 enumeration
    /// (<https://zips.z.cash/zip-0318#amountselectioncanonicalquantization>).
    /// The denominations feed the consent hash, so a derivation bug or a
    /// moved bound would silently invalidate every recorded consent; this
    /// vector is the tripwire.
    #[test]
    fn ladder_matches_the_zip318_enumeration() {
        assert_eq!(
            MigrationParams::provisional(ChainType::Mainnet).denominations,
            vec![
                10_000 * COIN,
                5_000 * COIN,
                2_000 * COIN,
                1_000 * COIN,
                500 * COIN,
                200 * COIN,
                100 * COIN,
                50 * COIN,
                20 * COIN,
                10 * COIN,
                5 * COIN,
                2 * COIN,
                COIN,
                COIN / 2,
                COIN / 5,
                COIN / 10,
                COIN / 20,
                COIN / 50,
                COIN / 100,
            ],
        );
    }

    /// Every denomination is `n × 10^k` with `n ∈ {1, 2, 5}`.
    #[test]
    fn denominations_follow_the_one_two_five_rule() {
        for denomination in MigrationParams::provisional(ChainType::Mainnet).denominations {
            let mut mantissa = denomination;
            while mantissa % 10 == 0 {
                mantissa /= 10;
            }
            assert!(
                matches!(mantissa, 1 | 2 | 5),
                "{denomination} is not a {{1, 2, 5}} × 10^k value"
            );
        }
    }
}
