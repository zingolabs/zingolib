//! The ZIP 318 Orchard → Ironwood migration, as a consumer drives it.
//!
//! # Usage
//!
//! The migration is a set of explicit commands over data. Each command does
//! one thing, refuses with a typed [`MigrationError`] when it is not valid,
//! and writes nothing on refusal. The consumer reads
//! [`LightClient::migration_status`] and the note list, decides, and calls
//! the one command that fits.
//!
//! ## The scheduled flow (private)
//!
//! 1. **Plan.** [`LightClient::plan_migration`] with
//!    [`MigrationMode::Scheduled`] returns the preparation rounds, the
//!    transfers (one denomination each), the fees, and the residual. It
//!    sends nothing. Show it to the user.
//! 2. **Commit the plan.** [`LightClient::commit_migration`] records the
//!    consent and reserves every pre-Ironwood Orchard note of the account.
//!    An ordinary send never selects a reserved note; a send the free notes
//!    cannot pay fails with
//!    [`ProposeSendError::ReservedForMigration`](crate::wallet::error::ProposeSendError::ReservedForMigration).
//! 3. **Prepare the notes.** While the phase is
//!    [`MigrationPhase::Committed`] or [`MigrationPhase::Preparing`], sync,
//!    then call [`LightClient::broadcast_preparation_round`]. It sends one
//!    round of Orchard self-sends. Sync until its transactions confirm;
//!    [`MigrationError::RoundPending`] says which ones are still in flight.
//!    Repeat until it answers [`MigrationError::AlreadyPrepared`]. A plan
//!    with no rounds skips this step. If the note set changes before the
//!    schedule is committed (a receipt, or a spend from another device),
//!    reconciliation moves the phase back to [`MigrationPhase::Committed`]
//!    or the round refuses with [`MigrationError::PlanMismatch`]; then
//!    plan again and [`LightClient::recommit_migration`].
//! 4. **Commit the schedule.** In [`MigrationPhase::Prepared`],
//!    [`LightClient::propose_schedule`] draws a concrete schedule: each
//!    transfer's window and scheduled broadcast time. Show it to the user,
//!    then [`LightClient::commit_schedule`] stores exactly that draw and
//!    narrows the reservation to the funding notes. Propose again before
//!    any transfer is signed to change the transfers per window.
//! 5. **Broadcast each window.** In [`MigrationPhase::Scheduled`], wake at
//!    each `window_opens_unix_time` of
//!    [`MigrationStatus::upcoming_windows`] and call
//!    [`LightClient::broadcast_due_transfers`]. It builds, signs, and
//!    broadcasts the transfers of the window the chain is inside, and it
//!    never synchronizes: a mobile background task calls this and nothing
//!    else in that session. An empty [`BatchReport`] means nothing is due.
//!    A wallet that last saw the chain more than one window ago is refused
//!    with [`MigrationError::StaleChainView`]: sync first.
//! 6. **Done.** The phase becomes [`MigrationPhase::Complete`] with the
//!    residual once every transfer is confirmed and the wallet has scanned
//!    to the tip.
//!
//! Reconciliation runs after each sync and at the start of each command: it
//! confirms, invalidates, demotes on reorg, reschedules a transfer whose
//! window closed (its `missed_windows` counts the miss), discards a dead
//! signature, and completes. The consumer never calls it. A transfer that
//! missed a window waits for its new window; the user may instead choose
//! [`LightClient::broadcast_missed_now`], which the consumer offers with the
//! ZIP 318 disclosure that the send then coincides with the user's activity.
//!
//! [`LightClient::release_transfer`] takes one pending transfer out and
//! frees its note. [`LightClient::cancel_migration`] releases everything
//! pending; confirmed transfers stand. A transaction already on the wire
//! keeps its note reserved until it confirms or is abandoned, and a
//! released transfer whose transaction mines anyway counts as confirmed.
//!
//! ## The immediate flow (not private)
//!
//! [`LightClient::plan_migration`] with [`MigrationMode::Immediate`], then
//! [`LightClient::migrate_immediately`] with that plan. Every spendable
//! Orchard note is swept into Ironwood now, and the amounts are visible on
//! chain.
//!
//! ## Reading state
//!
//! [`LightClient::migration_status`] is data only: the phase, each transfer
//! with its window, scheduled broadcast time, progress, and missed windows,
//! the transfers due now, the upcoming windows, and the value migrated.
//! [`LightClient::note_summaries`] lists the notes with their `reserved`
//! flag, and [`classify_note`](crate::wallet::migration::classify_note)
//! says what one note is to the migration. [`LightClient::migration_progress`]
//! reports build and broadcast progress inside one command.
//!
//! ## Persistence
//!
//! Every command sets the wallet's dirty flag. When the save task runs, the
//! command also writes the wallet file before it returns. A consumer that
//! saves the wallet bytes itself reads the flag as before.

use std::time::Duration;

use nonempty::NonEmpty;
use tokio::sync::watch;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, MigrationError};
use crate::lightclient::sync::SyncPauseGuard;
use crate::wallet::LightWallet;
use crate::wallet::error::WalletError;
use crate::wallet::migration::{
    BoundNote, BroadcastClient, BroadcastReceipt, BroadcastWindow, BuildResult, ChainView,
    ImmediateMigrationPlan, MigrationMode, MigrationParams, MigrationPhase, MigrationState,
    PlanCommitment, RecommendedAction, ReconcileReport, ScheduledMigrationPlan, SigningStrategy,
    TransferClass, TransferId, TransferRecord, TransferState, WindowReport, due_now_transfers,
    plan_hash, plan_migration as plan_preparation_of, plan_schedule, reconcile, schedule,
};

pub mod broadcast_grpc;
pub mod broadcast_route;

/// A migration replans after every round. A real plan converges in
/// `~log_K(N)` rounds, so far more than this means something is wrong.
pub(crate) const MAX_ROUNDS: usize = 64;
/// How many buckets ahead [`LightClient::migration_status`] reports windows
/// for.
const WAKE_HORIZON_BUCKETS: u64 = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationPlan {
    Scheduled(ScheduledMigrationPlan),
    Immediate(ImmediateMigrationPlan),
}

impl MigrationPlan {
    pub fn mode(&self) -> MigrationMode {
        match self {
            MigrationPlan::Scheduled(_) => MigrationMode::Scheduled,
            MigrationPlan::Immediate(_) => MigrationMode::Immediate,
        }
    }

    /// Value (zatoshis) left unmigrated in the Orchard pool under this plan.
    pub fn residual(&self) -> u64 {
        match self {
            MigrationPlan::Scheduled(plan) => plan.residual,
            MigrationPlan::Immediate(plan) => plan.residual,
        }
    }
}

/// Where one transfer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferProgress {
    /// Not broadcast yet. It waits for its window, or its window is open.
    Pending,
    /// Submitted and not yet confirmed.
    Broadcast,
    /// Mined and confirmed.
    Confirmed,
    /// Its note was spent outside the migration. Its value stays in Orchard.
    Invalid,
    /// The user took it out of the migration.
    Released,
}

/// One transfer's place in the schedule and its progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferStatus {
    pub id: TransferId,
    /// Its denomination, in zatoshis.
    pub denomination: u64,
    /// The window it broadcasts in, `None` before it is scheduled.
    pub window: Option<u64>,
    /// The window's opening height.
    pub boundary: Option<BlockHeight>,
    /// The estimated unix time of its scheduled broadcast height.
    pub scheduled_broadcast_unix_time: Option<u64>,
    pub progress: TransferProgress,
    /// How many windows it missed so far.
    pub missed_windows: u32,
}

impl TransferStatus {
    fn of(
        transfer: &TransferRecord,
        class: TransferClass,
        now: Option<(BlockHeight, u64)>,
        params: &MigrationParams,
    ) -> Self {
        let progress = match class {
            TransferClass::Confirmed => TransferProgress::Confirmed,
            TransferClass::Invalidated => TransferProgress::Invalid,
            TransferClass::Released => TransferProgress::Released,
            TransferClass::Reorged => TransferProgress::Broadcast,
            TransferClass::OnTrack | TransferClass::Missed | TransferClass::Expired => {
                match transfer.state {
                    TransferState::Broadcast => TransferProgress::Broadcast,
                    _ => TransferProgress::Pending,
                }
            }
        };
        let boundary = transfer
            .bucket_index
            .map(|bucket| schedule::boundary_of(bucket, params.bucket_modulus));
        let scheduled_broadcast_unix_time = match (now, transfer.target_height.or(boundary)) {
            (Some((now_height, now_unix)), Some(height)) => {
                Some(schedule::estimated_unix_at(height, now_height, now_unix))
            }
            _ => None,
        };
        TransferStatus {
            id: transfer.id,
            denomination: transfer.denomination,
            window: transfer.bucket_index,
            boundary,
            scheduled_broadcast_unix_time,
            progress,
            missed_windows: transfer.missed_windows,
        }
    }
}

/// One note-preparation round that was built and broadcast.
#[must_use]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparationRound {
    /// The round, counted from zero.
    pub round: u32,
    /// Its transactions. Sync until they confirm.
    pub txids: Vec<TxId>,
}

/// One transfer of a proposed schedule. Plain data: a consumer can hold or
/// serialise it between the screen that shows it and the commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedTransfer {
    pub denomination: u64,
    /// The funding note.
    pub note: BoundNote,
    /// The window it broadcasts in.
    pub window: u64,
    /// The window's opening height.
    pub boundary: BlockHeight,
    /// The bucket whose boundary it anchors to.
    pub anchor_bucket: u64,
    pub scheduled_broadcast_height: BlockHeight,
    /// The estimated unix time of the scheduled broadcast height.
    pub scheduled_broadcast_unix_time: u64,
}

/// A concrete schedule with its random draws made. [`LightClient::commit_schedule`]
/// stores it exactly as proposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedSchedule {
    pub transfers_per_window: u32,
    pub transfers: Vec<ProposedTransfer>,
    /// The chain height the draw was made at.
    pub drawn_at: BlockHeight,
}

impl ProposedSchedule {
    /// A digest of the whole draw: the windows, anchors, and scheduled
    /// broadcast heights beside the funding notes. Two proposals with the
    /// same digest commit the same schedule.
    pub fn schedule_hash(&self) -> [u8; 32] {
        let mut hasher = blake2b_simd::Params::new()
            .hash_length(32)
            .personal(b"ZingoMigSchedV0_")
            .to_state();
        hasher.update(&self.transfers_per_window.to_le_bytes());
        hasher.update(&u32::from(self.drawn_at).to_le_bytes());
        hasher.update(&(self.transfers.len() as u64).to_le_bytes());
        for transfer in &self.transfers {
            hasher.update(&transfer.denomination.to_le_bytes());
            hasher.update(transfer.note.output_id.txid().as_ref());
            hasher.update(&transfer.note.output_id.output_index().to_le_bytes());
            hasher.update(&transfer.note.nullifier);
            hasher.update(&transfer.note.commitment);
            hasher.update(&transfer.window.to_le_bytes());
            hasher.update(&transfer.anchor_bucket.to_le_bytes());
            hasher.update(&u32::from(transfer.scheduled_broadcast_height).to_le_bytes());
        }
        hasher
            .finalize()
            .as_bytes()
            .try_into()
            .expect("hash length is 32")
    }

    fn matches(&self, bound: &[TransferRecord]) -> bool {
        self.transfers.len() == bound.len()
            && self.transfers.iter().zip(bound).all(|(proposed, fresh)| {
                proposed.denomination == fresh.denomination && Some(proposed.note) == fresh.note
            })
    }

    fn records(&self, first_id: u32) -> Result<Vec<TransferRecord>, LightClientError> {
        self.transfers
            .iter()
            .enumerate()
            .map(|(index, proposed)| {
                let mut record = TransferRecord::new(
                    TransferId(first_id + index as u32),
                    proposed.denomination,
                    proposed.note,
                );
                record.assign(proposed.window)?;
                record.anchor_bucket = Some(proposed.anchor_bucket);
                record.target_height = Some(proposed.scheduled_broadcast_height);
                Ok(record)
            })
            .collect()
    }
}

/// The transactions of a completed immediate migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImmediateMigrationSummary {
    /// The immediate migration transactions, in transmission order. More than one only when the
    /// account held more notes than fit in a single transaction.
    pub txids: Vec<TxId>,
    /// Value (zatoshis) sent into the Ironwood pool.
    pub migrated: u64,
    /// Total fees paid, in zatoshis.
    pub fee: u64,
    /// Dust value (zatoshis) left unmigrated in the Orchard pool.
    pub residual: u64,
}

/// The coarse stage an in-progress immediate migration is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImmediateMigrationPhase {
    /// Proving and signing the planned transactions.
    Building,
    /// Transmitting the built transactions.
    Transmitting,
}

/// A snapshot of an in-progress immediate Orchard→Ironwood migration, for rendering
/// "built i/N, sent i/N".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImmediateMigrationStatus {
    /// Total transactions in the plan (N), fixed when the immediate migration begins.
    pub total: u32,
    /// Transactions built (proved + signed) so far, `0..=total`.
    pub built: u32,
    /// Transactions transmitted so far, `0..=total`.
    pub sent: u32,
    /// Which phase the immediate migration is in.
    pub phase: ImmediateMigrationPhase,
}

/// The coarse stage a running note-preparation round is in, mirroring
/// [`ImmediateMigrationPhase`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparationPhase {
    /// Proving and signing the round's transactions.
    Building,
    /// Transmitting the built transactions.
    Transmitting,
}

/// A snapshot of the note-preparation round a step is building, for rendering
/// "built i/N, sent i/N" within that call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparationStatus {
    /// Transactions in this round (N), fixed when the round begins.
    pub total: u32,
    /// Transactions built (proved + signed) so far, `0..=total`.
    pub built: u32,
    /// Transactions transmitted so far, `0..=total`.
    pub sent: u32,
    /// Which phase the round is in.
    pub phase: PreparationPhase,
}

/// A snapshot of a running transfer batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchStatus {
    /// Transfers owed this session.
    pub total: u32,
    /// Transfers resolved so far (sent, slid, or found not due).
    pub resolved: u32,
    /// Transfers accepted by the transmission endpoint so far.
    pub sent: u32,
    /// What the batch is doing right now.
    pub phase: BatchPhase,
}

/// What a running transfer batch is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchPhase {
    /// Proving and submitting the current transfer.
    Sending,
    /// Waiting out the spacing before the next transfer.
    Spacing,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MigrationProgress {
    #[default]
    Idle,
    Preparing(PreparationStatus),
    Immediate(ImmediateMigrationStatus),
    Sending(BatchStatus),
}

#[derive(Debug, Clone)]
pub(crate) struct MigrationProgressHandle(watch::Sender<MigrationProgress>);

impl Default for MigrationProgressHandle {
    fn default() -> Self {
        Self(watch::Sender::new(MigrationProgress::Idle))
    }
}

impl MigrationProgressHandle {
    pub(crate) fn subscribe(&self) -> watch::Receiver<MigrationProgress> {
        self.0.subscribe()
    }

    pub(crate) fn begin_immediate(&self, total: u32) {
        self.0
            .send_replace(MigrationProgress::Immediate(ImmediateMigrationStatus {
                total,
                built: 0,
                sent: 0,
                phase: ImmediateMigrationPhase::Building,
            }));
    }

    pub(crate) fn begin_preparation(&self, total: u32) {
        self.0
            .send_replace(MigrationProgress::Preparing(PreparationStatus {
                total,
                built: 0,
                sent: 0,
                phase: PreparationPhase::Building,
            }));
    }

    pub(crate) fn begin_batch(&self, total: u32) {
        self.0.send_replace(MigrationProgress::Sending(BatchStatus {
            total,
            resolved: 0,
            sent: 0,
            phase: BatchPhase::Sending,
        }));
    }

    /// Publishes the number of transactions built so far. No-op unless a
    /// build is armed.
    pub(crate) fn set_built(&self, built: u32) {
        self.0.send_if_modified(|progress| match progress {
            MigrationProgress::Immediate(status) => {
                status.built = built;
                true
            }
            MigrationProgress::Preparing(status) => {
                status.built = built;
                true
            }
            _ => false,
        });
    }

    pub(crate) fn enter_transmit(&self) {
        self.0.send_if_modified(|progress| match progress {
            MigrationProgress::Immediate(status) => {
                status.phase = ImmediateMigrationPhase::Transmitting;
                true
            }
            MigrationProgress::Preparing(status) => {
                status.phase = PreparationPhase::Transmitting;
                true
            }
            _ => false,
        });
    }

    /// Publishes the number of transactions transmitted so far. No-op
    /// unless a build is armed.
    pub(crate) fn set_sent(&self, sent: u32) {
        self.0.send_if_modified(|progress| match progress {
            MigrationProgress::Immediate(status) => {
                status.sent = sent;
                true
            }
            MigrationProgress::Preparing(status) => {
                status.sent = sent;
                true
            }
            _ => false,
        });
    }

    /// Publishes one more transfer resolved and the running sent count. No-op
    /// unless a batch is armed.
    pub(crate) fn resolve(&self, resolved: u32, sent: u32) {
        self.0.send_if_modified(|progress| match progress {
            MigrationProgress::Sending(status) => {
                status.resolved = resolved;
                status.sent = sent;
                true
            }
            _ => false,
        });
    }

    pub(crate) fn set_phase(&self, phase: BatchPhase) {
        self.0.send_if_modified(|progress| match progress {
            MigrationProgress::Sending(status) => {
                status.phase = phase;
                true
            }
            _ => false,
        });
    }

    pub(crate) fn clear(&self) {
        self.0.send_replace(MigrationProgress::Idle);
    }
}

/// Clears the progress on drop, so a failed or early-returning step never
/// leaves a stale value behind. Owns a clone (not a borrow of the client)
/// so it can live across the `&mut self` calls it brackets.
struct ProgressScope(MigrationProgressHandle);

impl Drop for ProgressScope {
    fn drop(&mut self) {
        self.0.clear();
    }
}

/// The progress side channel one shared build/transmit batch reports into.
/// Both the immediate migration and a note-preparation round drive the shared
/// [`LightClient::build_and_transmit`] primitive. The internal drivers that
/// report nowhere pass `()`.
trait BuildProgressSink {
    /// Publish that `built` transactions have been proved and signed.
    fn on_built(&self, built: u32);
    /// Publish that the batch has moved from building to transmitting.
    fn on_transmit(&self);
}

impl BuildProgressSink for () {
    fn on_built(&self, _built: u32) {}
    fn on_transmit(&self) {}
}

impl BuildProgressSink for MigrationProgressHandle {
    fn on_built(&self, built: u32) {
        self.set_built(built);
    }
    fn on_transmit(&self) {
        self.enter_transmit();
    }
}

/// One transfer's result from a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferOutcome {
    /// The transfer.
    pub transfer: TransferId,
    /// Its denomination, in zatoshis.
    pub denomination: u64,
    /// What happened to it.
    pub result: TransferBroadcastResult,
}

/// What one batch did with one transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferBroadcastResult {
    /// Accepted by the transmission endpoint, with the route it traveled.
    Sent(BroadcastReceipt),
    /// Not sendable this session: its window boundary is no longer
    /// witnessable from the wallet's tree. Reconciliation carries it to a
    /// coming window. Nothing is lost.
    Slid,
    /// Its random target is still ahead. Come back around the estimate.
    NotDue {
        /// Rough unix time the target block is expected.
        window_opens_unix_time: u64,
    },
    /// Submission failed and the batch halted here.
    Failed {
        /// The failure's whole cause chain, outermost layer first.
        error: String,
    },
}

#[must_use]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BatchReport {
    /// Per-transfer outcomes, in send order.
    pub outcomes: Vec<TransferOutcome>,
    /// Set when a submission error halted the batch early. Transfers without
    /// an outcome entry were not attempted and remain due.
    pub halted: Option<String>,
}

/// What one pass of the due-transfer transmission loop achieved.
#[must_use]
enum BroadcastPass {
    /// Every submission the pass attempted was accepted, in submission order.
    Complete(Vec<BroadcastReceipt>),
    /// A submission failed and the pass stopped there.
    Halted {
        /// The transfer whose submission failed. It stays signed (its
        /// transaction already recorded in the wallet) and due, so a later
        /// attempt resubmits it.
        transfer: TransferId,
        /// Why the endpoint did not take it.
        error: crate::wallet::migration::TransferBroadcastError,
    },
}

impl BatchReport {
    pub fn sent_txids(&self) -> Vec<TxId> {
        self.outcomes
            .iter()
            .filter_map(|outcome| match &outcome.result {
                TransferBroadcastResult::Sent(receipt) => Some(receipt.txid),
                _ => None,
            })
            .collect()
    }
}

/// The batch a broadcast would attempt this instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueBatch {
    /// The current bucket's opening boundary.
    pub boundary: BlockHeight,
    /// The transfers due right now, all in the current window.
    pub transfer_ids: Vec<TransferId>,
    /// The transfers' denominations in zatoshis, aligned element-for-element
    /// with `transfer_ids`.
    pub denominations: Vec<u64>,
}

/// The migration's progress, as data.
#[derive(Debug, Clone)]
pub struct MigrationStatus {
    /// Confirmed-spendable balance left in the *Orchard pool* specifically.
    /// ZIP 318 requires displaying this figure, never only a unified total.
    pub orchard_confirmed_spendable: u64,
    /// Where the migration is, `None` when none is in progress.
    pub phase: Option<MigrationPhase>,
    /// Every transfer of the committed schedule.
    pub transfers: Vec<TransferStatus>,
    /// Transfers in total. Before the schedule is committed this is the
    /// plan's transfer count over the live notes.
    pub transfers_total: u32,
    /// Transfers confirmed so far.
    pub transfers_confirmed: u32,
    /// Total value across all transfers, in zatoshis. Projected from the
    /// plan before the schedule is committed, like [`Self::transfers_total`].
    pub value_total: u64,
    /// Value already confirmed into the Ironwood pool, in zatoshis.
    pub value_migrated: u64,
    /// Coming windows, what a mobile platform scheduler feeds into its
    /// earliest-begin requests. Strictly *future* windows: the window the
    /// chain is currently inside is reported by [`Self::due_now`], not here.
    pub upcoming_windows: Vec<BroadcastWindow>,
    /// The batch [`LightClient::broadcast_due_transfers`] would attempt right
    /// now, or `None` when nothing is due.
    pub due_now: Option<DueBatch>,
    /// The window timeline around the chain tip: the window the tip is
    /// inside plus one entry per scheduled window, past and future. `None`
    /// only when the wallet has no chain height yet.
    pub windows: Option<Vec<WindowReport>>,
}

impl LightClient {
    /// Plans a migration of `account`'s spendable Orchard notes in `mode`.
    ///
    /// Pure and deterministic: nothing is signed or sent, so the plan (its
    /// transaction count, fees and residual dust) can be shown to the user
    /// before [`Self::commit_migration`] commits to it.
    pub async fn plan_migration(
        &self,
        account: zip32::AccountId,
        mode: MigrationMode,
    ) -> Result<MigrationPlan, LightClientError> {
        let wallet = self.wallet().read().await;
        Ok(match mode {
            MigrationMode::Scheduled => {
                MigrationPlan::Scheduled(wallet.plan_ironwood_migration_now(account)?)
            }
            MigrationMode::Immediate => {
                MigrationPlan::Immediate(wallet.plan_immediate_migration(account)?)
            }
        })
    }

    pub(crate) async fn plan_preparation(
        &self,
        account: zip32::AccountId,
    ) -> Result<ScheduledMigrationPlan, LightClientError> {
        let wallet = self.wallet().read().await;
        Ok(wallet.plan_ironwood_migration_now(account)?)
    }

    /// Commits to a scheduled `plan`: the first of the two commit points.
    ///
    /// The plan is the [`MigrationPlan::Scheduled`] the user was shown. The
    /// call re-plans and fails with [`MigrationError::PlanMismatch`] when the
    /// wallet's notes changed in between. From this call on, every
    /// pre-Ironwood Orchard note of `account` is reserved: ordinary sends do
    /// not select them. Nothing is broadcast.
    ///
    /// A plan with nothing to migrate is refused with
    /// [`WalletError::NothingToMigrate`]. A plan with no note-preparation
    /// rounds lands in [`MigrationPhase::Prepared`], ready for
    /// [`Self::propose_schedule`].
    /// Otherwise the migration starts in [`MigrationPhase::Committed`] and
    /// [`Self::broadcast_preparation_round`] drives the rounds.
    pub async fn commit_migration(
        &mut self,
        account: zip32::AccountId,
        plan: &MigrationPlan,
    ) -> Result<(), LightClientError> {
        let MigrationPlan::Scheduled(plan) = plan else {
            return Err(MigrationError::WrongPlanMode.into());
        };
        self.reconcile_migration().await?;
        let mut wallet = self.wallet().write().await;
        if let Some(state) = &wallet.migration
            && !matches!(state.phase, MigrationPhase::Complete { .. })
        {
            return Err(MigrationError::AlreadyInProgress.into());
        }
        let current = wallet.plan_ironwood_migration_now(account)?;
        let hash = plan_hash(&current);
        if hash != plan_hash(plan) {
            return Err(MigrationError::PlanMismatch.into());
        }
        if current.transfers.is_empty() && current.preparation_rounds.is_empty() {
            return Err(WalletError::NothingToMigrate.into());
        }
        wallet.migration = None;
        let params = MigrationParams::provisional(wallet.chain_type());
        let phase = if current.is_prepared() {
            MigrationPhase::Prepared
        } else {
            MigrationPhase::Committed
        };
        wallet.migration = Some(MigrationState {
            commitment: PlanCommitment {
                params_hash: params.params_hash(),
                plan_hash: hash,
                committed_at: u64::from(crate::utils::now()),
            },
            params,
            strategy: SigningStrategy::LazyAtBoundary,
            mode: MigrationMode::Scheduled,
            account,
            phase,
            transfers: Vec::new(),
        });
        wallet.save_required = true;
        self.persist_or_restore(&mut wallet, None).await
    }

    /// Records fresh consent to `plan` for a migration whose notes changed
    /// before its schedule was committed: after a receipt, or after a spend
    /// from another device. Valid in [`MigrationPhase::Committed`],
    /// [`MigrationPhase::Prepared`], and [`MigrationPhase::Preparing`] with
    /// no round in flight. The reservation is unchanged. Fails with
    /// [`MigrationError::PlanMismatch`] when `plan` is not the wallet's
    /// current plan.
    pub async fn recommit_migration(
        &mut self,
        plan: &MigrationPlan,
    ) -> Result<(), LightClientError> {
        let MigrationPlan::Scheduled(plan) = plan else {
            return Err(MigrationError::WrongPlanMode.into());
        };
        self.change_migration(|wallet, state| {
            match &state.phase {
                MigrationPhase::Committed | MigrationPhase::Prepared => (),
                MigrationPhase::Preparing { pending_txids, .. } => {
                    let pending = pending_round_txids(wallet, pending_txids)?;
                    if !pending.is_empty() {
                        return Err(MigrationError::RoundPending { txids: pending }.into());
                    }
                }
                MigrationPhase::Scheduled | MigrationPhase::Complete { .. } => {
                    return Err(MigrationError::AlreadyPrepared.into());
                }
            }
            let current = wallet.plan_ironwood_migration_now(state.account)?;
            let hash = plan_hash(&current);
            if hash != plan_hash(plan) {
                return Err(MigrationError::PlanMismatch.into());
            }
            state.commitment = PlanCommitment {
                params_hash: state.params.params_hash(),
                plan_hash: hash,
                committed_at: u64::from(crate::utils::now()),
            };
            state.phase = if current.is_prepared() {
                MigrationPhase::Prepared
            } else {
                MigrationPhase::Committed
            };
            Ok(Record::Keep)
        })
        .await
    }

    /// Builds and broadcasts the next note-preparation round.
    ///
    /// Valid in [`MigrationPhase::Committed`], and in
    /// [`MigrationPhase::Preparing`] once the previous round confirmed and
    /// its outputs reached the anchor. Refuses with
    /// [`MigrationError::RoundPending`] while a round is in flight and with
    /// [`MigrationError::AlreadyPrepared`] once preparation is complete.
    /// The round is written to the wallet file before it is broadcast, and a
    /// transaction that fails to transmit is marked failed, so a relaunch
    /// replans over its notes.
    pub async fn broadcast_preparation_round(
        &mut self,
    ) -> Result<PreparationRound, LightClientError> {
        self.reconcile_migration().await?;
        let sync = self.pause_sync_scoped()?;

        let (account, committed_plan_hash, next_round) = {
            let wallet = self.wallet().read().await;
            let state = wallet.migration_state()?;
            let next_round = match &state.phase {
                MigrationPhase::Prepared
                | MigrationPhase::Scheduled
                | MigrationPhase::Complete { .. } => {
                    return Err(MigrationError::AlreadyPrepared.into());
                }
                MigrationPhase::Committed => 0,
                MigrationPhase::Preparing {
                    round,
                    pending_txids,
                } => {
                    let pending = pending_round_txids(&wallet, pending_txids)?;
                    if !pending.is_empty() {
                        return Err(MigrationError::RoundPending { txids: pending }.into());
                    }
                    let confirmed = pending_txids
                        .iter()
                        .any(|txid| wallet.transaction_confirmed_height(txid).is_some());
                    if confirmed { round + 1 } else { *round }
                }
            };
            (state.account, state.commitment.plan_hash, next_round)
        };

        if next_round as usize >= MAX_ROUNDS {
            return Err(MigrationError::PreparationDidNotConverge(MAX_ROUNDS).into());
        }

        let plan = self.plan_preparation(account).await?;
        if next_round == 0 && plan_hash(&plan) != committed_plan_hash {
            return Err(MigrationError::PlanMismatch.into());
        }
        if plan.is_prepared() {
            let mut wallet = self.wallet().write().await;
            wallet
                .with_migration_state(|wallet, state| {
                    state.phase = MigrationPhase::Prepared;
                    wallet.save_required = true;
                })
                .ok_or(MigrationError::NoMigration)?;
            self.persist(&mut wallet).await?;
            return Err(MigrationError::AlreadyPrepared.into());
        }

        let round = plan
            .preparation_rounds
            .into_iter()
            .next()
            .expect("an unprepared plan has at least one round");
        self.migration_progress
            .begin_preparation(round.len() as u32);
        let _scope = ProgressScope(self.migration_progress.clone());
        let progress = self.migration_progress.clone();
        let txids = self
            .build_transactions(&round, &progress, |wallet, planned| {
                wallet.build_preparation_transaction(account, planned)
            })
            .await?;
        progress.on_transmit();

        {
            let mut wallet = self.wallet().write().await;
            wallet
                .with_migration_state(|wallet, state| {
                    state.phase = MigrationPhase::Preparing {
                        round: next_round,
                        pending_txids: txids.clone(),
                    };
                    wallet.save_required = true;
                })
                .ok_or(MigrationError::NoMigration)?;
            self.persist(&mut wallet).await?;
        }

        let transmitted = self
            .transmit_transactions(
                NonEmpty::from_vec(txids.clone()).expect("planned rounds are never empty"),
            )
            .await;
        drop(sync);
        if let Err(e) = transmitted {
            self.fail_unsent_transactions(&txids).await;
            return Err(e);
        }

        Ok(PreparationRound {
            round: next_round,
            txids,
        })
    }

    /// Draws a concrete schedule: `transfers_per_window` transfers (at least
    /// one) share each window, and every transfer gets its window, anchor,
    /// and scheduled broadcast height. Pure over the wallet: nothing is
    /// stored. [`Self::commit_schedule`] stores exactly this draw.
    ///
    /// Valid in [`MigrationPhase::Prepared`], and again in
    /// [`MigrationPhase::Scheduled`] while no transfer is signed.
    pub async fn propose_schedule(
        &self,
        transfers_per_window: u32,
    ) -> Result<ProposedSchedule, LightClientError> {
        let wallet = self.wallet().read().await;
        let state = wallet.migration_state()?;
        schedule_is_open(state)?;
        let mut params = state.params.clone();
        params.k_max = transfers_per_window.max(1);
        let records = draw_schedule(&wallet, state, &params)?;
        let now_height = wallet.known_chain_height()?;
        let now_unix = u64::from(crate::utils::now());
        let transfers = records
            .iter()
            .map(|record| {
                let window = record
                    .bucket_index
                    .expect("placed transfers carry a window");
                let boundary = schedule::boundary_of(window, params.bucket_modulus);
                let scheduled_broadcast_height = record.target_height.unwrap_or(boundary);
                ProposedTransfer {
                    denomination: record.denomination,
                    note: record.note.expect("bound transfers carry a note"),
                    window,
                    boundary,
                    anchor_bucket: record
                        .anchor_bucket
                        .expect("placed transfers carry an anchor"),
                    scheduled_broadcast_height,
                    scheduled_broadcast_unix_time: schedule::estimated_unix_at(
                        scheduled_broadcast_height,
                        now_height,
                        now_unix,
                    ),
                }
            })
            .collect();
        Ok(ProposedSchedule {
            transfers_per_window: params.k_max,
            transfers,
            drawn_at: now_height,
        })
    }

    /// Commits `proposed`: the second commit point. Binds the funding notes,
    /// stores the transfers with the windows and anchors that were proposed,
    /// and reserves exactly those notes. Released transfers keep their
    /// records. Fails with [`MigrationError::ScheduleMismatch`] when the
    /// wallet's funding notes changed since the proposal, or when the chain
    /// already passed one of the proposed windows.
    pub async fn commit_schedule(
        &mut self,
        proposed: &ProposedSchedule,
    ) -> Result<(), LightClientError> {
        self.change_migration(|wallet, state| {
            schedule_is_open(state)?;
            let mut params = state.params.clone();
            params.k_max = proposed.transfers_per_window.max(1);
            let fresh = bind_unreleased(wallet, state)?;
            if !proposed.matches(&fresh) {
                return Err(MigrationError::ScheduleMismatch.into());
            }
            let now_height = wallet.known_chain_height()?;
            let current_bucket = schedule::bucket_index(now_height, params.bucket_modulus);
            if proposed
                .transfers
                .iter()
                .any(|transfer| transfer.window < current_bucket)
            {
                return Err(MigrationError::ScheduleMismatch.into());
            }
            let mut transfers: Vec<TransferRecord> = state
                .transfers
                .iter()
                .filter(|transfer| transfer.state == TransferState::Released)
                .cloned()
                .collect();
            for (index, transfer) in transfers.iter_mut().enumerate() {
                transfer.id = TransferId(index as u32);
            }
            transfers.extend(proposed.records(transfers.len() as u32)?);
            state.params = params;
            state.commitment = PlanCommitment {
                params_hash: state.params.params_hash(),
                plan_hash: state.commitment.plan_hash,
                committed_at: u64::from(crate::utils::now()),
            };
            state.transfers = transfers;
            state.phase = MigrationPhase::Scheduled;
            Ok(Record::Keep)
        })
        .await
    }

    /// Builds, signs, and broadcasts every transfer of the window the chain
    /// is inside, `spacing` apart. Returns an empty report when nothing is
    /// due. Never synchronizes: a mobile background task calls this and
    /// nothing else.
    pub async fn broadcast_due_transfers(
        &mut self,
        spacing: Duration,
    ) -> Result<BatchReport, LightClientError> {
        self.reconcile_migration().await?;
        self.require_scheduled().await?;
        self.require_fresh_chain_view().await?;
        let client = self.migration_transmission_client()?;
        self.broadcast_due_transfers_with(&client, spacing).await
    }

    async fn require_fresh_chain_view(&self) -> Result<(), LightClientError> {
        let wallet = self.wallet().read().await;
        let Some((height, block)) = wallet.wallet_blocks.iter().next_back() else {
            return Ok(());
        };
        let modulus = wallet.migration.as_ref().map_or_else(
            || MigrationParams::provisional(wallet.chain_type()).bucket_modulus,
            |state| state.params.bucket_modulus,
        );
        let window_seconds = u64::from(modulus) * schedule::TARGET_BLOCK_SPACING_SECONDS;
        let age = u64::from(crate::utils::now()).saturating_sub(u64::from(block.time()));
        if age > window_seconds {
            return Err(MigrationError::StaleChainView {
                last_known_height: *height,
            }
            .into());
        }
        Ok(())
    }

    /// Moves every pending transfer that missed a window into the current
    /// window, then broadcasts as [`Self::broadcast_due_transfers`]. This
    /// is the disclosed "send now" of ZIP 318: the broadcast coincides with
    /// the user's activity, and the caller discloses that first.
    pub async fn broadcast_missed_now(
        &mut self,
        spacing: Duration,
    ) -> Result<BatchReport, LightClientError> {
        self.reconcile_migration().await?;
        self.require_scheduled().await?;
        self.place_missed_in_current_window().await?;
        let client = self.migration_transmission_client()?;
        self.broadcast_due_transfers_with(&client, spacing).await
    }

    /// Takes one pending transfer out of the migration. Its funding note is
    /// free again, unless a transaction of it is already on the wire: then
    /// the note stays reserved until that transaction confirms or is
    /// abandoned. A signature that never left the device is discarded.
    pub async fn release_transfer(&mut self, id: TransferId) -> Result<(), LightClientError> {
        self.change_migration(|wallet, state| {
            let transfer = state
                .transfers
                .get_mut(id.0 as usize)
                .ok_or(MigrationError::TransferNotPending(id.0))?;
            if transfer.state.is_terminal() {
                return Err(MigrationError::TransferNotPending(id.0).into());
            }
            release(wallet, transfer)?;
            Ok(Record::Keep)
        })
        .await
    }

    async fn submit_with_probes(
        &self,
        client: &impl BroadcastClient,
        raw_tx: Vec<u8>,
        txid: TxId,
        expiry_height: BlockHeight,
    ) -> Result<BroadcastReceipt, crate::wallet::migration::TransferBroadcastError> {
        use crate::lightclient::transmit::{MAX_QUEUED_PROBES, RejectionClass, classify_rejection};
        use crate::wallet::migration::TransferBroadcastError;

        let mut probes = 0u8;
        loop {
            match client.submit(raw_tx.clone(), expiry_height).await {
                Err(TransferBroadcastError::AlreadyKnown { route }) => {
                    return Ok(BroadcastReceipt { txid, route });
                }
                Err(TransferBroadcastError::Rejected { route, message })
                    if classify_rejection(&message) == RejectionClass::QueuedProbe =>
                {
                    if probes >= MAX_QUEUED_PROBES {
                        return Ok(BroadcastReceipt { txid, route });
                    }
                    probes += 1;
                    tokio::time::sleep(self.transmit_retry_interval).await;
                }
                submitted => return submitted,
            }
        }
    }

    /// Abandons the migration. Confirmed transfers stand. Every pending
    /// transfer is released. A transaction already on the wire keeps its
    /// note reserved until it confirms or is abandoned, and the migration
    /// record stays until then; otherwise the record is removed at once.
    pub async fn cancel_migration(&mut self) -> Result<(), LightClientError> {
        self.change_migration(|wallet, state| {
            for transfer in state.transfers.iter_mut() {
                if !transfer.state.is_terminal() {
                    release(wallet, transfer)?;
                }
            }
            state.phase = MigrationPhase::Scheduled;
            Ok(
                if state.transfers.iter().any(TransferRecord::is_on_the_wire) {
                    Record::Keep
                } else {
                    Record::Remove
                },
            )
        })
        .await
    }

    /// Runs the immediate migration `plan` the user was shown: every
    /// spendable Orchard note swept into Ironwood now, amounts visible on
    /// chain. Fails with [`MigrationError::PlanMismatch`] when the wallet's
    /// notes changed since the plan. Nothing is stored ahead of the send.
    pub async fn migrate_immediately(
        &mut self,
        account: zip32::AccountId,
        plan: &MigrationPlan,
    ) -> Result<ImmediateMigrationSummary, LightClientError> {
        let MigrationPlan::Immediate(plan) = plan else {
            return Err(MigrationError::WrongPlanMode.into());
        };
        if self
            .wallet()
            .read()
            .await
            .plan_immediate_migration(account)?
            != *plan
        {
            return Err(MigrationError::PlanMismatch.into());
        }
        if plan.is_empty() {
            return Err(WalletError::NothingToMigrate.into());
        }
        self.reconcile_migration().await?;
        let sync = self.pause_sync_scoped()?;
        self.migrate_immediately_presynced(account, &sync).await
    }

    /// Applies every safe action reconciliation recommends: confirmations,
    /// invalidations, reorg demotions, rescheduling of missed windows,
    /// discarding of dead signatures, completion, and the move from a
    /// confirmed preparation round to [`MigrationPhase::Prepared`]. Local
    /// only: it never synchronizes. Runs after each sync and at the start of
    /// each command. A wallet with no migration returns `None`.
    pub(crate) async fn reconcile_migration(
        &mut self,
    ) -> Result<Option<ReconcileReport>, LightClientError> {
        let mut wallet = self.wallet().write().await;
        let report = reconcile_wallet_migration(&mut wallet)?;
        if report.is_some() {
            self.persist(&mut wallet).await?;
        }
        Ok(report)
    }

    async fn require_scheduled(&self) -> Result<(), LightClientError> {
        let wallet = self.wallet().read().await;
        let state = wallet.migration_state()?;
        match state.phase {
            MigrationPhase::Scheduled => Ok(()),
            MigrationPhase::Complete { .. } => Ok(()),
            _ => Err(MigrationError::NotScheduled.into()),
        }
    }

    async fn place_missed_in_current_window(&mut self) -> Result<(), LightClientError> {
        let mut wallet = self.wallet().write().await;
        wallet.edit_migration(|wallet, state| {
            let now_height = wallet.known_chain_height()?;
            let current_bucket = schedule::bucket_index(now_height, state.params.bucket_modulus);
            let activation = wallet.ironwood_activation()?;
            for transfer in state.transfers.iter_mut() {
                let pending = matches!(
                    transfer.state,
                    TransferState::Assigned
                        | TransferState::Signed
                        | TransferState::Broadcast
                        | TransferState::Expired
                );
                let behind = transfer.missed_windows > 0
                    || transfer
                        .bucket_index
                        .is_some_and(|bucket| bucket < current_bucket);
                if !pending || !behind || transfer.bucket_index == Some(current_bucket) {
                    continue;
                }
                let floor = schedule::AnchorFloor::new(
                    activation,
                    wallet.bound_note_confirmed_at(transfer),
                );
                if schedule::draw_anchor_bucket(
                    current_bucket,
                    &floor,
                    &mut rand::rngs::OsRng,
                    state.params.bucket_modulus,
                )
                .is_none()
                {
                    continue;
                }
                if matches!(
                    transfer.state,
                    TransferState::Signed | TransferState::Broadcast
                ) {
                    discard_signature(wallet, transfer)?;
                }
                schedule::place_immediate(
                    transfer,
                    current_bucket,
                    &floor,
                    &mut rand::rngs::OsRng,
                    &state.params,
                )?;
            }
            wallet.save_required = true;
            Ok::<_, LightClientError>(())
        })?;
        self.persist(&mut wallet).await
    }

    /// The batch broadcast with an injectable client, for tests.
    pub(crate) async fn broadcast_due_transfers_with(
        &mut self,
        client: &impl BroadcastClient,
        spacing: Duration,
    ) -> Result<BatchReport, LightClientError> {
        let _sync = self.pause_sync_scoped()?;
        self.wallet().write().await.refresh_transfer_witnesses()?;

        let owed: Vec<(TransferId, u64)> = {
            let wallet = self.wallet().read().await;
            let state = wallet.migration_state()?;
            let now_height = wallet.known_chain_height()?;
            let current_bucket = schedule::bucket_index(now_height, state.params.bucket_modulus);
            state
                .transfers
                .iter()
                .filter(|transfer| schedule::transfer_in_current_bucket(transfer, current_bucket))
                .map(|transfer| (transfer.id, transfer.denomination))
                .collect()
        };

        self.migration_progress.begin_batch(owed.len() as u32);
        let _scope = ProgressScope(self.migration_progress.clone());

        let mut report = BatchReport::default();
        let mut sent = 0u32;
        for (index, (transfer, denomination)) in owed.iter().enumerate() {
            match self
                .transmit_transfers_selected(client, Some(*transfer))
                .await
            {
                Ok(BroadcastPass::Halted {
                    transfer: failed_transfer,
                    error,
                    ..
                }) => {
                    let error = error.to_string();
                    report.outcomes.push(TransferOutcome {
                        transfer: failed_transfer,
                        denomination: *denomination,
                        result: TransferBroadcastResult::Failed {
                            error: error.clone(),
                        },
                    });
                    report.halted = Some(error);
                    break;
                }
                Ok(BroadcastPass::Complete(mut receipts)) if !receipts.is_empty() => {
                    sent += 1;
                    report.outcomes.push(TransferOutcome {
                        transfer: *transfer,
                        denomination: *denomination,
                        result: TransferBroadcastResult::Sent(receipts.swap_remove(0)),
                    });
                    self.migration_progress
                        .resolve(report.outcomes.len() as u32, sent);
                    if index + 1 < owed.len() {
                        self.migration_progress.set_phase(BatchPhase::Spacing);
                        tokio::time::sleep(spacing).await;
                        self.migration_progress.set_phase(BatchPhase::Sending);
                    }
                }
                Ok(_) => {
                    report.outcomes.push(TransferOutcome {
                        transfer: *transfer,
                        denomination: *denomination,
                        result: TransferBroadcastResult::Slid,
                    });
                    self.migration_progress
                        .resolve(report.outcomes.len() as u32, sent);
                }
                Err(e) => {
                    let error = render_cause_chain(&e);
                    report.outcomes.push(TransferOutcome {
                        transfer: *transfer,
                        denomination: *denomination,
                        result: TransferBroadcastResult::Failed {
                            error: error.clone(),
                        },
                    });
                    report.halted = Some(error);
                    break;
                }
            }
        }
        Ok(report)
    }

    /// The migration's progress, everything a progress UI renders. Includes
    /// the Orchard-pool-specific confirmed-spendable figure, which ZIP 318
    /// requires displaying instead of a unified total.
    pub async fn migration_status(&self) -> Result<MigrationStatus, LightClientError> {
        let wallet = self.wallet().read().await;
        let timeline = window_timeline_of(&wallet);
        let now_height = wallet.sync_state.last_known_chain_height();
        let now_unix = u64::from(crate::utils::now());
        let Some(state) = &wallet.migration else {
            return Ok(MigrationStatus {
                orchard_confirmed_spendable: ChainView::orchard_confirmed_spendable(
                    &*wallet,
                    zip32::AccountId::ZERO,
                ),
                phase: None,
                transfers: Vec::new(),
                transfers_total: 0,
                transfers_confirmed: 0,
                value_total: 0,
                value_migrated: 0,
                upcoming_windows: Vec::new(),
                due_now: None,
                windows: timeline,
            });
        };
        let report = reconcile(state, &*wallet);
        let transfers: Vec<TransferStatus> = state
            .transfers
            .iter()
            .map(|transfer| {
                let class = report
                    .assessments
                    .iter()
                    .find(|assessment| assessment.id == transfer.id)
                    .map_or(TransferClass::OnTrack, |assessment| assessment.class);
                TransferStatus::of(
                    transfer,
                    class,
                    now_height.map(|height| (height, now_unix)),
                    &state.params,
                )
            })
            .collect();
        let confirmed: Vec<&TransferRecord> = state
            .transfers
            .iter()
            .zip(&transfers)
            .filter(|(_, status)| status.progress == TransferProgress::Confirmed)
            .map(|(transfer, _)| transfer)
            .collect();
        let upcoming_windows = now_height.map_or_else(Vec::new, |height| {
            crate::wallet::migration::upcoming_windows(
                &state.transfers,
                height,
                now_unix,
                WAKE_HORIZON_BUCKETS,
                &state.params,
            )
        });
        let due_now = match (now_height, &state.phase) {
            (Some(height), MigrationPhase::Scheduled) => {
                let due_ids = due_now_transfers(&state.transfers, &report, height, &state.params);
                (!due_ids.is_empty()).then(|| {
                    let current_bucket =
                        schedule::bucket_index(height, state.params.bucket_modulus);
                    let due: Vec<_> = state
                        .transfers
                        .iter()
                        .filter(|transfer| due_ids.contains(&transfer.id))
                        .collect();
                    DueBatch {
                        boundary: schedule::boundary_of(
                            current_bucket,
                            state.params.bucket_modulus,
                        ),
                        transfer_ids: due.iter().map(|transfer| transfer.id).collect(),
                        denominations: due.iter().map(|transfer| transfer.denomination).collect(),
                    }
                })
            }
            _ => None,
        };
        let value_migrated = confirmed.iter().map(|transfer| transfer.denomination).sum();
        let (transfers_total, value_total) = match &state.phase {
            MigrationPhase::Committed
            | MigrationPhase::Preparing { .. }
            | MigrationPhase::Prepared => {
                let plan = plan_preparation_of(
                    &wallet.live_v2_note_values(state.account),
                    wallet.preparations_confirm_post_activation(),
                    &state.params,
                );
                (plan.transfers.len() as u32, plan.transfers.iter().sum())
            }
            _ => (
                state.transfers.len() as u32,
                state
                    .transfers
                    .iter()
                    .map(|transfer| transfer.denomination)
                    .sum(),
            ),
        };
        Ok(MigrationStatus {
            orchard_confirmed_spendable: ChainView::orchard_confirmed_spendable(
                &*wallet,
                state.account,
            ),
            phase: Some(state.phase.clone()),
            transfers,
            transfers_total,
            transfers_confirmed: confirmed.len() as u32,
            value_total,
            value_migrated,
            upcoming_windows,
            due_now,
            windows: timeline,
        })
    }

    async fn change_migration(
        &mut self,
        change: impl FnOnce(&mut LightWallet, &mut MigrationState) -> Result<Record, LightClientError>,
    ) -> Result<(), LightClientError> {
        self.reconcile_migration().await?;
        let mut wallet = self.wallet().write().await;
        let previous = wallet.migration.clone();
        match wallet.edit_migration(change) {
            Err(error) => {
                wallet.migration = previous;
                return Err(error);
            }
            Ok(Record::Remove) => wallet.migration = None,
            Ok(Record::Keep) => (),
        }
        wallet.save_required = true;
        self.persist_or_restore(&mut wallet, previous).await
    }

    async fn persist_or_restore(
        &self,
        wallet: &mut LightWallet,
        previous: Option<MigrationState>,
    ) -> Result<(), LightClientError> {
        let persisted = self.persist(wallet).await;
        if persisted.is_err() {
            wallet.migration = previous;
        }
        persisted
    }

    /// Writes the wallet file now, when the save task owns the file. A
    /// client whose consumer saves the wallet bytes itself only sees
    /// `save_required` set.
    async fn persist(&self, wallet: &mut LightWallet) -> Result<(), LightClientError> {
        if !self.save_active.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        if let Some(bytes) = wallet.save().map_err(LightClientError::FileError)? {
            super::save::write_to_path(&self.wallet_path(), &bytes)
                .await
                .map_err(LightClientError::FileError)?;
            wallet.save_required = false;
        }
        Ok(())
    }

    /// The transmit-only client transfers are submitted through, resolved by the
    /// session's transmit policy (ADR 0011, amendments 2026-07-23 and
    /// 2026-09-11) like every other transmitting surface.
    fn migration_transmission_client(
        &self,
    ) -> Result<broadcast_route::RoutedBroadcastClient, LightClientError> {
        use broadcast_route::MigrationWire;

        let sync_indexer = self.indexer_uri();
        #[cfg(feature = "nym")]
        let wire = match self.send_route()? {
            // The guard travels into the client, which dials on every
            // submission long after this function returns.
            crate::mixnet::MixnetRoute::Mixnet(conduit) => MigrationWire::Mixnet(conduit.dial()),
            crate::mixnet::MixnetRoute::Clearnet => MigrationWire::Clearnet,
        };
        #[cfg(not(feature = "nym"))]
        let wire = MigrationWire::Clearnet;
        if matches!(wire, MigrationWire::Clearnet)
            && sync_indexer.is_none()
            && self.migration_transmission_uri.is_none()
        {
            return Err(LightClientError::Offline);
        }
        let candidates = broadcast_route::candidates(
            self.migration_transmission_uri.clone(),
            sync_indexer.as_ref(),
            &self.destination_servers,
            wire.transport(),
            &self.indexer_history.health().lock().expect("health mutex"),
        )?;
        let reaches_untrusted_sync = matches!(wire, MigrationWire::Clearnet)
            && sync_indexer.as_ref().is_some_and(|sync| {
                candidates.contains(sync)
                    && self.destination_servers.trust_of(sync)
                        == crate::destination::servers::Trust::Untrusted
            });
        if reaches_untrusted_sync {
            log::warn!(
                "no dedicated migration transmission endpoint configured; transfers will be \
                 transmitted to the synchronization endpoint, which lets that server \
                 correlate synchronization with migration activity"
            );
        }
        Ok(broadcast_route::RoutedBroadcastClient::new(
            wire, candidates,
        ))
    }

    /// The due-transfer transmission loop, optionally narrowed to a single transfer so
    /// the batch can sequence sends with spacing.
    ///
    /// Proving is parallelised across all due transfers via
    /// [`tokio::task::spawn_blocking`]: wallet reads happen under the write
    /// lock (Phase A), all Halo2/Groth16 work runs concurrently on the
    /// blocking thread pool without holding the lock (Phase B), and wallet
    /// writes + submission happen sequentially under the lock again (Phase C).
    async fn transmit_transfers_selected(
        &mut self,
        client: &impl BroadcastClient,
        only: Option<TransferId>,
    ) -> Result<BroadcastPass, LightClientError> {
        type ProveHandle = tokio::task::JoinHandle<
            Result<
                (usize, TxId, Vec<u8>, BlockHeight, BlockHeight),
                crate::wallet::error::WalletError,
            >,
        >;

        // ── Phase A: prepare inputs under the wallet write lock ──────────
        // Each Assigned transfer produces an owned proving closure; already-Signed
        // transfers yield their raw bytes directly. No expensive work happens here.
        let (prove_handles, pre_proven, strategy) = {
            let mut wallet = self.wallet().write().await;
            wallet
                .edit_migration(|wallet, state| {
                    let now_height = wallet.known_chain_height()?;
                    let current_bucket =
                        schedule::bucket_index(now_height, state.params.bucket_modulus);

                    let mut prove_handles: Vec<ProveHandle> = Vec::new();
                    let mut pre_proven: Vec<(usize, TxId, Vec<u8>, BlockHeight)> = Vec::new();

                    for index in 0..state.transfers.len() {
                        let due = {
                            let transfer = &state.transfers[index];
                            schedule::transfer_in_current_bucket(transfer, current_bucket)
                                && only.is_none_or(|transfer_id| transfer.id == transfer_id)
                        };
                        if !due {
                            continue;
                        }

                        if state.transfers[index].state == TransferState::Assigned {
                            let account = state.account;
                            let params = state.params.clone();
                            match wallet.build_transfer(account, &mut state.transfers[index], &params)? {
                                BuildResult::Ready {
                                    prove,
                                    target_height,
                                    expiry_height,
                                } => {
                                    prove_handles.push(tokio::task::spawn_blocking(move || {
                                        prove.prove().map(|(txid, raw_tx)| {
                                            (index, txid, raw_tx, target_height, expiry_height)
                                        })
                                    }));
                                }
                                BuildResult::Skip(reason) => {
                                    log::info!(
                                        "skipping transfer {index}: {reason:?}; it falls to reconciliation"
                                    );
                                }
                            }
                        } else {
                            // Signed already (an earlier submit failed): recover
                            // the bytes from the blob or the wallet's tx record.
                            let transfer = &state.transfers[index];
                            let txid = transfer.txid.expect("signed transfers have txids");
                            let expiry = transfer
                                .expiry_height
                                .expect("signed transfers have expiry heights");
                            let bytes = match &transfer.signed_blob {
                                Some(blob) => blob.clone(),
                                None => {
                                    let tx = wallet.wallet_transactions.get(&txid).ok_or(
                                        crate::wallet::error::WalletError::TransactionNotFound(
                                            txid,
                                        ),
                                    )?;
                                    let mut bytes = Vec::new();
                                    tx.transaction().write(&mut bytes).map_err(
                                        crate::wallet::error::WalletError::TransactionWrite,
                                    )?;
                                    bytes
                                }
                            };
                            pre_proven.push((index, txid, bytes, expiry));
                        }
                    }
                    Ok::<_, LightClientError>((prove_handles, pre_proven, state.strategy))
                })?
        }; // wallet write lock released: Phase B runs without the lock

        // ── Phase B: parallel proving (no wallet lock held) ───────────────
        // All Halo2 + Groth16 work runs concurrently on the blocking thread
        // pool. Wall-clock cost = slowest single proof, not the sum.
        let mut newly_proven: Vec<(usize, TxId, Vec<u8>, BlockHeight, BlockHeight)> = Vec::new();
        for handle in prove_handles {
            let result = handle
                .await
                .map_err(|e| crate::wallet::error::WalletError::MigrationBuild(e.to_string()))??;
            newly_proven.push(result);
        }

        // ── Phase C: record results + submit under the wallet write lock ──
        // The migration state is inside the wallet at every await point, so
        // neither an error nor a cancelled future can strand it outside.
        let mut wallet = self.wallet().write().await;

        // Record all newly proved transfers (mark Signed, store tx in wallet),
        // then combine proved and pre-proven in original transfer order.
        let all_to_submit = wallet.edit_migration(|wallet, state| {
            let mut newly_proven_with_expiry: Vec<(usize, TxId, Vec<u8>, BlockHeight)> = Vec::new();
            for (index, txid, raw_tx, target_height, expiry_height) in newly_proven {
                wallet.record_transfer_result(
                    &mut state.transfers[index],
                    txid,
                    &raw_tx,
                    target_height,
                    expiry_height,
                    strategy,
                )?;
                newly_proven_with_expiry.push((index, txid, raw_tx, expiry_height));
            }

            let mut all_to_submit: Vec<(usize, TxId, Vec<u8>, BlockHeight)> =
                newly_proven_with_expiry
                    .into_iter()
                    .chain(pre_proven)
                    .collect();
            all_to_submit.sort_by_key(|(index, ..)| *index);
            Ok::<_, LightClientError>(all_to_submit)
        })?;

        let mut sent = Vec::new();
        for (index, txid, raw_tx, expiry_height) in all_to_submit {
            // Record the attempt before submission so a crash between
            // submit and record is detectable via nullifier on reconcile.
            let transfer = wallet
                .with_migration_state(|wallet, state| {
                    state.transfers[index].record_attempt();
                    wallet.save_required = true;
                    state.transfers[index].id
                })
                .ok_or(MigrationError::NoMigration)?;
            self.persist(&mut wallet).await?;
            let started = std::time::Instant::now();
            let submitted = self
                .submit_with_probes(client, raw_tx, txid, expiry_height)
                .await;
            match submitted {
                Ok(receipt) => {
                    record_transfer_route(&self.indexer_history, &receipt.route, started, Ok(()));
                    wallet.edit_migration(|wallet, state| {
                        state.transfers[index].mark_broadcast()?;
                        wallet.save_required = true;
                        Ok::<_, LightClientError>(())
                    })?;
                    self.persist(&mut wallet).await?;
                    sent.push(BroadcastReceipt {
                        txid,
                        route: receipt.route,
                    });
                }
                Err(error) => {
                    if let Some(route) = error.route() {
                        let kind = crate::lightclient::indexer_history::FailureKind::classify(
                            &error.to_string(),
                        );
                        record_transfer_route(&self.indexer_history, route, started, Err(kind));
                    }
                    log::warn!("transfer submission failed, leaving the transfer signed: {error}");
                    return Ok(BroadcastPass::Halted { transfer, error });
                }
            }
        }
        Ok(BroadcastPass::Complete(sent))
    }

    /// Plans an immediate migration of the account's Orchard pool into Ironwood.
    ///
    /// Pure and deterministic, nothing is signed or sent, so the plan
    /// can be shown to the user for consent before [`Self::migrate_immediately`] executes it.
    pub(crate) async fn plan_immediate_migration(
        &self,
        account: zip32::AccountId,
    ) -> Result<ImmediateMigrationPlan, LightClientError> {
        let wallet = self.wallet().read().await;
        Ok(wallet.plan_immediate_migration(account)?)
    }

    /// Transmits the immediate Orchard→Ironwood migration against the wallet's
    /// *current* state, without syncing first.
    ///
    /// This is the immediate migration minus a leading
    /// `sync_and_await`, for consumers that own the sync lifecycle and keep a
    /// background sync running continuously (e.g. zingo-mobile). Calling the
    /// syncing variant from such a consumer collides with the running sync
    /// and fails with [`pepper_sync::error::SyncModeError::SyncAlreadyRunning`].
    /// This entry point lets the caller drive sync itself.
    ///
    /// The caller is responsible for keeping the wallet synced before
    /// calling, and proves it has paused its sync by presenting the
    /// [`SyncPauseGuard`]. [`Self::pause_sync_scoped`] pauses a running
    /// engine and resumes it when the guard drops. Planning and building
    /// therefore observe one stable wallet state, the same
    /// pause-before-proposing invariant the `send`/`shield` mutation paths
    /// establish. The plan, the chunked transmission, and the idempotent cleanup
    /// on partial failure are identical to the syncing variant.
    ///
    /// Calling without the guard does not compile. A stable wallet state
    /// across plan and build is a compile-time precondition, not a runtime
    /// courtesy:
    ///
    /// ```compile_fail
    /// # async fn caller(client: &mut zingolib::lightclient::LightClient) {
    /// let _ = client
    ///     .migrate_immediately_presynced(zip32::AccountId::ZERO)
    ///     .await;
    /// # }
    /// ```
    pub(crate) async fn migrate_immediately_presynced(
        &mut self,
        account: zip32::AccountId,
        sync: &SyncPauseGuard,
    ) -> Result<ImmediateMigrationSummary, LightClientError> {
        // A scheduled migration reserves the notes its transfers are bound to.
        // Migrating them immediately would invalidate those transfers behind its back.
        {
            let mut wallet = self.wallet().write().await;
            match &wallet.migration {
                None => (),
                Some(state) if matches!(state.phase, MigrationPhase::Complete { .. }) => {
                    wallet.migration = None;
                    wallet.save_required = true;
                }
                Some(_) => return Err(MigrationError::AlreadyInProgress.into()),
            }
        }

        let plan = self.plan_immediate_migration(account).await?;
        if plan.is_empty() {
            return Err(crate::wallet::error::WalletError::NothingToMigrate.into());
        }

        // Arm per-transaction progress for the poll side channel. The scope
        // guard owns an `Arc` clone (not a borrow of `self`), so it survives the
        // `&mut self` `build_and_transmit` call and clears the snapshot on every
        // exit: success, `?`-propagated error, or panic.
        self.migration_progress
            .begin_immediate(plan.transactions.len() as u32);
        let _scope = ProgressScope(self.migration_progress.clone());
        let progress = self.migration_progress.clone();

        let txids = self
            .build_and_transmit(&plan.transactions, sync, &progress, |wallet, planned| {
                wallet.build_immediate_migration_transaction(account, planned)
            })
            .await?;

        Ok(ImmediateMigrationSummary {
            txids,
            migrated: plan.migrated,
            fee: plan.fee,
            residual: plan.residual,
        })
    }

    /// Builds and broadcasts one batch of planned migration transactions under
    /// a caller-held [`SyncPauseGuard`], enforcing the shared cleanup
    /// contract: a build failure fails the transactions already built, and a
    /// transmit failure fails every transaction still unsent, so no note
    /// stays spent by a transaction that will never reach the network. Both
    /// the immediate migration flow and the note-preparation rounds send through here. The
    /// guard parameter is pure proof. The caller's guard performs the
    /// pause and its drop the resume, on every exit path.
    async fn build_and_transmit<T>(
        &mut self,
        planned: &[T],
        _sync: &SyncPauseGuard,
        progress: &impl BuildProgressSink,
        build: impl Fn(&mut LightWallet, &T) -> Result<TxId, WalletError>,
    ) -> Result<Vec<TxId>, LightClientError> {
        let txids = self.build_transactions(planned, progress, build).await?;

        // Build is done; the transmit loop below publishes "sent i/N". No-op
        // unless the caller (an immediate migration or a note-preparation round)
        // armed its side channel.
        progress.on_transmit();

        let transmitted = self
            .transmit_transactions(
                NonEmpty::from_vec(txids.clone()).expect("planned batches are never empty"),
            )
            .await;

        if let Err(e) = transmitted {
            // `transmit_transactions` marks the transaction that failed, but
            // the ones queued behind it stay `Calculated`: their notes would
            // remain spent by transactions that will never reach the network.
            // Fail them so the next pass re-plans them.
            self.fail_unsent_transactions(&txids).await;
            return Err(e);
        }

        Ok(txids)
    }

    /// Builds every planned transaction under one wallet lock. On failure,
    /// fails the transactions already built so their notes do not stay spent
    /// by transactions that will never be sent.
    async fn build_transactions<T>(
        &mut self,
        planned: &[T],
        progress: &impl BuildProgressSink,
        build: impl Fn(&mut LightWallet, &T) -> Result<TxId, WalletError>,
    ) -> Result<Vec<TxId>, LightClientError> {
        let mut wallet = self.wallet().write().await;
        let mut txids = Vec::with_capacity(planned.len());

        for item in planned {
            match build(&mut wallet, item) {
                Ok(txid) => {
                    txids.push(txid);
                    // Publish "built i/N". No-op unless the caller armed a side
                    // channel, so ordinary sends stay untouched.
                    progress.on_built(txids.len() as u32);
                }
                Err(e) => {
                    if !txids.is_empty() {
                        pepper_sync::set_transactions_failed(
                            &mut wallet.wallet_transactions,
                            txids,
                        );
                        wallet.save_required = true;
                    }
                    return Err(e.into());
                }
            }
        }

        Ok(txids)
    }

    /// Marks every transaction still sitting in `Calculated` as failed,
    /// releasing the notes it reserved.
    async fn fail_unsent_transactions(&mut self, txids: &[TxId]) {
        let mut wallet = self.wallet().write().await;
        let unsent: Vec<TxId> = txids
            .iter()
            .copied()
            .filter(|txid| {
                wallet.wallet_transactions.get(txid).is_some_and(|tx| {
                    matches!(
                        tx.status(),
                        zingo_status::confirmation_status::ConfirmationStatus::Calculated(_)
                    )
                })
            })
            .collect();
        if !unsent.is_empty() {
            pepper_sync::set_transactions_failed(&mut wallet.wallet_transactions, unsent);
            wallet.save_required = true;
            if let Err(error) = self.persist(&mut wallet).await {
                log::warn!("failed transactions were not persisted: {error}");
            }
        }
    }
}

const RECONCILIATION_PASSES: usize = 3;

/// The wallet half of reconciliation: reconcile and apply, without the file
/// write. The sync task runs it when a sync completes.
pub(crate) fn reconcile_wallet_migration(
    wallet: &mut LightWallet,
) -> Result<Option<ReconcileReport>, LightClientError> {
    if wallet.migration.is_none() {
        return Ok(None);
    }
    wallet.refresh_transfer_witnesses()?;
    let report = wallet.edit_migration(|wallet, state| {
        let mut report = reconcile(state, &*wallet);
        for pass in 1..=RECONCILIATION_PASSES {
            for action in &report.actions {
                apply_action(wallet, state, action)?;
            }
            if !report.actions.iter().any(changes_a_transfer) || pass == RECONCILIATION_PASSES {
                break;
            }
            report = reconcile(state, &*wallet);
        }
        settle_preparation_phase(wallet, state, &report)?;
        wallet.save_required = true;
        Ok::<_, LightClientError>(report)
    })?;
    Ok(Some(report))
}

fn changes_a_transfer(action: &RecommendedAction) -> bool {
    matches!(
        action,
        RecommendedAction::PromoteConfirmed { .. }
            | RecommendedAction::MarkInvalidated { .. }
            | RecommendedAction::Reschedule { .. }
            | RecommendedAction::DiscardAndReschedule { .. }
            | RecommendedAction::Demote { .. }
            | RecommendedAction::Reopen
            | RecommendedAction::AbandonWire { .. }
    )
}

fn schedule_is_open(state: &MigrationState) -> Result<(), LightClientError> {
    match state.phase {
        MigrationPhase::Prepared => Ok(()),
        MigrationPhase::Scheduled
            if state.transfers.iter().all(|transfer| {
                matches!(
                    transfer.state,
                    TransferState::Bound | TransferState::Assigned | TransferState::Released
                )
            }) =>
        {
            Ok(())
        }
        MigrationPhase::Scheduled => Err(MigrationError::ScheduleFixed.into()),
        _ => Err(MigrationError::NotPrepared.into()),
    }
}

fn bind_unreleased(
    wallet: &LightWallet,
    state: &MigrationState,
) -> Result<Vec<TransferRecord>, LightClientError> {
    let released: Vec<BoundNote> = state
        .transfers
        .iter()
        .filter(|transfer| transfer.state == TransferState::Released)
        .filter_map(|transfer| transfer.note)
        .collect();
    let mut fresh = state.clone();
    fresh.transfers = Vec::new();
    wallet.bind_transfers_to_notes(&mut fresh, state.account)?;
    let mut transfers: Vec<TransferRecord> = fresh
        .transfers
        .into_iter()
        .filter(|transfer| !transfer.note.is_some_and(|note| released.contains(&note)))
        .collect();
    for (index, transfer) in transfers.iter_mut().enumerate() {
        transfer.id = TransferId(index as u32);
    }
    Ok(transfers)
}

fn draw_schedule(
    wallet: &LightWallet,
    state: &MigrationState,
    params: &MigrationParams,
) -> Result<Vec<TransferRecord>, LightClientError> {
    let mut transfers = bind_unreleased(wallet, state)?;
    let now_height = wallet.known_chain_height()?;
    let activation = wallet.ironwood_activation()?;
    plan_schedule(
        &mut transfers,
        now_height,
        activation,
        |transfer| wallet.bound_note_confirmed_at(transfer),
        params,
        &mut rand::rngs::OsRng,
    )?;
    Ok(transfers)
}

fn pending_round_txids(
    wallet: &LightWallet,
    pending_txids: &[TxId],
) -> Result<Vec<TxId>, LightClientError> {
    let unresolved: Vec<TxId> = pending_txids
        .iter()
        .filter(|txid| {
            !wallet.transaction_failed(txid) && wallet.transaction_confirmed_height(txid).is_none()
        })
        .copied()
        .collect();
    if !unresolved.is_empty() {
        return Ok(unresolved);
    }
    let (_, anchor_height) = wallet
        .get_migration_heights()?
        .ok_or(WalletError::NoSyncData)?;
    Ok(pending_txids
        .iter()
        .filter(|txid| {
            wallet
                .transaction_confirmed_height(txid)
                .is_some_and(|height| height > anchor_height)
        })
        .copied()
        .collect())
}

fn settle_preparation_phase(
    wallet: &LightWallet,
    state: &mut MigrationState,
    report: &ReconcileReport,
) -> Result<(), LightClientError> {
    match &state.phase {
        MigrationPhase::Preparing { .. }
            if report
                .actions
                .contains(&RecommendedAction::ContinueNotePreparation)
                && round_is_anchored(wallet, state)? =>
        {
            let plan = wallet.plan_ironwood_migration_now(state.account)?;
            if plan.is_prepared() {
                state.phase = MigrationPhase::Prepared;
            }
        }
        MigrationPhase::Prepared => {
            let plan = wallet.plan_ironwood_migration_now(state.account)?;
            if !plan.is_prepared() {
                state.commitment.plan_hash = plan_hash(&plan);
                state.phase = MigrationPhase::Committed;
            }
        }
        _ => (),
    }
    Ok(())
}

fn round_is_anchored(
    wallet: &LightWallet,
    state: &MigrationState,
) -> Result<bool, LightClientError> {
    let MigrationPhase::Preparing { pending_txids, .. } = &state.phase else {
        return Ok(false);
    };
    Ok(pending_round_txids(wallet, pending_txids)?.is_empty())
}

fn apply_action(
    wallet: &mut LightWallet,
    state: &mut MigrationState,
    action: &RecommendedAction,
) -> Result<(), LightClientError> {
    match action {
        RecommendedAction::PromoteConfirmed { transfer, height } => {
            state.transfers[transfer.0 as usize].mark_confirmed(*height)?;
        }
        RecommendedAction::MarkInvalidated { transfer } => {
            state.transfers[transfer.0 as usize].mark_invalidated()?;
        }
        RecommendedAction::Reschedule { transfer } => {
            let index = transfer.0 as usize;
            state.transfers[index].record_missed_window();
            place_in_next_window(wallet, state, index)?;
        }
        RecommendedAction::DiscardAndReschedule { transfer } => {
            let index = transfer.0 as usize;
            discard_signature(wallet, &mut state.transfers[index])?;
            state.transfers[index].record_missed_window();
            place_in_next_window(wallet, state, index)?;
        }
        RecommendedAction::AbandonWire { transfer } => {
            let transfer = &mut state.transfers[transfer.0 as usize];
            if let Some(txid) = transfer.forget_wire() {
                pepper_sync::set_transactions_failed(&mut wallet.wallet_transactions, vec![txid]);
            }
        }
        RecommendedAction::MarkComplete { residual } => {
            state.phase = MigrationPhase::Complete {
                residual: *residual,
            };
        }
        RecommendedAction::Demote { transfer } => {
            state.transfers[transfer.0 as usize].mark_reorged()?;
        }
        RecommendedAction::Reopen => {
            state.phase = MigrationPhase::Scheduled;
        }
        RecommendedAction::RetryPreparation { .. }
        | RecommendedAction::AwaitPreparationConfirmation
        | RecommendedAction::ContinueNotePreparation => (),
    }
    Ok(())
}

fn release(
    wallet: &mut LightWallet,
    transfer: &mut TransferRecord,
) -> Result<(), LightClientError> {
    if transfer.state == TransferState::Signed && transfer.attempts == 0 {
        discard_signature(wallet, transfer)?;
    }
    transfer.mark_released()?;
    Ok(())
}

fn discard_signature(
    wallet: &mut LightWallet,
    transfer: &mut TransferRecord,
) -> Result<(), LightClientError> {
    if let Some(txid) = transfer.discard_signature()? {
        pepper_sync::set_transactions_failed(&mut wallet.wallet_transactions, vec![txid]);
    }
    Ok(())
}

enum Record {
    Keep,
    Remove,
}

fn place_in_next_window(
    wallet: &LightWallet,
    state: &mut MigrationState,
    index: usize,
) -> Result<(), LightClientError> {
    let now_height = wallet.known_chain_height()?;
    let activation = wallet.ironwood_activation()?;
    let floor = schedule::AnchorFloor::new(
        activation,
        wallet.bound_note_confirmed_at(&state.transfers[index]),
    );
    let capacity = usize::try_from(state.params.k_max.max(1)).expect("u32 fits usize");
    let occupancy = |window: u64| {
        state
            .transfers
            .iter()
            .enumerate()
            .filter(|(other, transfer)| {
                *other != index
                    && transfer.bucket_index == Some(window)
                    && !matches!(
                        transfer.state,
                        TransferState::Released | TransferState::Invalidated
                    )
            })
            .count()
    };
    let window = (schedule::first_permitted_bucket(now_height, &floor, &state.params)..)
        .find(|window| occupancy(*window) < capacity)
        .expect("the transfers occupy finitely many windows");
    schedule::place(
        &mut state.transfers[index],
        window,
        &floor,
        &mut rand::rngs::OsRng,
        &state.params,
    )?;
    Ok(())
}

impl crate::wallet::LightWallet {
    fn migration_state(&self) -> Result<&MigrationState, MigrationError> {
        self.migration.as_ref().ok_or(MigrationError::NoMigration)
    }

    fn edit_migration<R, E: From<MigrationError>>(
        &mut self,
        edit: impl FnOnce(&mut Self, &mut MigrationState) -> Result<R, E>,
    ) -> Result<R, E> {
        self.with_migration_state(edit)
            .ok_or(MigrationError::NoMigration)?
    }

    fn known_chain_height(&self) -> Result<BlockHeight, WalletError> {
        self.sync_state
            .last_known_chain_height()
            .ok_or(WalletError::NoSyncData)
    }

    /// Plans the migration from the wallet's current state: pure over the
    /// wallet, no lock management of its own. The read-only public planner
    /// calls it under a read guard. The consent brackets of
    /// `commit_migration` and the immediate path call it under the
    /// same write guard that binds, so the plan hashed and the notes bound
    /// come from one uninterrupted wallet view (issue #2493, finding 11).
    #[allow(clippy::result_large_err)]
    pub(crate) fn plan_ironwood_migration_now(
        &self,
        account: zip32::AccountId,
    ) -> Result<ScheduledMigrationPlan, crate::wallet::error::WalletError> {
        let params = MigrationParams::provisional(self.chain_type());
        Ok(plan_preparation_of(
            &self.migration_note_values(account)?,
            self.preparations_confirm_post_activation(),
            &params,
        ))
    }

    /// Whether a transaction built now confirms at or after NU6.3
    /// activation. Note-splitting fees depend on it (the Orchard bundle's
    /// cross-address rules change the action count).
    pub(crate) fn preparations_confirm_post_activation(&self) -> bool {
        match (
            self.sync_state.last_known_chain_height(),
            pepper_sync::wallet::PoolActivation::of(
                &self.chain_type(),
                zcash_protocol::ShieldedPool::Ironwood,
            ),
        ) {
            (Some(chain_height), Some(activation)) => chain_height + 1 >= activation.height(),
            _ => false,
        }
    }

    /// The Ironwood Pool Activation, or the migration-build error every
    /// migration path shares when the chain never activates NU6.3.
    #[allow(clippy::result_large_err)]
    pub(crate) fn ironwood_activation(
        &self,
    ) -> Result<pepper_sync::wallet::PoolActivation, crate::wallet::error::WalletError> {
        pepper_sync::wallet::PoolActivation::of(
            &self.chain_type(),
            zcash_protocol::ShieldedPool::Ironwood,
        )
        .ok_or_else(|| {
            crate::wallet::error::WalletError::MigrationBuild(
                "NU6.3 has no activation height".to_string(),
            )
        })
    }
}

/// The separator between two layers of a rendered cause chain, matching the
/// rendering `zingo_net_diag` gives its own failure records.
pub(crate) const CAUSE_CHAIN_SEPARATOR: &str = ": ";

/// Renders every layer of a failure's cause chain into the one text a batch
/// report carries across serde.
fn render_cause_chain(error: &LightClientError) -> String {
    zingo_net_diag::chain_texts(error).join(CAUSE_CHAIN_SEPARATOR)
}

/// Records one transfer submission's own route evidence in the cross-session
/// indexer history, so an audit reads the wire each transfer actually traveled
/// rather than inferring it from the session's policy afterwards. The
/// evidence never enters the wallet file: the history is its home, and the
/// wallet's persisted grammar is untouched.
fn record_transfer_route(
    history: &crate::lightclient::indexer_history::IndexerHistoryHandle,
    route: &crate::wallet::migration::BroadcastRoute,
    started: std::time::Instant,
    outcome: Result<(), crate::lightclient::indexer_history::FailureKind>,
) {
    use crate::lightclient::indexer_history::{
        AttemptKind, AttemptRoute, IndexerAttempt, now_unix_secs,
    };
    use crate::wallet::migration::BroadcastRoute;

    let (host, attempt_route) = match route {
        BroadcastRoute::Mixnet { destination, .. } => (
            crate::destination::Host::of_host_str(destination),
            AttemptRoute::Mixnet,
        ),
        BroadcastRoute::Clearnet { endpoint } => (
            crate::destination::Host::of_host_str(endpoint),
            AttemptRoute::Clearnet,
        ),
    };
    history.record(&IndexerAttempt {
        unix_secs: now_unix_secs(),
        host,
        route: attempt_route,
        kind: AttemptKind::Send,
        millis: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        fault_domain: None,
        outcome,
    });
}

fn window_timeline_of(wallet: &LightWallet) -> Option<Vec<WindowReport>> {
    let now_height = wallet.sync_state.last_known_chain_height()?;
    Some(match &wallet.migration {
        Some(state) => {
            crate::wallet::migration::window_timeline(&state.transfers, now_height, &state.params)
        }
        None => crate::wallet::migration::window_timeline(
            &[],
            now_height,
            &MigrationParams::provisional(wallet.chain_type()),
        ),
    })
}

#[cfg(test)]
mod tests;
