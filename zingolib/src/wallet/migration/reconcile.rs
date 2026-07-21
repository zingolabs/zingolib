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

use super::parts::{PartId, PartState};
use super::{MigrationPhase, MigrationState};

/// About two hours at the 75-second target spacing: the slip a best-effort
/// background scheduler is allowed before reconciliation surfaces a part as
/// overdue (ZIP 318 treats such slips as normal operation).
const SLIP_TOLERANCE_BLOCKS: u32 = 96;

/// Read-only chain facts, implemented by the wallet and by test mocks.
/// Everything reconciliation knows about the chain flows through here.
pub trait ChainView {
    /// The highest block height the wallet knows of.
    fn chain_tip(&self) -> Option<BlockHeight>;

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
    /// Its window passed beyond the slip tolerance without a broadcast.
    Overdue,
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
    /// A note-splitting transaction failed or expired. Rebuild it against a
    /// fresh anchor.
    RetrySplit {
        /// The failed transaction.
        txid: TxId,
    },
    /// Every split confirmed: bind parts to notes and schedule them. Safe
    /// unattended (the schedule was already consented to as part of the
    /// plan).
    BindAndSchedule,
    /// Note splitting has not started yet. Drive the next round.
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
    /// time correlates the broadcasts with the user's activity.
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
                report.actions.push(RecommendedAction::BindAndSchedule);
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
    let all_confirmed = state
        .parts
        .iter()
        .all(|part| matches!(part.state, PartState::Confirmed { .. }));

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
    if any_invalidated && chain.orchard_confirmed_spendable(state.account) > 0 {
        // The ZIP 318 invalidation predicate: confirmed-spendable Orchard
        // balance remains while a scheduled part is invalid.
        report.actions.push(RecommendedAction::ReplanRemainder);
    }
    if all_confirmed && !state.parts.is_empty() {
        let residual = chain.orchard_confirmed_spendable(state.account);
        if residual <= state.params.sweep_min {
            report
                .actions
                .push(RecommendedAction::MarkComplete { residual });
        } else {
            // Fee rounding left an economic amount behind: quantize it into
            // a final transfer (fresh consent, it is a new plan).
            report.actions.push(RecommendedAction::ReplanRemainder);
        }
    }

    report
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

    // Expiry: a signed-or-broadcast transaction past its expiry is dead.
    if matches!(part.state, PartState::Signed | PartState::Broadcast)
        && part
            .expiry_height
            .is_some_and(|expiry_height| tip > expiry_height)
    {
        report
            .actions
            .push(RecommendedAction::Rebuild { part: part.id });
        return PartClass::Expired;
    }

    // Window position for parts that still await broadcast.
    if matches!(part.state, PartState::Assigned | PartState::Signed)
        && let Some(bucket) = part.bucket_index
    {
        let window_end = super::schedule::boundary_of(bucket + 1, state.params.bucket_modulus);
        if tip >= window_end {
            let blocks_past = u32::from(tip) - u32::from(window_end);
            return if blocks_past <= SLIP_TOLERANCE_BLOCKS {
                PartClass::SlippedWithinTolerance
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
        let spending_txid = note.spending_transaction()?;
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

        self.wallet_transactions
            .values()
            .filter(|transaction| transaction.status().is_confirmed())
            .flat_map(|transaction| {
                pepper_sync::wallet::OrchardNote::transaction_outputs(transaction)
            })
            .filter(|note| {
                note.spending_transaction().is_none()
                    && note.key_id().account_id() == account
                    && note.note().version() == orchard::note::NoteVersion::V2
            })
            .map(pepper_sync::wallet::OutputInterface::value)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::super::parts::{BoundNote, PartRecord};
    use super::super::{ConsentBinding, MigrationParams, SigningStrategy};
    use super::*;
    use crate::config::ChainType;
    use std::collections::HashMap;

    struct MockChainView {
        tip: Option<BlockHeight>,
        note_spends: HashMap<OutputId, (TxId, Option<BlockHeight>)>,
        confirmed: HashMap<TxId, BlockHeight>,
        failed: Vec<TxId>,
        orchard_spendable: u64,
    }

    impl Default for MockChainView {
        fn default() -> Self {
            MockChainView {
                tip: Some(BlockHeight::from_u32(10_000)),
                note_spends: HashMap::new(),
                confirmed: HashMap::new(),
                failed: Vec::new(),
                orchard_spendable: 0,
            }
        }
    }

    impl ChainView for MockChainView {
        fn chain_tip(&self) -> Option<BlockHeight> {
            self.tip
        }
        fn note_spend(&self, output_id: OutputId) -> Option<(TxId, Option<BlockHeight>)> {
            self.note_spends.get(&output_id).copied()
        }
        fn transaction_confirmed_height(&self, txid: &TxId) -> Option<BlockHeight> {
            self.confirmed.get(txid).copied()
        }
        fn transaction_failed(&self, txid: &TxId) -> bool {
            self.failed.contains(txid)
        }
        fn orchard_confirmed_spendable(&self, _account: zip32::AccountId) -> u64 {
            self.orchard_spendable
        }
    }

    fn params() -> MigrationParams {
        MigrationParams::provisional(ChainType::Mainnet)
    }

    fn output_id(seed: u8) -> OutputId {
        OutputId::new(TxId::from_bytes([seed; 32]), u32::from(seed))
    }

    fn scheduled_state(parts: Vec<PartRecord>) -> MigrationState {
        let params = params();
        MigrationState {
            consent: ConsentBinding {
                params_hash: params.params_hash(),
                plan_hash: [0; 32],
                consented_at: 0,
            },
            params,
            strategy: SigningStrategy::LazyAtBoundary,
            mode: crate::wallet::migration::MigrationMode::Scheduled,
            account: zip32::AccountId::ZERO,
            phase: MigrationPhase::PartsScheduled,
            parts,
        }
    }

    fn assigned_part(id: u32, bucket: u64) -> PartRecord {
        let mut part = PartRecord::new(
            PartId(id),
            100_000_000,
            BoundNote {
                output_id: output_id(id as u8),
                nullifier: [id as u8; 32],
                commitment: [id as u8; 32],
            },
        );
        part.assign(bucket).unwrap();
        part
    }

    /// Bucket arithmetic for the provisional M = 256 and tip 10_000: the tip
    /// sits in bucket 39.
    const TIP_BUCKET: u64 = 10_000 / 256;

    /// Issue #2493, finding 8: a migration whose every part is terminal
    /// and whose replannable balance is zero must reach a terminal
    /// recommendation. An `Invalidated` part with nothing left to replan
    /// currently satisfies neither `MarkComplete` (not every part is
    /// `Confirmed`) nor `ReplanRemainder` (no spendable balance), so
    /// reconcile recommends nothing forever and the stuck state blocks
    /// both the immediate migration and the drain until the user finds
    /// `cancel`.
    #[test]
    fn terminal_parts_with_nothing_replannable_reach_complete() {
        let mut confirmed = assigned_part(0, TIP_BUCKET - 2);
        confirmed
            .mark_confirmed(BlockHeight::from_u32(9_000))
            .unwrap();
        let mut invalidated = assigned_part(1, TIP_BUCKET - 2);
        invalidated.mark_invalidated().unwrap();
        let state = scheduled_state(vec![confirmed, invalidated]);

        // The default view's confirmed-spendable Orchard balance is zero:
        // nothing remains to replan.
        let report = reconcile(&state, &MockChainView::default());

        assert!(
            report
                .actions
                .iter()
                .any(|action| matches!(action, RecommendedAction::MarkComplete { .. })),
            "every part is terminal and nothing is replannable, yet the \
             migration cannot conclude: {:?}",
            report.actions,
        );
    }

    #[test]
    fn future_and_open_windows_are_on_track() {
        let state = scheduled_state(vec![
            assigned_part(0, TIP_BUCKET),     // window open
            assigned_part(1, TIP_BUCKET + 3), // future
        ]);
        let report = reconcile(&state, &MockChainView::default());
        assert!(
            report
                .assessments
                .iter()
                .all(|a| a.class == PartClass::OnTrack)
        );
        assert!(report.actions.is_empty());
    }

    #[test]
    fn recent_slips_are_silent_and_older_ones_prompt_catch_up() {
        // Bucket TIP_BUCKET - 1's window closed at the tip's own boundary,
        // so a tip just past that boundary is within the slip tolerance.
        let state = scheduled_state(vec![assigned_part(0, TIP_BUCKET - 1)]);
        let mut chain = MockChainView {
            tip: Some(BlockHeight::from_u32(
                (TIP_BUCKET as u32) * 256 + SLIP_TOLERANCE_BLOCKS,
            )),
            ..Default::default()
        };
        let report = reconcile(&state, &chain);
        assert_eq!(
            report.assessments[0].class,
            PartClass::SlippedWithinTolerance
        );
        assert!(report.actions.is_empty(), "slips are not surfaced");

        chain.tip = Some(BlockHeight::from_u32(
            (TIP_BUCKET as u32) * 256 + SLIP_TOLERANCE_BLOCKS + 1,
        ));
        let report = reconcile(&state, &chain);
        assert_eq!(report.assessments[0].class, PartClass::Overdue);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::PromptCatchUp {
                parts: vec![PartId(0)],
                disclosure_required: true,
            }]
        );
    }

    #[test]
    fn expired_transactions_are_rebuilt() {
        let mut part = assigned_part(0, TIP_BUCKET - 2);
        part.mark_signed(
            TxId::from_bytes([9; 32]),
            BlockHeight::from_u32(9_000),
            None,
        )
        .unwrap();
        part.mark_broadcast().unwrap();
        let state = scheduled_state(vec![part]);
        let report = reconcile(&state, &MockChainView::default());
        assert_eq!(report.assessments[0].class, PartClass::Expired);
        assert!(
            report
                .actions
                .contains(&RecommendedAction::Rebuild { part: PartId(0) })
        );
    }

    #[test]
    fn external_spend_invalidates_and_replans() {
        let state = scheduled_state(vec![assigned_part(0, TIP_BUCKET + 1)]);
        let mut chain = MockChainView::default();
        chain.note_spends.insert(
            output_id(0),
            (
                TxId::from_bytes([250; 32]),
                Some(BlockHeight::from_u32(9_990)),
            ),
        );
        chain.orchard_spendable = 50_000_000; // change from the external spend
        let report = reconcile(&state, &chain);
        assert_eq!(report.assessments[0].class, PartClass::Invalidated);
        assert!(
            report
                .actions
                .contains(&RecommendedAction::MarkInvalidated { part: PartId(0) })
        );
        assert!(report.actions.contains(&RecommendedAction::ReplanRemainder));
    }

    #[test]
    fn crash_between_broadcast_and_record_promotes_via_nullifier() {
        // The part is persisted as Signed (the crash lost the Broadcast
        // record), but its own transaction spent the bound note on-chain.
        let own_txid = TxId::from_bytes([9; 32]);
        let mut part = assigned_part(0, TIP_BUCKET - 1);
        part.mark_signed(own_txid, BlockHeight::from_u32(20_000), None)
            .unwrap();
        let state = scheduled_state(vec![part]);
        let mut chain = MockChainView::default();
        chain
            .note_spends
            .insert(output_id(0), (own_txid, Some(BlockHeight::from_u32(9_995))));
        let report = reconcile(&state, &chain);
        assert_eq!(report.assessments[0].class, PartClass::Confirmed);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::PromoteConfirmed {
                part: PartId(0),
                height: BlockHeight::from_u32(9_995),
            }]
        );
    }

    #[test]
    fn all_confirmed_with_dust_residual_completes() {
        let mut part = assigned_part(0, TIP_BUCKET - 2);
        part.mark_signed(
            TxId::from_bytes([9; 32]),
            BlockHeight::from_u32(20_000),
            None,
        )
        .unwrap();
        part.mark_broadcast().unwrap();
        part.mark_confirmed(BlockHeight::from_u32(9_999)).unwrap();
        let state = scheduled_state(vec![part]);
        let mut chain = MockChainView {
            orchard_spendable: 5_000, // below the sweep minimum
            ..Default::default()
        };
        let report = reconcile(&state, &chain);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::MarkComplete { residual: 5_000 }]
        );

        // An economic leftover instead asks for a final transfer.
        chain.orchard_spendable = 5_000_000;
        let report = reconcile(&state, &chain);
        assert_eq!(report.actions, vec![RecommendedAction::ReplanRemainder]);
    }

    #[test]
    fn split_rounds_gate_on_confirmation() {
        let split_txid = TxId::from_bytes([1; 32]);
        let mut state = scheduled_state(Vec::new());
        state.phase = MigrationPhase::NoteSplitting {
            round: 0,
            pending_txids: vec![split_txid],
        };
        let mut chain = MockChainView::default();

        let report = reconcile(&state, &chain);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::AwaitSplitConfirmation]
        );

        chain.failed.push(split_txid);
        let report = reconcile(&state, &chain);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::RetrySplit { txid: split_txid }]
        );

        chain.failed.clear();
        chain
            .confirmed
            .insert(split_txid, BlockHeight::from_u32(9_000));
        let report = reconcile(&state, &chain);
        assert_eq!(report.actions, vec![RecommendedAction::BindAndSchedule]);
    }

    #[test]
    fn reinstall_offers_a_fresh_migration() {
        // No persisted migration state at all: reconcile is never reached.
        // The wallet's remaining Orchard balance produces a fresh plan. This
        // test pins the adjacent behavior: a Planned state only recommends
        // continuing note splitting.
        let mut state = scheduled_state(Vec::new());
        state.phase = MigrationPhase::Planned;
        let report = reconcile(&state, &MockChainView::default());
        assert_eq!(
            report.actions,
            vec![RecommendedAction::ContinueNoteSplitting]
        );
    }

    /// After a restore from backup the wallet's transaction map no longer
    /// contains an in-flight split transaction, but the persisted migration
    /// state still names it in `pending_txids`. An unknown txid is not
    /// *recorded as failed* (the `ChainView::transaction_failed` contract),
    /// so reconciliation must keep waiting rather than retry the split and
    /// race its own in-flight transaction. This exercises the real
    /// `LightWallet` implementation, not `MockChainView`.
    #[test]
    fn unknown_txid_after_restore_is_not_classified_failed() {
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

        let wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();

        let in_flight_txid = TxId::from_bytes([42; 32]);
        assert!(!wallet.wallet_transactions.contains_key(&in_flight_txid));

        // Contract-level assertion: an unknown transaction is not failed.
        assert!(
            !wallet.transaction_failed(&in_flight_txid),
            "an unknown txid must not be reported as failed",
        );

        // Behavior-level assertion: reconciliation awaits confirmation
        // instead of recommending a retry that races the in-flight split.
        let mut state = scheduled_state(Vec::new());
        state.phase = MigrationPhase::NoteSplitting {
            round: 0,
            pending_txids: vec![in_flight_txid],
        };
        let report = reconcile(&state, &wallet);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::AwaitSplitConfirmation],
        );
    }
}
