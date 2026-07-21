//! Bucket math and schedule assignment (ZIP 318 Phase 2).
//!
//! Buckets are delimited by *boundaries*: block heights ≡ 0 (mod `M`).
//! Bucket `i` spans heights `[i·M, (i+1)·M)`; a part assigned to bucket `i`
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

/// Zcash target block spacing in seconds, used to estimate wake times.
const TARGET_BLOCK_SPACING_SECONDS: u64 = 75;

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
/// `now_height`'s bucket, and never below the pool's activation floor —
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
    let activation_bucket =
        u64::from(u32::from(activation.height())).div_ceil(u64::from(params.bucket_modulus.max(1)));
    (bucket_index(now_height, params.bucket_modulus) + 1).max(activation_bucket)
}

/// Places a part in `bucket` with a fresh random target inside the
/// bucket's window.
///
/// This and [`place_immediate`] are the only placement operations: every
/// move of a part between buckets — initial scheduling, rebuild after
/// expiry — passes through one of them, so a part can never carry a
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

/// Places a part in `bucket` due the moment the window is open — the
/// catch-up and immediate-mode operation, where firing now is the
/// disclosed intent. See [`place`] for the placement monopoly.
#[allow(clippy::result_large_err)]
pub fn place_immediate(part: &mut PartRecord, bucket: u64) -> Result<(), WalletError> {
    transition_to_bucket(part, bucket)?;
    part.target_height = None;
    Ok(())
}

/// Routes a placement through the part's legal state transition: fresh
/// parts assign, expired parts reassign, assigned parts shift; any other
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
/// [`first_permitted_bucket`] — after `now_height`'s bucket and at or above
/// the pool's activation floor — largest denominations first. Bucket
/// assignments are deterministic; target heights within each window are
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

    let first_bucket = first_permitted_bucket(now_height, activation, params);
    for (rank, index) in ranked.into_iter().enumerate() {
        let bucket = first_bucket + rank as u64 / k;
        place(&mut parts[index], bucket, rng, params)?;
    }
    Ok(())
}

/// One future broadcast window: what a platform scheduler (for example
/// `BGTaskScheduler` or `WorkManager`) feeds into its earliest-begin request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakePoint {
    /// The bucket the parts are assigned to.
    pub bucket_index: u64,
    /// The bucket's opening boundary, also the parts' anchor height.
    pub boundary: BlockHeight,
    /// The parts due in this window.
    pub part_ids: Vec<PartId>,
    /// Rough unix time the boundary is expected to be mined, extrapolated
    /// from `now` at the target block spacing.
    pub estimated_unix_time: u64,
}

/// The broadcast windows within the next `horizon` buckets, soonest first.
///
/// Pure: reads only the given parts and clock inputs. Parts whose bucket has
/// already passed are reconciliation's business and are not listed here.
pub fn next_wakes(
    parts: &[PartRecord],
    now_height: BlockHeight,
    now_unix: u64,
    horizon: u64,
    params: &MigrationParams,
) -> Vec<WakePoint> {
    let current_bucket = bucket_index(now_height, params.bucket_modulus);
    let mut buckets: std::collections::BTreeMap<u64, Vec<PartId>> =
        std::collections::BTreeMap::new();
    for part in parts {
        if !matches!(part.state, PartState::Assigned | PartState::Signed) {
            continue;
        }
        let Some(bucket) = part.bucket_index else {
            continue;
        };
        if bucket > current_bucket && bucket <= current_bucket.saturating_add(horizon) {
            buckets.entry(bucket).or_default().push(part.id);
        }
    }

    buckets
        .into_iter()
        .map(|(bucket, part_ids)| {
            let boundary = boundary_of(bucket, params.bucket_modulus);
            let blocks_until = u64::from(u32::from(boundary).saturating_sub(u32::from(now_height)));
            WakePoint {
                bucket_index: bucket,
                boundary,
                part_ids,
                estimated_unix_time: now_unix + blocks_until * TARGET_BLOCK_SPACING_SECONDS,
            }
        })
        .collect()
}

/// Shifts the remaining schedule after missed windows (ZIP 318: "shift the
/// remaining schedule by the offset").
///
/// If any [`PartState::Assigned`] part sits in a bucket at or before
/// `now_height`'s, every assigned part moves forward by the offset that puts
/// the earliest one in the next bucket. Relative cohort structure is
/// preserved. Parts already signed, broadcast or terminal are untouched.
/// No-op when nothing was missed.
#[allow(clippy::result_large_err)]
pub fn catch_up_shift(
    parts: &mut [PartRecord],
    now_height: BlockHeight,
    params: &MigrationParams,
) -> Result<(), WalletError> {
    let current_bucket = bucket_index(now_height, params.bucket_modulus);
    let Some(earliest) = parts
        .iter()
        .filter(|part| part.state == PartState::Assigned)
        .filter_map(|part| part.bucket_index)
        .min()
    else {
        return Ok(());
    };
    if earliest > current_bucket {
        return Ok(());
    }

    let offset = current_bucket + 1 - earliest;
    for part in parts.iter_mut() {
        if part.state == PartState::Assigned {
            let bucket = part
                .bucket_index
                .expect("assigned parts always carry a bucket");
            part.shift(bucket + offset)?;
        }
    }
    Ok(())
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
    /// the activation — no ironwood output can anchor there, so every
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
    /// window. A stale target from the old bucket — or a cleared-to-None
    /// target treated as immediately due — would fire the rebuilt part at
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

        let first_bucket = bucket_index(now, params.bucket_modulus) + 1;
        for part in &parts {
            assert_eq!(part.state, PartState::Assigned);
            let bucket = part.bucket_index.unwrap();
            assert!(bucket >= first_bucket, "future bucket");
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
                prop_assert!(bucket > current_bucket, "future buckets only");
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
    fn catch_up_shift_is_a_no_op_when_nothing_missed() {
        let params = params();
        let mut parts = parts_with_denominations(&[100, 200, 300]);
        let now = BlockHeight::from_u32(10_000);
        plan_schedule(&mut parts, now, no_floor(), &params, &mut rand::rngs::OsRng).unwrap();
        let before = parts.clone();
        catch_up_shift(&mut parts, now, &params).unwrap();
        assert_eq!(parts, before);
    }

    #[test]
    fn catch_up_shift_moves_the_whole_remaining_schedule() {
        let params = params();
        let mut parts = parts_with_denominations(&[100, 200, 300, 400, 500, 600, 700]);
        let now = BlockHeight::from_u32(10_000);
        plan_schedule(&mut parts, now, no_floor(), &params, &mut rand::rngs::OsRng).unwrap();
        let buckets_before: Vec<u64> = parts.iter().map(|p| p.bucket_index.unwrap()).collect();

        // The chain has advanced two buckets past the first window.
        let later = BlockHeight::from_u32(u32::from(boundary_of(
            buckets_before.iter().min().unwrap() + 2,
            params.bucket_modulus,
        )));
        catch_up_shift(&mut parts, later, &params).unwrap();

        let current_bucket = bucket_index(later, params.bucket_modulus);
        let offset = current_bucket + 1 - buckets_before.iter().min().unwrap();
        for (part, before) in parts.iter().zip(&buckets_before) {
            assert_eq!(part.bucket_index.unwrap(), before + offset);
        }
    }

    #[test]
    fn next_wakes_lists_future_windows_within_the_horizon() {
        let params = params();
        let mut parts = parts_with_denominations(&[100, 200, 300, 400, 500, 600, 700]);
        let now = BlockHeight::from_u32(10_000);
        let now_unix = 1_780_000_000;
        plan_schedule(&mut parts, now, no_floor(), &params, &mut rand::rngs::OsRng).unwrap();

        let wakes = next_wakes(&parts, now, now_unix, u64::MAX, &params);
        let listed: usize = wakes.iter().map(|w| w.part_ids.len()).sum();
        assert_eq!(listed, parts.len(), "every assigned part appears");
        for pair in wakes.windows(2) {
            assert!(pair[0].bucket_index < pair[1].bucket_index, "soonest first");
        }
        for wake in &wakes {
            assert_eq!(
                wake.boundary,
                boundary_of(wake.bucket_index, params.bucket_modulus)
            );
            assert!(wake.estimated_unix_time > now_unix);
        }

        // A confirmed part never appears in a wake.
        parts[0]
            .mark_confirmed(BlockHeight::from_u32(20_000))
            .unwrap();
        let wakes = next_wakes(&parts, now, now_unix, u64::MAX, &params);
        assert!(wakes.iter().all(|wake| !wake.part_ids.contains(&PartId(0))));

        // The horizon bounds the listing.
        assert!(next_wakes(&parts, now, now_unix, 0, &params).is_empty());
    }
}
