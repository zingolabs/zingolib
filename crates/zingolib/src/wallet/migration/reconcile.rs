//! On-launch reconciliation (ZIP 318: the primary catch-up mechanism).
//!
//! [`reconcile`] is a pure function of the persisted [`MigrationState`] and a
//! read-only [`ChainView`], so a client can call it on every launch and as
//! often as it needs to. It classifies every part and returns the recommended
//! actions. Applying them (and obtaining user consent where required) is the
//! caller's job.

use pepper_sync::wallet::OutputId;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

use super::parts::{PartId, PartRecord, PartState};
use super::schedule::BucketGrid;
use super::{MigrationPhase, MigrationState};

/// About two hours, in reference blocks (75 seconds each; the chain's block
/// spacing converts it, so it stays two hours from NU7): the slip a
/// best-effort background scheduler is allowed before reconciliation
/// surfaces a part as overdue (ZIP 318 treats such slips as normal
/// operation).
const SLIP_TOLERANCE_REFERENCE_BLOCKS: u32 = 96;

/// Read-only chain facts, implemented by the wallet and by test mocks.
/// Everything reconciliation knows about the chain flows through here.
pub trait ChainView {
    /// The highest block height the wallet knows of.
    fn chain_tip(&self) -> Option<BlockHeight>;

    /// The chain's ZIP 318 anchor grid under `params`' bucket modulus: the
    /// windows this view's heights fall in.
    fn bucket_grid(&self, params: &super::params::MigrationParams) -> BucketGrid;

    /// The Spend-Evidence Height: the height through which the wallet's
    /// evidence of spends and transaction inclusion is complete, with every
    /// block at or below it scanned with its nullifiers mapped. This is
    /// the only lawful input to judgments that condemn (a transaction
    /// expired-unmined, a part dead): the chain tip runs ahead of
    /// scanning, and condemning against it invites false invalidation
    /// (issue #2493, finding 8). Forward planning may use
    /// [`Self::chain_tip`].
    fn spend_evidence_height(&self) -> Option<BlockHeight>;

    /// The confirmed or pending spend of the given note, if the wallet has
    /// observed one: the spending txid and its confirmation height.
    fn note_spend(&self, output_id: OutputId) -> Option<(TxId, Option<BlockHeight>)>;

    /// The height a transaction confirmed at, if it did.
    fn transaction_confirmed_height(&self, txid: &TxId) -> Option<BlockHeight>;

    /// Whether a transaction is recorded as failed (rejected or expired).
    fn transaction_failed(&self, txid: &TxId) -> bool;

    /// The account's confirmed unspent pre-Ironwood Orchard balance, the
    /// input to the ZIP 318 invalidation predicate and the residual check.
    fn orchard_confirmed_spendable(&self, account: zip32::AccountId) -> u64;
}

/// How reconciliation classified one part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartClass {
    /// Nothing to do: waiting for its window, or its window is open.
    OnTrack,
    /// Its window passed recently (within the slip tolerance). Normal
    /// operation, never surfaced as an error.
    SlippedWithinTolerance,
    /// Its window passed beyond the slip tolerance without a transmission.
    Overdue,
    /// Signed, its window passed, and its transaction is still valid:
    /// transmitting it now would mine a permanent lateness fingerprint
    /// (the cleartext expiry and the stale anchor single the part out
    /// within its denomination cohort), so it waits out its expiry and
    /// rebuilds fresh, on-chain indistinguishable from an on-schedule
    /// part. Not part of the catch-up batch. Surfaced so status can say
    /// why nothing was sent and when the rebuild comes.
    AwaitingExpiry {
        /// The expiry height being waited out.
        expiry: BlockHeight,
    },
    /// Its transaction reached expiry unmined.
    Expired,
    /// Its bound note was spent outside the migration.
    Invalidated,
    /// Mined and confirmed.
    Confirmed,
}

/// What the caller should do next. Actions marked "safe unattended" are
/// applied by `LightClient::reconcile_migration` without user interaction.
/// The others need consent or a user-facing disclosure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecommendedAction {
    /// A note-splitting round is still confirming. Keep waiting.
    AwaitSplitConfirmation,
    /// A note-splitting transaction failed or expired. The failure released
    /// its notes, so the retry is a replan: drive
    /// [`crate::lightclient::LightClient::continue_note_splitting`] once the
    /// round's remaining transactions resolve.
    RetrySplit {
        /// The failed transaction.
        txid: TxId,
    },
    /// Note splitting needs driving: it has not started, or the pending
    /// round fully confirmed.
    /// [`crate::lightclient::LightClient::continue_note_splitting`] replans
    /// from the wallet's notes and either executes the next round or, once
    /// every note is part-ready, binds the parts and schedules them.
    /// Whether the confirmed round was the last one is only decidable by
    /// that replan, which pure reconciliation cannot perform.
    ContinueNoteSplitting,
    /// The part's own transaction mined but the record lagged (for example a
    /// crash between submit and record). Safe unattended.
    PromoteConfirmed {
        /// The part to promote.
        part: PartId,
        /// The confirmation height.
        height: BlockHeight,
    },
    /// The part expired unmined: rebuild with the same denomination against
    /// a fresh boundary. Safe unattended, no fresh consent (denomination and
    /// total are unchanged).
    Rebuild {
        /// The part to rebuild.
        part: PartId,
    },
    /// The part's bound note was spent outside the migration. Safe
    /// unattended (marking only, the replan below needs consent).
    MarkInvalidated {
        /// The part to mark.
        part: PartId,
    },
    /// Invalidated value remains in the Orchard pool: re-quantize the
    /// remainder into a new plan. Requires fresh consent.
    ReplanRemainder,
    /// Overdue parts should be offered for immediate, sequenced sending.
    /// Requires the user-facing disclosure that sending at application-open
    /// time correlates the transmissions with the user's activity.
    PromptCatchUp {
        /// The overdue parts.
        parts: Vec<PartId>,
        /// Always true: the disclosure is a ZIP 318 MUST.
        disclosure_required: bool,
    },
    /// Every part confirmed and the leftover Orchard balance is below the
    /// economic threshold. Safe unattended. The client surfaces the residual
    /// disclosure.
    MarkComplete {
        /// The unmigrated leftover, in zatoshis.
        residual: u64,
    },
}

/// One part's classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartAssessment {
    /// The part.
    pub id: PartId,
    /// Its classification.
    pub class: PartClass,
}

/// The outcome of a reconciliation pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcileReport {
    /// Per-part classifications (empty before parts are bound).
    pub assessments: Vec<PartAssessment>,
    /// What to do about them, deduplicated.
    pub actions: Vec<RecommendedAction>,
}

/// Classifies every part of `state` against the chain view and recommends
/// actions. Pure: reads nothing but its arguments and writes nothing.
pub fn reconcile(state: &MigrationState, chain: &impl ChainView) -> ReconcileReport {
    let mut report = ReconcileReport::default();

    match &state.phase {
        MigrationPhase::Planned => {
            report
                .actions
                .push(RecommendedAction::ContinueNoteSplitting);
            return report;
        }
        MigrationPhase::NoteSplitting { pending_txids, .. } => {
            let mut all_confirmed = true;
            for txid in pending_txids {
                if chain.transaction_failed(txid) {
                    report
                        .actions
                        .push(RecommendedAction::RetrySplit { txid: *txid });
                    all_confirmed = false;
                } else if chain.transaction_confirmed_height(txid).is_none() {
                    all_confirmed = false;
                }
            }
            if all_confirmed {
                report
                    .actions
                    .push(RecommendedAction::ContinueNoteSplitting);
            } else if report.actions.is_empty() {
                report
                    .actions
                    .push(RecommendedAction::AwaitSplitConfirmation);
            }
            return report;
        }
        MigrationPhase::Complete { .. } => return report,
        MigrationPhase::PartsScheduled => (),
    }

    let chain_tip = chain.chain_tip();
    let mut overdue = Vec::new();
    let mut any_invalidated = false;
    // Completion is judged on the persisted states only: promotions
    // recommended by this very pass complete on the next one, after they
    // have been applied and saved.
    let all_terminal = state.parts.iter().all(|part| {
        matches!(
            part.state,
            PartState::Confirmed { .. } | PartState::Invalidated
        )
    });

    for part in &state.parts {
        let class = classify(part, state, chain, chain_tip, &mut report);
        match class {
            PartClass::Overdue => overdue.push(part.id),
            PartClass::Invalidated => any_invalidated = true,
            _ => (),
        }
        report
            .assessments
            .push(PartAssessment { id: part.id, class });
    }

    if !overdue.is_empty() {
        report.actions.push(RecommendedAction::PromptCatchUp {
            parts: overdue,
            disclosure_required: true,
        });
    }
    let spendable = chain.orchard_confirmed_spendable(state.account);
    if any_invalidated && spendable > state.params.sweep_min {
        // The ZIP 318 invalidation predicate: a worthwhile
        // confirmed-spendable Orchard balance remains while a scheduled
        // part is invalid. At or below the Sweep Minimum a replan would
        // leave everything it planned as residual, so no replan is offered.
        report.actions.push(RecommendedAction::ReplanRemainder);
    }
    // The completion rule: when every part is terminal, the migration
    // concludes unless a worthwhile remainder exists. Complete means
    // "nothing left for this migration to do", not "everything migrated".
    // An insistently spent-away migration completes with zero confirmed
    // parts, reported faithfully by the status surface.
    if all_terminal && !state.parts.is_empty() {
        if spendable <= state.params.sweep_min {
            report.actions.push(RecommendedAction::MarkComplete {
                residual: spendable,
            });
        } else if !any_invalidated {
            // Fee rounding left an economic amount behind: quantize it into
            // a final transfer (fresh consent, it is a new plan). The
            // invalidated case pushed ReplanRemainder above.
            report.actions.push(RecommendedAction::ReplanRemainder);
        }
    }

    report
}

/// The parts a user-triggered
/// [`crate::lightclient::LightClient::execute_due_parts`] would transmit this
/// instant, computed read-only from a reconcile `report` and the schedule.
///
/// This is the current window's parts whose random target the chain has
/// reached, plus the overdue parts catch-up folds into the current window,
/// only the [`PartState::Assigned`] ones, since a [`PartState::Signed`]
/// overdue part keeps its stale anchor and is rebuilt rather than folded.
/// Parts reconciliation would instead confirm, invalidate or rebuild are
/// excluded: a natively-current part is admitted only while it is still
/// classified [`PartClass::OnTrack`].
///
/// Mirrors `execute_due_parts` (which folds via the same reconcile pass) so a
/// "batch due now" shown to the user can never name a part a send would not
/// build. `report` must come from [`reconcile`] over the same `parts`.
pub fn due_now_parts(
    parts: &[PartRecord],
    report: &ReconcileReport,
    now_height: BlockHeight,
    grid: &BucketGrid,
) -> Vec<PartId> {
    let current_bucket = grid.index_of(now_height);
    let overdue: std::collections::HashSet<PartId> = report
        .actions
        .iter()
        .find_map(|action| match action {
            RecommendedAction::PromptCatchUp { parts, .. } => Some(parts.iter().copied().collect()),
            _ => None,
        })
        .unwrap_or_default();
    let on_track: std::collections::HashSet<PartId> = report
        .assessments
        .iter()
        .filter(|assessment| assessment.class == PartClass::OnTrack)
        .map(|assessment| assessment.id)
        .collect();

    parts
        .iter()
        .filter(|part| {
            if overdue.contains(&part.id) {
                // Catch-up folds only Assigned overdue parts into the current
                // window (target reset, due at once); a Signed one is left for
                // a rebuild against a fresh boundary and is not sent this batch.
                part.state == PartState::Assigned
            } else {
                // A natively-current part goes out as soon as its window is
                // open. Gating on OnTrack drops the parts reconciliation will
                // confirm, invalidate or expire instead of sending. They
                // would otherwise leak through the state/bucket check while
                // still Assigned or Signed.
                on_track.contains(&part.id)
                    && super::schedule::part_in_current_bucket(part, current_bucket)
            }
        })
        .map(|part| part.id)
        .collect()
}

fn classify(
    part: &super::parts::PartRecord,
    state: &MigrationState,
    chain: &impl ChainView,
    chain_tip: Option<BlockHeight>,
    report: &mut ReconcileReport,
) -> PartClass {
    match part.state {
        PartState::Confirmed { .. } => return PartClass::Confirmed,
        PartState::Invalidated => return PartClass::Invalidated,
        PartState::Expired => {
            report
                .actions
                .push(RecommendedAction::Rebuild { part: part.id });
            return PartClass::Expired;
        }
        PartState::Bound | PartState::Assigned | PartState::Signed | PartState::Broadcast => (),
    }

    // The bound note's spend is the ground truth: our own txid means the
    // part mined (even if a crash lost the Broadcast record), any other
    // txid means an external spend invalidated it.
    if let Some(bound) = part.note
        && let Some((spending_txid, height)) = chain.note_spend(bound.output_id)
    {
        if part.txid == Some(spending_txid) {
            if let Some(height) = height {
                report.actions.push(RecommendedAction::PromoteConfirmed {
                    part: part.id,
                    height,
                });
                return PartClass::Confirmed;
            }
            // Our transaction is in flight, nothing to do.
            return PartClass::OnTrack;
        }
        report
            .actions
            .push(RecommendedAction::MarkInvalidated { part: part.id });
        return PartClass::Invalidated;
    }

    let Some(tip) = chain_tip else {
        return PartClass::OnTrack;
    };

    // Expiry: a signed-or-broadcast transaction is condemned as dead only
    // on complete evidence. The Spend-Evidence Height must reach the
    // expiry, so every block the transaction could have mined in has been
    // scanned with its nullifiers mapped. Judging against the chain tip
    // here condemned parts whose exonerating spend sat in the unscanned
    // gap (issue #2493, finding 8).
    if matches!(part.state, PartState::Signed | PartState::Broadcast)
        && part.expiry_height.is_some_and(|expiry_height| {
            chain
                .spend_evidence_height()
                .is_some_and(|evidence| evidence >= expiry_height)
        })
    {
        report
            .actions
            .push(RecommendedAction::Rebuild { part: part.id });
        return PartClass::Expired;
    }

    // Window position for parts that still await transmission.
    if matches!(part.state, PartState::Assigned | PartState::Signed)
        && let Some(bucket) = part.bucket_index
    {
        let grid = chain.bucket_grid(&state.params);
        let window_end = grid.boundary_of(bucket + 1);
        if tip >= window_end {
            let slipped_within_tolerance =
                !grid
                    .spacing()
                    .exceeds(window_end, tip, SLIP_TOLERANCE_REFERENCE_BLOCKS);
            return if slipped_within_tolerance {
                PartClass::SlippedWithinTolerance
            } else if part.state == PartState::Signed {
                // An overdue signed part is not catch-up material: its
                // still-valid transaction waits out its expiry rather than
                // minting a lateness fingerprint (see
                // [`PartClass::AwaitingExpiry`]).
                match part.expiry_height {
                    Some(expiry) => PartClass::AwaitingExpiry { expiry },
                    None => PartClass::Overdue,
                }
            } else {
                PartClass::Overdue
            };
        }
    }

    PartClass::OnTrack
}

impl ChainView for crate::wallet::LightWallet {
    fn chain_tip(&self) -> Option<BlockHeight> {
        self.sync_state.last_known_chain_height()
    }

    fn bucket_grid(&self, params: &super::params::MigrationParams) -> BucketGrid {
        params.grid(self.chain_type)
    }

    fn spend_evidence_height(&self) -> Option<BlockHeight> {
        // `fully_scanned_height`, not `highest_scanned_height`: scan ranges
        // complete out of order, and a scanned-but-unmapped block still
        // lacks spend evidence. Only the gap-free, nullifier-mapped
        // frontier is evidence-complete.
        self.sync_state.fully_scanned_height()
    }

    fn note_spend(&self, output_id: OutputId) -> Option<(TxId, Option<BlockHeight>)> {
        use pepper_sync::wallet::OutputInterface as _;

        let note = self
            .wallet_transactions
            .get(&output_id.txid())
            .and_then(|transaction| {
                pepper_sync::wallet::OrchardNote::transaction_outputs(transaction)
                    .iter()
                    .find(|note| note.output_id() == output_id)
            })?;
        let spending_txid = match note.spending_transaction() {
            Some(spending_txid) => spending_txid,
            None => *pepper_sync::pending_spends(&self.wallet_transactions).get(&output_id)?,
        };
        Some((
            spending_txid,
            self.transaction_confirmed_height(&spending_txid),
        ))
    }

    fn transaction_confirmed_height(&self, txid: &TxId) -> Option<BlockHeight> {
        self.wallet_transactions
            .get(txid)
            .and_then(|transaction| transaction.status().get_confirmed_height())
    }

    fn transaction_failed(&self, txid: &TxId) -> bool {
        // An absent record is *unknown*, not failed: after a restore from
        // backup an in-flight split transaction has no wallet record yet may
        // still confirm. Reporting it failed would retry the split and race
        // the original.
        self.wallet_transactions
            .get(txid)
            .is_some_and(|transaction| transaction.status().is_failed())
    }

    fn orchard_confirmed_spendable(&self, account: zip32::AccountId) -> u64 {
        use pepper_sync::wallet::{KeyIdInterface as _, NoteInterface as _, OutputInterface as _};

        let pending_spends = pepper_sync::pending_spends(&self.wallet_transactions);
        self.wallet_transactions
            .values()
            .filter(|transaction| transaction.status().is_confirmed())
            .flat_map(|transaction| {
                pepper_sync::wallet::OrchardNote::transaction_outputs(transaction)
            })
            .filter(|note| {
                note.spending_transaction().is_none()
                    && !pending_spends.contains_key(&note.output_id())
                    && note.key_id().account_id() == account
                    && note.note().version() == orchard::note::NoteVersion::V2
            })
            .map(pepper_sync::wallet::OutputInterface::value)
            .sum()
    }
}

#[cfg(test)]
mod tests;
