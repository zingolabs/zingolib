use super::super::parts::{BoundNote, PartRecord};
use super::super::{ConsentBinding, MigrationParams, SigningStrategy};
use super::*;
use crate::config::ChainType;
use std::collections::HashMap;

struct MockChainView {
    tip: Option<BlockHeight>,
    spend_evidence: Option<BlockHeight>,
    note_spends: HashMap<OutputId, (TxId, Option<BlockHeight>)>,
    confirmed: HashMap<TxId, BlockHeight>,
    failed: Vec<TxId>,
    orchard_spendable: u64,
}

impl Default for MockChainView {
    fn default() -> Self {
        MockChainView {
            tip: Some(BlockHeight::from_u32(10_000)),
            // Evidence keeps pace with the tip unless a test says
            // otherwise.
            spend_evidence: Some(BlockHeight::from_u32(10_000)),
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
    /// Mainnet's grid, uniform at every height these tests reach.
    fn bucket_grid(&self, params: &MigrationParams) -> BucketGrid {
        params.grid(ChainType::Mainnet)
    }
    fn spend_evidence_height(&self) -> Option<BlockHeight> {
        self.spend_evidence
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

/// Bucket arithmetic for the provisional M = 144 and tip 10_000: the tip
/// sits in bucket 69.
const M: u32 = 144;
const TIP_BUCKET: u64 = 10_000 / M as u64;

/// Issue #2493, finding 8: a migration whose every part is terminal
/// and whose replannable balance is zero must reach a terminal
/// recommendation. An `Invalidated` part with nothing left to replan
/// currently satisfies neither `MarkComplete` (not every part is
/// `Confirmed`) nor `ReplanRemainder` (no spendable balance), so
/// reconcile recommends nothing forever and the stuck state blocks
/// both the immediate migration and the immediate migration until the user finds
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
            (TIP_BUCKET as u32) * M + SLIP_TOLERANCE_REFERENCE_BLOCKS,
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
        (TIP_BUCKET as u32) * M + SLIP_TOLERANCE_REFERENCE_BLOCKS + 1,
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
fn due_now_reports_the_current_window_regardless_of_the_target() {
    // Tip 10_000 sits in bucket TIP_BUCKET; its window is
    // [TIP_BUCKET*144, (TIP_BUCKET+1)*144) = [9936, 10080).
    let mut part = assigned_part(0, TIP_BUCKET);
    part.target_height = Some(BlockHeight::from_u32(10_040)); // advisory only
    let state = scheduled_state(vec![part]);

    // The tip is below the random target, yet the window is open: the part
    // is due, because the target no longer gates sendability.
    let chain = MockChainView::default();
    let report = reconcile(&state, &chain);
    assert_eq!(
        due_now_parts(
            &state.parts,
            &report,
            chain.chain_tip().unwrap(),
            &chain.bucket_grid(&state.params),
        ),
        vec![PartId(0)],
        "a current-window part is due even with its target ahead",
    );

    // And it stays due later in the window, past the target.
    let chain = MockChainView {
        tip: Some(BlockHeight::from_u32(10_060)),
        ..Default::default()
    };
    let report = reconcile(&state, &chain);
    assert_eq!(
        due_now_parts(
            &state.parts,
            &report,
            BlockHeight::from_u32(10_060),
            &chain.bucket_grid(&state.params),
        ),
        vec![PartId(0)],
    );
}

#[test]
fn due_now_folds_assigned_overdue_parts_but_not_signed_ones() {
    // Bucket TIP_BUCKET - 2 closed well beyond the slip tolerance. The
    // Assigned part is Overdue and folds into the batch; the Signed one is
    // classified AwaitingExpiry (it waits out its expiry and rebuilds
    // rather than transmitting late), so it is outside the catch-up batch
    // and never folds.
    let assigned = assigned_part(0, TIP_BUCKET - 2);
    let mut signed = assigned_part(1, TIP_BUCKET - 2);
    signed
        .mark_signed(
            TxId::from_bytes([9; 32]),
            BlockHeight::from_u32(20_000),
            None,
        )
        .unwrap();
    let state = scheduled_state(vec![assigned, signed]);

    let chain = MockChainView::default();
    let report = reconcile(&state, &chain);
    assert!(
        report.actions.contains(&RecommendedAction::PromptCatchUp {
            parts: vec![PartId(0)],
            disclosure_required: true,
        }),
        "only the Assigned overdue part is surfaced for catch-up",
    );
    assert!(
        matches!(
            report
                .assessments
                .iter()
                .find(|assessment| assessment.id == PartId(1))
                .map(|assessment| assessment.class),
            Some(PartClass::AwaitingExpiry { .. })
        ),
        "the signed overdue part awaits its expiry instead of catching up",
    );
    assert_eq!(
        due_now_parts(
            &state.parts,
            &report,
            chain.chain_tip().unwrap(),
            &chain.bucket_grid(&state.params),
        ),
        vec![PartId(0)],
        "only the Assigned overdue part is sendable this instant",
    );
}

#[test]
fn due_now_excludes_future_windows() {
    let part = assigned_part(0, TIP_BUCKET + 3);
    let state = scheduled_state(vec![part]);
    let chain = MockChainView::default();
    let report = reconcile(&state, &chain);
    assert!(
        due_now_parts(
            &state.parts,
            &report,
            chain.chain_tip().unwrap(),
            &chain.bucket_grid(&state.params),
        )
        .is_empty(),
        "a part whose window has not opened is a future window, not a due batch",
    );
}

#[test]
fn due_now_omits_a_current_part_reconciliation_will_invalidate() {
    // A current-window Assigned part whose bound note was spent outside the
    // migration: reconcile classifies it Invalidated, so a tap would not
    // send it and `due_now` must not advertise it. With no random target
    // it clears the bucket predicate, so only the OnTrack class gate drops
    // it, the property that gate exists for.
    let mut part = assigned_part(0, TIP_BUCKET);
    part.target_height = None;
    let state = scheduled_state(vec![part]);
    let mut chain = MockChainView::default();
    chain.note_spends.insert(
        output_id(0),
        (
            TxId::from_bytes([250; 32]),
            Some(BlockHeight::from_u32(9_990)),
        ),
    );
    let report = reconcile(&state, &chain);
    assert_eq!(report.assessments[0].class, PartClass::Invalidated);
    assert!(
        due_now_parts(
            &state.parts,
            &report,
            chain.chain_tip().unwrap(),
            &chain.bucket_grid(&state.params),
        )
        .is_empty(),
        "an externally-spent part is not sendable and must not be advertised",
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
    assert_eq!(
        report.actions,
        vec![RecommendedAction::ContinueNoteSplitting],
        "a fully confirmed round hands back to the splitting driver, \
             which alone can tell a finished split from one needing more rounds"
    );
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
