//! Phase 1 planning and execution: note splitting, plus the fee model shared
//! with the Phase 2 parts.
//!
//! [`plan_migration`] turns the wallet's Orchard note values into a
//! [`MigrationPlan`]: the note-splitting rounds that resize the notes to
//! exactly `denomination + part_fee`, and the denominations of the parts the
//! wallet will send once splitting completes.

use std::collections::HashSet;
use zcash_protocol::value::Zatoshis;

use super::params::MigrationParams;
use super::quantize::decompose;
use crate::wallet::utils;

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
pub(crate) const MIN_BUNDLE_ACTIONS: u64 = 2;

/// The canonical ZIP-317 fee of one part. A part carries TWO bundles, an
/// Orchard bundle (1 spend, padded to 2 actions) and an Ironwood bundle
/// (1 output, padded to 2 actions), so it pays for 4 logical actions. Every
/// split note is sized `denomination + part_fee` so the part balances
/// exactly.
///
/// ZIP 318 says only "the canonical fee (provisionally the ZIP 317 minimum
/// fee)" at
/// <https://zips.z.cash/zip-0318#canonicalmigrationtransactionstructure>,
/// and the reference crate models a 2-source-action, 1-unpadded-destination
/// shape, so the three readings price a part at 10 000, 15 000, and (here)
/// 20 000 zatoshis. The value feeds part sizing and the consent hash, so
/// aligning it is a ratification decision, not an import.
pub(crate) const CANONICAL_PART_FEE: u64 = MARGINAL_FEE * 2 * MIN_BUNDLE_ACTIONS;

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
/// rule ignores the network and target height it is handed and the
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

/// The ZIP-317 conventional fee for an Orchard-only note-splitting
/// transaction with `n_in` spends and `n_out` outputs.
pub(crate) fn note_split_fee(n_in: usize, n_out: usize, post_activation: bool) -> u64 {
    zip317_fee(
        bundle_actions(orchard_version(post_activation), n_in, n_out),
        0,
    )
}

/// If a note of value `v` is already part-ready, sized exactly
/// `denomination + part_fee`, returns that denomination.
pub(crate) fn part_denomination(value: u64, params: &MigrationParams) -> Option<u64> {
    value
        .checked_sub(params.part_fee)
        .filter(|d| params.denominations.contains(d))
}

/// One planned Orchard→Orchard note-splitting transaction. All values are
/// zatoshis. The fee is implied: `sum(inputs) − sum(outputs)`, always at
/// least the ZIP-317 conventional fee for its action count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSplitTx {
    /// Values of the notes this transaction spends (bounded by
    /// [`MigrationParams::max_actions_per_split_tx`]).
    pub inputs: Vec<u64>,
    /// Values of the self-notes this transaction creates (bounded by
    /// [`MigrationParams::max_actions_per_split_tx`]).
    pub outputs: Vec<u64>,
}

impl NoteSplitTx {
    /// The implied fee: `sum(inputs) − sum(outputs)`.
    pub fn fee(&self) -> u64 {
        self.inputs.iter().sum::<u64>() - self.outputs.iter().sum::<u64>()
    }
}

/// The output side of a migration transaction. The input side is always
/// pre-Ironwood (V2) Orchard notes. Only the destination differs.
#[derive(Debug, Clone, Copy)]
pub(in crate::wallet::migration) enum MigrationOutputs<'a> {
    /// Phase 1 note splitting: Orchard→Orchard self-sends, one output per
    /// value. Nothing crosses a pool boundary.
    Orchard(&'a [u64]),
    /// A pool-crossing transfer into Ironwood: exactly one output, no change.
    /// Both the ZIP 318 parts and the immediate migration are built this way.
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

/// A complete migration plan: note-splitting rounds, then one part per
/// denomination. Pure data, nothing is signed or sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationPlan {
    /// Note-splitting rounds, executed in order. Transactions within a round
    /// are independent. Each round's outputs must confirm before the next
    /// round spends them.
    pub split_rounds: Vec<Vec<NoteSplitTx>>,
    /// The denominations of the parts to send once splitting completes
    /// (largest first). Includes denominations of notes that are already
    /// part-ready. Each costs one [`MigrationParams::part_fee`] on top.
    pub parts: Vec<u64>,
    /// Total value residual in dust notes (each at most
    /// [`MigrationParams::sweep_min`]) plus any balance too small to form
    /// even the smallest denomination.
    pub residual: u64,
}

impl MigrationPlan {
    /// True when no note splitting is required and the parts can be sent now.
    pub fn is_split(&self) -> bool {
        self.split_rounds.is_empty()
    }

    /// Total fees across all note-splitting transactions.
    pub fn split_fee(&self) -> u64 {
        self.split_rounds
            .iter()
            .flatten()
            .map(NoteSplitTx::fee)
            .sum()
    }

    /// Total fees the parts will pay (one part fee per denomination).
    pub fn parts_fee(&self, params: &MigrationParams) -> u64 {
        self.parts.len() as u64 * params.part_fee
    }
}

/// A collision-resistant digest of a plan, recorded at consent time (with
/// [`MigrationParams::params_hash`]) so any later replan requires fresh
/// consent.
pub fn plan_hash(plan: &MigrationPlan) -> [u8; 32] {
    let mut hasher = blake2b_simd::Params::new()
        .hash_length(32)
        .personal(b"ZingoMigPlanV0__")
        .to_state();
    hasher.update(&(plan.split_rounds.len() as u64).to_le_bytes());
    for round in &plan.split_rounds {
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
    hasher.update(&(plan.parts.len() as u64).to_le_bytes());
    for denomination in &plan.parts {
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
/// Notes already sized `denomination + part_fee` are left untouched (their
/// denominations appear directly in `parts`), so a migration interrupted and
/// replanned never re-splits finished notes. Everything else worth more than
/// [`MigrationParams::sweep_min`] is split:
///
/// 1. **Reduction**: while more notes remain than one transaction's spend
///    budget, merge groups of at most that many notes into one note each
///    (smallest first).
/// 2. **Sizing**: spend the remaining notes into notes sized exactly
///    `denomination + part_fee` — consolidating the smallest inputs into one
///    funding note first whenever the pool plus the targets would exceed one
///    transaction's total budget — where the denominations are the
///    [`decompose`]-ition of what is left after all fees. When the balance
///    needs more target notes than fit one transaction (whale balances),
///    splitting recurses through intermediate notes.
///
/// `post_activation` selects the note-splitting fee model (see
/// `note_split_fee`): pass whether the transactions will confirm at or
/// after the NU6.3 activation height.
pub fn plan_migration(
    note_values: &[u64],
    post_activation: bool,
    params: &MigrationParams,
) -> MigrationPlan {
    let mut parts: Vec<u64> = Vec::new();
    let mut pool: Vec<u64> = Vec::new();
    let mut residual: u64 = 0;
    let max_notes = side_budget(params);

    for &value in note_values {
        if let Some(denomination) = part_denomination(value, params) {
            parts.push(denomination);
        } else if value > params.sweep_min {
            pool.push(value);
        } else {
            residual += value;
        }
    }

    let mut split_rounds: Vec<Vec<NoteSplitTx>> = Vec::new();

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
                group.iter().sum::<u64>() - note_split_fee(group.len(), 1, post_activation);
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
            round.push(NoteSplitTx {
                inputs: group.to_vec(),
                outputs: vec![merged],
            });
            next.push(merged);
        }
        split_rounds.push(round);
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
            let overhead = denominations.len() as u64 * params.part_fee
                + split_into_fee(pool.len(), denominations.len(), post_activation, params);
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
                + split_rounds
                    .iter()
                    .flatten()
                    .map(NoteSplitTx::fee)
                    .sum::<u64>();
            split_rounds.clear();
        } else {
            // The pool never contains part-ready notes (those were diverted
            // into `parts` above), so sizing is always required here.
            let targets: Vec<u64> = denominations.iter().map(|d| d + params.part_fee).collect();
            split_rounds.extend(split_into(pool, &targets, post_activation, params));
            parts.extend(denominations);
        }
    }

    parts.sort_unstable_by(|a, b| b.cmp(a));

    MigrationPlan {
        split_rounds,
        parts,
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
/// Count-based only, so [`split_into_fee`] models it exactly. Callers
/// guarantee `n_in + m` exceeds the budget.
fn merge_group_len(n_in: usize, m: usize, params: &MigrationParams) -> usize {
    (n_in + m + 1 - params.max_actions_per_split_tx)
        .max(3)
        .min(n_in)
}

/// Total ZIP-317 fee of splitting `n_in` notes into `m` target notes,
/// recursing through intermediates when `m` exceeds the per-transaction
/// budget, and consolidating inputs first when spends plus outputs would
/// exceed the total budget (mirroring [`split_into`]).
fn split_into_fee(n_in: usize, m: usize, post_activation: bool, params: &MigrationParams) -> u64 {
    let max_notes = side_budget(params);
    if m <= max_notes {
        if n_in + m > params.max_actions_per_split_tx {
            let group_len = merge_group_len(n_in, m, params);
            note_split_fee(group_len, 1, post_activation)
                + note_split_fee(n_in - group_len + 1, m, post_activation)
        } else {
            note_split_fee(n_in, m, post_activation)
        }
    } else {
        // One future transaction per chunk of `max_notes` targets, funded by
        // an intermediate note each. The intermediates are themselves split.
        let chunks = m.div_ceil(max_notes);
        let mut total = 0;
        let mut remaining = m;
        for _ in 0..chunks {
            let len = remaining.min(max_notes);
            total += note_split_fee(1, len, post_activation);
            remaining -= len;
        }
        total + split_into_fee(n_in, chunks, post_activation, params)
    }
}

/// Materializes the note-splitting rounds that turn `inputs` into exactly
/// `targets`. Mirrors [`split_into_fee`]. The final transaction's fee absorbs
/// any slack between the input total and the exact target-side requirement.
///
/// The budget is a total: a transaction's spends and outputs together must
/// fit [`MigrationParams::max_actions_per_split_tx`] (ZIP 318's 16-action
/// preparation shape). When the pool plus the targets exceed it, the
/// smallest inputs consolidate into one funding note first, in a round of
/// their own, and the sizing transaction spends the consolidated note
/// beside the survivors.
fn split_into(
    mut inputs: Vec<u64>,
    targets: &[u64],
    post_activation: bool,
    params: &MigrationParams,
) -> Vec<Vec<NoteSplitTx>> {
    let max_notes = side_budget(params);
    if targets.len() <= max_notes {
        let mut rounds = Vec::new();
        if inputs.len() + targets.len() > params.max_actions_per_split_tx {
            let group_len = merge_group_len(inputs.len(), targets.len(), params);
            inputs.sort_unstable();
            let group: Vec<u64> = inputs.drain(..group_len).collect();
            let merged =
                group.iter().sum::<u64>() - note_split_fee(group.len(), 1, post_activation);
            rounds.push(vec![NoteSplitTx {
                inputs: group,
                outputs: vec![merged],
            }]);
            inputs.push(merged);
        }
        rounds.push(vec![NoteSplitTx {
            inputs,
            outputs: targets.to_vec(),
        }]);
        return rounds;
    }
    let chunks: Vec<&[u64]> = targets.chunks(max_notes).collect();
    let intermediates: Vec<u64> = chunks
        .iter()
        .map(|chunk| chunk.iter().sum::<u64>() + note_split_fee(1, chunk.len(), post_activation))
        .collect();
    let mut rounds = split_into(inputs, &intermediates, post_activation, params);
    rounds.push(
        intermediates
            .iter()
            .zip(&chunks)
            .map(|(&intermediate, chunk)| NoteSplitTx {
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
                &HashSet::new(),
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
    /// Phase 1 status projection plans over this set, so a note-splitting
    /// round in flight reads as its pending outputs instead of as vanished
    /// value, the trap [`Self::note_split_in_flight`] documents rules the
    /// anchored [`Self::migration_note_values`] out for status use. Failed
    /// transactions' outputs never existed on chain and are excluded.
    pub(crate) fn live_v2_note_values(&self, account: zip32::AccountId) -> Vec<u64> {
        use pepper_sync::wallet::{KeyIdInterface as _, NoteInterface as _, OutputInterface as _};

        let pending_spends = pepper_sync::pending_spends(&self.wallet_transactions);
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
                    && !pending_spends.contains_key(&note.output_id())
            })
            .map(|note| note.value())
            .collect()
    }

    /// Given an account, returns whether any wallet transaction confirmed
    /// above the migration anchor holds an unspent V2 (pre-Ironwood) Orchard
    /// note that account owns, a note that is mined but not yet spendable.
    /// Errors when the wallet holds no sync data to take the anchor from.
    #[allow(clippy::result_large_err)]
    pub(crate) fn unanchored_v2_outputs(
        &self,
        account: zip32::AccountId,
    ) -> Result<bool, crate::wallet::error::WalletError> {
        use pepper_sync::wallet::{KeyIdInterface as _, NoteInterface as _, OutputInterface as _};

        let (_, anchor_height) = self
            .get_migration_heights()?
            .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
        // A failed transaction carries no confirmed height
        let pending_spends = pepper_sync::pending_spends(&self.wallet_transactions);
        Ok(self
            .wallet_transactions
            .values()
            .filter(|transaction| {
                transaction
                    .status()
                    .get_confirmed_height()
                    .is_some_and(|height| height > anchor_height)
            })
            .flat_map(|transaction| {
                pepper_sync::wallet::OrchardNote::transaction_outputs(transaction)
            })
            .any(|note| {
                note.key_id().account_id() == account
                    && note.note().version() == orchard::note::NoteVersion::V2
                    && note.spending_transaction().is_none()
                    && !pending_spends.contains_key(&note.output_id())
            }))
    }

    /// True when a note-splitting round this account initiated is still
    /// confirming: a pending (built, transmitted, or mempool, but not yet
    /// confirmed) transaction holds account-owned pre-Ironwood (V2) Orchard
    /// outputs. Those are the self-notes a round creates. While they are
    /// unconfirmed the round's spent inputs have also left the spendable set,
    /// so a replan sees an empty pool and would otherwise report the migration
    /// falsely complete.
    ///
    /// This is the stateless, derived replacement for a round's stored
    /// `pending_txids`: it lets [`crate::lightclient::LightClient::quick_split`]
    /// tell "a round is still in flight, sync and retry" apart from "fully
    /// split, done" without persisting any migration phase. It reads the
    /// round's *outputs* rather than its inputs' spend marks, which a
    /// not-yet-transmitted transaction does not carry. Under the flow's
    /// sync-between-rounds contract there are no unrelated pending Orchard
    /// receipts, so this is precise. Were there, treating them as "wait" is
    /// the safe reading.
    pub(crate) fn note_split_in_flight(&self, account: zip32::AccountId) -> bool {
        use pepper_sync::wallet::{KeyIdInterface as _, NoteInterface as _, OutputInterface as _};

        self.wallet_transactions
            .values()
            .filter(|transaction| transaction.status().is_pending())
            .flat_map(|transaction| {
                pepper_sync::wallet::OrchardNote::transaction_outputs(transaction)
            })
            .any(|note| {
                note.key_id().account_id() == account
                    && note.note().version() == orchard::note::NoteVersion::V2
            })
    }

    /// Builds, proves, signs and records one planned note-splitting
    /// transaction (Orchard→Orchard self-send). Returns its txid. Transmission
    /// is the caller's step.
    #[allow(clippy::result_large_err)]
    pub(crate) fn build_note_split_transaction(
        &mut self,
        account: zip32::AccountId,
        planned: &NoteSplitTx,
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
                    &HashSet::new(),
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
        let mut builder =
            Builder::new(self.chain_type, target_height, build_config).with_expiry_height(
                crate::wallet::expiry::tx_expiry_height(&self.chain_type, target_height),
            );
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

        let sapling_prover = utils::LazySaplingProver::default();
        let build_result = builder
            .build(
                &zcash_transparent::builder::TransparentSigningSet::new(),
                &[usk.sapling().clone()],
                &[usk.orchard().into()],
                crate::utils::system_rng(),
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
            zl_common::status::ConfirmationStatus::Calculated(target_height),
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
mod tests;
