//! Phase 1 planning and execution: note preparation, plus the fee model shared
//! with the Phase 2 transfers.
//!
//! [`plan_migration`] turns the wallet's Orchard note values into a
//! [`ScheduledMigrationPlan`]: the note-preparation rounds that resize the notes to
//! exactly `denomination + transfer_fee`, and the denominations of the transfers the
//! wallet will send once splitting completes.

use zcash_protocol::value::Zatoshis;

use super::params::MigrationParams;
use super::quantize::decompose;

/// ZIP-317 marginal fee per logical action, in zatoshis.
/// [`zcash_primitives::transaction::fees::zip317::MARGINAL_FEE`] as a `u64`,
/// mirrored because `Zatoshis` cannot be unwrapped in `const` context. Pinned
/// to the crate constant by test.
pub(crate) const MARGINAL_FEE: u64 = 5_000;

/// One side's per-transaction note budget, derived from the total budget
/// ([`MigrationParams::max_actions_per_split_tx`], ZIP 318's 16-action
/// preparation shape at
/// <https://zips.z.cash/zip-0318#notepreparationtransactions>): a split
/// transaction's spends and outputs together must fit the budget, so each
/// side's chunk is the budget less the one note on the other side (15
/// spends merging into one note, or one note dividing into 15). The
/// immediate migration chunks under the same law: many spends beside its
/// single Ironwood output (see [`super::immediate`]).
pub(crate) fn side_budget(params: &MigrationParams) -> usize {
    params.max_actions_per_split_tx.saturating_sub(1).max(1)
}

/// The Sweep Minimum (ZIP 318 policy): a migration never selects a note worth
/// at most this, and never manufactures an output worth at most this.
/// Provisionally twice the marginal fee: a selected note must return strictly
/// more than double the marginal action cost it adds, not merely break even.
/// Shared by both migration paths through [`MigrationParams::sweep_min`],
/// so the two leave identical residuals.
///
/// A local policy, not a ZIP 318 value: the ZIP leaves whether consuming a
/// small note is economic to ZIP 317's marginal fee
/// (<https://zips.z.cash/zip-0318#amountselectioncanonicalquantization>).
pub(crate) const SWEEP_MIN: u64 = 2 * MARGINAL_FEE;

/// Orchard pads every non-empty bundle to at least this many actions
/// (`orchard::builder`'s `MIN_ACTIONS`, which is private). Pinned to
/// `BundleType::num_actions` by test.
const MIN_BUNDLE_ACTIONS: u64 = 2;

/// The canonical ZIP-317 fee of one transfer. A transfer carries TWO bundles, an
/// Orchard bundle (1 spend, padded to 2 actions) and an Ironwood bundle
/// (1 output, padded to 2 actions), so it pays for 4 logical actions. Every
/// funding note is sized `denomination + transfer_fee` so the transfer balances
/// exactly.
///
/// ZIP 318 says only "the canonical fee (provisionally the ZIP 317 minimum
/// fee)" at
/// <https://zips.z.cash/zip-0318#canonicalmigrationtransactionstructure>,
/// and the reference crate models a 2-source-action, 1-unpadded-destination
/// shape, so the three readings price a transfer at 10 000, 15 000, and (here)
/// 20 000 zatoshis. The value feeds transfer sizing and the consent hash, so
/// aligning it is a ratification decision, not an import.
pub(crate) const CANONICAL_TRANSFER_FEE: u64 = MARGINAL_FEE * 2 * MIN_BUNDLE_ACTIONS;

/// The number of logical actions the builder will produce for a bundle of
/// `n_in` spends and `n_out` outputs at the given bundle version, asked of the
/// orchard crate rather than re-derived: it owns both the minimum-action
/// padding and the rule that a bundle with cross-address transfers disabled
/// (Orchard from NU6.3, but never Ironwood) cannot share an action between a
/// spend and an output.
pub(in crate::wallet::migration) fn bundle_actions(
    version: orchard::bundle::BundleVersion,
    n_in: usize,
    n_out: usize,
) -> usize {
    orchard::builder::BundleType::DEFAULT
        .num_actions(version.default_flags(), n_in, n_out)
        .expect("the default bundle type permits both spends and outputs")
}

/// The ZIP-317 conventional fee of a fully-shielded transaction whose bundles
/// carry the given action counts, from the crate's own fee rule.
///
/// ZIP-317's conventional fee depends on nothing but the action counts, so the
/// rule ignores the network and scheduled broadcast height it is handed and the
/// placeholders below never reach a decision.
pub(in crate::wallet::migration) fn zip317_fee(
    orchard_actions: usize,
    ironwood_actions: usize,
) -> u64 {
    use zcash_primitives::transaction::fees::{FeeRule as _, transparent, zip317};

    zip317::FeeRule::standard()
        .fee_required(
            &zcash_protocol::consensus::MAIN_NETWORK,
            zcash_protocol::consensus::BlockHeight::from_u32(1),
            std::iter::empty::<transparent::InputSize>(),
            std::iter::empty::<usize>(),
            0,
            0,
            orchard_actions,
            ironwood_actions,
        )
        .expect("a shielded action count cannot overflow the fee")
        .into_u64()
}

/// The bundle version of the Orchard bundle a migration transaction spends
/// from, on either side of the NU6.3 boundary. The two differ in whether
/// cross-address transfers are permitted, which changes the action count.
fn orchard_version(post_activation: bool) -> orchard::bundle::BundleVersion {
    if post_activation {
        orchard::bundle::BundleVersion::orchard_v3()
    } else {
        orchard::bundle::BundleVersion::orchard_v2()
    }
}

/// The ZIP-317 conventional fee for an Orchard-only note-preparation
/// transaction with `n_in` spends and `n_out` outputs.
pub(crate) fn note_preparation_fee(n_in: usize, n_out: usize, post_activation: bool) -> u64 {
    zip317_fee(
        bundle_actions(orchard_version(post_activation), n_in, n_out),
        0,
    )
}

/// If a note of value `v` is already a funding note, sized exactly
/// `denomination + transfer_fee`, returns that denomination.
pub(crate) fn funding_denomination(value: u64, params: &MigrationParams) -> Option<u64> {
    value
        .checked_sub(params.transfer_fee)
        .filter(|d| params.denominations.contains(d))
}

/// What one Orchard note is to the migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteClass {
    /// Sized exactly `denomination + transfer_fee`: it funds one transfer as
    /// it is.
    FundingNote {
        /// The denomination the note funds.
        denomination: u64,
    },
    /// Above the sweep minimum but not a funding note: note preparation
    /// spends it.
    NeedsPreparation,
    /// At or below the sweep minimum: never selected, always residual.
    Residual,
}

/// Classifies one note value under `params`. Pure.
pub fn classify_note(value: u64, params: &MigrationParams) -> NoteClass {
    match funding_denomination(value, params) {
        Some(denomination) => NoteClass::FundingNote { denomination },
        None if value > params.sweep_min => NoteClass::NeedsPreparation,
        None => NoteClass::Residual,
    }
}

/// One planned Orchard→Orchard note-preparation transaction. All values are
/// zatoshis. The fee is implied: `sum(inputs) − sum(outputs)`, always at
/// least the ZIP-317 conventional fee for its action count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparationTx {
    /// Values of the notes this transaction spends (bounded by
    /// [`MigrationParams::max_actions_per_split_tx`]).
    pub inputs: Vec<u64>,
    /// Values of the self-notes this transaction creates (bounded by
    /// [`MigrationParams::max_actions_per_split_tx`]).
    pub outputs: Vec<u64>,
}

impl PreparationTx {
    /// The implied fee: `sum(inputs) − sum(outputs)`.
    pub fn fee(&self) -> u64 {
        self.inputs.iter().sum::<u64>() - self.outputs.iter().sum::<u64>()
    }
}

/// The output side of a migration transaction. The input side is always
/// pre-Ironwood (V2) Orchard notes. Only the destination differs.
#[derive(Debug, Clone, Copy)]
pub(in crate::wallet::migration) enum MigrationOutputs<'a> {
    /// Phase 1 note preparation: Orchard→Orchard self-sends, one output per
    /// value. Nothing crosses a pool boundary.
    Orchard(&'a [u64]),
    /// A pool-crossing transfer into Ironwood: exactly one output, no change.
    /// Both the ZIP 318 transfers and the immediate migration are built this way.
    Ironwood(u64),
}

impl MigrationOutputs<'_> {
    /// Total value across the outputs, in zatoshis.
    fn total(&self) -> u64 {
        match self {
            MigrationOutputs::Orchard(values) => values.iter().sum(),
            MigrationOutputs::Ironwood(value) => *value,
        }
    }
}

/// A complete migration plan: note-preparation rounds, then one transfer per
/// denomination. Pure data, nothing is signed or sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledMigrationPlan {
    /// Note-preparation rounds, executed in order. Transactions within a round
    /// are independent. Each round's outputs must confirm before the next
    /// round spends them.
    pub preparation_rounds: Vec<Vec<PreparationTx>>,
    /// The denominations of the transfers to send once splitting completes
    /// (largest first). Includes denominations of notes that are already
    /// funding. Each costs one [`MigrationParams::transfer_fee`] on top.
    pub transfers: Vec<u64>,
    /// Total value residual in dust notes (each at most
    /// [`MigrationParams::sweep_min`]) plus any balance too small to form
    /// even the smallest denomination.
    pub residual: u64,
}

impl ScheduledMigrationPlan {
    /// True when no note preparation is required and the transfers can be sent now.
    pub fn is_prepared(&self) -> bool {
        self.preparation_rounds.is_empty()
    }

    /// Total fees across all note-preparation transactions.
    pub fn preparation_fee(&self) -> u64 {
        self.preparation_rounds
            .iter()
            .flatten()
            .map(PreparationTx::fee)
            .sum()
    }

    /// Total fees the transfers will pay (one transfer fee per denomination).
    pub fn transfers_fee(&self, params: &MigrationParams) -> u64 {
        self.transfers.len() as u64 * params.transfer_fee
    }
}

/// A collision-resistant digest of a plan, recorded at consent time (with
/// [`MigrationParams::params_hash`]) so any later replan requires fresh
/// consent.
pub fn plan_hash(plan: &ScheduledMigrationPlan) -> [u8; 32] {
    let mut hasher = blake2b_simd::Params::new()
        .hash_length(32)
        .personal(b"ZingoMigPlanV0__")
        .to_state();
    hasher.update(&(plan.preparation_rounds.len() as u64).to_le_bytes());
    for round in &plan.preparation_rounds {
        hasher.update(&(round.len() as u64).to_le_bytes());
        for tx in round {
            hasher.update(&(tx.inputs.len() as u64).to_le_bytes());
            for input in &tx.inputs {
                hasher.update(&input.to_le_bytes());
            }
            hasher.update(&(tx.outputs.len() as u64).to_le_bytes());
            for output in &tx.outputs {
                hasher.update(&output.to_le_bytes());
            }
        }
    }
    hasher.update(&(plan.transfers.len() as u64).to_le_bytes());
    for denomination in &plan.transfers {
        hasher.update(&denomination.to_le_bytes());
    }
    hasher.update(&plan.residual.to_le_bytes());
    hasher
        .finalize()
        .as_bytes()
        .try_into()
        .expect("hash length is 32")
}

/// Plans a full migration for a wallet whose spendable Orchard notes have the
/// given values (zatoshis). Deterministic and pure.
///
/// Notes already sized `denomination + transfer_fee` are left untouched (their
/// denominations appear directly in `transfers`), so a migration interrupted and
/// replanned never re-splits finished notes. Everything else worth more than
/// [`MigrationParams::sweep_min`] is split:
///
/// 1. **Reduction**: while more notes remain than one transaction's spend
///    budget, merge groups of at most that many notes into one note each
///    (smallest first).
/// 2. **Sizing**: spend the remaining notes into notes sized exactly
///    `denomination + transfer_fee` — consolidating the smallest inputs into one
///    funding note first whenever the pool plus the targets would exceed one
///    transaction's total budget — where the denominations are the
///    [`decompose`]-ition of what is left after all fees. When the balance
///    needs more target notes than fit one transaction (whale balances),
///    splitting recurses through intermediate notes.
///
/// `post_activation` selects the note-preparation fee model (see
/// `note_preparation_fee`): pass whether the transactions will confirm at or
/// after the NU6.3 activation height.
pub fn plan_migration(
    note_values: &[u64],
    post_activation: bool,
    params: &MigrationParams,
) -> ScheduledMigrationPlan {
    let mut transfers: Vec<u64> = Vec::new();
    let mut pool: Vec<u64> = Vec::new();
    let mut residual: u64 = 0;
    let max_notes = side_budget(params);
    let mut funded_rounds: Vec<Vec<PreparationTx>> = Vec::new();

    let mut note_values = note_values.to_vec();
    note_values.sort_unstable();
    for value in note_values {
        if let Some(denomination) = funding_denomination(value, params) {
            transfers.push(denomination);
        } else if value <= params.sweep_min {
            residual += value;
        } else if let Some(denominations) = funded_denominations(value, post_activation, params) {
            let targets: Vec<u64> = denominations
                .iter()
                .map(|d| d + params.transfer_fee)
                .collect();
            let rounds = prepare_into(vec![value], &targets, None, post_activation, params);
            for (index, round) in rounds.into_iter().enumerate() {
                if funded_rounds.len() <= index {
                    funded_rounds.push(Vec::new());
                }
                funded_rounds[index].extend(round);
            }
            transfers.extend(denominations);
        } else {
            pool.push(value);
        }
    }

    let mut preparation_rounds: Vec<Vec<PreparationTx>> = Vec::new();

    // Reduction: k-ary fold until at most `max_notes` notes remain. Merging
    // the smallest notes first absorbs the most numerous fragments earliest.
    while pool.len() > max_notes {
        pool.sort_unstable();
        let mut round = Vec::new();
        let mut next = Vec::new();
        for group in pool.chunks(max_notes) {
            if group.len() < 2 {
                next.push(group[0]);
                continue;
            }
            let merged =
                group.iter().sum::<u64>() - note_preparation_fee(group.len(), 1, post_activation);
            // Post-activation, a trailing pair of near-floor notes can merge
            // to at most `sweep_min`: a note the residual policy refuses to
            // spend, and one a replan after an interruption would leave as residual.
            // Carry such a group unmerged: each note exceeds `sweep_min` on
            // its own, and the skipped merge fee outweighs the marginal
            // inputs it would have saved. Only a partial group may carry:
            // full groups always merge (at three or more notes their merge
            // always clears `sweep_min`), so every round still shrinks the
            // pool and the loop terminates.
            if group.len() < max_notes && merged <= params.sweep_min {
                next.extend_from_slice(group);
                continue;
            }
            round.push(PreparationTx {
                inputs: group.to_vec(),
                outputs: vec![merged],
            });
            next.push(merged);
        }
        preparation_rounds.push(round);
        pool = next;
    }

    // Sizing: find the denominations fundable from the pooled value after
    // all remaining fees, by fixed point (overhead shrinks the decomposable
    // value, which can only shrink the denomination count, so this
    // terminates).
    if !pool.is_empty() {
        let total: u64 = pool.iter().sum();
        let mut decomposable = total;
        let denominations = loop {
            let candidate = decompose(Zatoshis::const_from_u64(decomposable), params);
            let denominations: Vec<u64> =
                candidate.outputs().iter().map(|z| u64::from(*z)).collect();
            if denominations.is_empty() {
                break denominations;
            }
            let overhead = denominations.len() as u64 * params.transfer_fee
                + prepare_into_fee(
                    pool.len(),
                    denominations.len(),
                    false,
                    post_activation,
                    params,
                );
            let denomination_sum: u64 = denominations.iter().sum();
            if denomination_sum + overhead <= total {
                break denominations;
            }
            let Some(next) = total.checked_sub(overhead) else {
                break Vec::new();
            };
            decomposable = next;
        };

        if denominations.is_empty() {
            // Not even the smallest denomination is fundable: the pooled
            // value is residual. Undo any pointless consolidation rounds.
            residual += total
                + preparation_rounds
                    .iter()
                    .flatten()
                    .map(PreparationTx::fee)
                    .sum::<u64>();
            preparation_rounds.clear();
        } else {
            // The pool never contains funding notes (those were diverted
            // into `transfers` above), so sizing is always required here.
            let targets: Vec<u64> = denominations
                .iter()
                .map(|d| d + params.transfer_fee)
                .collect();
            let funded = targets.iter().sum::<u64>()
                + prepare_into_fee(pool.len(), targets.len(), true, post_activation, params);
            let change = total
                .checked_sub(funded)
                .filter(|change| *change > params.sweep_min);
            residual += change.unwrap_or(0);
            preparation_rounds.extend(prepare_into(
                pool,
                &targets,
                change,
                post_activation,
                params,
            ));
            transfers.extend(denominations);
        }
    }

    for (index, round) in funded_rounds.into_iter().enumerate() {
        if preparation_rounds.len() <= index {
            preparation_rounds.push(Vec::new());
        }
        preparation_rounds[index].extend(round);
    }

    transfers.sort_unstable_by(|a, b| b.cmp(a));

    ScheduledMigrationPlan {
        preparation_rounds,
        transfers,
        residual,
    }
}

/// The input-consolidation group a sizing transaction needs when its spends
/// plus its outputs would exceed the total budget
/// ([`MigrationParams::max_actions_per_split_tx`]): merging `k` inputs into
/// one note removes `k − 1` spends, so the minimal group removes the excess
/// exactly. At least three inputs merge whenever the pool has them: every
/// pooled note strictly exceeds `sweep_min`, and three such notes clear the
/// merge fee with a note the residual policy accepts, where a pair might
/// not (the reduction loop guards the same hazard by carrying pairs).
/// Count-based only, so [`prepare_into_fee`] models it exactly. Callers
/// guarantee `n_in + m` exceeds the budget.
fn merge_group_len(n_in: usize, m: usize, params: &MigrationParams) -> usize {
    (n_in + m + 1 - params.max_actions_per_split_tx)
        .max(3)
        .min(n_in)
}

/// Total ZIP-317 fee of splitting `n_in` notes into `m` target notes,
/// recursing through intermediates when `m` exceeds the per-transaction
/// budget, and consolidating inputs first when spends plus outputs would
/// exceed the total budget (mirroring [`prepare_into`]).
fn prepare_into_fee(
    n_in: usize,
    m: usize,
    with_change: bool,
    post_activation: bool,
    params: &MigrationParams,
) -> u64 {
    let max_notes = side_budget(params);
    let n_out = m + usize::from(with_change);
    if n_out <= max_notes {
        if n_in + n_out > params.max_actions_per_split_tx {
            let group_len = merge_group_len(n_in, n_out, params);
            note_preparation_fee(group_len, 1, post_activation)
                + note_preparation_fee(n_in - group_len + 1, n_out, post_activation)
        } else {
            note_preparation_fee(n_in, n_out, post_activation)
        }
    } else {
        // One future transaction per chunk of `max_notes` targets, funded by
        // an intermediate note each. The intermediates are themselves split.
        let chunks = m.div_ceil(max_notes);
        let mut total = 0;
        let mut remaining = m;
        for _ in 0..chunks {
            let len = remaining.min(max_notes);
            total += note_preparation_fee(1, len, post_activation);
            remaining -= len;
        }
        total + prepare_into_fee(n_in, chunks, with_change, post_activation, params)
    }
}

fn funded_denominations(
    value: u64,
    post_activation: bool,
    params: &MigrationParams,
) -> Option<Vec<u64>> {
    let upper = decompose(Zatoshis::const_from_u64(value), params)
        .outputs()
        .len();
    (1..=upper).find_map(|count| {
        let overhead = count as u64 * params.transfer_fee
            + prepare_into_fee(1, count, false, post_activation, params);
        let candidate = decompose(
            Zatoshis::const_from_u64(value.checked_sub(overhead)?),
            params,
        );
        (candidate.outputs().len() == count && u64::from(candidate.remainder()) == 0)
            .then(|| candidate.outputs().iter().map(|z| u64::from(*z)).collect())
    })
}

/// Materializes the note-preparation rounds that turn `inputs` into exactly
/// `targets`. Mirrors [`prepare_into_fee`]. `change` is the residual note,
/// created by the transaction that spends `inputs`. That transaction's fee
/// absorbs any slack too small to form a note.
///
/// The budget is a total: a transaction's spends and outputs together must
/// fit [`MigrationParams::max_actions_per_split_tx`] (ZIP 318's 16-action
/// preparation shape). When the pool plus the targets exceed it, the
/// smallest inputs consolidate into one funding note first, in a round of
/// their own, and the sizing transaction spends the consolidated note
/// beside the survivors.
fn prepare_into(
    mut inputs: Vec<u64>,
    targets: &[u64],
    change: Option<u64>,
    post_activation: bool,
    params: &MigrationParams,
) -> Vec<Vec<PreparationTx>> {
    let max_notes = side_budget(params);
    let n_out = targets.len() + usize::from(change.is_some());
    if n_out <= max_notes {
        let mut rounds = Vec::new();
        if inputs.len() + n_out > params.max_actions_per_split_tx {
            let group_len = merge_group_len(inputs.len(), n_out, params);
            inputs.sort_unstable();
            let group: Vec<u64> = inputs.drain(..group_len).collect();
            let merged =
                group.iter().sum::<u64>() - note_preparation_fee(group.len(), 1, post_activation);
            rounds.push(vec![PreparationTx {
                inputs: group,
                outputs: vec![merged],
            }]);
            inputs.push(merged);
        }
        let mut outputs = targets.to_vec();
        outputs.extend(change);
        rounds.push(vec![PreparationTx { inputs, outputs }]);
        return rounds;
    }
    let chunks: Vec<&[u64]> = targets.chunks(max_notes).collect();
    let intermediates: Vec<u64> = chunks
        .iter()
        .map(|chunk| {
            chunk.iter().sum::<u64>() + note_preparation_fee(1, chunk.len(), post_activation)
        })
        .collect();
    let mut rounds = prepare_into(inputs, &intermediates, change, post_activation, params);
    rounds.push(
        intermediates
            .iter()
            .zip(&chunks)
            .map(|(&intermediate, chunk)| PreparationTx {
                inputs: vec![intermediate],
                outputs: chunk.to_vec(),
            })
            .collect(),
    );
    rounds
}

impl crate::wallet::LightWallet {
    /// The values (zatoshis) of the account's spendable pre-Ironwood (V2)
    /// Orchard notes, the input to [`plan_migration`].
    ///
    /// Errors with [`WalletError::SyncIncomplete`] when the spend horizon
    /// withholds every V2 note the anchor would otherwise offer.
    ///
    /// [`WalletError::SyncIncomplete`]: crate::wallet::error::WalletError::SyncIncomplete
    #[allow(clippy::result_large_err)]
    pub(crate) fn migration_note_values(
        &self,
        account: zip32::AccountId,
    ) -> Result<Vec<u64>, crate::wallet::error::WalletError> {
        let (_, anchor_height) = self
            .get_migration_heights()?
            .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
        let known_unspent = self.anchored_v2_known_unspent(account, anchor_height)?;
        let not_known_spent = self.anchored_v2_not_known_spent(account, anchor_height)?;
        // The two sets differ only in the spend-confirmation filter, so
        // an empty first with a non-empty second means spend detection
        // is incomplete below the recorded tip for every note.
        if known_unspent.is_empty() && !not_known_spent.is_empty() {
            return Err(crate::wallet::error::WalletError::SyncIncomplete);
        }
        Ok(known_unspent)
    }

    /// The values of the account's anchored V2 notes that sync has proven
    /// unspent: every block in each note's possible spend window is scanned
    /// and no spend appeared. Only this set is safe to plan spends from.
    #[allow(clippy::result_large_err)]
    fn anchored_v2_known_unspent(
        &self,
        account: zip32::AccountId,
        anchor_height: zcash_protocol::consensus::BlockHeight,
    ) -> Result<Vec<u64>, crate::wallet::error::WalletError> {
        self.anchored_v2_values(account, anchor_height, false)
    }

    /// The values of the account's anchored V2 notes with no recorded
    /// spending transaction: the known-unspent set plus the notes whose
    /// spend status sync cannot yet vouch for. A superset of
    /// [`Self::anchored_v2_known_unspent`]; the difference is exactly the
    /// notes awaiting spend detection.
    #[allow(clippy::result_large_err)]
    fn anchored_v2_not_known_spent(
        &self,
        account: zip32::AccountId,
        anchor_height: zcash_protocol::consensus::BlockHeight,
    ) -> Result<Vec<u64>, crate::wallet::error::WalletError> {
        self.anchored_v2_values(account, anchor_height, true)
    }

    /// The shared query behind the named pair above; callers reach it
    /// through them so the spend-certainty mode always has a name.
    #[allow(clippy::result_large_err)]
    fn anchored_v2_values(
        &self,
        account: zip32::AccountId,
        anchor_height: zcash_protocol::consensus::BlockHeight,
        include_potentially_spent: bool,
    ) -> Result<Vec<u64>, crate::wallet::error::WalletError> {
        use pepper_sync::wallet::{NoteInterface as _, OutputInterface as _};

        Ok(self
            .spendable_notes::<pepper_sync::wallet::OrchardNote>(
                anchor_height,
                &[],
                account,
                include_potentially_spent,
            )?
            .into_iter()
            .filter(|note| note.note().version() == orchard::note::NoteVersion::V2)
            .map(|note| note.value())
            .collect())
    }

    /// The values of the account's live pre-Ironwood (V2) Orchard notes:
    /// unspent, whether anchored, freshly confirmed, or still pending. The
    /// Phase 1 status projection plans over this set, so a note-preparation
    /// round in flight reads as its pending outputs instead of as vanished
    /// value, which rules the anchored [`Self::migration_note_values`] out
    /// for status use. Failed
    /// transactions' outputs never existed on chain and are excluded.
    pub(crate) fn live_v2_note_values(&self, account: zip32::AccountId) -> Vec<u64> {
        use pepper_sync::wallet::{KeyIdInterface as _, NoteInterface as _, OutputInterface as _};

        self.wallet_transactions
            .values()
            .filter(|transaction| !transaction.status().is_failed())
            .flat_map(|transaction| {
                pepper_sync::wallet::OrchardNote::transaction_outputs(transaction)
            })
            .filter(|note| {
                note.key_id().account_id() == account
                    && note.note().version() == orchard::note::NoteVersion::V2
                    && note.spending_transaction().is_none()
            })
            .map(|note| note.value())
            .collect()
    }

    /// Builds, proves, signs and records one planned note-preparation
    /// transaction (Orchard→Orchard self-send). Returns its txid. Transmission
    /// is the caller's step.
    #[allow(clippy::result_large_err)]
    pub(crate) fn build_preparation_transaction(
        &mut self,
        account: zip32::AccountId,
        planned: &PreparationTx,
    ) -> Result<zcash_primitives::transaction::TxId, crate::wallet::error::WalletError> {
        self.build_migration_transaction_inner(
            account,
            &planned.inputs,
            MigrationOutputs::Orchard(&planned.outputs),
        )
    }

    pub(crate) fn get_migration_heights(
        &self,
    ) -> Result<
        Option<(
            zcash_protocol::consensus::BlockHeight,
            zcash_protocol::consensus::BlockHeight,
        )>,
        crate::wallet::error::WalletError,
    > {
        use zcash_client_backend::data_api::WalletRead as _;
        Ok(self
            .get_target_and_anchor_heights(self.wallet_settings.min_confirmations)
            .expect("infallible")
            .map(|(target, anchor)| (target.into(), anchor)))
    }

    #[allow(clippy::result_large_err)]
    pub(in crate::wallet::migration) fn build_migration_transaction_inner(
        &mut self,
        account: zip32::AccountId,
        input_values: &[u64],
        outputs: MigrationOutputs<'_>,
    ) -> Result<zcash_primitives::transaction::TxId, crate::wallet::error::WalletError> {
        use pepper_sync::wallet::{NoteInterface as _, OutputInterface as _};
        use zcash_primitives::transaction::builder::{BuildConfig, Builder, BundlePadding};
        use zcash_protocol::memo::MemoBytes;

        use crate::wallet::error::WalletError;

        let (target_height, anchor_height) = self
            .get_migration_heights()?
            .ok_or(WalletError::NoSyncData)?;

        // Pick one spendable V2 note per planned input value (distinct notes
        // for repeated values), copying out what the builder needs so the
        // wallet borrow ends before witnessing.
        let (notes, positions): (Vec<orchard::Note>, Vec<incrementalmerkletree::Position>) = {
            let mut candidates: Vec<&pepper_sync::wallet::OrchardNote> = self
                .spendable_notes::<pepper_sync::wallet::OrchardNote>(
                    anchor_height,
                    &[],
                    account,
                    false,
                )?
                .into_iter()
                .filter(|note| note.note().version() == orchard::note::NoteVersion::V2)
                .collect();

            let mut notes = Vec::with_capacity(input_values.len());
            let mut positions = Vec::with_capacity(input_values.len());
            for &value in input_values {
                let index = candidates
                    .iter()
                    .position(|note| note.value() == value)
                    .ok_or(WalletError::MigrationNoteNotFound(value))?;
                let selected = candidates.swap_remove(index);
                notes.push(*selected.note());
                positions.push(selected.position().expect("spendable notes have positions"));
            }
            (notes, positions)
        };

        // Anchor and witnesses from the wallet's Orchard shard tree.
        let orchard_anchor: orchard::Anchor = self
            .shard_trees
            .orchard
            .root_at_checkpoint_id(&anchor_height)?
            .ok_or(WalletError::CheckpointNotFound {
                shielded_protocol: zcash_protocol::ShieldedPool::Orchard,
                height: anchor_height,
            })?
            .into();
        let merkle_paths = positions
            .into_iter()
            .map(|position| {
                self.shard_trees
                    .orchard
                    .witness_at_checkpoint_id_caching(position, &anchor_height)?
                    .ok_or(WalletError::CheckpointNotFound {
                        shielded_protocol: zcash_protocol::ShieldedPool::Orchard,
                        height: anchor_height,
                    })
                    .map(orchard::tree::MerklePath::from)
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Keys: everything is wallet-internal.
        let usk: zcash_keys::keys::UnifiedSpendingKey = self
            .unified_key_store
            .get(&account)
            .ok_or(crate::wallet::error::KeyError::NoAccountKeys)?
            .try_into()?;
        let orchard_fvk = orchard::keys::FullViewingKey::from(usk.orchard());
        let recipient = orchard_fvk.address_at(0u32, zip32::Scope::Internal);
        let internal_ovk = orchard_fvk.to_ovk(zip32::Scope::Internal);

        // The plan fixes the exact fee (inputs − outputs), which is always at
        // least the ZIP-317 conventional fee and may exceed it where residue
        // was folded in. A non-standard fixed fee rule pins the builder to it.
        let input_sum: u64 = input_values.iter().sum();
        let output_sum: u64 = outputs.total();
        let fee_rule = zcash_primitives::transaction::fees::fixed::FeeRule::non_standard(
            Zatoshis::from_u64(input_sum.checked_sub(output_sum).ok_or(
                WalletError::MigrationDeviation(format!(
                    "planned outputs {output_sum} exceed planned inputs {input_sum}"
                )),
            )?)?,
        );

        let build_config = BuildConfig::Standard {
            sapling_anchor: None,
            orchard_anchor: Some(orchard_anchor),
            // The Ironwood bundle holds outputs only, never spends, so its
            // anchor is the empty tree. `None` would leave no bundle to put
            // the output in.
            ironwood_anchor: match outputs {
                MigrationOutputs::Orchard(_) => None,
                MigrationOutputs::Ironwood(_) => Some(orchard::Anchor::empty_tree()),
            },
            orchard_padding: BundlePadding::DEFAULT,
            ironwood_padding: BundlePadding::DEFAULT,
        };
        let mut builder = Builder::new(self.chain_type, target_height, build_config);
        for (note, merkle_path) in notes.into_iter().zip(merkle_paths) {
            builder
                .add_orchard_spend::<std::convert::Infallible>(
                    orchard_fvk.clone(),
                    note,
                    merkle_path,
                )
                .map_err(|e| WalletError::MigrationBuild(format!("{e}")))?;
        }
        match outputs {
            MigrationOutputs::Orchard(output_values) => {
                for &value in output_values {
                    // Post-NU6.3, cross-address Orchard transfers are banned; only
                    // wallet-controlled change is allowed. Note-splitting outputs are
                    // always wallet-internal self-sends, so add_orchard_change_output
                    // is correct in both eras.
                    builder
                        .add_orchard_change_output::<std::convert::Infallible>(
                            orchard_fvk.clone(),
                            Some(internal_ovk.clone()),
                            recipient,
                            Zatoshis::from_u64(value)?,
                            MemoBytes::empty(),
                        )
                        .map_err(|e| WalletError::MigrationBuild(format!("{e}")))?;
                }
            }
            MigrationOutputs::Ironwood(value) => {
                // Below NU6.3 the builder has no Ironwood bundle to put this
                // in, and says so with `Error::IronwoodBuilderNotAvailable`.
                // The output is never silently dropped.
                builder
                    .add_ironwood_output::<std::convert::Infallible>(
                        Some(internal_ovk.clone()),
                        recipient,
                        Zatoshis::from_u64(value)?,
                        MemoBytes::empty(),
                    )
                    .map_err(|e| WalletError::MigrationBuild(format!("{e}")))?;
            }
        }

        let (sapling_output, sapling_spend) = crate::wallet::utils::read_sapling_params();
        let sapling_prover =
            zcash_proofs::prover::LocalTxProver::from_bytes(&sapling_spend, &sapling_output);
        let build_result = builder
            .build(
                &zcash_transparent::builder::TransparentSigningSet::new(),
                &[usk.sapling().clone()],
                &[usk.orchard().into()],
                rand::rngs::OsRng,
                &sapling_prover,
                &sapling_prover,
                &fee_rule,
            )
            .map_err(|e| WalletError::MigrationBuild(format!("{e:?}")))?;

        let mut raw_tx = Vec::new();
        build_result
            .transaction()
            .write(&mut raw_tx)
            .map_err(WalletError::TransactionWrite)?;
        self.record_migration_transaction(&raw_tx, target_height)
    }

    /// Records a calculated migration transaction in the wallet (marks the
    /// spent notes, stores the tx for `transmit_transactions`), mirroring
    /// `store_transactions_to_be_sent`.
    #[allow(clippy::result_large_err)]
    pub(in crate::wallet::migration) fn record_migration_transaction(
        &mut self,
        raw_tx: &[u8],
        target_height: zcash_protocol::consensus::BlockHeight,
    ) -> Result<zcash_primitives::transaction::TxId, crate::wallet::error::WalletError> {
        use crate::wallet::error::WalletError;

        let transaction = zcash_primitives::transaction::Transaction::read(
            raw_tx,
            zcash_protocol::consensus::BranchId::for_height(&self.chain_type, target_height),
        )
        .map_err(WalletError::TransactionRead)?;
        let txid = transaction.txid();

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock after unix epoch")
            .as_secs() as u32;
        let chain_type = self.chain_type;
        let ufvks = pepper_sync::wallet::traits::SyncWallet::get_unified_full_viewing_keys(self)?;
        match pepper_sync::scan_pending_transaction(
            &chain_type,
            &ufvks,
            self,
            transaction,
            zingo_status::confirmation_status::ConfirmationStatus::Calculated(target_height),
            timestamp,
        ) {
            Ok(()) => (),
            Err(pepper_sync::error::SyncError::ScanError(e)) => return Err(e.into()),
            Err(pepper_sync::error::SyncError::WalletError(e)) => return Err(e),
            Err(_) => {
                panic!("`scan_pending_transaction` should only return scan or wallet errors")
            }
        }
        self.save_required = true;

        Ok(txid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ChainType;
    use proptest::prelude::*;
    use zcash_protocol::value::COIN;

    fn params() -> MigrationParams {
        MigrationParams::provisional(ChainType::Mainnet)
    }

    /// `MARGINAL_FEE` and `MIN_BUNDLE_ACTIONS` are the only numbers this module
    /// still restates, and each has a reason: `Zatoshis` cannot be unwrapped in
    /// `const` context, and orchard's `MIN_ACTIONS` is private. Pin both to
    /// their sources.
    #[test]
    fn mirrored_constants_match_the_crates() {
        use orchard::bundle::BundleVersion;
        use zcash_primitives::transaction::fees::zip317;

        assert_eq!(MARGINAL_FEE, zip317::MARGINAL_FEE.into_u64());
        // Every non-empty bundle is padded to the minimum, whatever it holds.
        assert_eq!(
            bundle_actions(BundleVersion::orchard_v3(), 1, 0) as u64,
            MIN_BUNDLE_ACTIONS
        );
        assert_eq!(
            bundle_actions(BundleVersion::ironwood_v3(), 0, 1) as u64,
            MIN_BUNDLE_ACTIONS
        );
    }

    /// Economic pin for the residual policy: `sweep_min` may carry any
    /// safety factor above the ZIP-317 marginal fee, but must never sit
    /// below it: below that line a selected note could cost more to spend
    /// than the value it provides.
    #[test]
    fn sweep_min_covers_the_marginal_input_cost() {
        assert!(params().sweep_min >= MARGINAL_FEE);
    }

    /// The action-count rule the fee model rests on: an Orchard bundle from
    /// NU6.3 disables cross-address transfers, so a spend and an output no
    /// longer share an action. The crates own that rule and the split fee
    /// follows it by asking them. These figures pin the answer.
    #[test]
    fn note_preparation_fee_follows_the_activation_boundary() {
        assert_eq!(note_preparation_fee(1, 1, false), 10_000);
        assert_eq!(note_preparation_fee(32, 1, false), 160_000);
        assert_eq!(note_preparation_fee(32, 1, true), 165_000);
    }

    #[test]
    fn transfer_fee_matches_the_crates() {
        use orchard::bundle::BundleVersion;

        // A transfer has an Orchard bundle (1 spend, cross-address disabled
        // post-NU6.3) and an Ironwood bundle (1 output, cross-address
        // enabled). The fee covers the sum of both bundles' padded actions.
        assert_eq!(
            CANONICAL_TRANSFER_FEE,
            zip317_fee(
                bundle_actions(BundleVersion::orchard_v3(), 1, 0),
                bundle_actions(BundleVersion::ironwood_v3(), 0, 1),
            )
        );
        assert_eq!(CANONICAL_TRANSFER_FEE, 20_000);
        assert_eq!(params().transfer_fee, CANONICAL_TRANSFER_FEE);
    }

    #[test]
    fn funding_denomination_requires_exact_sizing() {
        let params = params();
        let fee = params.transfer_fee;
        assert_eq!(
            funding_denomination(1_000_000 + fee, &params),
            Some(1_000_000)
        );
        assert_eq!(
            funding_denomination(100 * COIN + fee, &params),
            Some(100 * COIN)
        );
        assert_eq!(funding_denomination(1_000_000, &params), None); // fee missing
        assert_eq!(funding_denomination(1_000_000 + fee + 1, &params), None);
        assert_eq!(funding_denomination(0, &params), None);
    }

    #[test]
    fn plan_hash_binds_the_plan_content() {
        let plan = plan_migration(&[123_456_789], true, &params());
        let hash = plan_hash(&plan);
        assert_eq!(hash, plan_hash(&plan.clone()));

        let mut altered = plan.clone();
        altered.residual += 1;
        assert_ne!(hash, plan_hash(&altered));
    }

    /// Runs [`assert_plan_executes_in_era`] in both fee eras and returns the
    /// post-activation plan.
    fn assert_plan_executes(notes: &[u64], params: &MigrationParams) -> ScheduledMigrationPlan {
        assert_plan_executes_in_era(notes, false, params);
        assert_plan_executes_in_era(notes, true, params)
    }

    /// Simulates plan execution: every round's inputs must be present in the
    /// wallet's note multiset at that point, and the final multiset must be
    /// exactly the funding notes. Returns the plan for further checks.
    fn assert_plan_executes_in_era(
        notes: &[u64],
        post_activation: bool,
        params: &MigrationParams,
    ) -> ScheduledMigrationPlan {
        use std::collections::HashMap;
        let max_notes = side_budget(params);
        let plan = plan_migration(notes, post_activation, params);

        let mut available: HashMap<u64, i64> = HashMap::new();
        let mut unselected: u64 = 0;
        for &note in notes {
            if funding_denomination(note, params).is_none() && note <= params.sweep_min {
                unselected += note;
                continue; // residual, never spent
            }
            *available.entry(note).or_insert(0) += 1;
        }

        for round in &plan.preparation_rounds {
            for tx in round {
                assert!((2..=max_notes).contains(&tx.inputs.len()) || tx.inputs.len() == 1);
                assert!(tx.inputs.len() <= max_notes, "too many inputs");
                assert!(!tx.outputs.is_empty() && tx.outputs.len() <= max_notes);
                assert!(
                    tx.inputs.len() + tx.outputs.len() <= params.max_actions_per_split_tx,
                    "spends plus outputs ({} + {}) exceed the total budget of {}",
                    tx.inputs.len(),
                    tx.outputs.len(),
                    params.max_actions_per_split_tx
                );
                let min_fee =
                    note_preparation_fee(tx.inputs.len(), tx.outputs.len(), post_activation);
                assert!(tx.fee() >= min_fee, "fee below ZIP-317 conventional");
                for input in &tx.inputs {
                    let count = available.entry(*input).or_insert(0);
                    assert!(*count > 0, "spends a note the wallet does not hold");
                    *count -= 1;
                }
            }
            // Outputs only become spendable after the whole round confirms.
            for tx in round {
                for output in &tx.outputs {
                    assert!(*output > params.sweep_min, "note preparation created dust");
                    *available.entry(*output).or_insert(0) += 1;
                }
            }
        }

        // What remains must be the transfers, each note d + transfer_fee, plus the
        // residual notes.
        for &denomination in &plan.transfers {
            let count = available
                .entry(denomination + params.transfer_fee)
                .or_insert(0);
            assert!(*count > 0, "a planned transfer has no funding note");
            *count -= 1;
        }
        let leftover: u64 = available
            .iter()
            .map(|(value, count)| {
                assert!(
                    funding_denomination(*value, params).is_none() || *count == 0,
                    "a funding note is not booked as a transfer"
                );
                value * u64::try_from(*count).expect("counts never go negative")
            })
            .sum();
        assert_eq!(
            leftover + unselected,
            plan.residual,
            "post-splitting residual notes ≠ plan residual"
        );

        plan
    }

    /// Asserts the selection invariant: no planned transaction spends a note
    /// worth at most `sweep_min`, whether it is a wallet note or an
    /// intermediate created by an earlier round.
    fn assert_planned_inputs_exceed_sweep_min(
        plan: &ScheduledMigrationPlan,
        params: &MigrationParams,
    ) {
        for (round, transactions) in plan.preparation_rounds.iter().enumerate() {
            for tx in transactions {
                for &input in &tx.inputs {
                    assert!(
                        input > params.sweep_min,
                        "round {round} spends a {input}-zatoshi note, \
                         at or below sweep_min ({})",
                        params.sweep_min
                    );
                }
            }
        }
    }

    /// ZIP 318's preparation shape is a total budget: a transaction's
    /// spends and outputs together must fit
    /// [`MigrationParams::max_actions_per_split_tx`]. A pool small enough
    /// to skip reduction (at most one spend side's worth of notes) that
    /// funds several denominations used to emit one sizing transaction
    /// carrying the whole pool beside every target — up to 30 actions
    /// post-NU6.3. The planner now consolidates the smallest inputs into
    /// one funding note first, and every planned transaction fits the
    /// budget (the executes-in-era validator asserts the bound for every
    /// test in this module).
    #[test]
    fn sizing_respects_the_total_action_budget() {
        let params = params();
        // Fifteen unquantized ~11-ZEC notes: reduction is skipped, and the
        // ~165-ZEC total decomposes into several denominations, so the
        // sizing round mixes many spends with many outputs.
        let notes = vec![1_100_000_007u64; 15];
        for post_activation in [false, true] {
            let plan = assert_plan_executes_in_era(&notes, post_activation, &params);
            assert!(
                plan.transfers.len() >= 2,
                "the shape under test needs several targets, got {:?}",
                plan.transfers
            );
            // The consolidation prepends its own round, and the sizing
            // transaction still mixes multiple spends with multiple
            // outputs — the shape that used to bust the budget.
            assert!(
                plan.preparation_rounds.len() >= 2,
                "expected consolidation + sizing"
            );
            assert!(
                plan.preparation_rounds
                    .iter()
                    .flatten()
                    .any(|tx| tx.inputs.len() >= 2 && tx.outputs.len() >= 2),
                "the plan under test must exercise a mixed spend/output transaction"
            );
        }
    }

    /// The selection boundary is strict: a note worth exactly `sweep_min` is
    /// residual, one zatoshi more is selected. Fails whenever the planner's
    /// residual filter admits a note at or below the threshold.
    #[test]
    fn notes_at_or_below_sweep_min_are_never_selected() {
        let params = params();
        let dust = [1, MARGINAL_FEE, params.sweep_min - 1, params.sweep_min];
        let mut notes = dust.to_vec();
        notes.extend([params.sweep_min + 1, 2_000_000, 3_000_000]);
        for post_activation in [false, true] {
            let plan = plan_migration(&notes, post_activation, &params);
            assert_planned_inputs_exceed_sweep_min(&plan, &params);
            let change: u64 = plan
                .preparation_rounds
                .last()
                .expect("the pool is split")
                .iter()
                .flat_map(|tx| tx.outputs.iter())
                .filter(|output| funding_denomination(**output, &params).is_none())
                .sum();
            assert_eq!(plan.residual, dust.iter().sum::<u64>() + change);
            // The note one zatoshi above the boundary is selected.
            assert!(
                plan.preparation_rounds[0]
                    .iter()
                    .any(|tx| tx.inputs.contains(&(params.sweep_min + 1)))
            );
        }
    }

    /// Guards the one shape where consolidation could violate the selection
    /// invariant. Post-activation, a trailing group of exactly two notes,
    /// each at most 12_500 zatoshis, would merge to `sum - 15_000` (a pair
    /// of 12_000s to 9_000): at or below `sweep_min`. The sizing round
    /// would then spend a note the residual policy refuses, and a replan
    /// after an interruption would leave it as residual, abandoning the pair's value
    /// after fees were paid to create it. The planner instead carries such
    /// a group into the next pool unmerged. Every other shape merges clear
    /// of the threshold: a group of three yields at least 10_003, and a
    /// pre-activation pair at least 10_002, because spends and outputs
    /// share actions there.
    #[test]
    fn consolidation_never_creates_a_sub_sweep_min_intermediate() {
        let params = params();
        // Twelve full consolidation groups (enough to fund a 0.01-ZEC transfer
        // after the merge fees of the 16-action shape) plus a trailing pair
        // that would merge to 9_000 zatoshis, below the sweep minimum.
        let notes = vec![12_000u64; 12 * side_budget(&params) + 2];
        for post_activation in [false, true] {
            let plan = assert_plan_executes_in_era(&notes, post_activation, &params);
            assert_planned_inputs_exceed_sweep_min(&plan, &params);
        }

        // Post-activation the pair is carried unmerged: the consolidation round
        // merges only the full groups, and the sizing round spends the pair
        // directly.
        let plan = plan_migration(&notes, true, &params);
        assert_eq!(plan.preparation_rounds[0].len(), 12);
        assert_eq!(
            plan.preparation_rounds[1][0]
                .inputs
                .iter()
                .filter(|&&value| value == 12_000)
                .count(),
            2
        );
    }

    #[test]
    fn already_split_wallet_skips_splitting() {
        // Notes already sized denomination + fee: no splitting needed.
        let params = params();
        let fee = params.transfer_fee;
        let notes = vec![COIN + fee, COIN / 10 + fee, 1_000_000 + fee];
        let plan = assert_plan_executes(&notes, &params);
        assert!(plan.is_prepared());
        assert_eq!(plan.transfers, vec![COIN, COIN / 10, 1_000_000]);
        assert_eq!(plan.residual, 0);
    }

    #[test]
    fn dust_only_wallet_leaves_everything_residual() {
        let params = params();
        let notes = vec![5_000, params.sweep_min, 1];
        let plan = assert_plan_executes(&notes, &params);
        assert!(plan.preparation_rounds.is_empty());
        assert!(plan.transfers.is_empty());
        assert_eq!(plan.residual, 5_000 + params.sweep_min + 1);
    }

    #[test]
    fn sub_denomination_pool_is_residual_without_splitting() {
        // Two sweepable notes that cannot fund even the smallest target.
        let notes = vec![20_000, 30_000];
        let plan = assert_plan_executes(&notes, &params());
        assert!(plan.preparation_rounds.is_empty());
        assert!(plan.transfers.is_empty());
        assert_eq!(plan.residual, 50_000);
    }

    #[test]
    fn single_note_is_prepared_in_one_transaction() {
        let params = params();
        let notes = vec![123_456_789]; // 1.23456789 ZEC
        let plan = assert_plan_executes(&notes, &params);
        assert_eq!(plan.preparation_rounds.len(), 1);
        assert_eq!(plan.preparation_rounds[0].len(), 1);
        let tx = &plan.preparation_rounds[0][0];
        assert_eq!(tx.inputs, vec![123_456_789]);
        // Conservation: everything is either a denomination, a transfer fee, or
        // a splitting fee.
        let denomination_sum: u64 = plan.transfers.iter().sum();
        assert_eq!(
            123_456_789,
            denomination_sum + plan.transfers_fee(&params) + plan.preparation_fee() + plan.residual
        );
    }

    #[test]
    fn fragmented_wallet_consolidates_in_rounds() {
        // 500 notes of 0.0015 ZEC: consolidation (500 → 16 at a budget of 32)
        // then sizing.
        let params = params();
        let notes = vec![150_000u64; 500];
        let plan = assert_plan_executes(&notes, &params);
        assert!(
            plan.preparation_rounds.len() >= 2,
            "expected consolidation + sizing rounds"
        );
        // First round: one merge per full-or-partial chunk of the spend budget.
        let max_notes = side_budget(&params);
        assert_eq!(
            plan.preparation_rounds[0].len(),
            500usize.div_ceil(max_notes)
        );
        for tx in &plan.preparation_rounds[0] {
            assert!(tx.inputs.len() <= max_notes);
            assert_eq!(tx.outputs.len(), 1);
        }
        assert!(!plan.transfers.is_empty());
    }

    #[test]
    fn whale_balance_splits_through_intermediates() {
        // 500 000 ZEC in one note: ~50 denominations of 10 000 ZEC.
        let params = params();
        let notes = vec![500_000 * COIN];
        let plan = assert_plan_executes(&notes, &params);
        assert!(plan.transfers.len() >= 49);
        for round in &plan.preparation_rounds {
            for tx in round {
                assert!(tx.outputs.len() <= side_budget(&params));
            }
        }
    }

    #[test]
    fn very_large_balance_recurses_splitting() {
        // 12,000,000 ZEC: ~1,200 targets of 10 000 ZEC → recursive splitting
        // through intermediate levels, within the budget throughout.
        let params = params();
        let notes = vec![12_000_000 * COIN];
        let plan = assert_plan_executes(&notes, &params);
        assert!(plan.transfers.len() > params.max_actions_per_split_tx);
        assert!(plan.preparation_rounds.len() >= 3);
    }

    #[test]
    fn mixed_wallet_leaves_ready_notes_untouched() {
        // One funding note + fragments: the ready note is never spent by
        // note preparation (verified by assert_plan_executes) and its
        // denomination appears in the transfers.
        let params = params();
        let mut notes = vec![10 * COIN + params.transfer_fee];
        notes.extend(vec![200_000u64; 50]);
        let plan = assert_plan_executes(&notes, &params);
        assert!(plan.transfers.contains(&(10 * COIN)));
    }

    proptest! {
        // Fundamental invariant: the plan executes (round linkage holds, no
        // dust created, ZIP-317 fees respected, ends exactly funding)
        // and value is conserved across the whole migration.
        #[test]
        fn plan_executes_and_conserves_value(
            notes in proptest::collection::vec(1u64..=10_000_000_000, 0..300)
        ) {
            let params = params();
            let plan = assert_plan_executes(&notes, &params);
            let total: u64 = notes.iter().sum();
            let denomination_sum: u64 = plan.transfers.iter().sum();
            prop_assert_eq!(
                total,
                denomination_sum + plan.transfers_fee(&params) + plan.preparation_fee() + plan.residual
            );
        }

        // Small-note-heavy wallets (the fragmentation case the consolidation
        // rounds exist for).
        #[test]
        fn fragmented_plans_execute(
            notes in proptest::collection::vec(10_001u64..=500_000, 100..400)
        ) {
            let params = params();
            let plan = assert_plan_executes(&notes, &params);
            let total: u64 = notes.iter().sum();
            let denomination_sum: u64 = plan.transfers.iter().sum();
            prop_assert_eq!(
                total,
                denomination_sum + plan.transfers_fee(&params) + plan.preparation_fee() + plan.residual
            );
            // log_K bound: 400 notes at a budget of 32 → one consolidation round
            // plus sizing.
            prop_assert!(plan.preparation_rounds.len() <= 4);
        }

        // The per-transaction budget is honored whatever its value. The
        // per-tx asserts inside `assert_plan_executes` enforce it.
        #[test]
        fn planner_honors_any_action_budget(
            notes in proptest::collection::vec(10_001u64..=500_000, 0..200),
            budget in prop_oneof![Just(8usize), Just(32usize), Just(100usize)],
        ) {
            let mut params = params();
            params.max_actions_per_split_tx = budget;
            assert_plan_executes(&notes, &params);
        }
    }

    #[test]
    fn scheduled_plan_conserves_value() {
        let params = params();
        let cases: [&[u64]; 4] = [
            &[1_999_999],
            &[3_000_000],
            &[5_999_999],
            &[1_400_000, 1_400_000],
        ];
        for note_values in cases {
            let plan = plan_migration(note_values, true, &params);
            let total: u64 = note_values.iter().sum();
            let conventional: u64 = plan
                .preparation_rounds
                .iter()
                .flatten()
                .map(|tx| note_preparation_fee(tx.inputs.len(), tx.outputs.len(), true))
                .sum();
            let parts_sum: u64 = plan.transfers.iter().sum();
            let accounted = parts_sum + plan.transfers_fee(&params) + conventional + plan.residual;
            assert_eq!(
                accounted,
                total,
                "plan over {note_values:?} loses {} zatoshis: transfers {parts_sum} + transfers_fee {} + \
                 conventional split fees {conventional} + residual {} != balance {total}; plan {plan:?}",
                total - accounted,
                plan.transfers_fee(&params),
                plan.residual,
            );
        }
    }

    #[test]
    fn plan_hash_is_invariant_under_note_enumeration_order() {
        let params = params();
        let ascending = [2_000_000u64, 3_000_000];
        let descending = [3_000_000u64, 2_000_000];
        let one = plan_migration(&ascending, true, &params);
        let other = plan_migration(&descending, true, &params);
        assert_eq!(
            (one.transfers.clone(), one.residual),
            (other.transfers.clone(), other.residual),
            "the two enumerations must describe the same migration"
        );
        assert_eq!(
            plan_hash(&one),
            plan_hash(&other),
            "the consent digest of the same note set changes with enumeration order: {ascending:?} \
             against {descending:?}; plans {one:?} and {other:?}"
        );
    }

    fn drive_rounds(start: &[u64], params: &MigrationParams, limit: usize) -> (Vec<u64>, usize) {
        let mut notes = start.to_vec();
        let mut executed = 0;
        while executed < limit {
            let plan = plan_migration(&notes, true, params);
            if plan.is_prepared() {
                break;
            }
            let round = plan.preparation_rounds[0].clone();
            for transaction in &round {
                for input in &transaction.inputs {
                    let index = notes
                        .iter()
                        .position(|note| note == input)
                        .expect("a planned input names a note the wallet holds");
                    notes.remove(index);
                }
            }
            for transaction in &round {
                notes.extend(transaction.outputs.iter().copied());
            }
            executed += 1;
        }
        (notes, executed)
    }

    #[test]
    fn replanned_rounds_converge_within_round_bound() {
        use crate::lightclient::migrate::MAX_ROUNDS;

        let params = params();
        let balance = 4_700_000_000_000u64;
        let planned = plan_migration(&[balance], true, &params)
            .preparation_rounds
            .len();

        let (notes, executed) = drive_rounds(&[balance], &params, MAX_ROUNDS);
        assert!(
            executed < MAX_ROUNDS,
            "a plan of {planned} rounds must not exhaust the {MAX_ROUNDS}-round bound; \
             after {executed} replanned rounds the notes are still {notes:?}"
        );
    }

    #[test]
    fn replanned_round_does_not_respend_note_at_same_value() {
        let params = params();
        let balance = 1_000_000_000_000u64;

        let first = plan_migration(&[balance], true, &params);
        assert_eq!(
            first.preparation_rounds.len(),
            2,
            "the plan for {balance} zatoshis announces two rounds: {first:?}"
        );
        let after_first_round: Vec<u64> = first.preparation_rounds[0]
            .iter()
            .flat_map(|transaction| transaction.outputs.iter().copied())
            .collect();

        let next = plan_migration(&after_first_round, true, &params);
        assert!(
            !next.is_prepared(),
            "the wallet after round zero still needs splitting: {next:?}"
        );
        for transaction in &next.preparation_rounds[0] {
            let recreated: Vec<u64> = transaction
                .inputs
                .iter()
                .copied()
                .filter(|input| transaction.outputs.contains(input))
                .collect();
            assert!(
                recreated.is_empty(),
                "replanning after round zero yields a round-zero transaction that spends \
                 {recreated:?} and recreates the same value, burning a fee of {} for no progress: \
                 {transaction:?}",
                transaction.fee()
            );
        }
    }

    #[test]
    fn classify_note_names_a_funding_note_by_its_denomination() {
        let params = params();
        let fee = params.transfer_fee;
        for denomination in params.denominations().iter().copied() {
            assert_eq!(
                classify_note(denomination + fee, &params),
                NoteClass::FundingNote { denomination },
                "a note sized denomination + transfer fee funds one transfer as it is"
            );
        }
        assert_eq!(
            classify_note(1_000_000 + fee + 1, &params),
            NoteClass::NeedsPreparation,
            "one zatoshi off the funding size is not a funding note"
        );
        assert_eq!(
            classify_note(1_000_000, &params),
            NoteClass::NeedsPreparation,
            "a bare denomination without the transfer fee still needs preparation"
        );
    }

    #[test]
    fn classify_note_splits_preparation_from_residual_at_the_sweep_minimum() {
        let params = params();
        assert_eq!(
            classify_note(params.sweep_min + 1, &params),
            NoteClass::NeedsPreparation
        );
        assert_eq!(
            classify_note(params.sweep_min, &params),
            NoteClass::Residual
        );
        assert_eq!(classify_note(1, &params), NoteClass::Residual);
        assert_eq!(classify_note(0, &params), NoteClass::Residual);
    }

    #[test]
    fn classify_note_agrees_with_plan_migration_over_the_same_notes() {
        let params = params();
        let fee = params.transfer_fee;
        let notes = [
            1_000_000 + fee,
            2_000_000 + fee,
            2_000_000 + fee,
            params.sweep_min,
            5_000,
            1,
            123_456_789,
        ];
        let plan = plan_migration(&notes, true, &params);

        let mut funding: Vec<u64> = notes
            .iter()
            .filter_map(|value| match classify_note(*value, &params) {
                NoteClass::FundingNote { denomination } => Some(denomination),
                _ => None,
            })
            .collect();
        funding.sort_unstable();
        let mut planned = plan.transfers.clone();
        planned.sort_unstable();
        for denomination in &funding {
            let position = planned
                .iter()
                .position(|planned| planned == denomination)
                .unwrap_or_else(|| {
                    panic!("funding denomination {denomination} is missing from the transfers")
                });
            planned.remove(position);
        }

        let residual: u64 = notes
            .iter()
            .filter(|value| classify_note(**value, &params) == NoteClass::Residual)
            .sum();
        assert!(
            plan.residual >= residual,
            "every residual-classified value is booked as residual: plan {} < classified {residual}",
            plan.residual
        );
        assert!(
            !plan.is_prepared(),
            "a note that needs preparation leaves the plan unprepared"
        );
    }

    #[test]
    fn a_wallet_without_notes_needing_preparation_is_prepared_with_the_exact_residual() {
        let params = params();
        let fee = params.transfer_fee;
        let notes = [5_000_000 + fee, 1_000_000 + fee, params.sweep_min, 7];
        assert!(
            notes
                .iter()
                .all(|value| classify_note(*value, &params) != NoteClass::NeedsPreparation)
        );

        let plan = plan_migration(&notes, true, &params);

        assert!(plan.is_prepared());
        assert_eq!(plan.transfers, [5_000_000, 1_000_000]);
        assert_eq!(plan.residual, params.sweep_min + 7);
    }
}
