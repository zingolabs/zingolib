//! Bucket math and schedule assignment (ZIP 318 Phase 2).
//!
//! Buckets are delimited by *boundaries*: block heights ≡ 0 (mod `M`).
//! Bucket `i` spans heights `[i·M, (i+1)·M)`. A part assigned to bucket `i`
//! broadcasts while the chain is inside it.
//!
//! A part's *anchor* is a separate bucket from its broadcast window. The
//! anchor sits at [`draw_anchor_age`] buckets below the window, never zero
//! ([`ANCHOR_AGE_CAP`] bounds how far), so a part always proves against a
//! boundary the chain has already left. Because that boundary is identical
//! for every wallet anchoring there, it carries no per-wallet timing
//! information, and because its window has closed, its ZIP 318 *cohort* (the
//! transfers network-wide sharing it) has had time to accumulate. Anchoring
//! at the window's own boundary, which the chain is still inside, would be
//! age zero: the newest tree state, whose cohort is empty. See ADR 0018.

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
/// Sourced from `zcash_pool_migration::scheduling`, the ZIP 318 reference
/// implementation, so it can only move by an explicit, reviewed pin bump.
pub const EXPIRY_MODULUS: u32 = zcash_pool_migration::scheduling::EXPIRY_MODULUS;

/// The canonical validity window past an expiry bucket's opening.
pub const EXPIRY_WINDOW: u32 = zcash_pool_migration::scheduling::EXPIRY_WINDOW;

/// The canonical ZIP 318 expiry for a transfer scheduled to broadcast at
/// `broadcast_height`: the most recent multiple of [`EXPIRY_MODULUS`] at or
/// below it, plus [`EXPIRY_WINDOW`]. Identical for every transfer scheduled
/// in the same 30-day period, so the committed expiry reveals only that
/// coarse period.
pub fn canonical_expiry_height(broadcast_height: BlockHeight) -> BlockHeight {
    zcash_pool_migration::scheduling::expiry_height(broadcast_height)
}

/// The bucket containing `height`.
pub fn bucket_index(height: BlockHeight, bucket_modulus: u32) -> u64 {
    u64::from(u32::from(height)) / u64::from(bucket_modulus)
}

/// The boundary that opens `bucket_index`. It is the anchor height of every
/// part whose *anchor bucket* this is, which is never the same bucket the
/// part broadcasts in.
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

/// `ANCHOR_AGE_CAP`: the greatest anchor age the draw accepts, in buckets.
/// A draw above it is discarded and redrawn, bounding how stale a part's
/// anchor may be (16 buckets is about two days at `M` = 144). Deliberately
/// not a [`MigrationParams`] field: it does not feed the consent hash, so
/// adopting it costs no existing consent. Sourced from
/// `zcash_pool_migration::scheduling`, as above.
pub const ANCHOR_AGE_CAP: u32 = zcash_pool_migration::scheduling::ANCHOR_AGE_CAP;

/// The two floors a part's candidate anchor bucket must clear, resolved for
/// one part.
///
/// The *era floor* is network-wide: an anchor must sit strictly above the
/// Pool Activation's bucket. The *anchorability floor* is per part: an
/// anchor must sit at or above the boundary covering the part's own bound
/// note, because a note has no Merkle path under a root that predates it.
/// Both are ZIP 318 candidate-set conditions (a) and (b).
#[derive(Debug, Clone, Copy)]
pub struct AnchorFloor {
    activation: PoolActivation,
    note_confirmed_at: Option<BlockHeight>,
}

impl AnchorFloor {
    /// The floors for a part whose bound note confirmed at
    /// `note_confirmed_at`. `None` (still unconfirmed) floors nothing: an
    /// unconfirmed note has no tree position to anchor at any boundary, and
    /// [`crate::wallet::LightWallet::prepare_part`] declines it on that
    /// ground rather than this one.
    #[must_use]
    pub fn new(activation: PoolActivation, note_confirmed_at: Option<BlockHeight>) -> Self {
        AnchorFloor {
            activation,
            note_confirmed_at,
        }
    }

    /// The lowest bucket this part may anchor at: the higher of the two
    /// floors.
    #[must_use]
    pub fn lowest_anchor_bucket(&self, bucket_modulus: u32) -> u64 {
        let era = bucket_index(self.activation.height(), bucket_modulus) + 1;
        let anchorability = self
            .note_confirmed_at
            .map_or(0, |height| bucket_at_or_after(height, bucket_modulus));
        era.max(anchorability)
    }

    /// The earliest broadcast window this part may be scheduled into: one
    /// bucket above its lowest legal anchor, because an anchor always sits
    /// at least one bucket below its window.
    ///
    /// This is why the era floor no longer needs restating on the window.
    /// Every legal anchor is above the activation bucket and every window is
    /// above its anchor, so a scheduled window clears the activation by two
    /// buckets without being asked to.
    #[must_use]
    pub fn earliest_window(&self, bucket_modulus: u32) -> u64 {
        self.lowest_anchor_bucket(bucket_modulus) + 1
    }
}

/// Draws an anchor age `a ≥ 1` from the recency-weighted `Geometric(1/2)`
/// distribution: the number of failed fair coin flips plus one, so
/// `P(a = 1) = 1/2`, `P(a = 2) = 1/4`, and the mean age is two buckets.
///
/// Age zero is never produced. That is the whole point: an age-zero anchor
/// is the boundary of the window the chain is still inside, the newest tree
/// state there is, whose cohort has not accumulated yet.
///
/// Counting stops one past [`ANCHOR_AGE_CAP`], which the caller rejects
/// anyway, so a single 64-bit draw always suffices and the loop is bounded.
pub fn draw_anchor_age(rng: &mut impl Rng) -> u32 {
    // Each bit of one fresh word is one fair coin flip, as upstream's
    // `draw_anchor_age` does it. The cap is far below 64, so one word is
    // always enough and the loop needs no refill.
    let mut bits = rng.next_u64();
    let mut age: u32 = 1;
    while age <= ANCHOR_AGE_CAP {
        if bits & 1 == 1 {
            return age;
        }
        bits >>= 1;
        age += 1;
    }
    age
}

/// The anchor bucket for a part broadcasting in `window`: `window − a` for a
/// drawn age, redrawn until it clears `floor` and [`ANCHOR_AGE_CAP`].
///
/// `None` when the candidate set is empty, which is the caller's signal that
/// `window` is too early for this part rather than a transient condition.
/// Age one always yields `window − 1`, the highest candidate, so whenever the
/// set is non-empty the rejection loop terminates with probability one half
/// per draw.
pub fn draw_anchor_bucket(
    window: u64,
    floor: &AnchorFloor,
    rng: &mut impl Rng,
    bucket_modulus: u32,
) -> Option<u64> {
    // The highest candidate is one full bucket below the window, since the
    // age is never zero. An empty set means the floors reach the window.
    let highest = window.checked_sub(1)?;
    let lowest = floor.lowest_anchor_bucket(bucket_modulus);
    if lowest > highest {
        return None;
    }
    loop {
        let age = draw_anchor_age(rng);
        if age > ANCHOR_AGE_CAP {
            continue;
        }
        // An age reaching past bucket zero is a too-old anchor: redraw.
        if let Some(candidate) = window.checked_sub(u64::from(age))
            && candidate >= lowest
        {
            return Some(candidate);
        }
    }
}

/// The first bucket a part may be scheduled into: strictly after
/// `now_height`'s bucket, and never below the earliest window its anchor
/// floors permit.
///
/// This is the single bucket chooser: every site that schedules a part
/// into a future bucket derives the bucket from here, never from raw
/// arithmetic.
pub fn first_permitted_bucket(
    now_height: BlockHeight,
    floor: &AnchorFloor,
    params: &MigrationParams,
) -> u64 {
    (bucket_index(now_height, params.bucket_modulus) + 1)
        .max(floor.earliest_window(params.bucket_modulus))
}

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

/// The first bucket whose boundary sits at or above `height`. The modulus
/// is nonzero by the [`MigrationParams::bucket_modulus`] invariant
/// (enforced at store read), as in every bucket computation.
pub fn bucket_at_or_after(height: BlockHeight, bucket_modulus: u32) -> u64 {
    u64::from(u32::from(height)).div_ceil(u64::from(bucket_modulus))
}

/// Places a part in `bucket` with a fresh random target inside the
/// bucket's window and a freshly drawn anchor below it.
///
/// This and [`place_immediate`] are the only placement operations: every
/// move of a part between buckets (initial scheduling, rebuild after
/// expiry) passes through one of them, so a part can never carry a
/// target or an anchor left over from a previous bucket (issue #2493,
/// finding 7), and every placement chooses, by name, between jittered and
/// immediate.
///
/// Re-drawing the anchor here is what keeps the age honest. A part shifted
/// into a later window while keeping an old anchor would silently age past
/// [`ANCHOR_AGE_CAP`]; because every move lands here, none can.
#[allow(clippy::result_large_err)]
pub fn place(
    part: &mut PartRecord,
    bucket: u64,
    floor: &AnchorFloor,
    rng: &mut impl Rng,
    params: &MigrationParams,
) -> Result<(), WalletError> {
    let anchor = anchor_for(bucket, floor, rng, params)?;
    transition_to_bucket(part, bucket)?;
    part.anchor_bucket = Some(anchor);
    part.target_height = Some(random_target_in_bucket(bucket, rng, params));
    Ok(())
}

/// Places a part in `bucket` due the moment the window is open, the
/// catch-up and immediate-mode operation, where firing now is the
/// disclosed intent. See [`place`] for the placement monopoly.
///
/// Immediate is about *when the part fires*, not what it proves against:
/// the anchor is drawn at age one or more here exactly as in [`place`].
/// Disclosing the send time is the accepted cost of catch-up; handing the
/// part an empty cohort as well is not.
#[allow(clippy::result_large_err)]
pub fn place_immediate(
    part: &mut PartRecord,
    bucket: u64,
    floor: &AnchorFloor,
    rng: &mut impl Rng,
    params: &MigrationParams,
) -> Result<(), WalletError> {
    let anchor = anchor_for(bucket, floor, rng, params)?;
    transition_to_bucket(part, bucket)?;
    part.anchor_bucket = Some(anchor);
    part.target_height = None;
    Ok(())
}

/// Draws the anchor both placements need, turning an empty candidate set
/// into the typed refusal rather than a silent age-zero fallback.
#[allow(clippy::result_large_err)]
fn anchor_for(
    bucket: u64,
    floor: &AnchorFloor,
    rng: &mut impl Rng,
    params: &MigrationParams,
) -> Result<u64, WalletError> {
    draw_anchor_bucket(bucket, floor, rng, params.bucket_modulus).ok_or(
        WalletError::MigrationNoLegalAnchor {
            window: bucket,
            lowest_anchor: floor.lowest_anchor_bucket(params.bucket_modulus),
        },
    )
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

/// Assigns every [`PartState::Bound`] part to a broadcast window, draws its
/// anchor below that window, and picks a random broadcast target inside it.
///
/// Multiplicity `k = clamp(ceil(parts / target_sessions), 1, k_max)` parts
/// share each *batch*. Batches fill consecutive buckets, largest
/// denominations first. Bucket assignments are deterministic; target heights
/// and anchor ages within each window are randomized.
///
/// A batch is this wallet's own parts sharing one window, and is not a ZIP
/// 318 *cohort*: with per-part anchor ages the parts of one batch no longer
/// share an anchor, and a cohort is the set of parts network-wide that do.
///
/// `note_confirmed_at` resolves each part's bound note's confirmation
/// height, the per-part half of [`AnchorFloor`]. The first window is floored
/// cohort-wide at the highest [`AnchorFloor::earliest_window`] across the
/// parts, because windows are shared even though anchors are not: a window
/// no part could anchor under would strand the whole batch.
#[allow(clippy::result_large_err)]
pub fn plan_schedule(
    parts: &mut [PartRecord],
    now_height: BlockHeight,
    activation: PoolActivation,
    note_confirmed_at: impl Fn(&PartRecord) -> Option<BlockHeight>,
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

    // The first batch opens in the *current* bucket where the anchor floors
    // allow it, so a wallet whose notes have been settled at least a bucket
    // is sendable the instant Phase 2 is scheduled. Later batches fill
    // consecutive buckets from there.
    //
    // A wallet that has just split pays one window. Its notes' own
    // confirmation floors the anchor at the next boundary, and a window sits
    // a bucket above its anchor, so the first batch opens one bucket further
    // out than the pre-anchor-age schedule placed it. That is the cost ADR
    // 0018 accepted for a non-empty cohort.
    //
    // This is local to initial scheduling; `first_permitted_bucket` remains
    // the chooser for expiry rebuild and reassignment, which target a fresh
    // future window.
    let first_bucket = ranked.iter().fold(
        bucket_index(now_height, params.bucket_modulus),
        |earliest, &index| {
            let floor = AnchorFloor::new(activation, note_confirmed_at(&parts[index]));
            earliest.max(floor.earliest_window(params.bucket_modulus))
        },
    );
    for (rank, index) in ranked.into_iter().enumerate() {
        let floor = AnchorFloor::new(activation, note_confirmed_at(&parts[index]));
        let bucket = first_bucket + rank as u64 / k;
        place(&mut parts[index], bucket, &floor, rng, params)?;
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
    /// broadcast attempt skips and the whole consented batch slides into the
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
            &mut rand::rngs::OsRng,
        )
        .unwrap();

        for part in &parts {
            let anchor = boundary_of(part.anchor_bucket.unwrap(), params.bucket_modulus);
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
                "part {:?} must broadcast above the bucket it anchors in",
                part.id,
            );
        }
    }

    /// The anchor age is never zero: a part must never prove against the
    /// boundary of the window it is broadcasting in. That boundary is the
    /// newest tree state at broadcast time, so its ZIP 318 cohort has not
    /// accumulated and the anonymity set the anchor draw exists to build is
    /// empty. Upstream states it as a MUST ("age 0 is NEVER produced").
    #[test]
    fn anchor_age_is_never_zero() {
        for _ in 0..2_000 {
            let age = draw_anchor_age(&mut rand::rngs::OsRng);
            assert!(age >= 1, "drew age {age}, which is the open window itself");
        }
    }

    /// Every accepted anchor sits inside the candidate set: at least one
    /// bucket below the window, at or above both floors, and within the age
    /// cap. The rejection loop, not the raw geometric draw, is what enforces
    /// the lower end.
    #[test]
    fn drawn_anchors_land_inside_the_candidate_set() {
        let params = params();
        let activation = PoolActivation::new_for_test(BlockHeight::from_u32(1_000));
        let note = BlockHeight::from_u32(2_000);
        let floor = AnchorFloor::new(activation, Some(note));
        let lowest = floor.lowest_anchor_bucket(params.bucket_modulus);
        let window = lowest + 40;

        for _ in 0..2_000 {
            let anchor = draw_anchor_bucket(
                window,
                &floor,
                &mut rand::rngs::OsRng,
                params.bucket_modulus,
            )
            .expect("the set is non-empty forty buckets above the floor");
            assert!(anchor < window, "the anchor is below the open window");
            assert!(anchor >= lowest, "the anchor clears both floors");
            assert!(
                window - anchor <= u64::from(ANCHOR_AGE_CAP),
                "the age {} exceeds the cap",
                window - anchor
            );
            let boundary = boundary_of(anchor, params.bucket_modulus);
            assert!(boundary > activation.height(), "anchor above activation");
            assert!(boundary >= note, "anchor at or above its own note");
        }
    }

    /// A window with no legal anchor below it is refused, not silently given
    /// an age-zero anchor. This is the state a hand-computed window can reach
    /// and `first_permitted_bucket` cannot.
    #[test]
    fn a_window_with_no_legal_anchor_is_refused() {
        let params = params();
        let floor = AnchorFloor::new(
            PoolActivation::new_for_test(BlockHeight::from_u32(0)),
            Some(BlockHeight::from_u32(0)),
        );
        let lowest = floor.lowest_anchor_bucket(params.bucket_modulus);

        assert!(
            draw_anchor_bucket(
                lowest,
                &floor,
                &mut rand::rngs::OsRng,
                params.bucket_modulus
            )
            .is_none(),
            "the lowest legal anchor cannot also be the window"
        );
        assert_eq!(
            floor.earliest_window(params.bucket_modulus),
            lowest + 1,
            "the earliest window is one bucket above the lowest anchor"
        );

        let mut part = bound_part(0, 1_000_000);
        assert!(matches!(
            place(&mut part, lowest, &floor, &mut rand::rngs::OsRng, &params),
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

        place(
            &mut part,
            5,
            &weakest_floor(),
            &mut rand::rngs::OsRng,
            &params,
        )
        .unwrap();

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
        plan_schedule(
            &mut parts,
            now,
            no_floor(),
            old_notes,
            &params,
            &mut rand::rngs::OsRng,
        )
        .unwrap();

        // The first batch opens in the current bucket, not the next.
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
            &mut rand::rngs::OsRng,
        )
        .unwrap();

        let current_bucket = bucket_index(now, params.bucket_modulus);
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
            &mut rand::rngs::OsRng,
        )
        .unwrap();

        let earliest = parts.iter().filter_map(|p| p.bucket_index).min().unwrap();
        assert_eq!(
            earliest,
            bucket_index(now, params.bucket_modulus) + 2,
            "the anchor clears the fresh notes and the window clears the anchor"
        );
        for part in &parts {
            let anchor = boundary_of(part.anchor_bucket.unwrap(), params.bucket_modulus);
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
            &mut rand::rngs::OsRng,
        ).unwrap();

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
        plan_schedule(
            &mut parts,
            now,
            no_floor(),
            old_notes,
            &params,
            &mut rand::rngs::OsRng,
        )
        .unwrap();

        let windows = upcoming_windows(&parts, now, now_unix, u64::MAX, &params);
        let listed: usize = windows.iter().map(|w| w.part_ids.len()).sum();

        // upcoming_windows lists strictly future windows. The first batch opens
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
    /// `zcash_pool_migration`, the reference implementation.
    ///
    /// The scheduling constants ([`EXPIRY_MODULUS`], [`EXPIRY_WINDOW`],
    /// [`ANCHOR_AGE_CAP`], [`MigrationParams::bucket_modulus`]) and
    /// [`canonical_expiry_height`] are sourced directly from
    /// `zcash_pool_migration::scheduling` rather than reimplemented, so there
    /// is nothing left to assert equal for those — a divergence is a compile
    /// error, not a test failure. What remains local, and so still needs
    /// conformance-testing here, are the note-splitting denomination
    /// constants: they feed the consent hash and stay independently defined
    /// so they can only move by an explicit commit. This suite is what
    /// catches upstream ratifying different ones: bump the pinned dependency
    /// and red means adopt deliberately, with a params version bump.
    mod zip318_conformance {
        use zcash_pool_migration::note_splitting::{
            MIGRATION_MAX_DENOMINATION_ZEC, RESIDUAL_MIGRATION_MIN,
        };

        use super::*;
        use crate::wallet::migration::params::COIN;

        #[test]
        fn params_match_upstream() {
            let params = MigrationParams::provisional(ChainType::Mainnet);
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
