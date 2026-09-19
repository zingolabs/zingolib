//! Reconciliation (ZIP 318: the on-launch reconciliation).
//!
//! [`reconcile`] is a pure function of the persisted [`MigrationState`] and a
//! read-only [`ChainView`], so a client can call it after every sync and at
//! the start of every command. It classifies every transfer and returns the
//! actions to apply. Applying them is the caller's job.

use pepper_sync::wallet::OutputId;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

use super::transfers::{TransferId, TransferRecord, TransferState};
use super::{MigrationPhase, MigrationState};

/// Read-only chain facts, implemented by the wallet and by test mocks.
/// Everything reconciliation knows about the chain flows through here.
pub trait ChainView {
    /// The highest block height the wallet knows of.
    fn chain_tip(&self) -> Option<BlockHeight>;

    /// The Spend-Evidence Height: the height through which the wallet's
    /// evidence of spends and transaction inclusion is complete, with every
    /// block at or below it scanned with its nullifiers mapped. This is
    /// the only lawful input to judgments that condemn (a transaction
    /// expired-unmined, a transfer dead): the chain tip runs ahead of
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

/// How reconciliation classified one transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferClass {
    /// Nothing to do: waiting for its window, its window is open, or its
    /// transaction is in flight.
    OnTrack,
    /// Its window closed without a confirmation. Reconciliation reschedules
    /// it into a later window.
    Missed,
    /// Its transaction reached expiry unmined. Rescheduled the same way.
    Expired,
    /// Its bound note was spent outside the migration.
    Invalidated,
    /// Mined and confirmed.
    Confirmed,
    /// Recorded as confirmed, but the chain no longer holds its transaction.
    Reorged,
    /// The user took it out of the migration.
    Released,
}

/// What the caller should do next. Every action is safe unattended and is
/// applied by the client's reconciliation pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecommendedAction {
    /// A note-preparation round is still confirming. Keep waiting.
    AwaitPreparationConfirmation,
    /// A note-preparation transaction failed or expired. Its notes are free
    /// again, and the next preparation round replans over them.
    RetryPreparation {
        /// The failed transaction.
        txid: TxId,
    },
    /// Note preparation needs driving: it has not started, or the pending
    /// round fully confirmed. Only a replan can tell whether more rounds
    /// remain, which pure reconciliation cannot perform.
    ContinueNotePreparation,
    /// The transfer's own transaction mined but the record lagged (for
    /// example a crash between submit and record).
    PromoteConfirmed {
        /// The transfer to promote.
        transfer: TransferId,
        /// The confirmation height.
        height: BlockHeight,
    },
    /// The transfer's bound note was spent outside the migration.
    MarkInvalidated {
        /// The transfer to mark.
        transfer: TransferId,
    },
    /// The transfer missed its window and carries no signature. Place it in
    /// a later window.
    Reschedule {
        /// The transfer to place again.
        transfer: TransferId,
    },
    /// The transfer carries a signature that will not mine: it never left
    /// the device, or the chain shows no trace of it one window after its
    /// own closed, or it expired. Drop the signature and place it in a
    /// later window.
    DiscardAndReschedule {
        /// The transfer to re-sign later.
        transfer: TransferId,
    },
    /// Every transfer is terminal and the evidence is complete. Only the
    /// residual remains in Orchard.
    MarkComplete {
        /// The unmigrated leftover, in zatoshis.
        residual: u64,
    },
    /// A confirmed transfer lost its block. Demote it to broadcast.
    Demote {
        /// The transfer to demote.
        transfer: TransferId,
    },
    /// A completed migration holds a reorged transfer. Return to `Scheduled`.
    Reopen,
    /// A released transfer's transaction did not mine one window after its
    /// own closed, or reached its expiry. Mark it failed and forget it.
    AbandonWire {
        /// The released transfer.
        transfer: TransferId,
    },
}

/// One transfer's classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferAssessment {
    /// The transfer.
    pub id: TransferId,
    /// Its classification.
    pub class: TransferClass,
}

/// The outcome of a reconciliation pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcileReport {
    /// Per-transfer classifications (empty before transfers are bound).
    pub assessments: Vec<TransferAssessment>,
    /// What to do about them, deduplicated.
    pub actions: Vec<RecommendedAction>,
}

/// Classifies every transfer of `state` against the chain view and
/// recommends actions. Pure: reads nothing but its arguments and writes
/// nothing.
pub fn reconcile(state: &MigrationState, chain: &impl ChainView) -> ReconcileReport {
    let mut report = ReconcileReport::default();

    match &state.phase {
        MigrationPhase::Committed => {
            report
                .actions
                .push(RecommendedAction::ContinueNotePreparation);
            return report;
        }
        MigrationPhase::Prepared => return report,
        MigrationPhase::Preparing { pending_txids, .. } => {
            let mut all_confirmed = true;
            for txid in pending_txids {
                if chain.transaction_failed(txid) {
                    report
                        .actions
                        .push(RecommendedAction::RetryPreparation { txid: *txid });
                    all_confirmed = false;
                } else if chain.transaction_confirmed_height(txid).is_none() {
                    all_confirmed = false;
                }
            }
            if all_confirmed {
                report
                    .actions
                    .push(RecommendedAction::ContinueNotePreparation);
            } else if report.actions.is_empty() {
                report
                    .actions
                    .push(RecommendedAction::AwaitPreparationConfirmation);
            }
            return report;
        }
        MigrationPhase::Complete { .. } => {
            let chain_tip = chain.chain_tip();
            for transfer in &state.transfers {
                let class = classify(transfer, state, chain, chain_tip, &mut report);
                report.assessments.push(TransferAssessment {
                    id: transfer.id,
                    class,
                });
            }
            if report
                .assessments
                .iter()
                .any(|assessment| assessment.class == TransferClass::Reorged)
            {
                report.actions.push(RecommendedAction::Reopen);
            }
            return report;
        }
        MigrationPhase::Scheduled => (),
    }

    let chain_tip = chain.chain_tip();
    let all_terminal = state
        .transfers
        .iter()
        .all(|transfer| transfer.state.is_terminal());
    let mut all_settled = true;
    for transfer in &state.transfers {
        let class = classify(transfer, state, chain, chain_tip, &mut report);
        all_settled &= matches!(
            (transfer.state, class),
            (TransferState::Confirmed { .. }, TransferClass::Confirmed)
                | (TransferState::Invalidated, TransferClass::Invalidated)
                | (TransferState::Released, TransferClass::Released)
        );
        report.assessments.push(TransferAssessment {
            id: transfer.id,
            class,
        });
    }

    let evidence_complete = chain_tip.is_some_and(|tip| {
        chain
            .spend_evidence_height()
            .is_some_and(|evidence| evidence >= tip)
    });
    if all_terminal && all_settled && evidence_complete {
        report.actions.push(RecommendedAction::MarkComplete {
            residual: chain.orchard_confirmed_spendable(state.account),
        });
    }

    report
}

/// The transfers a broadcast would attempt this instant: the ones in the
/// window the chain is inside, still awaiting broadcast, that reconciliation
/// leaves on track. `report` must come from [`reconcile`] over the same
/// `transfers`.
pub fn due_now_transfers(
    transfers: &[TransferRecord],
    report: &ReconcileReport,
    now_height: BlockHeight,
    params: &super::params::MigrationParams,
) -> Vec<TransferId> {
    let current_bucket = super::schedule::bucket_index(now_height, params.bucket_modulus);
    transfers
        .iter()
        .filter(|transfer| {
            report.assessments.iter().any(|assessment| {
                assessment.id == transfer.id && assessment.class == TransferClass::OnTrack
            }) && super::schedule::transfer_in_current_bucket(transfer, current_bucket)
        })
        .map(|transfer| transfer.id)
        .collect()
}

fn classify_released_wire(
    transfer: &TransferRecord,
    state: &MigrationState,
    chain: &impl ChainView,
    chain_tip: Option<BlockHeight>,
    report: &mut ReconcileReport,
) -> TransferClass {
    let spend = transfer
        .note
        .and_then(|bound| chain.note_spend(bound.output_id));
    match spend {
        Some((txid, Some(height))) if transfer.owns_txid(&txid) => {
            report.actions.push(RecommendedAction::PromoteConfirmed {
                transfer: transfer.id,
                height,
            });
            return TransferClass::Confirmed;
        }
        Some((txid, Some(_))) if !transfer.owns_txid(&txid) => {
            report.actions.push(RecommendedAction::AbandonWire {
                transfer: transfer.id,
            });
            return TransferClass::OnTrack;
        }
        _ => (),
    }
    let evidence = chain.spend_evidence_height();
    let expired = transfer
        .expiry_height
        .is_some_and(|expiry| evidence.is_some_and(|evidence| evidence >= expiry));
    let modulus = state.params.bucket_modulus;
    let lost = chain_tip.is_some()
        && transfer.bucket_index.is_some_and(|bucket| {
            let window_end = super::schedule::boundary_of(bucket + 1, modulus);
            evidence.is_some_and(|evidence| evidence >= window_end + modulus)
        });
    if expired || lost {
        report.actions.push(RecommendedAction::AbandonWire {
            transfer: transfer.id,
        });
    }
    TransferClass::OnTrack
}

fn classify(
    transfer: &TransferRecord,
    state: &MigrationState,
    chain: &impl ChainView,
    chain_tip: Option<BlockHeight>,
    report: &mut ReconcileReport,
) -> TransferClass {
    match transfer.state {
        TransferState::Confirmed { height } => {
            let evidence_reached = chain
                .spend_evidence_height()
                .is_some_and(|evidence| evidence >= height);
            if !evidence_reached {
                return TransferClass::Confirmed;
            }
            let spend = transfer
                .note
                .and_then(|bound| chain.note_spend(bound.output_id));
            return match spend {
                Some((txid, Some(_))) if transfer.owns_txid(&txid) => TransferClass::Confirmed,
                Some((_, Some(_))) => {
                    report.actions.push(RecommendedAction::MarkInvalidated {
                        transfer: transfer.id,
                    });
                    TransferClass::Invalidated
                }
                _ => {
                    report.actions.push(RecommendedAction::Demote {
                        transfer: transfer.id,
                    });
                    TransferClass::Reorged
                }
            };
        }
        TransferState::Invalidated => return TransferClass::Invalidated,
        TransferState::Released if transfer.txid.is_none() => return TransferClass::Released,
        TransferState::Released => {
            return classify_released_wire(transfer, state, chain, chain_tip, report);
        }
        TransferState::Expired => {
            report.actions.push(RecommendedAction::Reschedule {
                transfer: transfer.id,
            });
            return TransferClass::Expired;
        }
        TransferState::Bound
        | TransferState::Assigned
        | TransferState::Signed
        | TransferState::Broadcast => (),
    }

    if let Some(bound) = transfer.note
        && let Some((spending_txid, height)) = chain.note_spend(bound.output_id)
    {
        if !transfer.owns_txid(&spending_txid) {
            report.actions.push(RecommendedAction::MarkInvalidated {
                transfer: transfer.id,
            });
            return TransferClass::Invalidated;
        }
        if let Some(height) = height {
            report.actions.push(RecommendedAction::PromoteConfirmed {
                transfer: transfer.id,
                height,
            });
            return TransferClass::Confirmed;
        }
    }

    let Some(tip) = chain_tip else {
        return TransferClass::OnTrack;
    };
    let evidence = chain.spend_evidence_height();
    let signed = matches!(
        transfer.state,
        TransferState::Signed | TransferState::Broadcast
    );

    if signed
        && transfer
            .expiry_height
            .is_some_and(|expiry_height| evidence.is_some_and(|evidence| evidence >= expiry_height))
    {
        report
            .actions
            .push(RecommendedAction::DiscardAndReschedule {
                transfer: transfer.id,
            });
        return TransferClass::Expired;
    }

    let Some(bucket) = transfer.bucket_index else {
        return TransferClass::OnTrack;
    };
    let modulus = state.params.bucket_modulus;
    let window_end = super::schedule::boundary_of(bucket + 1, modulus);
    if tip < window_end {
        return TransferClass::OnTrack;
    }
    match transfer.state {
        TransferState::Assigned => {
            report.actions.push(RecommendedAction::Reschedule {
                transfer: transfer.id,
            });
            TransferClass::Missed
        }
        TransferState::Signed if transfer.attempts == 0 => {
            report
                .actions
                .push(RecommendedAction::DiscardAndReschedule {
                    transfer: transfer.id,
                });
            TransferClass::Missed
        }
        TransferState::Signed | TransferState::Broadcast => {
            let lost = evidence.is_some_and(|evidence| evidence >= window_end + modulus);
            if lost {
                report
                    .actions
                    .push(RecommendedAction::DiscardAndReschedule {
                        transfer: transfer.id,
                    });
                TransferClass::Missed
            } else {
                TransferClass::OnTrack
            }
        }
        _ => TransferClass::OnTrack,
    }
}

impl ChainView for crate::wallet::LightWallet {
    fn chain_tip(&self) -> Option<BlockHeight> {
        self.sync_state.last_known_chain_height()
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
        // backup an in-flight preparation transactions has no wallet record yet may
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
    use std::collections::HashMap;

    use super::super::schedule::{boundary_of, bucket_index};
    use super::super::transfers::{BoundNote, TransferRecord};
    use super::super::{MigrationMode, MigrationParams, PlanCommitment, SigningStrategy};
    use super::*;
    use crate::config::ChainType;

    const TIP: u32 = 10_000;

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
                tip: Some(height(TIP)),
                spend_evidence: Some(height(TIP)),
                note_spends: HashMap::new(),
                confirmed: HashMap::new(),
                failed: Vec::new(),
                orchard_spendable: 0,
            }
        }
    }

    impl MockChainView {
        fn at(tip: u32, evidence: u32) -> Self {
            MockChainView {
                tip: Some(height(tip)),
                spend_evidence: Some(height(evidence)),
                ..Default::default()
            }
        }

        fn with_note_spend(mut self, seed: u8, txid: TxId, mined_at: Option<u32>) -> Self {
            self.note_spends
                .insert(output_id(seed), (txid, mined_at.map(height)));
            self
        }
    }

    impl ChainView for MockChainView {
        fn chain_tip(&self) -> Option<BlockHeight> {
            self.tip
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

    fn height(h: u32) -> BlockHeight {
        BlockHeight::from_u32(h)
    }

    fn txid(seed: u8) -> TxId {
        TxId::from_bytes([seed; 32])
    }

    fn params() -> MigrationParams {
        MigrationParams::provisional(ChainType::Mainnet)
    }

    fn modulus() -> u32 {
        params().bucket_modulus
    }

    fn tip_bucket() -> u64 {
        bucket_index(height(TIP), modulus())
    }

    fn window_end(bucket: u64) -> u32 {
        u32::from(boundary_of(bucket + 1, modulus()))
    }

    fn output_id(seed: u8) -> OutputId {
        OutputId::new(TxId::from_bytes([seed; 32]), u32::from(seed))
    }

    fn scheduled_state(transfers: Vec<TransferRecord>) -> MigrationState {
        let params = params();
        MigrationState {
            commitment: PlanCommitment {
                params_hash: params.params_hash(),
                plan_hash: [0; 32],
                committed_at: 0,
            },
            params,
            strategy: SigningStrategy::LazyAtBoundary,
            mode: MigrationMode::Scheduled,
            account: zip32::AccountId::ZERO,
            phase: MigrationPhase::Scheduled,
            transfers,
        }
    }

    fn assigned_transfer(id: u32, bucket: u64) -> TransferRecord {
        let seed = u8::try_from(id).expect("test ids fit u8");
        let mut transfer = TransferRecord::new(
            TransferId(id),
            100_000_000,
            BoundNote {
                output_id: output_id(seed),
                nullifier: [seed; 32],
                commitment: [seed; 32],
            },
        );
        transfer.assign(bucket).unwrap();
        transfer
    }

    fn signed_transfer(id: u32, bucket: u64, own: TxId, expiry: u32) -> TransferRecord {
        let mut transfer = assigned_transfer(id, bucket);
        transfer.mark_signed(own, height(expiry), None).unwrap();
        transfer
    }

    fn broadcast_transfer(id: u32, bucket: u64, own: TxId, expiry: u32) -> TransferRecord {
        let mut transfer = signed_transfer(id, bucket, own, expiry);
        transfer.record_attempt();
        transfer.mark_broadcast().unwrap();
        transfer
    }

    fn confirmed_transfer(id: u32, own: TxId, confirmed_at: u32) -> TransferRecord {
        let mut transfer = broadcast_transfer(id, tip_bucket() - 2, own, 20_000);
        transfer.mark_confirmed(height(confirmed_at)).unwrap();
        transfer
    }

    fn class_of(report: &ReconcileReport, id: u32) -> TransferClass {
        report
            .assessments
            .iter()
            .find(|assessment| assessment.id == TransferId(id))
            .map(|assessment| assessment.class)
            .unwrap_or_else(|| panic!("transfer {id} was not assessed: {report:?}"))
    }

    fn recommends_completion(report: &ReconcileReport) -> bool {
        report
            .actions
            .iter()
            .any(|action| matches!(action, RecommendedAction::MarkComplete { .. }))
    }

    fn due_now(state: &MigrationState, report: &ReconcileReport, now: u32) -> Vec<TransferId> {
        due_now_transfers(&state.transfers, report, height(now), &state.params)
    }

    #[test]
    fn committed_phase_continues_note_preparation() {
        let mut state = scheduled_state(Vec::new());
        state.phase = MigrationPhase::Committed;

        let report = reconcile(&state, &MockChainView::default());

        assert_eq!(
            report.actions,
            vec![RecommendedAction::ContinueNotePreparation]
        );
        assert!(report.assessments.is_empty());
    }

    #[test]
    fn prepared_phase_recommends_nothing() {
        let mut state = scheduled_state(Vec::new());
        state.phase = MigrationPhase::Prepared;

        let report = reconcile(&state, &MockChainView::default());

        assert_eq!(report, ReconcileReport::default());
    }

    #[test]
    fn preparing_phase_awaits_an_unconfirmed_round() {
        let mut state = scheduled_state(Vec::new());
        state.phase = MigrationPhase::Preparing {
            round: 0,
            pending_txids: vec![txid(1), txid(2)],
        };
        let mut chain = MockChainView::default();
        chain.confirmed.insert(txid(1), height(9_000));

        let report = reconcile(&state, &chain);

        assert_eq!(
            report.actions,
            vec![RecommendedAction::AwaitPreparationConfirmation],
            "one unconfirmed preparation transaction holds the round"
        );
    }

    #[test]
    fn preparing_phase_retries_a_failed_round_transaction() {
        let mut state = scheduled_state(Vec::new());
        state.phase = MigrationPhase::Preparing {
            round: 0,
            pending_txids: vec![txid(1), txid(2)],
        };
        let mut chain = MockChainView::default();
        chain.failed.push(txid(1));

        let report = reconcile(&state, &chain);

        assert_eq!(
            report.actions,
            vec![RecommendedAction::RetryPreparation { txid: txid(1) }],
            "a failed preparation transaction is retried and the round is not awaited"
        );
    }

    #[test]
    fn preparing_phase_continues_note_preparation_once_the_round_confirms() {
        let mut state = scheduled_state(Vec::new());
        state.phase = MigrationPhase::Preparing {
            round: 1,
            pending_txids: vec![txid(1), txid(2)],
        };
        let mut chain = MockChainView::default();
        chain.confirmed.insert(txid(1), height(9_000));
        chain.confirmed.insert(txid(2), height(9_001));

        let report = reconcile(&state, &chain);

        assert_eq!(
            report.actions,
            vec![RecommendedAction::ContinueNotePreparation]
        );
    }

    #[test]
    fn an_unknown_preparation_txid_after_restore_awaits_confirmation() {
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

        let wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build();
        let in_flight = txid(42);
        assert!(!wallet.wallet_transactions.contains_key(&in_flight));
        assert!(
            !wallet.transaction_failed(&in_flight),
            "an unknown txid is not recorded as failed"
        );

        let mut state = scheduled_state(Vec::new());
        state.phase = MigrationPhase::Preparing {
            round: 0,
            pending_txids: vec![in_flight],
        };
        let report = reconcile(&state, &wallet);

        assert_eq!(
            report.actions,
            vec![RecommendedAction::AwaitPreparationConfirmation],
            "reconciliation waits rather than retrying a round that may still confirm"
        );
    }

    #[test]
    fn complete_phase_reopens_on_a_reorged_transfer() {
        let own = txid(9);
        let mut state = scheduled_state(vec![confirmed_transfer(0, own, 9_990)]);
        state.phase = MigrationPhase::Complete { residual: 0 };

        let report = reconcile(&state, &MockChainView::default());

        assert_eq!(class_of(&report, 0), TransferClass::Reorged);
        assert!(report.actions.contains(&RecommendedAction::Demote {
            transfer: TransferId(0)
        }));
        assert!(report.actions.contains(&RecommendedAction::Reopen));
    }

    #[test]
    fn complete_phase_with_settled_transfers_recommends_nothing() {
        let own = txid(9);
        let mut state = scheduled_state(vec![confirmed_transfer(0, own, 9_990)]);
        state.phase = MigrationPhase::Complete { residual: 0 };
        let chain = MockChainView::default().with_note_spend(0, own, Some(9_990));

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 0), TransferClass::Confirmed);
        assert!(report.actions.is_empty(), "{:?}", report.actions);
    }

    #[test]
    fn assigned_transfers_in_the_open_and_future_windows_are_on_track() {
        let state = scheduled_state(vec![
            assigned_transfer(0, tip_bucket()),
            assigned_transfer(1, tip_bucket() + 3),
        ]);

        let report = reconcile(&state, &MockChainView::default());

        assert_eq!(class_of(&report, 0), TransferClass::OnTrack);
        assert_eq!(class_of(&report, 1), TransferClass::OnTrack);
        assert!(report.actions.is_empty(), "{:?}", report.actions);
    }

    #[test]
    fn assigned_transfer_whose_window_closed_is_missed_and_rescheduled() {
        let state = scheduled_state(vec![assigned_transfer(0, tip_bucket() - 1)]);

        let report = reconcile(&state, &MockChainView::default());

        assert_eq!(class_of(&report, 0), TransferClass::Missed);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::Reschedule {
                transfer: TransferId(0)
            }]
        );
    }

    #[test]
    fn assigned_transfer_is_on_track_until_the_block_that_closes_its_window() {
        let bucket = tip_bucket() - 1;
        let state = scheduled_state(vec![assigned_transfer(0, bucket)]);
        let last_block = window_end(bucket) - 1;

        let report = reconcile(&state, &MockChainView::at(last_block, last_block));
        assert_eq!(class_of(&report, 0), TransferClass::OnTrack);

        let report = reconcile(&state, &MockChainView::at(last_block + 1, last_block));
        assert_eq!(
            class_of(&report, 0),
            TransferClass::Missed,
            "the window closes at the tip, without waiting for evidence"
        );
    }

    #[test]
    fn unsent_signed_transfer_past_its_window_is_missed_and_its_signature_discarded() {
        let transfer = signed_transfer(0, tip_bucket() - 1, txid(9), 20_000);
        assert_eq!(transfer.attempts, 0);
        let state = scheduled_state(vec![transfer]);

        let report = reconcile(&state, &MockChainView::default());

        assert_eq!(class_of(&report, 0), TransferClass::Missed);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::DiscardAndReschedule {
                transfer: TransferId(0)
            }]
        );
    }

    #[test]
    fn submitted_transfer_past_its_window_stays_on_track_for_one_more_window() {
        let bucket = tip_bucket() - 1;
        let state = scheduled_state(vec![broadcast_transfer(0, bucket, txid(9), 20_000)]);
        let cutoff = window_end(bucket) + modulus();

        let report = reconcile(&state, &MockChainView::at(cutoff, cutoff - 1));
        assert_eq!(class_of(&report, 0), TransferClass::OnTrack);
        assert!(report.actions.is_empty(), "{:?}", report.actions);

        let report = reconcile(&state, &MockChainView::at(cutoff, cutoff));
        assert_eq!(class_of(&report, 0), TransferClass::Missed);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::DiscardAndReschedule {
                transfer: TransferId(0)
            }]
        );
    }

    #[test]
    fn signed_transfer_with_an_attempt_recorded_is_treated_as_submitted() {
        let bucket = tip_bucket() - 1;
        let mut transfer = signed_transfer(0, bucket, txid(9), 20_000);
        transfer.record_attempt();
        let state = scheduled_state(vec![transfer]);
        let cutoff = window_end(bucket) + modulus();

        let report = reconcile(&state, &MockChainView::at(cutoff, cutoff - 1));
        assert_eq!(
            class_of(&report, 0),
            TransferClass::OnTrack,
            "a crash between the attempt record and the broadcast record keeps the signature"
        );

        let report = reconcile(&state, &MockChainView::at(cutoff, cutoff));
        assert_eq!(class_of(&report, 0), TransferClass::Missed);
        assert!(
            report
                .actions
                .contains(&RecommendedAction::DiscardAndReschedule {
                    transfer: TransferId(0)
                })
        );
    }

    #[test]
    fn submitted_transfer_whose_expiry_the_evidence_reached_is_expired_and_discarded() {
        let expiry = 9_990;
        let state = scheduled_state(vec![broadcast_transfer(
            0,
            tip_bucket() - 1,
            txid(9),
            expiry,
        )]);

        let report = reconcile(&state, &MockChainView::at(TIP, expiry - 1));
        assert_eq!(
            class_of(&report, 0),
            TransferClass::OnTrack,
            "the tip past the expiry is not evidence that the transaction did not mine"
        );

        let report = reconcile(&state, &MockChainView::at(TIP, expiry));
        assert_eq!(class_of(&report, 0), TransferClass::Expired);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::DiscardAndReschedule {
                transfer: TransferId(0)
            }]
        );
    }

    #[test]
    fn own_spend_mined_promotes_the_transfer_to_confirmed() {
        let own = txid(9);
        let state = scheduled_state(vec![signed_transfer(0, tip_bucket() - 1, own, 20_000)]);
        let chain = MockChainView::default().with_note_spend(0, own, Some(9_995));

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 0), TransferClass::Confirmed);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::PromoteConfirmed {
                transfer: TransferId(0),
                height: height(9_995),
            }]
        );
    }

    #[test]
    fn own_spend_by_a_discarded_signature_promotes_rather_than_invalidates() {
        let discarded = txid(9);
        let mut transfer = signed_transfer(0, tip_bucket() - 3, discarded, 9_500);
        transfer.discard_signature().unwrap();
        transfer.reassign(tip_bucket() + 1).unwrap();
        assert_eq!(transfer.previous_txids, vec![discarded]);
        let state = scheduled_state(vec![transfer]);
        let chain = MockChainView::default().with_note_spend(0, discarded, Some(9_400));

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 0), TransferClass::Confirmed);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::PromoteConfirmed {
                transfer: TransferId(0),
                height: height(9_400),
            }]
        );
    }

    #[test]
    fn own_spend_pending_keeps_the_transfer_on_track_while_its_window_checks_apply() {
        let own = txid(9);
        let state = scheduled_state(vec![broadcast_transfer(0, tip_bucket(), own, 20_000)]);
        let chain = MockChainView::default().with_note_spend(0, own, None);

        let report = reconcile(&state, &chain);
        assert_eq!(class_of(&report, 0), TransferClass::OnTrack);
        assert!(report.actions.is_empty(), "{:?}", report.actions);

        let bucket = tip_bucket() - 1;
        let state = scheduled_state(vec![broadcast_transfer(0, bucket, own, 20_000)]);
        let cutoff = window_end(bucket) + modulus();
        let chain = MockChainView::at(cutoff, cutoff).with_note_spend(0, own, None);

        let report = reconcile(&state, &chain);
        assert_eq!(
            class_of(&report, 0),
            TransferClass::Missed,
            "a pending own spend does not exempt the transfer from the extra-window cutoff"
        );
        assert!(
            report
                .actions
                .contains(&RecommendedAction::DiscardAndReschedule {
                    transfer: TransferId(0)
                })
        );
    }

    #[test]
    fn foreign_spend_invalidates_the_transfer() {
        let foreign = txid(250);
        let state = scheduled_state(vec![assigned_transfer(0, tip_bucket() + 1)]);

        let chain = MockChainView::default().with_note_spend(0, foreign, Some(9_990));
        let report = reconcile(&state, &chain);
        assert_eq!(class_of(&report, 0), TransferClass::Invalidated);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::MarkInvalidated {
                transfer: TransferId(0)
            }]
        );

        let chain = MockChainView::default().with_note_spend(0, foreign, None);
        let report = reconcile(&state, &chain);
        assert_eq!(
            class_of(&report, 0),
            TransferClass::Invalidated,
            "a pending foreign spend already takes the funding note"
        );
    }

    #[test]
    fn persisted_expired_transfer_is_rescheduled() {
        let mut transfer = assigned_transfer(0, tip_bucket() - 2);
        transfer.mark_expired().unwrap();
        let state = scheduled_state(vec![transfer]);

        let report = reconcile(&state, &MockChainView::default());

        assert_eq!(class_of(&report, 0), TransferClass::Expired);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::Reschedule {
                transfer: TransferId(0)
            }]
        );
    }

    #[test]
    fn released_transfer_is_released_with_no_action() {
        let mut transfer = assigned_transfer(0, tip_bucket() - 2);
        transfer.mark_released().unwrap();
        let state = scheduled_state(vec![transfer]);
        let chain = MockChainView::default().with_note_spend(0, txid(250), Some(9_990));

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 0), TransferClass::Released);
        assert!(
            !report
                .actions
                .contains(&RecommendedAction::MarkInvalidated {
                    transfer: TransferId(0)
                }),
            "a released note is the user's to spend: {:?}",
            report.actions
        );
    }

    fn released_on_the_wire(id: u32, bucket: u64, own: TxId, expiry: u32) -> TransferRecord {
        let mut transfer = broadcast_transfer(id, bucket, own, expiry);
        transfer.mark_released().unwrap();
        assert!(transfer.is_on_the_wire());
        transfer
    }

    #[test]
    fn released_transfer_whose_own_spend_mined_is_promoted_to_confirmed() {
        let own = txid(9);
        let state = scheduled_state(vec![released_on_the_wire(0, tip_bucket() - 1, own, 20_000)]);
        let chain = MockChainView::default().with_note_spend(0, own, Some(9_995));

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 0), TransferClass::Confirmed);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::PromoteConfirmed {
                transfer: TransferId(0),
                height: height(9_995),
            }]
        );
    }

    #[test]
    fn released_transfer_whose_discarded_signature_mined_is_promoted_to_confirmed() {
        let discarded = txid(9);
        let current = txid(10);
        let mut transfer = broadcast_transfer(0, tip_bucket() - 3, discarded, 9_500);
        transfer.discard_signature().unwrap();
        transfer.reassign(tip_bucket() - 1).unwrap();
        transfer.mark_signed(current, height(20_000), None).unwrap();
        transfer.record_attempt();
        transfer.mark_broadcast().unwrap();
        transfer.mark_released().unwrap();
        assert_eq!(transfer.previous_txids, vec![discarded]);
        let state = scheduled_state(vec![transfer]);
        let chain = MockChainView::default().with_note_spend(0, discarded, Some(9_400));

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 0), TransferClass::Confirmed);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::PromoteConfirmed {
                transfer: TransferId(0),
                height: height(9_400),
            }]
        );
    }

    #[test]
    fn released_transfer_whose_note_a_foreign_transaction_spent_abandons_the_wire() {
        let own = txid(9);
        let state = scheduled_state(vec![released_on_the_wire(0, tip_bucket() - 1, own, 20_000)]);
        let chain = MockChainView::default().with_note_spend(0, txid(250), Some(9_990));

        let report = reconcile(&state, &chain);

        assert_eq!(
            class_of(&report, 0),
            TransferClass::OnTrack,
            "the pass that abandons the wire does not settle the transfer"
        );
        assert!(report.actions.contains(&RecommendedAction::AbandonWire {
            transfer: TransferId(0)
        }));
        assert!(
            !report
                .actions
                .contains(&RecommendedAction::MarkInvalidated {
                    transfer: TransferId(0)
                }),
            "a released note is never invalidated: {:?}",
            report.actions
        );
    }

    #[test]
    fn released_transfer_with_a_pending_foreign_spend_stays_on_the_wire() {
        let own = txid(9);
        let state = scheduled_state(vec![released_on_the_wire(0, tip_bucket(), own, 20_000)]);
        let chain = MockChainView::default().with_note_spend(0, txid(250), None);

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 0), TransferClass::OnTrack);
        assert!(
            report.actions.is_empty(),
            "an unmined foreign spend decides nothing yet: {:?}",
            report.actions
        );
    }

    #[test]
    fn released_transfer_whose_expiry_the_evidence_reached_abandons_the_wire() {
        let expiry = 9_990;
        let state = scheduled_state(vec![released_on_the_wire(0, tip_bucket(), txid(9), expiry)]);

        let report = reconcile(&state, &MockChainView::at(TIP, expiry - 1));
        assert_eq!(class_of(&report, 0), TransferClass::OnTrack);
        assert!(
            report.actions.is_empty(),
            "the tip past the expiry is not evidence that the transaction did not mine: {:?}",
            report.actions
        );

        let report = reconcile(&state, &MockChainView::at(TIP, expiry));
        assert_eq!(class_of(&report, 0), TransferClass::OnTrack);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::AbandonWire {
                transfer: TransferId(0)
            }]
        );
    }

    #[test]
    fn released_transfer_unspent_one_window_past_its_window_end_abandons_the_wire() {
        let bucket = tip_bucket() - 1;
        let state = scheduled_state(vec![released_on_the_wire(0, bucket, txid(9), 20_000)]);
        let cutoff = window_end(bucket) + modulus();

        let report = reconcile(&state, &MockChainView::at(cutoff, cutoff - 1));
        assert_eq!(class_of(&report, 0), TransferClass::OnTrack);
        assert!(report.actions.is_empty(), "{:?}", report.actions);

        let report = reconcile(&state, &MockChainView::at(cutoff, cutoff));
        assert_eq!(class_of(&report, 0), TransferClass::OnTrack);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::AbandonWire {
                transfer: TransferId(0)
            }]
        );
    }

    #[test]
    fn released_transfer_on_the_wire_is_on_track_and_never_settled_for_completion() {
        let own = txid(9);
        let state = scheduled_state(vec![released_on_the_wire(0, tip_bucket(), own, 20_000)]);

        let report = reconcile(&state, &MockChainView::default());

        assert_eq!(class_of(&report, 0), TransferClass::OnTrack);
        assert!(report.actions.is_empty(), "{:?}", report.actions);
        assert!(
            !recommends_completion(&report),
            "a transaction still on the wire holds the migration open: {:?}",
            report.actions
        );

        let chain = MockChainView::default().with_note_spend(0, own, None);
        let report = reconcile(&state, &chain);
        assert_eq!(class_of(&report, 0), TransferClass::OnTrack);
        assert!(!recommends_completion(&report), "{:?}", report.actions);
    }

    #[test]
    fn released_transfer_without_a_txid_is_settled() {
        let mut transfer = released_on_the_wire(0, tip_bucket(), txid(9), 20_000);
        transfer.forget_wire();
        assert_eq!(transfer.txid, None);
        let state = scheduled_state(vec![transfer]);

        let report = reconcile(&state, &MockChainView::default());

        assert_eq!(class_of(&report, 0), TransferClass::Released);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::MarkComplete { residual: 0 }],
            "an abandoned wire leaves nothing to wait for"
        );
    }

    #[test]
    fn released_transfer_without_a_txid_ignores_spends_of_its_note() {
        let mut transfer = released_on_the_wire(0, tip_bucket(), txid(9), 20_000);
        transfer.forget_wire();
        let state = scheduled_state(vec![transfer]);
        let chain = MockChainView::at(TIP, 9_000).with_note_spend(0, txid(250), Some(9_990));

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 0), TransferClass::Released);
        assert!(
            report.actions.is_empty(),
            "the released note is the user's to spend: {:?}",
            report.actions
        );
    }

    #[test]
    fn confirmed_transfer_whose_note_is_unspent_at_evidence_is_reorged_and_demoted() {
        let state = scheduled_state(vec![confirmed_transfer(0, txid(9), 9_990)]);

        let report = reconcile(&state, &MockChainView::at(TIP, 9_990));

        assert_eq!(class_of(&report, 0), TransferClass::Reorged);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::Demote {
                transfer: TransferId(0)
            }]
        );
    }

    #[test]
    fn confirmed_transfer_whose_note_a_foreign_transaction_spent_is_invalidated() {
        let state = scheduled_state(vec![confirmed_transfer(0, txid(9), 9_990)]);
        let chain = MockChainView::default().with_note_spend(0, txid(250), Some(9_990));

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 0), TransferClass::Invalidated);
        assert!(
            report
                .actions
                .contains(&RecommendedAction::MarkInvalidated {
                    transfer: TransferId(0)
                })
        );
        assert!(!report.actions.contains(&RecommendedAction::Demote {
            transfer: TransferId(0)
        }));
    }

    #[test]
    fn migration_does_not_complete_while_invalidating_a_confirmed_transfer() {
        let state = scheduled_state(vec![confirmed_transfer(0, txid(9), 9_990)]);
        let chain = MockChainView::default().with_note_spend(0, txid(250), Some(9_990));

        let report = reconcile(&state, &chain);

        assert!(
            report
                .actions
                .contains(&RecommendedAction::MarkInvalidated {
                    transfer: TransferId(0)
                })
        );
        assert!(
            !recommends_completion(&report),
            "the pass that invalidates a confirmed transfer cannot also conclude: {:?}",
            report.actions
        );
    }

    #[test]
    fn confirmed_transfer_below_evidence_stays_confirmed() {
        let state = scheduled_state(vec![confirmed_transfer(0, txid(9), 9_990)]);

        let report = reconcile(&state, &MockChainView::at(TIP, 9_989));

        assert_eq!(class_of(&report, 0), TransferClass::Confirmed);
        assert!(report.actions.is_empty(), "{:?}", report.actions);
    }

    #[test]
    fn confirmed_transfer_with_its_own_spend_mined_stays_confirmed() {
        let own = txid(9);
        let state = scheduled_state(vec![confirmed_transfer(0, own, 9_990)]);
        let chain = MockChainView::at(TIP, 9_989).with_note_spend(0, own, Some(9_990));

        let report = reconcile(&state, &chain);
        assert_eq!(class_of(&report, 0), TransferClass::Confirmed);

        let chain = MockChainView::default().with_note_spend(0, own, Some(9_990));
        let report = reconcile(&state, &chain);
        assert_eq!(class_of(&report, 0), TransferClass::Confirmed);
    }

    #[test]
    fn migration_completes_when_every_transfer_is_settled_and_evidence_reaches_the_tip() {
        let own = txid(9);
        let mut invalidated = assigned_transfer(1, tip_bucket() - 2);
        invalidated.mark_invalidated().unwrap();
        let mut released = assigned_transfer(2, tip_bucket() - 2);
        released.mark_released().unwrap();
        let state = scheduled_state(vec![
            confirmed_transfer(0, own, 9_990),
            invalidated,
            released,
        ]);
        let mut chain = MockChainView::default().with_note_spend(0, own, Some(9_990));
        chain.orchard_spendable = 5_000_000;

        let report = reconcile(&state, &chain);
        assert_eq!(
            report.actions,
            vec![RecommendedAction::MarkComplete {
                residual: 5_000_000
            }]
        );

        chain.spend_evidence = Some(height(TIP - 1));
        let report = reconcile(&state, &chain);
        assert!(
            !recommends_completion(&report),
            "evidence short of the tip cannot conclude: {:?}",
            report.actions
        );
    }

    #[test]
    fn completion_residual_is_the_confirmed_spendable_balance_whatever_its_size() {
        let own = txid(9);
        let state = scheduled_state(vec![confirmed_transfer(0, own, 9_990)]);

        for residual in [0, 5_000, 5_000_000, 250_000_000] {
            let mut chain = MockChainView::default().with_note_spend(0, own, Some(9_990));
            chain.orchard_spendable = residual;
            let report = reconcile(&state, &chain);
            assert_eq!(
                report.actions,
                vec![RecommendedAction::MarkComplete { residual }],
                "a fundable leftover never stalls completion"
            );
        }
    }

    #[test]
    fn migration_with_no_transfers_completes() {
        let state = scheduled_state(Vec::new());
        let chain = MockChainView {
            orchard_spendable: 1_234,
            ..Default::default()
        };

        let report = reconcile(&state, &chain);

        assert_eq!(
            report.actions,
            vec![RecommendedAction::MarkComplete { residual: 1_234 }]
        );
    }

    #[test]
    fn migration_does_not_complete_while_demoting_a_transfer() {
        let own = txid(9);
        let state = scheduled_state(vec![confirmed_transfer(0, own, 9_990)]);
        let chain = MockChainView::default().with_note_spend(0, own, None);

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 0), TransferClass::Reorged);
        assert!(report.actions.contains(&RecommendedAction::Demote {
            transfer: TransferId(0)
        }));
        assert!(
            !recommends_completion(&report),
            "the pass that demotes a transfer cannot also conclude: {:?}",
            report.actions
        );
    }

    #[test]
    fn migration_does_not_complete_while_invalidating_a_transfer() {
        let own = txid(9);
        let state = scheduled_state(vec![
            confirmed_transfer(0, own, 9_990),
            assigned_transfer(1, tip_bucket() + 1),
        ]);
        let chain = MockChainView::default()
            .with_note_spend(0, own, Some(9_990))
            .with_note_spend(1, txid(250), Some(9_995));

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 1), TransferClass::Invalidated);
        assert!(
            report
                .actions
                .contains(&RecommendedAction::MarkInvalidated {
                    transfer: TransferId(1)
                })
        );
        assert!(
            !recommends_completion(&report),
            "a transfer classified terminal but not yet persisted so is not settled: {:?}",
            report.actions
        );
    }

    #[test]
    fn migration_does_not_complete_while_promoting_a_transfer() {
        let own = txid(9);
        let state = scheduled_state(vec![signed_transfer(0, tip_bucket() - 1, own, 20_000)]);
        let chain = MockChainView::default().with_note_spend(0, own, Some(9_995));

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 0), TransferClass::Confirmed);
        assert!(!recommends_completion(&report), "{:?}", report.actions);
    }

    #[test]
    fn due_now_names_the_on_track_transfers_of_the_current_window() {
        let current = tip_bucket();
        let state = scheduled_state(vec![
            assigned_transfer(0, current),
            signed_transfer(1, current, txid(11), 20_000),
            broadcast_transfer(2, current, txid(12), 20_000),
            assigned_transfer(3, current + 1),
            assigned_transfer(4, current - 1),
        ]);
        let chain = MockChainView::default();

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 2), TransferClass::OnTrack);
        assert_eq!(class_of(&report, 3), TransferClass::OnTrack);
        assert_eq!(class_of(&report, 4), TransferClass::Missed);
        assert_eq!(
            due_now(&state, &report, TIP),
            vec![TransferId(0), TransferId(1)],
            "assigned or signed in the open window, and nothing already broadcast, ahead, or missed"
        );
    }

    #[test]
    fn due_now_excludes_a_transfer_reconciliation_will_confirm_invalidate_or_reschedule() {
        let current = tip_bucket();
        let state = scheduled_state(vec![
            signed_transfer(0, current, txid(10), 20_000),
            assigned_transfer(1, current),
            signed_transfer(2, current, txid(12), TIP),
            assigned_transfer(3, current),
        ]);
        let chain = MockChainView::default()
            .with_note_spend(0, txid(10), Some(9_995))
            .with_note_spend(1, txid(250), Some(9_995));

        let report = reconcile(&state, &chain);

        assert_eq!(class_of(&report, 0), TransferClass::Confirmed);
        assert_eq!(class_of(&report, 1), TransferClass::Invalidated);
        assert_eq!(class_of(&report, 2), TransferClass::Expired);
        assert!(
            report
                .actions
                .contains(&RecommendedAction::PromoteConfirmed {
                    transfer: TransferId(0),
                    height: height(9_995),
                })
        );
        assert!(
            report
                .actions
                .contains(&RecommendedAction::MarkInvalidated {
                    transfer: TransferId(1)
                })
        );
        assert!(
            report
                .actions
                .contains(&RecommendedAction::DiscardAndReschedule {
                    transfer: TransferId(2)
                })
        );
        assert_eq!(
            due_now(&state, &report, TIP),
            vec![TransferId(3)],
            "only the transfer reconciliation leaves on track is due"
        );
    }

    #[test]
    fn due_now_ignores_the_target_height() {
        let mut transfer = assigned_transfer(0, tip_bucket());
        transfer.target_height = Some(height(TIP + 40));
        let state = scheduled_state(vec![transfer]);

        let report = reconcile(&state, &MockChainView::default());
        assert_eq!(due_now(&state, &report, TIP), vec![TransferId(0)]);

        let later = TIP + 60;
        let report = reconcile(&state, &MockChainView::at(later, later));
        assert_eq!(due_now(&state, &report, later), vec![TransferId(0)]);
    }

    #[test]
    fn due_now_is_empty_for_a_future_window() {
        let state = scheduled_state(vec![assigned_transfer(0, tip_bucket() + 1)]);

        let report = reconcile(&state, &MockChainView::default());

        assert!(due_now(&state, &report, TIP).is_empty());
    }
}
