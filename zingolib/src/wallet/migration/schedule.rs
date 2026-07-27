//! Boundary math, the Poisson broadcast schedule, and the proving-time
//! anchor draw (ZIP 318 Phase 2).
//!
//! Boundaries are the block heights ≡ 0 (mod `M`); buckets — the spans
//! between consecutive boundaries — are internal bookkeeping for wakes and
//! reconciliation. A part's broadcast height is drawn by the Poisson
//! schedule ([`plan_schedule`]): the parts are shuffled into a uniformly
//! random order and each is placed an exponential inter-arrival delay after
//! the previous one, so a wallet's transfers look like an unremarkable
//! Poisson process. Its anchor is drawn separately at proving time from
//! recent boundaries ([`draw_anchor_boundary`]), so transfers across all
//! wallets share a small set of common anchors.

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

/// Places a part at an exact scheduled broadcast height drawn by the
/// Poisson schedule ([`plan_schedule`]), deriving its bookkeeping bucket
/// from the target. See [`place`] for the placement monopoly.
#[allow(clippy::result_large_err)]
pub fn place_scheduled(
    part: &mut PartRecord,
    target: BlockHeight,
    params: &MigrationParams,
) -> Result<(), WalletError> {
    transition_to_bucket(part, bucket_index(target, params.bucket_modulus))?;
    part.target_height = Some(target);
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

/// Mean of the exponential inter-arrival delay between successive
/// transfers, in blocks; the exponential rate is `1 / MEAN_DELAY`. Equal to
/// the boundary modulus `M` by specification (about three hours at the
/// 75-second target spacing); `mean_delay_equals_the_boundary_modulus`
/// pins the equality against the provisional params.
///
/// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#transfer-scheduling>
/// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L71>
pub const MEAN_DELAY: u32 = 144;

/// Upper bound (inclusive) on a single inter-arrival delay, in blocks: a
/// draw exceeding this is discarded and redrawn, truncating the
/// exponential's heavy tail so no transfer is starved for an unbounded
/// time. `4 * MEAN_DELAY`, about twelve hours.
///
/// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#transfer-scheduling>
/// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L76>
pub const MAX_DELAY: u32 = 4 * MEAN_DELAY;

/// Width of one step when the unit interval `[0, 1)` is split into `2^53`
/// equal parts. Drawing 53 random bits and scaling by this yields a value
/// spread uniformly over those evenly spaced points.
const UNIT_STEP: f64 = 1.0 / ((1u64 << 53) as f64);

/// Number of high bits kept from a drawn `u64` to form a 53-bit mantissa
/// (an `f64` has a 53-bit significand, so this is the most uniform grid
/// representable without rounding bias).
const U64_TO_MANTISSA_SHIFT: u32 = 11;

/// Draw a uniform `f64` in `(0, 1]` from `rng`: the top 53 bits of a fresh
/// `u64` scaled onto `[0, 1)`, then mapped through `1 - u` so zero is
/// excluded (keeping `ln` finite) and one is included.
fn draw_unit_left_open(rng: &mut impl Rng) -> f64 {
    let u: u64 = rng.r#gen();
    1.0 - ((u >> U64_TO_MANTISSA_SHIFT) as f64) * UNIT_STEP
}

/// Mean of the exponential inter-arrival delay between successive
/// PREPARATION transactions, in blocks (about thirty minutes). Preparation
/// needs temporal decoupling — a burst of identically shaped padded
/// transactions at a single moment is a linkable cluster — but no anchor
/// bucketing, so its spacing is much tighter than the transfers'
/// [`MEAN_DELAY`].
///
/// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#note-preparation-transactions>
/// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L84>
pub const PREP_MEAN_DELAY: u32 = 24;

/// Upper bound (inclusive) on a single preparation inter-arrival delay, in
/// blocks (about two hours): `4 * PREP_MEAN_DELAY`, the same tail
/// truncation as the transfers' [`MAX_DELAY`]. A draw exceeding this is
/// discarded and redrawn.
///
/// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#note-preparation-transactions>
/// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L89>
pub const PREP_MAX_DELAY: u32 = 4 * PREP_MEAN_DELAY;

/// Draw one inter-arrival delay in blocks from a truncated exponential
/// distribution with the given `mean` by inverse CDF: `round(-mean *
/// ln(u))` for `u` uniform in `(0, 1]`, discarding and redrawing above
/// `cap`. The return is always in `[0, cap]`.
fn draw_delay_with(mean: u32, cap: u32, rng: &mut impl Rng) -> u32 {
    loop {
        let u = draw_unit_left_open(rng);
        // ln(u) <= 0 for u in (0, 1], so the product is non-negative, and
        // rounding (ties up) is truncation of x + 0.5.
        let delay = (-(f64::from(mean)) * u.ln() + 0.5) as u64 as u32;
        if delay <= cap {
            return delay;
        }
    }
}

/// Draw one TRANSFER inter-arrival delay in blocks: mean [`MEAN_DELAY`],
/// discard-and-redraw above [`MAX_DELAY`] (ZIP 318 "Transfer scheduling"
/// MUST).
///
/// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#transfer-scheduling>
/// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L192-L199>
pub fn draw_delay(rng: &mut impl Rng) -> u32 {
    draw_delay_with(MEAN_DELAY, MAX_DELAY, rng)
}

/// Draw one PREPARATION inter-arrival delay in blocks: mean
/// [`PREP_MEAN_DELAY`], discard-and-redraw above [`PREP_MAX_DELAY`]
/// (ZIP 318 "Note preparation transactions" SHOULD).
///
/// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#note-preparation-transactions>
/// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L201-L206>
pub fn draw_prep_delay(rng: &mut impl Rng) -> u32 {
    draw_delay_with(PREP_MEAN_DELAY, PREP_MAX_DELAY, rng)
}

/// Maximum anchor AGE, in boundaries, that the recency-weighted draw will
/// accept: age counts boundaries strictly before the most recent boundary
/// observed at proving time, so a draw past this cap (a very old anchor) is
/// discarded and redrawn. Sixteen boundaries is about two days. A property
/// of the draw algorithm rather than a consented schedule parameter, so it
/// lives here beside the draw, exactly as in the reference implementation.
///
/// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#anchor-height-bucketing-and-cohorts>
/// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L101>
pub const ANCHOR_AGE_CAP: u32 = 16;

/// A recency-weighted anchor age: `Geometric(1/2)` over `1, 2, 3, ...`, so
/// the most probable age is one boundary, the mean is two, and age zero
/// (the most recent boundary) is NEVER produced. Each bit of a fresh `u64`
/// is one fair coin flip; a set bit stops the count.
///
/// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#anchor-height-bucketing-and-cohorts>
/// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L330-L349>
fn draw_anchor_age(rng: &mut impl Rng) -> u32 {
    let mut age: u32 = 1;
    loop {
        let mut bits: u64 = rng.r#gen();
        for _ in 0..u64::BITS {
            if bits & 1 == 1 {
                return age;
            }
            bits >>= 1;
            age += 1;
        }
    }
}

/// Select the boundary height a transfer proves its anchor against, drawn
/// at PROVING time (ZIP 318's anchor-selection rule). Returns the chosen
/// boundary HEIGHT, or `None` if the candidate set is empty; the caller
/// resolves the tree state (the cached witness) at that height.
///
/// The CANDIDATE ANCHOR SET is the boundaries that are simultaneously
/// strictly above `nu63_activation`, at or after `funding_creation_height`
/// (the funding note must already be in the tree at the anchor), and
/// strictly below the most recent boundary at or below `chain_tip_height`
/// (age is always at least one). A recency-weighted age in
/// `[1, ANCHOR_AGE_CAP]` is drawn (`draw_anchor_age`, private) and the candidate
/// is `previous_boundary(chain_tip) - age * M`; a draw exceeding the cap or
/// landing outside the candidate set is discarded and redrawn.
///
/// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#anchor-height-bucketing-and-cohorts>
/// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L351-L399>
pub fn draw_anchor_boundary(
    nu63_activation: BlockHeight,
    funding_creation_height: BlockHeight,
    chain_tip_height: BlockHeight,
    params: &MigrationParams,
    rng: &mut impl Rng,
) -> Option<BlockHeight> {
    let modulus = params.bucket_modulus;
    let most_recent = u32::from(previous_boundary(chain_tip_height, modulus));
    let (lowest, highest) = candidate_boundary_bounds(
        u32::from(nu63_activation),
        u32::from(funding_creation_height),
        most_recent,
        modulus,
    )?;

    // Rejection-sample the geometric age until the candidate lands in
    // [lowest, highest].
    loop {
        let age = draw_anchor_age(rng);
        if age > ANCHOR_AGE_CAP {
            continue;
        }
        let Some(candidate) = most_recent.checked_sub(age * modulus) else {
            continue;
        };
        if candidate >= lowest && candidate <= highest {
            return Some(BlockHeight::from_u32(candidate));
        }
    }
}

/// The inclusive `[lowest, highest]` boundary-height bounds of the
/// candidate anchor set, or `None` if the set is empty. The highest usable
/// boundary is the one strictly below `most_recent` (age is at least one);
/// the lowest is the first boundary both strictly above `nu63_activation`
/// and at or after `funding_creation_height`.
fn candidate_boundary_bounds(
    nu63_activation: u32,
    funding_creation_height: u32,
    most_recent: u32,
    modulus: u32,
) -> Option<(u32, u32)> {
    let highest = most_recent.checked_sub(modulus)?;
    let above_activation = (nu63_activation - (nu63_activation % modulus)).saturating_add(modulus);
    let at_or_after_funding = boundary_at_or_after(funding_creation_height, modulus);
    let lowest = above_activation.max(at_or_after_funding);
    (lowest <= highest).then_some((lowest, highest))
}

/// The smallest boundary height at or after `height`: `height` rounded UP
/// to a multiple of the modulus. Saturates at `u32::MAX`.
fn boundary_at_or_after(height: u32, modulus: u32) -> u32 {
    let remainder = height % modulus;
    if remainder == 0 {
        height
    } else {
        height.saturating_add(modulus - remainder)
    }
}

/// The canonical rolling expiry height for a part scheduled to broadcast at
/// `scheduled_height`: the most recent multiple of `params.expiry_modulus`
/// at or below it, plus twice that modulus, giving every part between one
/// and two modulus periods (about one to two months) of validity.
///
/// A pure function of the scheduled broadcast height, so the committed
/// expiry is identical for every migration transaction — from any wallet —
/// whose broadcast falls in the same period, and reveals nothing else. ZIP
/// 318 derives it from the *scheduled* height, never the construction
/// height, which would leak when the wallet planned its schedule. Because
/// the expiry modulus is an exact multiple of the bucket modulus
/// (34 560 = 240 × 144), a bucket never straddles a period: every scheduled
/// target within a bucket yields the same expiry as the bucket's boundary.
/// Saturates at `u32::MAX`.
///
/// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#canonical-migration-transaction-structure>
/// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L301-L313>
pub fn canonical_expiry_height(
    scheduled_height: BlockHeight,
    params: &MigrationParams,
) -> BlockHeight {
    let h = u32::from(scheduled_height);
    // `BlockHeight`'s delta addition saturates at `u32::MAX`.
    BlockHeight::from_u32(h - (h % params.expiry_modulus)) + (2 * params.expiry_modulus)
}

/// Draws the Poisson broadcast schedule for every [`PartState::Bound`]
/// part (ZIP 318 "Transfer scheduling", issue #2519 deviation 1).
///
/// The parts are first SHUFFLED into a uniformly random order, so the
/// temporal sequence of denominations a wallet emits is not a predictable
/// function of its balance; each part's scheduled height then advances a
/// running height by an independent exponential inter-arrival delay
/// ([`draw_delay`]), so the broadcasts approximate a Poisson process.
/// There is deliberately no per-wallet multiplicity cap (deviation 5). The
/// running height starts one full boundary interval past the lowest
/// candidate anchor boundary — the first boundary strictly above the
/// pool's activation and at or after the funding notes' creation
/// (conservatively, `now_height`) — so every transfer's candidate anchor
/// set is non-empty by construction. Buckets remain derived bookkeeping:
/// each part's bucket is the one containing its drawn target.
///
/// Specification: <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#transfer-scheduling>
/// Reference implementation: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L279-L291>
#[allow(clippy::result_large_err)]
/// The first broadcast window boundary that can hold an Ironwood part at
/// all: two buckets above the Pool Activation's bucket, since the lowest
/// legal anchor is the bucket above the activation's and a window sits a
/// further bucket above its anchor.
///
/// Below this height the Ironwood era is too young to hold both, whatever
/// the wallet's notes look like. Named for the era, not for anchorability:
/// the constraint is which consensus branch the window's implied target
/// commits to, not whether a Merkle path exists (issue #2493, finding 6,
/// and ADR 0014's corrected reason).
pub fn first_ironwood_era_window_boundary(
    activation: PoolActivation,
    bucket_modulus: u32,
) -> BlockHeight {
    boundary_of(
        bucket_index(activation.height(), bucket_modulus) + 2,
        bucket_modulus,
    )
}

pub fn plan_schedule(
    parts: &mut [PartRecord],
    now_height: BlockHeight,
    activation: PoolActivation,
    params: &MigrationParams,
    rng: &mut impl Rng,
) -> Result<(), WalletError> {
    let mut unassigned: Vec<usize> = parts
        .iter()
        .enumerate()
        .filter(|(_, part)| part.state == PartState::Bound)
        .map(|(index, _)| index)
        .collect();
    if unassigned.is_empty() {
        return Ok(());
    }

    // SHUFFLE MUST: a uniformly random broadcast order.
    use rand::seq::SliceRandom as _;
    unassigned.shuffle(rng);

    let modulus = params.bucket_modulus;
    let above_activation = u32::from(first_anchorable_boundary(activation, modulus))
        .max(u32::from(previous_boundary(activation.height(), modulus)).saturating_add(modulus));
    let lowest_candidate =
        above_activation.max(boundary_at_or_after(u32::from(now_height), modulus));
    let mut running = lowest_candidate.saturating_add(modulus);

    for index in unassigned {
        running = running.saturating_add(draw_delay(rng));
        place_scheduled(&mut parts[index], BlockHeight::from_u32(running), params)?;
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
/// ([`PartState::Assigned`] or [`PartState::Signed`]) and its window is open —
/// it is assigned to `current_bucket`, whose boundary is at or below the tip
/// by definition. The part's random `target_height` no longer gates
/// sendability; it is advisory, exposed only as the reminder hint
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

    /// `MEAN_DELAY` is specified equal to the boundary modulus `M`; pin
    /// the equality against the provisional params so neither drifts.
    #[test]
    fn mean_delay_equals_the_boundary_modulus() {
        assert_eq!(MEAN_DELAY, params().bucket_modulus);
        assert_eq!(MAX_DELAY, 4 * MEAN_DELAY);
    }

    /// A drawn `u` of exactly 1 (all-zero mantissa bits) yields ln(1) = 0,
    /// so the delay is zero: the inverse-CDF's fixed point, hand-derivable.
    #[test]
    fn zero_entropy_draws_a_zero_delay() {
        use rand::rngs::mock::StepRng;
        assert_eq!(draw_delay(&mut StepRng::new(0, 0)), 0);
        assert_eq!(draw_prep_delay(&mut StepRng::new(0, 0)), 0);
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
        // A target high in the window — above where an early-window tip sits.
        part.target_height = Some(boundary_of(current_bucket, params.bucket_modulus) + 200);

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

    proptest! {
        // Every drawn delay is within its truncated tail: transfers within
        // MAX_DELAY (DELAYS MUST), preparations within PREP_MAX_DELAY.
        #[test]
        fn delay_within_bounds(seed_step in 1u64..=u64::MAX) {
            use rand::rngs::mock::StepRng;
            let mut rng = StepRng::new(0, seed_step);
            for _ in 0..8 {
                prop_assert!(draw_delay(&mut rng) <= MAX_DELAY);
                prop_assert!(draw_prep_delay(&mut rng) <= PREP_MAX_DELAY);
            }
        }

        // The Poisson schedule: every Bound part is assigned, targets sit
        // strictly past both `now` and the activation floor plus one full
        // boundary interval, consecutive drawn gaps never exceed MAX_DELAY
        // (so the whole span is bounded), and each part's bookkeeping
        // bucket is the one containing its drawn target.
        #[test]
        fn schedule_properties(
            denominations in proptest::collection::vec(1u64..=10_000_000_000, 1..80),
            now in 0u32..=10_000_000,
        ) {
            let params = params();
            let modulus = params.bucket_modulus;
            let now = BlockHeight::from_u32(now);

            let mut parts = parts_with_denominations(&denominations);
            plan_schedule(&mut parts, now, no_floor(), &params, &mut rand::rngs::OsRng).unwrap();

            let start = {
                let mut lowest = u32::from(now).div_ceil(modulus) * modulus;
                if lowest < u32::from(now) + 1 {
                    lowest += modulus;
                }
                // One full interval past the lowest candidate; the floor
                // here is permissive because no_floor() has activation 0.
                u32::from(now).max(lowest)
            };
            let mut targets: Vec<u32> = Vec::new();
            for part in &parts {
                prop_assert_eq!(part.state, PartState::Assigned, "total");
                let target = u32::from(part.target_height.unwrap());
                prop_assert!(target > u32::from(now), "future targets only");
                prop_assert!(target >= start, "past the candidate floor");
                prop_assert_eq!(
                    part.bucket_index.unwrap(),
                    bucket_index(BlockHeight::from_u32(target), modulus),
                    "bucket derived from the target"
                );
                targets.push(target);
            }
            targets.sort_unstable();
            for gap in targets.windows(2) {
                prop_assert!(gap[1] - gap[0] <= MAX_DELAY, "drawn gaps are capped");
            }
        }

        // The canonical expiry lies in (scheduled, scheduled + 2·modulus],
        // is anchored at a multiple of the expiry modulus, and always
        // leaves strictly more than one modulus period of validity.
        #[test]
        fn canonical_expiry_in_rolling_window(scheduled in 0u32..=100_000_000) {
            let params = params();
            let expiry = u32::from(canonical_expiry_height(
                BlockHeight::from_u32(scheduled),
                &params,
            ));
            prop_assert!(expiry > scheduled);
            prop_assert!(expiry <= scheduled + 2 * params.expiry_modulus);
            prop_assert_eq!((expiry - 2 * params.expiry_modulus) % params.expiry_modulus, 0);
            prop_assert!(expiry - scheduled > params.expiry_modulus);
        }

        // Expiry periods align with anchor buckets (the expiry modulus is
        // an exact multiple of the bucket modulus), so every scheduled
        // target inside a bucket shares its boundary's expiry.
        #[test]
        fn canonical_expiry_is_constant_across_a_bucket(
            bucket in 0u64..=100_000,
            offset in 0u32..=4095,
        ) {
            let params = params();
            prop_assert_eq!(params.expiry_modulus % params.bucket_modulus, 0);
            let boundary = boundary_of(bucket, params.bucket_modulus);
            let target =
                BlockHeight::from_u32(u32::from(boundary) + offset % params.bucket_modulus);
            prop_assert_eq!(
                canonical_expiry_height(target, &params),
                canonical_expiry_height(boundary, &params)
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

    /// Hand-derived goldens for the geometric age draw: `StepRng` yields a
    /// fixed word whose low bits encode the coin flips, so the expected age
    /// is one plus the number of trailing zero bits.
    #[test]
    fn anchor_age_goldens() {
        use rand::rngs::mock::StepRng;
        // (word, expected age): bit 0 set -> age 1; k trailing zeros -> k+1.
        let cases: [(u64, u32); 5] = [
            (u64::MAX, 1),
            (0b10, 2),
            (0b100, 3),
            (0b1000_0000, 8),
            (1 << 20, 21),
        ];
        for (word, expected) in cases {
            let mut rng = StepRng::new(word, 0);
            assert_eq!(draw_anchor_age(&mut rng), expected, "word {word:#b}");
        }
    }

    proptest! {
        // The drawn boundary is always a candidate: a boundary multiple,
        // strictly below the most recent boundary (age >= 1, so the most
        // recent boundary is never used), within ANCHOR_AGE_CAP boundaries
        // of it, strictly above the activation, and at or after the funding
        // note's creation.
        #[test]
        fn drawn_anchor_is_always_a_candidate(
            activation in 0u32..=1_000_000,
            funding_offset in 0u32..=2_000,
            tip_offset in 0u32..=5_000,
        ) {
            let params = params();
            let modulus = params.bucket_modulus;
            let funding = activation + funding_offset;
            let tip = funding + tip_offset;
            let drawn = draw_anchor_boundary(
                BlockHeight::from_u32(activation),
                BlockHeight::from_u32(funding),
                BlockHeight::from_u32(tip),
                &params,
                &mut rand::rngs::OsRng,
            );
            let most_recent = u32::from(previous_boundary(BlockHeight::from_u32(tip), modulus));
            match drawn {
                None => {
                    // Empty candidate set: no boundary is simultaneously
                    // strictly above activation, at or after funding, and
                    // strictly below the most recent boundary.
                    let lowest = (activation - (activation % modulus) + modulus)
                        .max(boundary_at_or_after(funding, modulus));
                    prop_assert!(
                        most_recent < modulus || lowest > most_recent - modulus,
                        "draw returned None on a non-empty candidate set"
                    );
                }
                Some(boundary) => {
                    let boundary = u32::from(boundary);
                    prop_assert_eq!(boundary % modulus, 0, "a boundary multiple");
                    prop_assert!(boundary < most_recent, "never the most recent boundary");
                    prop_assert!(
                        most_recent - boundary <= ANCHOR_AGE_CAP * modulus,
                        "within the age cap"
                    );
                    prop_assert!(boundary > activation, "strictly above activation");
                    prop_assert!(boundary >= funding, "funding note in the tree");
                }
            }
        }
    }

    /// Mirrors the reference implementation's `most_recent_boundary` golden
    /// vectors, hand-derived from the shared modulus `M == 144`, so our
    /// boundary arithmetic and the reference agree on the same data.
    ///
    /// Test data: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L493-L521>
    #[test]
    fn previous_boundary_matches_reference_golden_vectors() {
        let modulus = params().bucket_modulus;
        assert_eq!(modulus, 144);
        // Each case is (input height, height rounded down to a multiple of 144).
        let cases: [(u32, u32); 7] = [
            (0, 0),
            (143, 0),
            (144, 144), // an exact boundary maps to itself
            (287, 144),
            (288, 288),           // 2 * 144
            (300, 288),           // 300 = 2*144 + 12
            (1_000_000, 999_936), // 6944 * 144 = 999_936, rem 64
        ];
        for (height, expected) in cases {
            assert_eq!(
                previous_boundary(BlockHeight::from_u32(height), modulus),
                BlockHeight::from_u32(expected),
                "previous_boundary({height})"
            );
        }
    }

    /// Mirrors the reference implementation's `expiry_examples` edge cases,
    /// so our canonical expiry and the reference agree on the same data.
    ///
    /// Test data: <https://github.com/zcash/librustzcash/blob/eb25d234d272ab6e83b1ea10e578b92139f75725/zcash_pool_migration_backend/src/scheduling.rs#L805-L816>
    #[test]
    fn canonical_expiry_matches_reference_edge_cases() {
        let params = params();
        let window = 2 * params.expiry_modulus;
        // At an exact modulus boundary the window is the full 2 * modulus.
        assert_eq!(
            canonical_expiry_height(BlockHeight::from_u32(0), &params),
            BlockHeight::from_u32(window)
        );
        assert_eq!(
            canonical_expiry_height(BlockHeight::from_u32(params.expiry_modulus), &params),
            BlockHeight::from_u32(params.expiry_modulus + window)
        );
        // Just before the next modulus, validity is just over one modulus.
        assert_eq!(
            canonical_expiry_height(BlockHeight::from_u32(params.expiry_modulus - 1), &params),
            BlockHeight::from_u32(window)
        );
    }

    /// Pins the worked example in ZIP 318's canonical-expiry text: NU6.3
    /// activates at Mainnet height 3428143, so a part scheduled before
    /// height 3456000 expires at 3490560, and one scheduled between
    /// 3456000 and 3490559 expires at 3525120.
    ///
    /// <https://github.com/zcash/zips/blob/main/zips/zip-0318.md#canonical-migration-transaction-structure>
    #[test]
    fn canonical_expiry_matches_the_zip_worked_example() {
        let params = params();
        assert_eq!(params.expiry_modulus, 34_560);
        for scheduled in [3_428_143, 3_455_999] {
            assert_eq!(
                canonical_expiry_height(BlockHeight::from_u32(scheduled), &params),
                BlockHeight::from_u32(3_490_560),
            );
        }
        for scheduled in [3_456_000, 3_490_559] {
            assert_eq!(
                canonical_expiry_height(BlockHeight::from_u32(scheduled), &params),
                BlockHeight::from_u32(3_525_120),
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

        let wakes = upcoming_windows(&parts, now, now_unix, u64::MAX, &params);
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
            assert!(wake.window_opens_unix_time > now_unix);

            // The user-facing wake time is the window's latest per-part
            // target, never earlier than the window opening.
            let latest_target = parts
                .iter()
                .filter(|part| part.bucket_index == Some(wake.bucket_index))
                .filter_map(|part| part.target_height)
                .max()
                .expect("scheduled parts carry targets");
            assert_eq!(
                wake.latest_target_unix_time,
                estimated_unix_at(latest_target, now, now_unix)
            );
            assert!(wake.latest_target_unix_time >= wake.window_opens_unix_time);
        }

        // A confirmed part never appears in a wake.
        parts[0]
            .mark_confirmed(BlockHeight::from_u32(20_000))
            .unwrap();
        let wakes = upcoming_windows(&parts, now, now_unix, u64::MAX, &params);
        assert!(wakes.iter().all(|wake| !wake.part_ids.contains(&PartId(0))));

        // The horizon bounds the listing.
        assert!(upcoming_windows(&parts, now, now_unix, 0, &params).is_empty());
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
}
