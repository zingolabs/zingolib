//! Bucket math and schedule assignment (ZIP 318 Phase 2).
//!
//! Buckets are delimited by *boundaries*, the heights of the chain's ZIP 318
//! anchor grid ([`BucketGrid`]): the multiples of `M` below the NU7
//! activation and the multiples of `3·M` from it. Bucket `i` spans the
//! heights from the `i`-th boundary to the next. A part assigned to bucket
//! `i` transmits while the chain is inside it.
//!
//! A part's *anchor* is a separate bucket from its transmission window. The
//! anchor sits a canonically drawn age below the window, never zero
//! (`ANCHOR_AGE_CAP` bounds how far), so a part always proves against a
//! boundary the chain has already left. Because that boundary is identical
//! for every wallet anchoring there, it carries no per-wallet timing
//! information, and because its window has closed, its ZIP 318 *cohort* (the
//! transfers network-wide sharing it) has had time to accumulate. Anchoring
//! at the window's own boundary, which the chain is still inside, would be
//! age zero: the newest tree state, whose cohort is empty. See ADR 0018.

use pepper_sync::wallet::PoolActivation;
use zcash_protocol::consensus::{BlockHeight, NU7_POW_TARGET_SPACING_RATIO};
use zcash_protocol::zip318::{AnchorGrid, BlockSpacing};

use crate::config::ChainType;
use crate::wallet::error::WalletError;

use super::params::MigrationParams;
use super::parts::{PartId, PartRecord, PartState};

/// Draws the advisory transmission target for one part from the canonical
/// transfer-delay law: the ZIP 318 exponential inter-arrival distribution
/// (mean 144 blocks, capped at 576), offset from the part's window boundary
/// (<https://zips.z.cash/zip-0318#transferscheduling>). The draw can land
/// past the window; the target is advisory (ADR 0017), so a late target
/// only aims the reminder and never gates sendability. Chaining successive
/// transfers (each delay drawn from the previous transfer rather than the
/// boundary) arrives with the scheduling delegation; the divergence ledger
/// records that gap.
fn random_target_in_bucket(
    bucket: u64,
    rng: &mut impl CryptoRng,
    grid: &BucketGrid,
) -> BlockHeight {
    let boundary = grid.boundary_of(bucket);
    // The delay is drawn in reference blocks; the chain's spacing converts
    // it to blocks at the boundary, three per reference block from NU7.
    let offset = SchedulingParams::ZIP_318.transfer_delay().draw(rng);
    grid.spacing().advance(boundary, offset)
}

// The ZIP 318 expiry and anchor-age constants are imported from the
// canonical reference crate, never restated. `EXPIRY_MODULUS` (34 560
// blocks, 30 days) and `EXPIRY_WINDOW` (2x, about 60 days) are
// standardized at
// <https://zips.z.cash/zip-0318#canonicalmigrationtransactionstructure>;
// `ANCHOR_AGE_CAP` (4 boundaries, about twelve hours) at
// <https://zips.z.cash/zip-0318#anchor-heightbucketingandcohorts>.
// ANCHOR_AGE_CAP is deliberately not a `MigrationParams` field: it does
// not feed the consent hash, so adopting it costs no existing consent.
// The `zip318_conformance_tripwires` tests below pin every imported value to the
// ZIP's literal number, so a dependency bump that moved one fails red
// and is adopted deliberately, with a params version bump.
use std::num::NonZeroU32;

use rand::CryptoRng;
use zcash_pool_migration::scheduling::{AnchorBucketInterval, SchedulingParams};

/// The chain's ZIP 318 anchor grid, counted in buckets.
///
/// ZIP 318 states the bucket modulus `M` in reference blocks and scales it
/// by [`NU7_POW_TARGET_SPACING_RATIO`] from NU7 activation, so a boundary
/// interval keeps its length in time: the boundaries are the multiples of
/// `M` below the activation and the multiples of `3·M` from it
/// (<https://zips.z.cash/zip-0318#blockcountsacrossthenu7blockspacingchange>).
/// The canonical [`AnchorGrid`] answers which heights those are; this type
/// numbers them. A bucket index is the ordinal of a boundary on the grid:
/// bucket `i` opens at the `i`-th boundary and closes at the next, so
/// consecutive indices are consecutive windows whatever their length in
/// blocks, and the index arithmetic the schedule, the persisted parts and
/// the consumers' window reports share does not change at the activation.
/// Below it, and on a chain that never activates NU7, the index is
/// `height / M`, as it always was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketGrid {
    chain: ChainType,
    modulus: u32,
    spacing: BlockSpacing,
}

impl BucketGrid {
    /// The grid of `bucket_modulus` on `chain`. The modulus is nonzero by the
    /// [`MigrationParams::bucket_modulus`] invariant, enforced at store read.
    #[must_use]
    pub fn new(bucket_modulus: u32, chain: ChainType) -> Self {
        assert!(
            bucket_modulus != 0,
            "bucket modulus is nonzero by params invariant"
        );
        BucketGrid {
            chain,
            modulus: bucket_modulus,
            spacing: BlockSpacing::for_network(&chain),
        }
    }

    /// The bucket modulus `M`, in reference blocks.
    #[must_use]
    pub fn modulus(&self) -> u32 {
        self.modulus
    }

    /// The chain's block spacing, which converts ZIP 318 durations to blocks.
    #[must_use]
    pub fn spacing(&self) -> BlockSpacing {
        self.spacing
    }

    /// The chain the grid lies on.
    #[must_use]
    pub fn chain(&self) -> ChainType {
        self.chain
    }

    /// The canonical grid these buckets number, for the upstream draws.
    #[must_use]
    pub fn anchor_grid(&self) -> AnchorGrid {
        AnchorGrid::new(
            AnchorBucketInterval::custom(
                NonZeroU32::new(self.modulus)
                    .expect("bucket modulus is nonzero by params invariant"),
            ),
            self.spacing,
        )
    }

    /// The modulus in blocks from NU7 activation.
    fn post_activation_modulus(&self) -> u64 {
        u64::from(self.modulus) * u64::from(NU7_POW_TARGET_SPACING_RATIO)
    }

    /// Where the grid changes: the number of boundaries below the NU7
    /// activation (the multiples of `M` in `[0, activation)`) and the first
    /// boundary at or above it (a multiple of `3·M`). `None` on a chain
    /// that never activates NU7.
    fn activation_split(&self) -> Option<(u64, u64)> {
        let activation = u64::from(u32::from(self.spacing.nu7_activation()?));
        let modulus = u64::from(self.modulus);
        let post_modulus = self.post_activation_modulus();
        Some((
            activation.div_ceil(modulus),
            activation.div_ceil(post_modulus) * post_modulus,
        ))
    }

    /// The bucket containing `height`: the ordinal of the boundary at or
    /// below it.
    #[must_use]
    pub fn index_of(&self, height: BlockHeight) -> u64 {
        let height = u64::from(u32::from(height));
        let modulus = u64::from(self.modulus);
        match self.activation_split() {
            Some((pre_activation, first_post)) if height >= first_post => {
                pre_activation + (height - first_post) / self.post_activation_modulus()
            }
            // Between the activation and the first post-activation boundary
            // the chain is still inside the last pre-activation bucket.
            Some((pre_activation, _)) => (height / modulus).min(pre_activation.saturating_sub(1)),
            None => height / modulus,
        }
    }

    /// The boundary that opens bucket `index`.
    #[must_use]
    pub fn boundary_of(&self, index: u64) -> BlockHeight {
        let boundary = match self.activation_split() {
            Some((pre_activation, first_post)) if index >= pre_activation => {
                first_post + (index - pre_activation) * self.post_activation_modulus()
            }
            _ => index * u64::from(self.modulus),
        };
        BlockHeight::from_u32(
            u32::try_from(boundary).expect("bucket boundaries fit a block height"),
        )
    }

    /// Whether `height` opens a bucket.
    #[must_use]
    pub fn is_boundary(&self, height: BlockHeight) -> bool {
        self.boundary_of(self.index_of(height)) == height
    }

    /// The first bucket whose boundary sits at or above `height`.
    #[must_use]
    pub fn index_at_or_after(&self, height: BlockHeight) -> u64 {
        let index = self.index_of(height);
        if self.boundary_of(index) == height {
            index
        } else {
            index + 1
        }
    }

    /// Renumbers a bucket index of the uniform grid, where an index meant
    /// `height / M` at every height, onto this grid. Below the NU7
    /// activation, and on a chain without one, the index is its own number
    /// and no height is computed. Above the activation the uniform boundary
    /// `index × M` opens a bucket only when it is a multiple of `3·M`; an
    /// index whose boundary opens none is returned as that boundary, so the
    /// caller can choose the bucket at or after it.
    pub(crate) fn renumber_uniform_index(&self, index: u64) -> Result<u64, BlockHeight> {
        let Some((pre_activation, first_post)) = self.activation_split() else {
            return Ok(index);
        };
        if index < pre_activation {
            return Ok(index);
        }
        let boundary = index.saturating_mul(u64::from(self.modulus));
        let post_modulus = self.post_activation_modulus();
        if boundary >= first_post
            && boundary.is_multiple_of(post_modulus)
            && boundary <= u64::from(u32::MAX)
        {
            Ok(pre_activation + (boundary - first_post) / post_modulus)
        } else {
            Err(BlockHeight::from_u32(
                u32::try_from(boundary).unwrap_or(u32::MAX),
            ))
        }
    }

    /// Rough unix time `height` is expected to be mined, extrapolated from
    /// `now_height` at the chain's target block spacing, 75 seconds a block
    /// below NU7 activation and 25 from it (past heights estimate as now).
    fn estimated_unix_at(
        &self,
        height: BlockHeight,
        now_height: BlockHeight,
        now_unix: u64,
    ) -> u64 {
        now_unix
            + zcash_protocol::consensus::target_time_between(&self.chain, now_height, height)
                .as_secs()
    }
}

/// The target height of a part transmitting in `window`, as of `now_height`:
/// the block after the window's boundary, which selects the consensus branch
/// the transaction commits to.
///
/// The last bucket below the NU7 activation runs past it, since the grid's
/// next boundary is the first multiple of `3·M` at or above the activation,
/// so a part transmitting in that window after the chain has crossed the
/// activation would commit to the branch the network no longer validates.
/// Once `now_height` is at or above the activation the target is the
/// activation itself, the lowest height on the branch that validates the
/// transaction; below it the window's boundary stands, and a part signed
/// before the activation and still untransmitted when it passes is rebuilt
/// by reconciliation, as any part whose transaction the network refuses.
#[must_use]
pub fn transmission_target(window: u64, now_height: BlockHeight, grid: &BucketGrid) -> BlockHeight {
    let target = grid.boundary_of(window) + 1;
    match grid.spacing().nu7_activation() {
        Some(activation) if now_height >= activation && target < activation => activation,
        _ => target,
    }
}

/// The canonical ZIP 318 expiry for a transfer scheduled to transmit at
/// `transmission_height`: the most recent multiple of `EXPIRY_MODULUS` at or
/// below it, plus `EXPIRY_WINDOW`, both in reference blocks and so three
/// times as many blocks from NU7 activation. Identical for every transfer
/// scheduled in the same 30-day period, so the committed expiry reveals only
/// that coarse period. See
/// <https://zips.z.cash/zip-0318#canonicalmigrationtransactionstructure>.
pub fn canonical_expiry_height(grid: &BucketGrid, transmission_height: BlockHeight) -> BlockHeight {
    zcash_pool_migration::scheduling::expiry_height(&grid.spacing(), transmission_height)
}

/// The most recent boundary at or below `height`.
#[cfg(test)]
fn previous_boundary(height: BlockHeight, grid: &BucketGrid) -> BlockHeight {
    grid.boundary_of(grid.index_of(height))
}

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
    pub fn lowest_anchor_bucket(&self, grid: &BucketGrid) -> u64 {
        let era = grid.index_of(self.activation.height()) + 1;
        let anchorability = self
            .note_confirmed_at
            .map_or(0, |height| grid.index_at_or_after(height));
        era.max(anchorability)
    }

    /// The earliest transmission window this part may be scheduled into: one
    /// bucket above its lowest legal anchor, because an anchor always sits
    /// at least one bucket below its window.
    ///
    /// This is why the era floor no longer needs restating on the window.
    /// Every legal anchor is above the activation bucket and every window is
    /// above its anchor, so a scheduled window clears the activation by two
    /// buckets without being asked to.
    #[must_use]
    pub fn earliest_window(&self, grid: &BucketGrid) -> u64 {
        self.lowest_anchor_bucket(grid) + 1
    }
}

/// The anchor bucket for a part transmitting in `window`: `window − a` for a
/// canonically drawn age (`Geometric(1/2)`, never zero, capped at
/// `ANCHOR_AGE_CAP`), redrawn until it clears `floor`.
///
/// Delegates to the canonical
/// [`zcash_pool_migration::scheduling::draw_anchor_boundary`]
/// (<https://zips.z.cash/zip-0318#anchor-heightbucketingandcohorts>), mapped
/// into bucket space: the part's window boundary plays the observed chain
/// tip, so the most recent boundary is the window's own and the drawn anchor
/// is `window − a` exactly as the retired local draw computed it. Seeded
/// golden vectors captured from that local implementation pin the
/// equivalence. `None` when the candidate set is empty, which is the
/// caller's signal that `window` is too early for this part rather than a
/// transient condition.
pub fn draw_anchor_bucket(
    window: u64,
    floor: &AnchorFloor,
    rng: &mut impl CryptoRng,
    grid: &BucketGrid,
) -> Option<u64> {
    let anchor = zcash_pool_migration::scheduling::draw_anchor_boundary(
        grid.anchor_grid(),
        floor.activation.height(),
        floor
            .note_confirmed_at
            .unwrap_or_else(|| BlockHeight::from_u32(0)),
        grid.boundary_of(window),
        rng,
    )?;
    Some(grid.index_of(anchor))
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
    grid: &BucketGrid,
) -> u64 {
    (grid.index_of(now_height) + 1).max(floor.earliest_window(grid))
}

/// The first transmission window boundary that can hold an Ironwood part at
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
    grid: &BucketGrid,
) -> BlockHeight {
    grid.boundary_of(grid.index_of(activation.height()) + 2)
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
/// `ANCHOR_AGE_CAP`; because every move lands here, none can.
#[allow(clippy::result_large_err)]
pub fn place(
    part: &mut PartRecord,
    bucket: u64,
    floor: &AnchorFloor,
    rng: &mut impl CryptoRng,
    grid: &BucketGrid,
) -> Result<(), WalletError> {
    let anchor = anchor_for(bucket, floor, rng, grid)?;
    transition_to_bucket(part, bucket)?;
    part.anchor_bucket = Some(anchor);
    part.target_height = Some(random_target_in_bucket(bucket, rng, grid));
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
    rng: &mut impl CryptoRng,
    grid: &BucketGrid,
) -> Result<(), WalletError> {
    let anchor = anchor_for(bucket, floor, rng, grid)?;
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
    rng: &mut impl CryptoRng,
    grid: &BucketGrid,
) -> Result<u64, WalletError> {
    draw_anchor_bucket(bucket, floor, rng, grid).ok_or(WalletError::MigrationNoLegalAnchor {
        window: bucket,
        lowest_anchor: floor.lowest_anchor_bucket(grid),
    })
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

/// Assigns every [`PartState::Bound`] part to a transmission window, draws its
/// anchor below that window, and picks a random transmission target inside it.
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
    grid: &BucketGrid,
    rng: &mut impl CryptoRng,
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
    let first_bucket = ranked
        .iter()
        .fold(grid.index_of(now_height), |earliest, &index| {
            let floor = AnchorFloor::new(activation, note_confirmed_at(&parts[index]));
            earliest.max(floor.earliest_window(grid))
        });
    for (rank, index) in ranked.into_iter().enumerate() {
        let floor = AnchorFloor::new(activation, note_confirmed_at(&parts[index]));
        let bucket = first_bucket + rank as u64 / k;
        place(&mut parts[index], bucket, &floor, rng, grid)?;
    }
    Ok(())
}

/// One future transmission window: what a mobile platform scheduler (for example
/// `BGTaskScheduler` or `WorkManager`) feeds into its earliest-begin request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransmissionWindow {
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

/// Whether a part is due to transmit right now: it still awaits transmission
/// ([`PartState::Assigned`] or [`PartState::Signed`]) and its window is open,
/// meaning it is assigned to `current_bucket`, whose boundary is at or below the tip
/// by definition. The part's random `target_height` no longer gates
/// sendability. It is advisory, exposed only as the reminder hint
/// [`TransmissionWindow::latest_target_unix_time`], so a part is due for the whole
/// open window rather than only from its target onward.
///
/// The single-part rule shared by the transmission loop and the "due now" status
/// read, so a status can never advertise a part a send would decline. It does
/// *not* fold in earlier, missed windows: an overdue part sits in a bucket
/// below `current_bucket` and is catch-up's business.
pub fn part_in_current_bucket(part: &PartRecord, current_bucket: u64) -> bool {
    matches!(part.state, PartState::Assigned | PartState::Signed)
        && part.bucket_index == Some(current_bucket)
}

/// The transmission windows within the next `horizon` buckets, soonest first.
///
/// Pure: reads only the given parts and clock inputs. Parts whose bucket has
/// already passed are reconciliation's business and are not listed here.
pub fn upcoming_windows(
    parts: &[PartRecord],
    now_height: BlockHeight,
    now_unix: u64,
    horizon: u64,
    grid: &BucketGrid,
) -> Vec<TransmissionWindow> {
    let current_bucket = grid.index_of(now_height);
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
            let boundary = grid.boundary_of(bucket);
            // A part without a target (a catch-up shift) is due at the
            // window opening, which every in-window target is at or past.
            let latest_target = latest_target.unwrap_or(boundary + 1);
            TransmissionWindow {
                bucket_index: bucket,
                boundary,
                part_ids,
                window_opens_unix_time: grid.estimated_unix_at(boundary, now_height, now_unix),
                latest_target_unix_time: grid.estimated_unix_at(
                    latest_target,
                    now_height,
                    now_unix,
                ),
            }
        })
        .collect()
}

/// One window of the schedule's timeline: the bucket, its block range, and
/// how far its parts have come. The rendering counterpart to
/// [`TransmissionWindow`], which feeds mobile platform schedulers strictly future
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
    grid: &BucketGrid,
) -> Vec<WindowReport> {
    #[derive(Default)]
    struct Tally {
        parts_total: u32,
        parts_confirmed: u32,
        value_total: u64,
        value_migrated: u64,
    }

    let current_bucket = grid.index_of(now_height);
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
            boundary: grid.boundary_of(bucket),
            close: grid.boundary_of(bucket + 1),
            is_current: bucket == current_bucket,
            parts_total: tally.parts_total,
            parts_confirmed: tally.parts_confirmed,
            value_total: tally.value_total,
            value_migrated: tally.value_migrated,
        })
        .collect()
}

#[cfg(test)]
mod tests;
