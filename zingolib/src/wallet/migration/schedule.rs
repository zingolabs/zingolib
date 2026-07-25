//! Bucket math and schedule assignment (ZIP 318 Phase 2).
//!
//! Buckets are delimited by *boundaries*: block heights ≡ 0 (mod `M`).
//! Bucket `i` spans heights `[i·M, (i+1)·M)`. A part assigned to bucket `i`
//! broadcasts while the chain is inside it, anchored to the tree state at the
//! boundary `i·M`. Because that anchor is identical for every wallet sending
//! into the same bucket, it carries no per-wallet timing information.

use pepper_sync::wallet::PoolActivation;
use rand::Rng;
use zcash_protocol::consensus::BlockHeight;

use crate::wallet::error::WalletError;

use super::params::MigrationParams;
use super::parts::{PartId, PartRecord, PartState};

/// Picks a random block within `[boundary, boundary + M)` as the broadcast
/// target for one part, spreading sends across the window instead of
/// clustering them at the boundary.
pub fn random_target_in_bucket(
    bucket: u64,
    rng: &mut impl Rng,
    params: &MigrationParams,
) -> BlockHeight {
    let boundary = u32::from(boundary_of(bucket, params.bucket_modulus));
    let offset = rng.gen_range(0..params.bucket_modulus);
    BlockHeight::from_u32(boundary + offset)
}

/// Zcash target block spacing in seconds, used to estimate window times.
const TARGET_BLOCK_SPACING_SECONDS: u64 = 75;

/// `EXPIRY_MODULUS`: 30 days of blocks at the 75-second target spacing.
pub const EXPIRY_MODULUS: u32 = 34_560;

/// The canonical validity window past an expiry bucket's opening.
pub const EXPIRY_WINDOW: u32 = 2 * EXPIRY_MODULUS;

/// The canonical ZIP 318 expiry for a transfer scheduled to broadcast at
/// `broadcast_height`: the most recent multiple of [`EXPIRY_MODULUS`] at or
/// below it, plus [`EXPIRY_WINDOW`]. Identical for every transfer scheduled
/// in the same 30-day period, so the committed expiry reveals only that
/// coarse period.
pub fn canonical_expiry_height(broadcast_height: BlockHeight) -> BlockHeight {
    let height = u32::from(broadcast_height);
    BlockHeight::from_u32(height - (height % EXPIRY_MODULUS) + EXPIRY_WINDOW)
}

/// The bucket containing `height`.
pub fn bucket_index(height: BlockHeight, bucket_modulus: u32) -> u64 {
    u64::from(u32::from(height)) / u64::from(bucket_modulus)
}

/// The boundary that opens `bucket_index`, and the anchor height of every
/// part assigned to it.
pub fn boundary_of(bucket_index: u64, bucket_modulus: u32) -> BlockHeight {
    BlockHeight::from_u32(
        u32::try_from(bucket_index * u64::from(bucket_modulus))
            .expect("bucket boundaries fit a block height"),
    )
}

/// The most recent boundary at or below `height`.
pub fn previous_boundary(height: BlockHeight, bucket_modulus: u32) -> BlockHeight {
    boundary_of(bucket_index(height, bucket_modulus), bucket_modulus)
}

/// The first bucket a part may be scheduled into: strictly after
/// `now_height`'s bucket, and never below the pool's activation floor,
/// the first bucket whose boundary sits at or above the Pool Activation,
/// since a pre-activation boundary can anchor no Ironwood output (issue
/// #2493, finding 6).
///
/// This is the single bucket chooser: every site that schedules a part
/// into a future bucket derives the bucket from here, never from raw
/// arithmetic.
pub fn first_permitted_bucket(
    now_height: BlockHeight,
    activation: PoolActivation,
    params: &MigrationParams,
) -> u64 {
    (bucket_index(now_height, params.bucket_modulus) + 1)
        .max(activation_bucket(activation, params.bucket_modulus))
}

/// The first bucket boundary at or above the Pool Activation: the
/// earliest height an Ironwood part can anchor to.
pub fn first_anchorable_boundary(activation: PoolActivation, bucket_modulus: u32) -> BlockHeight {
    boundary_of(
        activation_bucket(activation, bucket_modulus),
        bucket_modulus,
    )
}

/// The first bucket whose boundary sits at or above the Pool Activation.
/// The modulus is nonzero by the [`MigrationParams::bucket_modulus`]
/// invariant (enforced at store read), as in every bucket computation.
fn activation_bucket(activation: PoolActivation, bucket_modulus: u32) -> u64 {
    u64::from(u32::from(activation.height())).div_ceil(u64::from(bucket_modulus))
}

/// Places a part in `bucket` with a fresh random target inside the
/// bucket's window.
///
/// This and [`place_immediate`] are the only placement operations: every
/// move of a part between buckets (initial scheduling, rebuild after
/// expiry) passes through one of them, so a part can never carry a
/// target left over from a previous bucket (issue #2493, finding 7), and
/// every placement chooses, by name, between jittered and immediate.
#[allow(clippy::result_large_err)]
pub fn place(
    part: &mut PartRecord,
    bucket: u64,
    rng: &mut impl Rng,
    params: &MigrationParams,
) -> Result<(), WalletError> {
    transition_to_bucket(part, bucket)?;
    part.target_height = Some(random_target_in_bucket(bucket, rng, params));
    Ok(())
}

/// Places a part in `bucket` due the moment the window is open, the
/// catch-up and immediate-mode operation, where firing now is the
/// disclosed intent. See [`place`] for the placement monopoly.
#[allow(clippy::result_large_err)]
pub fn place_immediate(part: &mut PartRecord, bucket: u64) -> Result<(), WalletError> {
    transition_to_bucket(part, bucket)?;
    part.target_height = None;
    Ok(())
}

/// Routes a placement through the part's legal state transition: fresh
/// parts assign, expired parts reassign, assigned parts shift. Any other
/// state yields the state machine's own transition error.
#[allow(clippy::result_large_err)]
fn transition_to_bucket(part: &mut PartRecord, bucket: u64) -> Result<(), WalletError> {
    match part.state {
        PartState::Bound => part.assign(bucket),
        PartState::Expired => part.reassign(bucket),
        _ => part.shift(bucket),
    }
}

/// Assigns every [`PartState::Bound`] part to an anchor-height bucket and
/// picks a random broadcast target within that bucket's window.
///
/// Multiplicity `k = clamp(ceil(parts / target_sessions), 1, k_max)` parts
/// share each cohort. Cohorts fill consecutive future buckets starting at
/// [`first_permitted_bucket`] (after `now_height`'s bucket and at or above
/// the pool's activation floor), largest denominations first. Bucket
/// assignments are deterministic, and target heights within each window are
/// randomized so parts do not cluster at the boundary.
#[allow(clippy::result_large_err)]
pub fn plan_schedule(
    parts: &mut [PartRecord],
    now_height: BlockHeight,
    activation: PoolActivation,
    params: &MigrationParams,
    rng: &mut impl Rng,
) -> Result<(), WalletError> {
    let unassigned: Vec<usize> = parts
        .iter()
        .enumerate()
        .filter(|(_, part)| part.state == PartState::Bound)
        .map(|(index, _)| index)
        .collect();
    if unassigned.is_empty() {
        return Ok(());
    }

    let k = u64::try_from(unassigned.len())
        .expect("part count fits u64")
        .div_ceil(u64::from(params.target_sessions.max(1)))
        .clamp(1, u64::from(params.k_max.max(1)));

    // Largest denominations first, ties broken by part id for determinism.
    let mut ranked = unassigned;
    ranked.sort_by_key(|&index| {
        (
            std::cmp::Reverse(parts[index].denomination),
            parts[index].id,
        )
    });

    // The first cohort opens in the *current* bucket (floored at activation:
    // a pre-activation boundary can anchor no Ironwood output, issue #2493
    // finding 6), so the first batch is sendable the instant Phase 2 is
    // scheduled. Its window is already open. Later cohorts fill consecutive
    // buckets from there, each one window sooner than the old `+ 1` start.
    // This is local to initial scheduling; `first_permitted_bucket` remains
    // the chooser for expiry rebuild and reassignment, which target a fresh
    // future window.
    let first_bucket = bucket_index(now_height, params.bucket_modulus)
        .max(activation_bucket(activation, params.bucket_modulus));
    for (rank, index) in ranked.into_iter().enumerate() {
        let bucket = first_bucket + rank as u64 / k;
        place(&mut parts[index], bucket, rng, params)?;
    }
    Ok(())
}

/// One future broadcast window: what a platform scheduler (for example
/// `BGTaskScheduler` or `WorkManager`) feeds into its earliest-begin request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BroadcastWindow {
    /// The bucket the parts are assigned to.
    pub bucket_index: u64,
    /// The bucket's opening boundary, also the parts' anchor height.
    pub boundary: BlockHeight,
    /// The parts due in this window.
    pub part_ids: Vec<PartId>,
    /// Rough unix time the boundary is expected to be mined, extrapolated
    /// from `now` at the target block spacing. Aim silent work here: the
    /// boundary's tree state is only witnessable for a finite retention
    /// after it, so a sync shortly after this moment secures the window.
    pub window_opens_unix_time: u64,
    /// Rough unix time the window's *latest* per-part random target is
    /// expected. Aim the user-facing wake here: at this moment every part
    /// of the window is due, so one visit sends the whole batch.
    pub latest_target_unix_time: u64,
}

/// Rough unix time `height` is expected to be mined, extrapolated from
/// `now_height` at the target block spacing (past heights estimate as now).
pub fn estimated_unix_at(height: BlockHeight, now_height: BlockHeight, now_unix: u64) -> u64 {
    let blocks_until = u64::from(u32::from(height).saturating_sub(u32::from(now_height)));
    now_unix + blocks_until * TARGET_BLOCK_SPACING_SECONDS
}

/// Whether a part is due to broadcast right now: it still awaits broadcast
/// ([`PartState::Assigned`] or [`PartState::Signed`]) and its window is open,
/// meaning it is assigned to `current_bucket`, whose boundary is at or below the tip
/// by definition. The part's random `target_height` no longer gates
/// sendability. It is advisory, exposed only as the reminder hint
/// [`BroadcastWindow::latest_target_unix_time`], so a part is due for the whole
/// open window rather than only from its target onward.
///
/// The single-part rule shared by the broadcast loop and the "due now" status
/// read, so a status can never advertise a part a send would decline. It does
/// *not* fold in earlier, missed windows: an overdue part sits in a bucket
/// below `current_bucket` and is catch-up's business.
pub fn part_in_current_bucket(part: &PartRecord, current_bucket: u64) -> bool {
    matches!(part.state, PartState::Assigned | PartState::Signed)
        && part.bucket_index == Some(current_bucket)
}

/// The broadcast windows within the next `horizon` buckets, soonest first.
///
/// Pure: reads only the given parts and clock inputs. Parts whose bucket has
/// already passed are reconciliation's business and are not listed here.
pub fn upcoming_windows(
    parts: &[PartRecord],
    now_height: BlockHeight,
    now_unix: u64,
    horizon: u64,
    params: &MigrationParams,
) -> Vec<BroadcastWindow> {
    let current_bucket = bucket_index(now_height, params.bucket_modulus);
    let mut buckets: std::collections::BTreeMap<u64, (Vec<PartId>, Option<BlockHeight>)> =
        std::collections::BTreeMap::new();
    for part in parts {
        if !matches!(part.state, PartState::Assigned | PartState::Signed) {
            continue;
        }
        let Some(bucket) = part.bucket_index else {
            continue;
        };
        if bucket > current_bucket && bucket <= current_bucket.saturating_add(horizon) {
            let (part_ids, latest_target) = buckets.entry(bucket).or_default();
            part_ids.push(part.id);
            *latest_target = (*latest_target).max(part.target_height);
        }
    }

    buckets
        .into_iter()
        .map(|(bucket, (part_ids, latest_target))| {
            let boundary = boundary_of(bucket, params.bucket_modulus);
            // A part without a target (a catch-up shift) is due at the
            // window opening, which every in-window target is at or past.
            let latest_target = latest_target.unwrap_or(boundary + 1);
            BroadcastWindow {
                bucket_index: bucket,
                boundary,
                part_ids,
                window_opens_unix_time: estimated_unix_at(boundary, now_height, now_unix),
                latest_target_unix_time: estimated_unix_at(latest_target, now_height, now_unix),
            }
        })
        .collect()
}

/// One window of the schedule's timeline: the bucket, its block range, and
/// how far its parts have come. The rendering counterpart to
/// [`BroadcastWindow`], which feeds platform schedulers strictly future
/// windows. This reports every window the schedule touches, finished ones
/// included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowReport {
    /// The bucket this window is.
    pub bucket_index: u64,
    /// The window's opening boundary (inclusive).
    pub boundary: BlockHeight,
    /// The window's closing height (exclusive): the next boundary.
    pub close: BlockHeight,
    /// Whether the chain tip is inside this window.
    pub is_current: bool,
    /// Parts assigned to this window.
    pub parts_total: u32,
    /// Parts confirmed.
    pub parts_confirmed: u32,
    /// Total value assigned to this window, in zatoshis.
    pub value_total: u64,
    /// Value confirmed into Ironwood from this window, in zatoshis.
    pub value_migrated: u64,
}

/// The window timeline, earliest first: one report per bucket holding at
/// least one part, plus always the window the tip is inside, so the
/// calendar exists (with zero tallies) before any migration is scheduled.
/// Pure over the given parts and tip. Parts not yet assigned to a bucket
/// have no window and are not listed.
pub fn window_timeline(
    parts: &[PartRecord],
    now_height: BlockHeight,
    params: &MigrationParams,
) -> Vec<WindowReport> {
    #[derive(Default)]
    struct Tally {
        parts_total: u32,
        parts_confirmed: u32,
        value_total: u64,
        value_migrated: u64,
    }

    let current_bucket = bucket_index(now_height, params.bucket_modulus);
    let mut buckets: std::collections::BTreeMap<u64, Tally> = std::collections::BTreeMap::new();
    buckets.entry(current_bucket).or_default();
    for part in parts {
        let Some(bucket) = part.bucket_index else {
            continue;
        };
        let tally = buckets.entry(bucket).or_default();
        tally.parts_total += 1;
        tally.value_total += part.denomination;
        if matches!(part.state, PartState::Confirmed { .. }) {
            tally.parts_confirmed += 1;
            tally.value_migrated += part.denomination;
        }
    }

    buckets
        .into_iter()
        .map(|(bucket, tally)| WindowReport {
            bucket_index: bucket,
            boundary: boundary_of(bucket, params.bucket_modulus),
            close: boundary_of(bucket + 1, params.bucket_modulus),
            is_current: bucket == current_bucket,
            parts_total: tally.parts_total,
            parts_confirmed: tally.parts_confirmed,
            value_total: tally.value_total,
            value_migrated: tally.value_migrated,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ChainType;
    use crate::wallet::migration::parts::BoundNote;
    use pepper_sync::wallet::OutputId;
    use proptest::prelude::*;
    use zcash_primitives::transaction::TxId;

    fn params() -> MigrationParams {
        MigrationParams::provisional(ChainType::Mainnet)
    }

    /// An inert activation floor for tests whose subject is the schedule
    /// arithmetic itself: height zero never constrains a bucket.
    fn no_floor() -> PoolActivation {
        PoolActivation::new_for_test(BlockHeight::from_u32(0))
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
    /// must not schedule any part into a bucket whose boundary predates
    /// the activation. No ironwood output can anchor there, so every
    /// broadcast attempt skips and the whole consented cohort slides into
    /// the correlation-disclosed catch-up path. The schedule must respect
    /// an activation floor. Note splitting is explicitly permitted before
    /// activation (module doc), so a fully split wallet at a
    /// pre-activation consent height is a supported state.
    #[test]
    fn schedule_respects_the_activation_floor() {
        let params = params();
        let mut parts = parts_with_denominations(&[1_000_000, 2_000_000]);
        // Consent at height 10; the activation lies far above it.
        let now = BlockHeight::from_u32(10);
        let activation = BlockHeight::from_u32(1_000);

        plan_schedule(
            &mut parts,
            now,
            PoolActivation::new_for_test(activation),
            &params,
            &mut rand::rngs::OsRng,
        )
        .unwrap();

        for part in &parts {
            let boundary = boundary_of(part.bucket_index.unwrap(), params.bucket_modulus);
            assert!(
                boundary >= activation,
                "part {:?} scheduled at boundary {boundary:?}, below the \
                 NU6.3 activation {activation:?}",
                part.id,
            );
        }
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
    fn place_draws_a_fresh_target_inside_the_new_window() {
        let params = params();
        let mut part = bound_part(0, 1_000_000);
        part.assign(1).unwrap();
        // The schedule's randomized target inside bucket 1's window.
        let old_target = boundary_of(1, params.bucket_modulus) + 17;
        part.target_height = Some(old_target);
        part.mark_signed(TxId::from_bytes([9; 32]), old_target + 40, None)
            .unwrap();
        part.mark_expired().unwrap();

        place(&mut part, 5, &mut rand::rngs::OsRng, &params).unwrap();

        assert_eq!(part.state, PartState::Assigned);
        let boundary = u32::from(boundary_of(5, params.bucket_modulus));
        let target = u32::from(
            part.target_height
                .expect("jittered placement draws a fresh target"),
        );
        assert!(
            target >= boundary && target < boundary + params.bucket_modulus,
            "the fresh target {target} must lie inside bucket 5's window \
             [{boundary}, {})",
            boundary + params.bucket_modulus,
        );
    }

    #[test]
    fn schedule_fills_consecutive_buckets_largest_first() {
        let params = params();
        // 13 parts, target 6 sessions → k = 3, so 5 buckets: 3+3+3+3+1.
        let denominations: Vec<u64> = (1..=13).map(|i| i * 1_000_000).collect();
        let mut parts = parts_with_denominations(&denominations);
        let now = BlockHeight::from_u32(10_000);
        plan_schedule(&mut parts, now, no_floor(), &params, &mut rand::rngs::OsRng).unwrap();

        // The first cohort opens in the current bucket now, not the next.
        let first_bucket = bucket_index(now, params.bucket_modulus);
        for part in &parts {
            assert_eq!(part.state, PartState::Assigned);
            let bucket = part.bucket_index.unwrap();
            assert!(bucket >= first_bucket, "current or future bucket");
            // Target is within the part's bucket window.
            let boundary = u32::from(boundary_of(bucket, params.bucket_modulus));
            let target = u32::from(part.target_height.unwrap());
            assert!(target >= boundary && target < boundary + params.bucket_modulus);
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

        let now = boundary_of(3, params.bucket_modulus) + 10;
        let timeline = window_timeline(&[done, pending, ahead, unassigned], now, &params);

        assert_eq!(
            timeline.len(),
            2,
            "assigned buckets plus the current window, which bucket 3 already is"
        );
        let current = &timeline[0];
        assert_eq!(current.bucket_index, 3);
        assert_eq!(current.boundary, boundary_of(3, params.bucket_modulus));
        assert_eq!(current.close, boundary_of(4, params.bucket_modulus));
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
        let params = params();
        let now = boundary_of(7, params.bucket_modulus) + 100;

        let bare = window_timeline(&[], now, &params);
        assert_eq!(bare.len(), 1);
        assert!(bare[0].is_current);
        assert_eq!(bare[0].bucket_index, 7);
        assert_eq!(bare[0].boundary, boundary_of(7, params.bucket_modulus));
        assert_eq!(bare[0].close, boundary_of(8, params.bucket_modulus));
        assert_eq!((bare[0].parts_total, bare[0].value_total), (0, 0));

        let mut ahead = bound_part(0, 1_000_000);
        ahead.assign(9).unwrap();
        let timeline = window_timeline(&[ahead], now, &params);
        assert_eq!(timeline.len(), 2);
        assert!(timeline[0].is_current);
        assert_eq!(timeline[0].parts_total, 0);
        assert_eq!(timeline[1].bucket_index, 9);
    }

    /// The random target no longer gates sendability: a current-bucket part
    /// awaiting broadcast is due for the whole open window, the exact case the
    /// old target-gated predicate rejected. Bucket membership plus
    /// awaiting-broadcast state is the whole rule.
    #[test]
    fn part_in_current_bucket_ignores_the_target_height() {
        let params = params();
        let current_bucket = 40;
        let mut part = bound_part(0, 1_000_000);
        part.assign(current_bucket).unwrap();
        // A target high in the window, above where an early-window tip sits.
        part.target_height = Some(boundary_of(current_bucket, params.bucket_modulus) + 100);

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

    /// The first cohort is placed in the bucket the chain is currently in, so
    /// its window is already open and the first batch is immediately sendable.
    #[test]
    fn first_cohort_opens_in_the_current_bucket() {
        let params = params();
        let mut parts = parts_with_denominations(&[3_000_000, 2_000_000, 1_000_000]);
        let now = BlockHeight::from_u32(10_000);
        plan_schedule(&mut parts, now, no_floor(), &params, &mut rand::rngs::OsRng).unwrap();

        let current_bucket = bucket_index(now, params.bucket_modulus);
        let earliest = parts.iter().filter_map(|p| p.bucket_index).min().unwrap();
        assert_eq!(
            earliest, current_bucket,
            "the first cohort opens in the current bucket"
        );
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
            plan_schedule(&mut parts, now, no_floor(), &params, &mut rand::rngs::OsRng).unwrap();

            let k = (denominations.len() as u64)
                .div_ceil(u64::from(target_sessions))
                .clamp(1, u64::from(k_max));
            let mut cohort_sizes: std::collections::BTreeMap<u64, u64> = Default::default();
            let current_bucket = bucket_index(now, params.bucket_modulus);
            for part in &parts {
                prop_assert_eq!(part.state, PartState::Assigned, "total");
                let bucket = part.bucket_index.unwrap();
                prop_assert!(bucket >= current_bucket, "current or future buckets");
                *cohort_sizes.entry(bucket).or_default() += 1;
                // Target is within the bucket's window.
                let boundary = u32::from(boundary_of(bucket, params.bucket_modulus));
                let target = u32::from(part.target_height.unwrap());
                prop_assert!(
                    target >= boundary && target < boundary + params.bucket_modulus,
                    "target in window"
                );
            }
            prop_assert!(cohort_sizes.values().all(|&size| size <= k), "k bound");
            prop_assert_eq!(
                cohort_sizes.len() as u64,
                (denominations.len() as u64).div_ceil(k),
                "session count"
            );
        }

        // previous_boundary is the closest height ≡ 0 (mod M) at or below.
        #[test]
        fn bucket_algebra(height in 0u32..=100_000_000, modulus in 1u32..=4096) {
            let height = BlockHeight::from_u32(height);
            let boundary = previous_boundary(height, modulus);
            prop_assert!(boundary <= height);
            prop_assert_eq!(u32::from(boundary) % modulus, 0);
            prop_assert!(u32::from(height) - u32::from(boundary) < modulus);
            prop_assert_eq!(
                bucket_index(boundary, modulus),
                bucket_index(height, modulus)
            );
        }
    }

    #[test]
    fn upcoming_windows_lists_future_windows_within_the_horizon() {
        let params = params();
        let mut parts = parts_with_denominations(&[100, 200, 300, 400, 500, 600, 700]);
        let now = BlockHeight::from_u32(10_000);
        let now_unix = 1_780_000_000;
        plan_schedule(&mut parts, now, no_floor(), &params, &mut rand::rngs::OsRng).unwrap();

        let windows = upcoming_windows(&parts, now, now_unix, u64::MAX, &params);
        let listed: usize = windows.iter().map(|w| w.part_ids.len()).sum();

        // upcoming_windows lists strictly future windows. The first cohort now opens
        // in the current bucket and is surfaced through `due_now`, not here, so
        // every future-bucket part appears and no current-bucket one does.
        let current_bucket = bucket_index(now, params.bucket_modulus);
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
            assert_eq!(
                window.boundary,
                boundary_of(window.bucket_index, params.bucket_modulus)
            );
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
                estimated_unix_at(latest_target, now, now_unix)
            );
            assert!(window.latest_target_unix_time >= window.window_opens_unix_time);
        }

        // A confirmed part never appears in a window.
        parts[0]
            .mark_confirmed(BlockHeight::from_u32(20_000))
            .unwrap();
        let windows = upcoming_windows(&parts, now, now_unix, u64::MAX, &params);
        assert!(
            windows
                .iter()
                .all(|window| !window.part_ids.contains(&PartId(0)))
        );

        // The horizon bounds the listing.
        assert!(upcoming_windows(&parts, now, now_unix, 0, &params).is_empty());
    }

    /// Checks our locally defined ZIP 318 values against
    /// `zcash_pool_migration`, the reference implementation. The constants
    /// stay local so they can only move by an explicit commit (they feed the
    /// consent hash), and this suite is what catches upstream ratifying
    /// different ones: bump the pinned dev-dependency and red means adopt
    /// deliberately, with a params version bump.
    mod zip318_conformance {
        use zcash_pool_migration::note_splitting::{
            MIGRATION_MAX_DENOMINATION_ZEC, RESIDUAL_MIGRATION_MIN,
        };
        use zcash_pool_migration::scheduling;

        use super::*;
        use crate::wallet::migration::params::COIN;

        #[test]
        fn expiry_constants_match_upstream() {
            assert_eq!(EXPIRY_MODULUS, scheduling::EXPIRY_MODULUS);
            assert_eq!(EXPIRY_WINDOW, scheduling::EXPIRY_WINDOW);
        }

        #[test]
        fn expiry_heights_match_upstream() {
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
            for height in sample.map(BlockHeight::from_u32) {
                assert_eq!(
                    canonical_expiry_height(height),
                    scheduling::expiry_height(height),
                    "diverged at {height}"
                );
            }
        }

        #[test]
        fn params_match_upstream() {
            let params = MigrationParams::provisional(ChainType::Mainnet);
            assert_eq!(params.bucket_modulus, scheduling::BOUNDARY_MODULUS);
            assert_eq!(params.denom_cap, MIGRATION_MAX_DENOMINATION_ZEC * COIN);
            assert_eq!(params.max_residual_value, u64::from(RESIDUAL_MIGRATION_MIN));
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
}
