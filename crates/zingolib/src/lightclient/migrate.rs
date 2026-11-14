//! Orchard→Ironwood migration orchestration.
//!
//! The scheduled flow a mobile client drives:
//! [`LightClient::plan_ironwood_migration`] →
//! [`LightClient::start_ironwood_migration`] (consent) →
//! [`LightClient::continue_note_splitting`] each time the wallet is synced to
//! the chain tip, until the parts are scheduled →
//! [`LightClient::reschedule_parts`] when the user picks the
//! Phase 2 cadence → [`LightClient::reconcile_migration`] on every launch →
//! [`LightClient::transmit_due_parts`] from background wakes →
//! [`LightClient::catch_up_migration`] when windows were missed.
//!
//! Several of these calls are made once the wallet is synced to the chain
//! tip. A Sync Session with `shutdown_on_completion` set returns at that
//! point, so the call follows [`LightClient::await_sync`] or a ready
//! [`LightClient::poll_sync`]. A continuous Sync Session keeps running and
//! scans each newly mined block, so the call follows a sync status for which
//! [`pepper_sync::sync::SyncStatus::is_complete`] returns true, read from
//! [`LightClient::subscribe_sync_status`] or
//! [`LightClient::latest_sync_status`]. A status is published after every
//! scan, so each newly mined block is followed by one.
//!
//! [`LightClient::sync_to_tip_and_await`] reaches the same point in one call
//! whatever `shutdown_on_completion` is set to: it stops a running Sync
//! Session, syncs to the chain tip and returns. It leaves sync stopped, so
//! it suits a caller that makes one migration call and relaunches sync
//! itself, as the CLI's migration commands do.
//!
//! [`LightClient::migrate_to_ironwood`] composes the same pieces into an
//! interactive one-call for CLI use, testing, and the user who prefers the
//! immediate migration ZIP 318 permits with a disclosed privacy trade-off.
//!
//! [`LightClient::migrate_immediately`] is the other option ZIP 318
//! offers the user: move everything at once, no note splitting and no
//! schedule, accepting that the transfers are correlated and the amounts are
//! the wallet's own.
//!
//! Part transmissions obey the Mixnet Mode policy (ADR 0011, amendment
//! 2026-07-23): while the mode is on they travel only over the mixnet, fail
//! closed while it is not ready, and never target the synchronization
//! endpoint's host. See [`transmission_route`].

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nonempty::NonEmpty;
use zcash_primitives::transaction::TxId;

use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, MigrationError};
use crate::lightclient::sync::SyncPauseGuard;
use zcash_protocol::consensus::BlockHeight;

use crate::wallet::LightWallet;
use crate::wallet::error::WalletError;
use crate::wallet::migration::{
    ChainView, ConsentBinding, ImmediateMigrationPlan, MigrationMode, MigrationParams,
    MigrationPhase, MigrationPlan, MigrationState, PartId, PartState, PrepareResult, ProvedPart,
    RecommendedAction, ReconcileReport, SigningStrategy, TransmissionClient, TransmissionWindow,
    WindowReport, due_now_parts, plan_hash, plan_migration, plan_schedule, reconcile, schedule,
};

pub mod transmission_grpc;
pub mod transmission_route;

use zl_netutils::time::CONFIRMATION_POLL_INTERVAL;
/// Give up waiting for a note-splitting round after this many polls.
const MAX_CONFIRMATION_POLLS: usize = 720;
/// A migration replans after every round. A real plan converges in
/// `~log_K(N)` rounds, so far more than this means something is wrong.
const MAX_ROUNDS: usize = 64;
/// How many buckets ahead [`LightClient::migration_status`] reports windows
/// for.
const WAKE_HORIZON_BUCKETS: u64 = 32;

/// The transactions of a completed immediate migration
/// ([`LightClient::migrate_immediately`]).
#[derive(Debug, Clone)]
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
/// "built i/N, sent i/N". The immediate-migration counterpart to [`MigrationStatus`].
/// `None` from [`ImmediateMigrationProgressHandle::status`] means no immediate migration is running.
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

/// A cloneable handle to an immediate migration's live progress, readable without
/// touching the wallet lock. A consumer that runs the immediate migration (which borrows the
/// client `&mut self`) grabs this via [`LightClient::immediate_migration_progress_handle`]
/// *before* starting the immediate migration, then polls [`Self::status`] concurrently.
///
/// The immediate migration holds the wallet write lock across its whole build and transmit
/// loops, so progress lives in this side channel instead of in wallet state:
/// a poll never contends with the immediate migration for the wallet lock.
#[derive(Debug, Clone, Default)]
pub struct ImmediateMigrationProgressHandle(Arc<Mutex<Option<ImmediateMigrationStatus>>>);

impl ImmediateMigrationProgressHandle {
    /// The current migration snapshot, or `None` when no immediate migration is running.
    pub fn status(&self) -> Option<ImmediateMigrationStatus> {
        self.0
            .lock()
            .expect("immediate-migration progress mutex poisoned")
            .clone()
    }

    /// Arms a fresh migration of `total` transactions. Every other mutator is a
    /// no-op until this has been called, which is what scopes progress to the
    /// immediate migration and leaves the shared build/transmit primitives
    /// untouched for every other caller.
    pub(crate) fn begin(&self, total: u32) {
        *self
            .0
            .lock()
            .expect("immediate-migration progress mutex poisoned") =
            Some(ImmediateMigrationStatus {
                total,
                built: 0,
                sent: 0,
                phase: ImmediateMigrationPhase::Building,
            });
    }

    /// Publishes the number of transactions built so far. No-op when idle.
    pub(crate) fn set_built(&self, built: u32) {
        if let Some(status) = self
            .0
            .lock()
            .expect("immediate-migration progress mutex poisoned")
            .as_mut()
        {
            status.built = built;
        }
    }

    /// Advances the phase to [`ImmediateMigrationPhase::Transmitting`]. No-op when idle.
    pub(crate) fn enter_transmit(&self) {
        if let Some(status) = self
            .0
            .lock()
            .expect("immediate-migration progress mutex poisoned")
            .as_mut()
        {
            status.phase = ImmediateMigrationPhase::Transmitting;
        }
    }

    /// Publishes the number of transactions transmitted so far. No-op when idle.
    pub(crate) fn set_sent(&self, sent: u32) {
        if let Some(status) = self
            .0
            .lock()
            .expect("immediate-migration progress mutex poisoned")
            .as_mut()
        {
            status.sent = sent;
        }
    }

    /// Returns to the idle state, so a poll reports `None` once more.
    pub(crate) fn clear(&self) {
        *self
            .0
            .lock()
            .expect("immediate-migration progress mutex poisoned") = None;
    }
}

/// Clears the immediate-migration progress on drop, so a failed or early-returning migration never
/// leaves a stale snapshot behind. Owns an `Arc` clone (not a borrow of the
/// client) so it can live across the `&mut self` [`LightClient::build_and_transmit`]
/// call.
struct ImmediateMigrationProgressScope(ImmediateMigrationProgressHandle);

impl Drop for ImmediateMigrationProgressScope {
    fn drop(&mut self) {
        self.0.clear();
    }
}

/// What one [`LightClient::quick_split`] call did. Phase 1 note splitting is
/// driven one round per call. The consumer loops on this until `Complete`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitOutcome {
    /// A round of Orchard self-sends was built and transmitted. Sync until
    /// these confirm, then call [`LightClient::quick_split`] again.
    Round {
        /// The round's transactions.
        txids: Vec<TxId>,
    },
    /// A previously transmitted round has not confirmed yet. Nothing was
    /// built or sent this call. Sync and retry.
    AwaitingConfirmation,
    /// Every note is part-ready. Phase 1 is complete.
    Complete,
}

/// The coarse stage a running note-splitting round is in, mirroring
/// [`ImmediateMigrationPhase`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitPhase {
    /// Proving and signing the round's transactions.
    Building,
    /// Transmitting the built transactions.
    Transmitting,
}

/// A snapshot of the note-splitting round a [`LightClient::quick_split`] call
/// is building, for rendering "built i/N, sent i/N" within that call. The
/// Phase 1 counterpart to [`ImmediateMigrationStatus`]. `None` from
/// [`SplitProgressHandle::status`] means no round is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitStatus {
    /// Transactions in this round (N), fixed when the round begins.
    pub total: u32,
    /// Transactions built (proved + signed) so far, `0..=total`.
    pub built: u32,
    /// Transactions transmitted so far, `0..=total`.
    pub sent: u32,
    /// Which phase the round is in.
    pub phase: SplitPhase,
}

/// A cloneable handle to a note-splitting round's live progress, readable
/// without touching the wallet lock, the side-channel pattern of
/// [`ImmediateMigrationProgressHandle`]. Grab it via [`LightClient::split_progress_handle`]
/// before calling [`LightClient::quick_split`], then poll [`Self::status`]
/// concurrently while the round holds the wallet write lock.
#[derive(Debug, Clone, Default)]
pub struct SplitProgressHandle(Arc<Mutex<Option<SplitStatus>>>);

impl SplitProgressHandle {
    /// The current round snapshot, or `None` when no round is running.
    pub fn status(&self) -> Option<SplitStatus> {
        self.0
            .lock()
            .expect("split progress mutex poisoned")
            .clone()
    }

    /// Arms a fresh round of `total` transactions. Every other mutator is a
    /// no-op until this has been called, which scopes progress to the one
    /// running round and leaves the shared build/transmit primitives
    /// untouched for every other caller.
    pub(crate) fn begin(&self, total: u32) {
        *self.0.lock().expect("split progress mutex poisoned") = Some(SplitStatus {
            total,
            built: 0,
            sent: 0,
            phase: SplitPhase::Building,
        });
    }

    /// Publishes the number of transactions built so far. No-op when idle.
    pub(crate) fn set_built(&self, built: u32) {
        if let Some(status) = self
            .0
            .lock()
            .expect("split progress mutex poisoned")
            .as_mut()
        {
            status.built = built;
        }
    }

    /// Advances the phase to [`SplitPhase::Transmitting`]. No-op when idle.
    pub(crate) fn enter_transmit(&self) {
        if let Some(status) = self
            .0
            .lock()
            .expect("split progress mutex poisoned")
            .as_mut()
        {
            status.phase = SplitPhase::Transmitting;
        }
    }

    /// Publishes the number of transactions transmitted so far. No-op when idle.
    pub(crate) fn set_sent(&self, sent: u32) {
        if let Some(status) = self
            .0
            .lock()
            .expect("split progress mutex poisoned")
            .as_mut()
        {
            status.sent = sent;
        }
    }

    /// Returns to the idle state, so a poll reports `None` once more.
    pub(crate) fn clear(&self) {
        *self.0.lock().expect("split progress mutex poisoned") = None;
    }
}

/// Clears the split progress on drop, so a failed or early-returning round
/// never leaves a stale snapshot behind.
struct SplitProgressScope(SplitProgressHandle);

impl Drop for SplitProgressScope {
    fn drop(&mut self) {
        self.0.clear();
    }
}

/// The progress side channel one shared build/transmit batch reports into.
/// Both the immediate migration and a note-splitting round drive the shared
/// [`LightClient::build_and_transmit`] primitive. Each arms its own handle so
/// a poll reads the right batch. The internal drivers (the scheduled
/// note-splitting loop, `migrate_to_ironwood`) pass `()` to report nowhere.
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

impl BuildProgressSink for ImmediateMigrationProgressHandle {
    fn on_built(&self, built: u32) {
        self.set_built(built);
    }
    fn on_transmit(&self) {
        self.enter_transmit();
    }
}

impl BuildProgressSink for SplitProgressHandle {
    fn on_built(&self, built: u32) {
        self.set_built(built);
    }
    fn on_transmit(&self) {
        self.enter_transmit();
    }
}

/// A cloneable handle to an execute batch's live progress
/// ([`LightClient::execute_due_parts`]), readable without touching the
/// wallet lock, the side-channel pattern of [`ImmediateMigrationProgressHandle`].
/// Grab it via [`LightClient::batch_progress_handle`] before starting the
/// batch, poll [`Self::status`] concurrently.
#[derive(Debug, Clone, Default)]
pub struct BatchProgressHandle(Arc<Mutex<Option<BatchStatus>>>);

impl BatchProgressHandle {
    /// The current batch snapshot, or `None` when no batch is running.
    pub fn status(&self) -> Option<BatchStatus> {
        self.0
            .lock()
            .expect("batch progress mutex poisoned")
            .clone()
    }

    /// Arms a fresh batch over `total` owed parts. Every other mutator is a
    /// no-op until then, scoping progress to the one running batch.
    fn begin(&self, total: u32) {
        *self.0.lock().expect("batch progress mutex poisoned") = Some(BatchStatus {
            total,
            resolved: 0,
            sent: 0,
            phase: BatchPhase::Sending,
        });
    }

    /// Publishes one more part resolved (sent, slid, or found not due),
    /// and the running sent count. No-op when idle.
    fn resolve(&self, resolved: u32, sent: u32) {
        if let Some(status) = self
            .0
            .lock()
            .expect("batch progress mutex poisoned")
            .as_mut()
        {
            status.resolved = resolved;
            status.sent = sent;
        }
    }

    /// Publishes the phase. No-op when idle.
    fn set_phase(&self, phase: BatchPhase) {
        if let Some(status) = self
            .0
            .lock()
            .expect("batch progress mutex poisoned")
            .as_mut()
        {
            status.phase = phase;
        }
    }

    /// Returns to the idle state, so a poll reports `None` once more.
    fn clear(&self) {
        *self.0.lock().expect("batch progress mutex poisoned") = None;
    }
}

/// Clears the batch progress on drop, so a failed or early-returning batch
/// never leaves a stale snapshot behind.
struct BatchProgressScope(BatchProgressHandle);

impl Drop for BatchProgressScope {
    fn drop(&mut self) {
        self.0.clear();
    }
}

/// A snapshot of a running execute batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchStatus {
    /// Parts owed this session.
    pub total: u32,
    /// Parts resolved so far (sent, slid, or found not due).
    pub resolved: u32,
    /// Parts accepted by the transmission endpoint so far.
    pub sent: u32,
    /// What the batch is doing right now.
    pub phase: BatchPhase,
}

/// What a running execute batch is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchPhase {
    /// Proving and submitting the current part.
    Sending,
    /// Waiting out the spacing before the next part.
    Spacing,
}

/// One part's result from an [`LightClient::execute_due_parts`] batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartOutcome {
    /// The part.
    pub part: PartId,
    /// Its denomination, in zatoshis.
    pub denomination: u64,
    /// What happened to it.
    pub result: PartSendResult,
}

/// What one execute batch did with one part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartSendResult {
    /// Accepted by the transmission endpoint.
    Sent(TxId),
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

/// The outcome of one [`LightClient::execute_due_parts`] batch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BatchReport {
    /// Per-part outcomes, in send order.
    pub outcomes: Vec<PartOutcome>,
    /// Set when a submission error halted the batch early. Parts without
    /// an outcome entry were not attempted and remain due.
    pub halted: Option<String>,
}

/// What one pass of the due-part transmission loop achieved.
#[must_use]
enum Transmission {
    /// Every submission the pass attempted was accepted, in submission order.
    Complete(Vec<TxId>),
    /// A submission failed and the pass stopped there.
    Halted {
        /// The parts accepted before the failure, in submission order.
        sent: Vec<TxId>,
        /// The part whose submission failed. It stays signed (its
        /// transaction already recorded in the wallet) and due, so a later
        /// attempt resubmits it.
        part: PartId,
        /// Why the endpoint did not take it.
        error: crate::wallet::migration::PartTransmissionError,
    },
}

impl Transmission {
    /// The accepted txids, for callers that carry on past a failed part.
    fn into_sent(self) -> Vec<TxId> {
        match self {
            Transmission::Complete(sent) | Transmission::Halted { sent, .. } => sent,
        }
    }
}

/// The transactions of a completed migration.
#[derive(Debug, Clone)]
pub struct MigrationSummary {
    /// Note-splitting (Orchard→Orchard) transactions, in transmission order.
    pub split_txids: Vec<TxId>,
    /// Parts (Orchard→Ironwood), one per denomination.
    pub part_txids: Vec<TxId>,
    /// Dust value (zatoshis) left unmigrated in the Orchard pool.
    pub residual: u64,
}

/// What one [`LightClient::continue_note_splitting`] call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitStep {
    /// The next note-splitting round was built and transmitted. Sync until
    /// its transactions confirm, then reconcile and call again.
    RoundTransmitted {
        /// The round just sent, counted from zero.
        round: u32,
        /// Its transactions.
        txids: Vec<TxId>,
    },
    /// The pending round is not replannable yet. `pending` lists its
    /// unconfirmed transactions. An empty list means every transaction
    /// confirmed but the anchor has not reached the round's outputs.
    /// Either way: sync and call again. Nothing was written.
    AwaitingConfirmation {
        /// The transactions still in flight.
        pending: Vec<TxId>,
    },
    /// Note splitting is finished and the parts are bound to their notes
    /// and scheduled. [`LightClient::transmit_due_parts`] takes over from
    /// here.
    SplittingComplete,
}

/// The batch a user-triggered [`LightClient::execute_due_parts`] would
/// transmit this instant.
///
/// A manual-execution client gates its "send batch" action on
/// [`MigrationStatus::due_now`] being `Some`. It is computed to match
/// `execute_due_parts` exactly (the current window's parts whose random
/// target the chain has reached, plus any overdue parts catch-up folds into
/// the current window), so the action never appears when a tap would build
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueBatch {
    /// The current bucket's opening boundary: the anchor height the batch
    /// transmits against.
    pub boundary: BlockHeight,
    /// The parts due right now, all transmitting in the current window.
    pub part_ids: Vec<PartId>,
    /// The parts' denominations in zatoshis, aligned element-for-element with
    /// `part_ids`.
    pub denominations: Vec<u64>,
}

/// The migration's progress, arranged for direct rendering.
#[derive(Debug, Clone)]
pub struct MigrationStatus {
    /// Confirmed-spendable balance left in the *Orchard pool* specifically.
    /// ZIP 318 requires displaying this figure, never only a unified total.
    pub orchard_confirmed_spendable: u64,
    /// Where the migration is, `None` when none is in progress.
    pub phase: Option<MigrationPhase>,
    /// Scheduled parts in total. During Phase 1 (planned or note splitting)
    /// no part records exist yet, so this is the projected plan's part
    /// count. The two agree at the moment parts bind.
    pub parts_total: u32,
    /// Parts confirmed so far.
    pub parts_confirmed: u32,
    /// Total value across all parts, in zatoshis. Projected from the plan
    /// during Phase 1, like [`Self::parts_total`].
    pub value_total: u64,
    /// Value already confirmed into the Ironwood pool, in zatoshis.
    pub value_migrated: u64,
    /// Coming transmission windows, what a mobile platform scheduler feeds into its
    /// earliest-begin requests. Strictly *future* windows: the window the
    /// chain is currently inside is reported by [`Self::due_now`], not here.
    pub upcoming_windows: Vec<TransmissionWindow>,
    /// The batch the client can transmit right now, or `None` when a send
    /// this instant would build nothing (no migration, wrong phase, no part
    /// assigned to the window the chain is inside, or all parts confirmed).
    /// A part's random target does not gate this: it is due for its whole
    /// open window (ADR 0017).
    /// Unlike [`Self::upcoming_windows`] this reports the window the chain is
    /// currently *inside*, which `upcoming_windows` structurally cannot carry.
    pub due_now: Option<DueBatch>,
}

impl LightClient {
    /// Plans a migration from the wallet's current spendable Orchard notes.
    ///
    /// Pure and deterministic: nothing is signed or sent, so the plan (its
    /// transaction count, fees and residual dust) can be shown to the user
    /// for consent before [`Self::migrate_to_ironwood`] executes it.
    pub async fn plan_ironwood_migration(
        &self,
        account: zip32::AccountId,
    ) -> Result<MigrationPlan, LightClientError> {
        let wallet = self.wallet().read().await;
        Ok(wallet.plan_ironwood_migration_now(account)?)
    }

    /// Records the user's consent to a proposed migration plan and persists
    /// the migration state (ZIP 318 requires the whole schedule confirmed
    /// before any transfer is sent).
    ///
    /// `consented_plan_hash` is the [`plan_hash`] of the plan the user was
    /// shown. If the wallet's notes changed in between, the call fails and
    /// the client re-plans. When the note set is already fully split, parts
    /// are bound to their notes and scheduled immediately. Otherwise the
    /// migration starts in the [`MigrationPhase::Planned`] phase and
    /// [`Self::continue_note_splitting`] drives the rounds from there.
    /// `per_bucket` overrides `k_max` in the migration params, capping how
    /// many parts share each transmission window. Lower values spread parts
    /// across more sessions (better privacy, slower completion). Higher values
    /// concentrate them (faster, more correlated). `None` keeps the default,
    /// and the choice can be made or revised later through
    /// [`Self::reschedule_parts`], any time before the first part is signed.
    pub async fn start_ironwood_migration(
        &mut self,
        account: zip32::AccountId,
        strategy: SigningStrategy,
        consented_plan_hash: [u8; 32],
        per_bucket: Option<u32>,
    ) -> Result<(), LightClientError> {
        if strategy == SigningStrategy::PreSigned {
            return Err(MigrationError::PreSignedUnavailable.into());
        }

        // One synchronous critical section under a single write guard:
        // plan, hash check, bind, schedule, persist. Every wallet mutation
        // (including a sync commit) needs this same lock, so the notes
        // hashed are the notes bound; no await point sits inside the
        // bracket, so a cancelled future cannot abandon it midway (issue
        // #2493, finding 11).
        let mut wallet = self.wallet().write().await;
        let plan = wallet.plan_ironwood_migration_now(account)?;
        let hash = plan_hash(&plan);
        if hash != consented_plan_hash {
            return Err(MigrationError::ConsentStale.into());
        }
        if wallet.migration.is_some() {
            return Err(MigrationError::AlreadyInProgress.into());
        }

        let mut params = MigrationParams::provisional(wallet.chain_type());
        if let Some(k) = per_bucket {
            params.k_max = k.max(1);
        }
        let mut state = MigrationState {
            consent: ConsentBinding {
                params_hash: params.params_hash(),
                plan_hash: hash,
                consented_at: u64::from(crate::utils::now()),
            },
            params,
            strategy,
            mode: MigrationMode::Scheduled,
            account,
            phase: MigrationPhase::Planned,
            parts: Vec::new(),
        };
        // When the notes are already fully split, bind the parts and schedule
        // Phase 2 now. Otherwise the migration stays in `Planned` and
        // `continue_note_splitting` drives the Phase 1 splitting rounds. (Issue
        // #2493 finding 1 refused unsplit plans outright, on the premise that
        // nothing drives the splitting phase; the mobile scheduled flow does
        // drive it, so refusing here strands Phase 1 before it can begin.)
        if plan.is_split() {
            let activation = wallet.ironwood_activation()?;
            wallet.bind_parts_to_notes(&mut state, account)?;
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
            plan_schedule(
                &mut state.parts,
                now_height,
                activation,
                |part| wallet.bound_note_confirmed_at(part),
                &state.params,
                &state.params.grid(wallet.chain_type()),
                &mut crate::utils::system_rng(),
            )?;
            state.phase = MigrationPhase::PartsScheduled;
        }
        wallet.migration = Some(state);
        wallet.save_required = true;
        Ok(())
    }

    /// Drives one step of note splitting for the scheduled migration flow:
    /// replans from the wallet's current notes, then either builds and
    /// transmits the next round of Orchard self-sends, or, once the replan
    /// shows every note part-ready, binds the parts to their notes and
    /// schedules them.
    ///
    /// Call it once the wallet is synced to the chain tip, as the module docs
    /// describe, whenever [`Self::reconcile_migration`] reports
    /// [`RecommendedAction::ContinueNoteSplitting`] or
    /// [`RecommendedAction::RetrySplit`], and keep the loop going until it
    /// returns [`SplitStep::SplittingComplete`]. Failed or expired split
    /// transactions need no dedicated handling: their notes come back into
    /// the replan, which re-derives whatever splitting remains. Blind calls
    /// are safe. While the pending round is still confirming, or its outputs
    /// have not reached the anchor, it returns
    /// [`SplitStep::AwaitingConfirmation`] and writes nothing.
    ///
    /// Consent (ZIP 318 FR7): the first round refuses to execute when the
    /// wallet's notes no longer hash to the consented plan
    /// ([`MigrationError::ConsentStale`]). Each later round replans from
    /// where the notes actually are, the continuation semantics of
    /// [`Self::migrate_to_ironwood`].
    ///
    /// Splits are Orchard self-sends, transmitted over the client's regular
    /// server connection rather than the decoupled part-transmission endpoint:
    /// they reveal no value and precede any pool-crossing transfer, and the
    /// caller is interactive here anyway (the sends already coincide with
    /// the user's sync activity).
    pub async fn continue_note_splitting(&mut self) -> Result<SplitStep, LightClientError> {
        // Hold the wallet stable from triage through build: the plan is
        // value-based and the build re-selects notes by value, so a scan
        // landing in between would surface as a spurious build failure.
        let sync = self.pause_sync_scoped()?;

        let (account, consented_plan_hash, next_round) = {
            let wallet = self.wallet().read().await;
            let state = wallet
                .migration
                .as_ref()
                .ok_or(MigrationError::NoMigration)?;
            let next_round = match &state.phase {
                MigrationPhase::PartsScheduled | MigrationPhase::Complete { .. } => {
                    return Ok(SplitStep::SplittingComplete);
                }
                MigrationPhase::Planned => 0,
                MigrationPhase::NoteSplitting {
                    round,
                    pending_txids,
                } => {
                    let pending: Vec<TxId> = pending_txids
                        .iter()
                        .filter(|txid| {
                            !wallet.transaction_failed(txid)
                                && wallet.transaction_confirmed_height(txid).is_none()
                        })
                        .copied()
                        .collect();
                    if !pending.is_empty() {
                        return Ok(SplitStep::AwaitingConfirmation { pending });
                    }
                    // The round's outputs enter planning once the anchor
                    // reaches their confirmation heights; replanning earlier
                    // would read a note set with the round half-applied.
                    let (_, anchor_height) = wallet
                        .get_migration_heights()?
                        .ok_or(WalletError::NoSyncData)?;
                    let unanchored = pending_txids.iter().any(|txid| {
                        wallet
                            .transaction_confirmed_height(txid)
                            .is_some_and(|height| height > anchor_height)
                    });
                    if unanchored {
                        return Ok(SplitStep::AwaitingConfirmation {
                            pending: Vec::new(),
                        });
                    }
                    round + 1
                }
            };
            (state.account, state.consent.plan_hash, next_round)
        };

        if next_round as usize >= MAX_ROUNDS {
            return Err(MigrationError::SplitDidNotConverge(MAX_ROUNDS).into());
        }

        let plan = self.plan_ironwood_migration(account).await?;
        if next_round == 0 && plan_hash(&plan) != consented_plan_hash {
            return Err(MigrationError::ConsentStale.into());
        }

        if plan.is_split() {
            let mut wallet = self.wallet().write().await;
            wallet
                .with_migration_state(|wallet, state| {
                    wallet.bind_parts_to_notes(state, account)?;
                    let now_height = wallet
                        .sync_state
                        .last_known_chain_height()
                        .ok_or(WalletError::NoSyncData)?;
                    let activation = wallet.ironwood_activation()?;
                    plan_schedule(
                        &mut state.parts,
                        now_height,
                        activation,
                        |part| wallet.bound_note_confirmed_at(part),
                        &state.params,
                        &state.params.grid(wallet.chain_type()),
                        &mut crate::utils::system_rng(),
                    )?;
                    state.phase = MigrationPhase::PartsScheduled;
                    wallet.save_required = true;
                    Ok::<_, LightClientError>(())
                })
                .ok_or(MigrationError::NoMigration)??;
            return Ok(SplitStep::SplittingComplete);
        }

        let round = plan
            .split_rounds
            .into_iter()
            .next()
            .expect("unsplit plan has at least one round");
        let txids = self
            .build_transactions(&round, &(), |wallet, planned| {
                wallet.build_note_split_transaction(account, planned)
            })
            .await?;

        // Persist the attempt before transmitting, so a transmit failure
        // (partial or total) leaves a reconcilable round: the failed
        // transactions are marked in the wallet, and the next call replans
        // over their released notes.
        {
            let mut wallet = self.wallet().write().await;
            wallet
                .with_migration_state(|wallet, state| {
                    state.phase = MigrationPhase::NoteSplitting {
                        round: next_round,
                        pending_txids: txids.clone(),
                    };
                    wallet.save_required = true;
                })
                .ok_or(MigrationError::NoMigration)?;
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

        Ok(SplitStep::RoundTransmitted {
            round: next_round,
            txids,
        })
    }

    /// Chooses the Phase 2 cadence: `per_bucket` parts (at least one) share
    /// each transmission window. Callable any time between consent and the
    /// first signed part, which lets a client defer the choice to the
    /// Phase 1 → Phase 2 boundary (the natural place for a "how many
    /// batches?" screen) instead of bundling it into the consent call.
    ///
    /// Before parts exist (the `Planned` and `NoteSplitting` phases) the
    /// choice is recorded and the terminal scheduling step uses it. Once
    /// parts are scheduled, the whole set is re-bucketed under the new
    /// cadence with fresh randomization, starting from the next bucket
    /// boundary. Either way the consent binding is re-recorded under the
    /// updated parameters: the cadence tap is itself the schedule consent
    /// (ZIP 318 FR7, `params_hash` covers `k_max`), and the schedule the
    /// user last confirmed is the one Phase 2 executes.
    ///
    /// Fails with [`MigrationError::CadenceFixed`] once any part is signed,
    /// transmitted, confirmed, or otherwise past `Assigned`: the cadence the
    /// remaining parts were consented under is then already partly executed.
    /// Afterwards, re-read [`Self::migration_status`] and re-arm mobile platform
    /// windows from `upcoming_windows`. The old schedule's times are void.
    pub async fn reschedule_parts(&mut self, per_bucket: u32) -> Result<(), LightClientError> {
        let mut wallet = self.wallet().write().await;
        wallet
            .with_migration_state(|wallet, state| {
                if matches!(state.phase, MigrationPhase::Complete { .. })
                    || state
                        .parts
                        .iter()
                        .any(|part| !matches!(part.state, PartState::Bound | PartState::Assigned))
                {
                    return Err(MigrationError::CadenceFixed.into());
                }

                state.params.k_max = per_bucket.max(1);
                state.consent = ConsentBinding {
                    params_hash: state.params.params_hash(),
                    plan_hash: state.consent.plan_hash,
                    consented_at: u64::from(crate::utils::now()),
                };

                for part in state.parts.iter_mut() {
                    if part.state == PartState::Assigned {
                        part.unassign()?;
                    }
                }
                if !state.parts.is_empty() {
                    let now_height = wallet
                        .sync_state
                        .last_known_chain_height()
                        .ok_or(WalletError::NoSyncData)?;
                    let activation = wallet.ironwood_activation()?;
                    plan_schedule(
                        &mut state.parts,
                        now_height,
                        activation,
                        |part| wallet.bound_note_confirmed_at(part),
                        &state.params,
                        &state.params.grid(wallet.chain_type()),
                        &mut crate::utils::system_rng(),
                    )?;
                }
                wallet.save_required = true;
                Ok::<_, LightClientError>(())
            })
            .ok_or(MigrationError::NoMigration)?
    }

    fn migration_transmission_client(
        &self,
    ) -> Result<transmission_route::RoutedTransmissionClient, LightClientError> {
        use transmission_route::MigrationWire;

        // The same resolution the send path runs, so an Indexerless session
        // and a not-ready mixnet refuse a part exactly as they refuse a send.
        #[cfg(feature = "nym")]
        let wire = {
            use crate::lightclient::send;
            match send::resolve_transmit_route(self.is_online(), self.send_route())? {
                // The guard travels into the client, which dials on every
                // submission long after this function returns.
                Some(dial) => MigrationWire::Mixnet(dial),
                None => MigrationWire::Nakednet,
            }
        };
        #[cfg(not(feature = "nym"))]
        let wire = MigrationWire::Nakednet;
        let indexer = self
            .connected_indexer_uri()
            .ok_or(LightClientError::Offline)?;
        Ok(transmission_route::RoutedTransmissionClient { wire, indexer })
    }

    /// Materializes and transmits every part whose bucket window is open.
    ///
    /// Works from persisted state and the local shard tree only: this path
    /// never synchronizes and never touches the synchronization client
    /// (ZIP 318's decoupling requirement). Parts whose tree state is
    /// unavailable are skipped and fall to reconciliation. Parts in earlier,
    /// missed buckets are catch-up's business, because sending them needs
    /// the user-facing disclosure.
    pub async fn transmit_due_parts(&mut self) -> Result<Vec<TxId>, LightClientError> {
        let client = self.migration_transmission_client()?;
        self.transmit_due_parts_with(&client).await
    }

    /// [`Self::transmit_due_parts`] with an injectable client, for tests
    /// and the one-call path.
    pub(crate) async fn transmit_due_parts_with(
        &mut self,
        client: &impl TransmissionClient,
    ) -> Result<Vec<TxId>, LightClientError> {
        Ok(self
            .transmit_due_parts_selected(client, None)
            .await?
            .into_sent())
    }

    /// The due-part transmission loop, optionally narrowed to a single part so
    /// catch-up can sequence sends with spacing.
    ///
    /// Proving is parallelised across all due parts via
    /// [`tokio::task::spawn_blocking`]: wallet reads happen under the write
    /// lock (Phase A), all Halo2/Groth16 work runs concurrently on the
    /// blocking thread pool without holding the lock (Phase B), and wallet
    /// writes + submission happen sequentially under the lock again (Phase C).
    async fn transmit_due_parts_selected(
        &mut self,
        client: &impl TransmissionClient,
        only: Option<PartId>,
    ) -> Result<Transmission, LightClientError> {
        type ProveHandle =
            tokio::task::JoinHandle<Result<(usize, ProvedPart), crate::wallet::error::WalletError>>;

        // ── Phase A: prepare inputs under the wallet write lock ──────────
        // Each Assigned part produces an owned proving closure; already-Signed
        // parts yield their raw bytes directly. No expensive work happens here.
        let (prove_handles, pre_proven, strategy) = {
            let mut wallet = self.wallet().write().await;
            wallet
                .with_migration_state(|wallet, state| {
                    let now_height = wallet
                        .sync_state
                        .last_known_chain_height()
                        .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
                    let current_bucket =
                        state.params.grid(wallet.chain_type()).index_of(now_height);

                    let mut prove_handles: Vec<ProveHandle> = Vec::new();
                    let mut pre_proven: Vec<(usize, TxId, Vec<u8>, BlockHeight)> = Vec::new();

                    for index in 0..state.parts.len() {
                        let due = {
                            let part = &state.parts[index];
                            schedule::part_in_current_bucket(part, current_bucket)
                                && only.is_none_or(|part_id| part.id == part_id)
                        };
                        if !due {
                            continue;
                        }

                        if state.parts[index].state == PartState::Assigned {
                            let account = state.account;
                            let params = state.params.clone();
                            match wallet.prepare_part(account, &mut state.parts[index], &params)? {
                                PrepareResult::Ready { prove, .. } => {
                                    prove_handles.push(tokio::task::spawn_blocking(move || {
                                        prove.prove().map(|proved| (index, proved))
                                    }));
                                }
                                PrepareResult::Skip(reason) => {
                                    log::info!(
                                        "skipping part {index}: {reason:?}; it falls to reconciliation"
                                    );
                                }
                            }
                        } else {
                            // Signed already (an earlier submit failed): recover
                            // the bytes from the blob or the wallet's tx record.
                            let part = &state.parts[index];
                            let txid = part.txid.expect("signed parts have txids");
                            let expiry = part
                                .expiry_height
                                .expect("signed parts have expiry heights");
                            let bytes = match &part.signed_blob {
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
                })
                .ok_or(MigrationError::NoMigration)??
        }; // wallet write lock released: Phase B runs without the lock

        // ── Phase B: parallel proving (no wallet lock held) ───────────────
        // All Halo2 + Groth16 work runs concurrently on the blocking thread
        // pool. Wall-clock cost = slowest single proof, not the sum.
        let mut newly_proven: Vec<(usize, ProvedPart)> = Vec::new();
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

        // Record all newly proved parts (mark Signed, store tx in wallet),
        // then combine proved and pre-proven in original part order.
        let all_to_submit = wallet
            .with_migration_state(|wallet, state| {
                let mut newly_proven_with_expiry: Vec<(usize, TxId, Vec<u8>, BlockHeight)> =
                    Vec::new();
                for (index, proved) in newly_proven {
                    // The heights the transaction was built against, not a
                    // recomputation: the chain may have crossed the NU7
                    // activation since the build.
                    let ProvedPart {
                        txid,
                        raw_tx,
                        target_height,
                        expiry_height,
                    } = proved;
                    wallet.record_part_result(
                        &mut state.parts[index],
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
            })
            .ok_or(MigrationError::NoMigration)??;

        let mut sent = Vec::new();
        for (index, txid, raw_tx, expiry_height) in all_to_submit {
            // Record the attempt before submission so a crash between
            // submit and record is detectable via nullifier on reconcile.
            wallet
                .with_migration_state(|wallet, state| {
                    state.parts[index].record_attempt();
                    wallet.save_required = true;
                })
                .ok_or(MigrationError::NoMigration)?;
            let started = std::time::Instant::now();
            match client.submit(raw_tx, expiry_height).await {
                Ok(receipt) => {
                    record_part_route(&self.indexer_history, &receipt.route, started, Ok(()));
                    wallet
                        .with_migration_state(|wallet, state| {
                            state.parts[index].mark_broadcast()?;
                            wallet.save_required = true;
                            Ok::<_, crate::wallet::error::WalletError>(())
                        })
                        .ok_or(MigrationError::NoMigration)??;
                    sent.push(txid);
                }
                Err(error) => {
                    log::warn!("part submission failed, leaving the part signed: {error}");
                    let part = wallet
                        .migration
                        .as_ref()
                        .ok_or(MigrationError::NoMigration)?
                        .parts[index]
                        .id;
                    return Ok(Transmission::Halted { sent, part, error });
                }
            }
        }
        Ok(Transmission::Complete(sent))
    }

    /// Abandons the migration. Parts already confirmed naturally stand.
    /// Everything pending is dropped and the soft reservation on the
    /// remaining split notes is lifted.
    pub async fn cancel_ironwood_migration(&mut self) -> Result<(), LightClientError> {
        let mut wallet = self.wallet().write().await;
        if wallet.migration.take().is_none() {
            return Err(MigrationError::NoMigration.into());
        }
        wallet.save_required = true;
        Ok(())
    }

    /// Reconciles the persisted migration against the wallet's chain view
    /// and applies the actions that are safe unattended: promoting
    /// nullifier-mined parts to confirmed, marking expiries and
    /// invalidations, rebuilding expired parts against a fresh boundary,
    /// and marking completion. Actions needing consent, a user-facing
    /// disclosure (catch-up, replanning the remainder), or the network
    /// (driving note splitting via [`Self::continue_note_splitting`]) are
    /// returned untouched in the report.
    ///
    /// Pure over persisted state plus the wallet's local chain view: call it
    /// on every launch. It never synchronizes.
    pub async fn reconcile_migration(&mut self) -> Result<ReconcileReport, LightClientError> {
        let mut wallet = self.wallet().write().await;
        wallet
            .with_migration_state(|wallet, state| {
                let report = reconcile(state, &*wallet);
                for action in &report.actions {
                    match action {
                        RecommendedAction::PromoteConfirmed { part, height } => {
                            state.parts[part.0 as usize].mark_confirmed(*height)?;
                        }
                        RecommendedAction::MarkInvalidated { part } => {
                            state.parts[part.0 as usize].mark_invalidated()?;
                        }
                        RecommendedAction::Rebuild { part } => {
                            let part = &mut state.parts[part.0 as usize];
                            if part.state != PartState::Expired {
                                part.mark_expired()?;
                            }
                            let now_height = wallet
                                .sync_state
                                .last_known_chain_height()
                                .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
                            let activation = wallet.ironwood_activation()?;
                            let floor = schedule::AnchorFloor::new(
                                activation,
                                wallet.bound_note_confirmed_at(part),
                            );
                            schedule::place(
                                part,
                                schedule::first_permitted_bucket(
                                    now_height,
                                    &floor,
                                    &state.params.grid(wallet.chain_type()),
                                ),
                                &floor,
                                &mut crate::utils::system_rng(),
                                &state.params.grid(wallet.chain_type()),
                            )?;
                        }
                        RecommendedAction::MarkComplete { residual } => {
                            state.phase = MigrationPhase::Complete {
                                residual: *residual,
                            };
                        }
                        // Left to the caller: user-facing disclosure or fresh
                        // consent required, a network-touching step
                        // (`continue_note_splitting`), or nothing to apply.
                        RecommendedAction::PromptCatchUp { .. }
                        | RecommendedAction::ReplanRemainder
                        | RecommendedAction::RetrySplit { .. }
                        | RecommendedAction::AwaitSplitConfirmation
                        | RecommendedAction::ContinueNoteSplitting => (),
                    }
                }
                wallet.save_required = true;
                Ok::<_, LightClientError>(report)
            })
            .ok_or(MigrationError::NoMigration)?
    }

    /// Sends overdue parts now, in sequence with `spacing` between
    /// transmits (never simultaneously), after the caller has shown the
    /// ZIP 318 disclosure that sending at application-open time correlates
    /// the transmissions with the user's activity.
    ///
    /// Each overdue part is shifted into the current bucket (its old anchor
    /// is stale) before materializing and transmitting.
    pub async fn catch_up_migration(
        &mut self,
        spacing: Duration,
    ) -> Result<Vec<TxId>, LightClientError> {
        let overdue = self.fold_in_overdue_parts().await?;
        if overdue.is_empty() {
            return Ok(Vec::new());
        }
        self.wallet().write().await.refresh_part_witnesses()?;

        let client = self.migration_transmission_client()?;
        let mut sent = Vec::new();
        for part_id in overdue {
            let txids = self
                .transmit_due_parts_selected(&client, Some(part_id))
                .await?
                .into_sent();
            if !txids.is_empty() {
                sent.extend(txids);
                tokio::time::sleep(spacing).await;
            }
        }
        Ok(sent)
    }

    /// Reconciles, then shifts every part reconciliation reports as overdue
    /// into the current bucket, ready to send now. Returns the shifted ids,
    /// empty when nothing was missed. Shared by catch-up and the
    /// user-triggered execute batch.
    async fn fold_in_overdue_parts(&mut self) -> Result<Vec<PartId>, LightClientError> {
        let overdue: Vec<PartId> = self
            .reconcile_migration()
            .await?
            .actions
            .iter()
            .find_map(|action| match action {
                RecommendedAction::PromptCatchUp { parts, .. } => Some(parts.clone()),
                _ => None,
            })
            .unwrap_or_default();
        if overdue.is_empty() {
            return Ok(overdue);
        }

        let mut wallet = self.wallet().write().await;
        wallet
            .with_migration_state(|wallet, state| {
                wallet.save_required = true;
                let now_height = wallet
                    .sync_state
                    .last_known_chain_height()
                    .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
                let current_bucket = state.params.grid(wallet.chain_type()).index_of(now_height);
                let activation = wallet.ironwood_activation()?;
                for part_id in &overdue {
                    let part = &mut state.parts[part_id.0 as usize];
                    if part.state == PartState::Assigned {
                        // Catch-up fires now by disclosed intent:
                        // explicitly immediate placement. Overdue
                        // signed parts never reach here. Reconcile
                        // classifies them AwaitingExpiry, outside the
                        // catch-up batch.
                        //
                        // The anchor is still drawn at age one or more: an
                        // overdue part's note has been settled for at least
                        // the window it missed, so the current bucket always
                        // has legal anchors below it.
                        let floor = schedule::AnchorFloor::new(
                            activation,
                            wallet.bound_note_confirmed_at(part),
                        );
                        schedule::place_immediate(
                            part,
                            current_bucket,
                            &floor,
                            &mut crate::utils::system_rng(),
                            &state.params.grid(wallet.chain_type()),
                        )?;
                    }
                }
                Ok::<_, crate::wallet::error::WalletError>(())
            })
            .ok_or(MigrationError::NoMigration)??;
        drop(wallet);
        Ok(overdue)
    }

    /// Sends everything the migration owes right now, in one user-triggered
    /// batch: the current window's due parts plus any missed windows' parts,
    /// folded in. Sends are sequenced `spacing` apart, never simultaneous.
    /// The report carries a per-part outcome, and
    /// [`Self::batch_progress_handle`] observes the batch live from another
    /// thread while this call holds `&mut self`.
    ///
    /// This is the manual-execution entry point for a client whose user
    /// triggers each window from a wake-up notification: sync first, then
    /// one call sends the whole batch. Every part of the open window is sent.
    /// The random target height is advisory (the reminder hint), not a gate.
    /// Parts whose window boundary is no longer witnessable report
    /// [`PartSendResult::Slid`] and fall to reconciliation for a coming
    /// window.
    ///
    /// Disclosure (ZIP 318): user-present sends correlate the transmissions
    /// with the user's activity. Under a manual-execution flow every send
    /// has this property, on time or late, so the client shows the
    /// disclosure once, when the cadence is chosen.
    pub async fn execute_due_parts(
        &mut self,
        spacing: Duration,
    ) -> Result<BatchReport, LightClientError> {
        let client = self.migration_transmission_client()?;
        self.execute_due_parts_with(&client, spacing).await
    }

    /// [`Self::execute_due_parts`] with an injectable client, for tests.
    pub(crate) async fn execute_due_parts_with(
        &mut self,
        client: &impl TransmissionClient,
        spacing: Duration,
    ) -> Result<BatchReport, LightClientError> {
        self.fold_in_overdue_parts().await?;
        self.wallet().write().await.refresh_part_witnesses()?;

        // The owed set: every part of the current window, shifted or not.
        // The window being open is the whole due condition now. A part's
        // random target no longer gates its send.
        let owed: Vec<(PartId, u64)> = {
            let wallet = self.wallet().read().await;
            let state = wallet
                .migration
                .as_ref()
                .ok_or(MigrationError::NoMigration)?;
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .ok_or(WalletError::NoSyncData)?;
            let current_bucket = state.params.grid(wallet.chain_type()).index_of(now_height);
            state
                .parts
                .iter()
                .filter(|part| schedule::part_in_current_bucket(part, current_bucket))
                .map(|part| (part.id, part.denomination))
                .collect()
        };

        self.batch_progress.begin(owed.len() as u32);
        let _scope = BatchProgressScope(self.batch_progress.clone());

        let mut report = BatchReport::default();
        let mut sent = 0u32;
        for (index, (part, denomination)) in owed.iter().enumerate() {
            match self.transmit_due_parts_selected(client, Some(*part)).await {
                // The part was built and signed (it is already in the wallet,
                // pending) but the endpoint did not take it. It stays signed
                // and due, so a retry resubmits it. Reporting it as `Slid`
                // would tell the caller it was not sendable and nothing was
                // attempted, when the transaction exists and never left.
                Ok(Transmission::Halted {
                    part: failed_part,
                    error,
                    ..
                }) => {
                    let error = error.to_string();
                    report.outcomes.push(PartOutcome {
                        part: failed_part,
                        denomination: *denomination,
                        result: PartSendResult::Failed {
                            error: error.clone(),
                        },
                    });
                    report.halted = Some(error);
                    break;
                }
                Ok(Transmission::Complete(txids)) if !txids.is_empty() => {
                    sent += 1;
                    report.outcomes.push(PartOutcome {
                        part: *part,
                        denomination: *denomination,
                        result: PartSendResult::Sent(txids[0]),
                    });
                    self.batch_progress
                        .resolve(report.outcomes.len() as u32, sent);
                    if index + 1 < owed.len() {
                        self.batch_progress.set_phase(BatchPhase::Spacing);
                        tokio::time::sleep(spacing).await;
                        self.batch_progress.set_phase(BatchPhase::Sending);
                    }
                }
                Ok(_) => {
                    report.outcomes.push(PartOutcome {
                        part: *part,
                        denomination: *denomination,
                        result: PartSendResult::Slid,
                    });
                    self.batch_progress
                        .resolve(report.outcomes.len() as u32, sent);
                }
                Err(e) => {
                    let error = render_cause_chain(&e);
                    report.outcomes.push(PartOutcome {
                        part: *part,
                        denomination: *denomination,
                        result: PartSendResult::Failed {
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

    /// Captures any still-missing migration boundary witnesses from the
    /// wallet's current tree state, while the boundary checkpoint is still
    /// retained. [`Self::await_sync`] does this when a Sync Session returns
    /// successfully. A consumer collecting the session's result through
    /// [`Self::poll_sync`] calls it then instead.
    ///
    /// A continuous Sync Session returns only once it is stopped, so a
    /// consumer running one calls this each time
    /// [`pepper_sync::sync::SyncStatus::is_complete`] returns true for a new
    /// sync status. A status is published after every scan, so each newly
    /// mined block is followed by one.
    pub async fn capture_migration_witnesses(&mut self) -> Result<(), LightClientError> {
        Ok(self.wallet().write().await.refresh_part_witnesses()?)
    }

    /// Transmits any parts whose bucket window and random target height are
    /// both reached, without synchronizing. Call this each time the wallet is
    /// synced to the chain tip to drive the scheduled migration
    /// automatically: after a Sync Session returns, or under continuous sync
    /// each time [`pepper_sync::sync::SyncStatus::is_complete`] returns true
    /// for a new sync status.
    ///
    /// No-op when no migration is active or no parts are due.
    pub async fn auto_transmit_if_due(
        &mut self,
    ) -> Result<Vec<zcash_primitives::transaction::TxId>, LightClientError> {
        {
            let wallet = self.wallet().read().await;
            if wallet.migration.is_none() {
                return Ok(Vec::new());
            }
        }
        self.wallet().write().await.refresh_part_witnesses()?;
        let client = self.migration_transmission_client()?;
        self.transmit_due_parts_with(&client).await
    }

    /// The migration's progress, everything a progress UI renders. Includes
    /// the Orchard-pool-specific confirmed-spendable figure, which ZIP 318
    /// requires displaying instead of a unified total.
    pub async fn migration_status(&self) -> Result<MigrationStatus, LightClientError> {
        let wallet = self.wallet().read().await;
        let (
            phase,
            parts_total,
            parts_confirmed,
            value_total,
            value_migrated,
            windows,
            due_now,
            account,
        ) = match &wallet.migration {
            Some(state) => {
                let confirmed: Vec<_> = state
                    .parts
                    .iter()
                    .filter(|part| matches!(part.state, PartState::Confirmed { .. }))
                    .collect();
                let now_height = wallet.sync_state.last_known_chain_height();
                let windows = now_height.map_or_else(Vec::new, |height| {
                    crate::wallet::migration::upcoming_windows(
                        &state.parts,
                        height,
                        u64::from(crate::utils::now()),
                        WAKE_HORIZON_BUCKETS,
                        &state.params.grid(wallet.chain_type()),
                    )
                });
                // The batch a tap would transmit now. `execute_due_parts`
                // folds overdue parts via a reconcile pass, so the read-only
                // status predicts what it sends from the same reconcile.
                // Only meaningful once parts are scheduled.
                let due_now = match (now_height, &state.phase) {
                    (Some(height), MigrationPhase::PartsScheduled) => {
                        let report = reconcile(state, &*wallet);
                        let due_ids = due_now_parts(
                            &state.parts,
                            &report,
                            height,
                            &state.params.grid(wallet.chain_type()),
                        );
                        (!due_ids.is_empty()).then(|| {
                            let current_bucket =
                                state.params.grid(wallet.chain_type()).index_of(height);
                            let due: Vec<_> = state
                                .parts
                                .iter()
                                .filter(|part| due_ids.contains(&part.id))
                                .collect();
                            DueBatch {
                                boundary: state
                                    .params
                                    .grid(wallet.chain_type())
                                    .boundary_of(current_bucket),
                                part_ids: due.iter().map(|part| part.id).collect(),
                                denominations: due.iter().map(|part| part.denomination).collect(),
                            }
                        })
                    }
                    _ => None,
                };
                // What this migration moved: the confirmed part denominations,
                // and nothing else. The account's whole Ironwood balance also
                // holds shields and ordinary receives, which are not migration
                // progress (issue #2493, finding 10).
                let value_migrated = confirmed.iter().map(|part| part.denomination).sum();
                // Phase 1 has no part records yet, so the totals project the
                // plan over every live V2 note. A round in flight counts as
                // its pending outputs. The progress denominator exists from
                // consent onward instead of appearing when parts bind.
                let (parts_total, value_total) = match &state.phase {
                    MigrationPhase::Planned | MigrationPhase::NoteSplitting { .. } => {
                        let plan = crate::wallet::migration::plan_migration(
                            &wallet.live_v2_note_values(state.account),
                            wallet.splits_confirm_post_activation(),
                            &state.params,
                        );
                        (plan.parts.len() as u32, plan.parts.iter().sum())
                    }
                    _ => (
                        state.parts.len() as u32,
                        state.parts.iter().map(|part| part.denomination).sum(),
                    ),
                };
                (
                    Some(state.phase.clone()),
                    parts_total,
                    confirmed.len() as u32,
                    value_total,
                    value_migrated,
                    windows,
                    due_now,
                    state.account,
                )
            }
            None => (None, 0, 0, 0, 0, Vec::new(), None, zip32::AccountId::ZERO),
        };

        Ok(MigrationStatus {
            orchard_confirmed_spendable: ChainView::orchard_confirmed_spendable(&*wallet, account),
            phase,
            parts_total,
            parts_confirmed,
            value_total,
            value_migrated,
            upcoming_windows: windows,
            due_now,
        })
    }

    /// The window timeline around the chain tip: always the window the tip
    /// is inside, plus one entry per scheduled window (past and future
    /// alike) when a migration is in progress. With no migration the
    /// current window reports zero tallies against the provisional
    /// parameters, so a client can render the ZIP 318 calendar before the
    /// user has consented to anything. `None` only when the wallet has no
    /// chain height yet.
    pub async fn window_timeline(&self) -> Result<Option<Vec<WindowReport>>, LightClientError> {
        let wallet = self.wallet().read().await;
        let Some(now_height) = wallet.sync_state.last_known_chain_height() else {
            return Ok(None);
        };
        Ok(Some(match &wallet.migration {
            Some(state) => crate::wallet::migration::window_timeline(
                &state.parts,
                now_height,
                &state.params.grid(wallet.chain_type()),
            ),
            None => crate::wallet::migration::window_timeline(
                &[],
                now_height,
                &MigrationParams::provisional(wallet.chain_type()).grid(wallet.chain_type()),
            ),
        }))
    }

    /// Plans an immediate migration of the account's Orchard pool into Ironwood.
    ///
    /// Pure and deterministic, nothing is signed or sent, so the plan
    /// can be shown to the user for consent before [`Self::migrate_immediately`] executes it.
    pub async fn plan_immediate_migration(
        &self,
        account: zip32::AccountId,
    ) -> Result<ImmediateMigrationPlan, LightClientError> {
        let wallet = self.wallet().read().await;
        Ok(wallet.plan_immediate_migration(account)?)
    }

    /// Spends every spendable Orchard note in `account` into the Ironwood pool,
    /// in one round of independent transactions.
    ///
    /// This is the *migrate immediately* path ZIP 318 offers alongside the
    /// private one. All the transfers are transmitted at once, so they correlate with each other and
    /// with the user's activity. Every one of those identifies the wallet on-chain.
    /// **The caller must disclose this.** For the private path, use
    /// [`Self::migrate_to_ironwood`].
    ///
    /// Notes worth at most [`MigrationParams::sweep_min`] are left behind.
    /// Spending one costs more than it carries, and their total is reported as
    /// [`ImmediateMigrationSummary::residual`].
    ///
    /// This function is idempotent over wallet state: a call that fails partway leaves the notes
    /// of every unsent transaction spendable. Calling it again re-plans and
    /// sends the remainder.
    ///
    /// Syncs the wallet to the chain tip before migrating, via
    /// [`Self::sync_to_tip_and_await`], whatever the wallet's
    /// `shutdown_on_completion` setting. A running sync is stopped first and
    /// is not relaunched. Consumers that own the sync lifecycle and keep sync
    /// running should call
    /// [`Self::quick_immediate_migration`] instead, which migrates
    /// against current wallet state without launching its own sync.
    pub async fn migrate_immediately(
        &mut self,
        account: zip32::AccountId,
    ) -> Result<ImmediateMigrationSummary, LightClientError> {
        // A scheduled migration rejects the immediate migration regardless of chain state,
        // so check before paying for a sync. The presynced body re-checks
        // after the sync lands.
        if self.wallet().read().await.migration.is_some() {
            return Err(MigrationError::AlreadyInProgress.into());
        }

        self.sync_to_tip_and_await().await?;
        let sync = self.pause_sync_scoped()?;
        self.migrate_immediately_presynced(account, &sync).await
    }

    /// Transmits the immediate Orchard→Ironwood migration against the wallet's
    /// *current* state, without syncing first.
    ///
    /// This is [`Self::migrate_immediately`] minus the leading
    /// `sync_to_tip_and_await`, for consumers that own the sync lifecycle and keep
    /// sync running continuously (e.g. zingo-mobile). The syncing
    /// variant stops such a consumer's running sync and leaves it stopped.
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
        // A scheduled migration soft-reserves the notes its parts are bound to.
        // Migrating them immediately would invalidate those parts behind its back.
        if self.wallet().read().await.migration.is_some() {
            return Err(MigrationError::AlreadyInProgress.into());
        }

        let plan = self.plan_immediate_migration(account).await?;
        if plan.is_empty() {
            return Err(crate::wallet::error::WalletError::NothingToMigrate.into());
        }

        // Arm per-transaction progress for the poll side channel. The scope
        // guard owns an `Arc` clone (not a borrow of `self`), so it survives the
        // `&mut self` `build_and_transmit` call and clears the snapshot on every
        // exit: success, `?`-propagated error, or panic.
        self.immediate_migration_progress
            .begin(plan.transactions.len() as u32);
        let _scope = ImmediateMigrationProgressScope(self.immediate_migration_progress.clone());
        let progress = self.immediate_migration_progress.clone();

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

    /// The immediate Orchard→Ironwood migration as a single send-shaped call, the
    /// mobile-facing counterpart to [`Self::quick_send`].
    ///
    /// Pauses sync internally (like [`Self::quick_send`] and
    /// [`Self::quick_shield`], and a no-op when no engine is running), migrates
    /// the account's spendable Orchard notes into Ironwood against the wallet's
    /// *current* state without synchronizing, and restores the prior sync mode
    /// on return unless `resume_sync` is `false`, in which case the pause is
    /// left for the caller (the shipped `resume_sync` protocol of the send
    /// paths).
    ///
    /// This is the send-family entry point for the immediate migration, and
    /// the only immediate-migration entry point that crosses the UniFFI boundary:
    /// [`Self::migrate_immediately`] self-syncs and so stops a consumer's
    /// continuous sync, and the internal
    /// `migrate_immediately_presynced` takes a
    /// [`SyncPauseGuard`] that cannot cross FFI. The caller keeps the wallet
    /// synced, exactly as it must before any send.
    ///
    /// Preview the plan first with [`Self::plan_immediate_migration`] (its
    /// transaction count, fee, and residual value), and observe live progress
    /// through [`Self::immediate_migration_progress_handle`]. Like every immediate path it
    /// puts the wallet's real amounts on-chain, correlated with each other and
    /// the caller's activity. The caller must disclose this (ZIP 318).
    pub async fn quick_immediate_migration(
        &mut self,
        account: zip32::AccountId,
        resume_sync: bool,
    ) -> Result<ImmediateMigrationSummary, LightClientError> {
        // Establish the stable-state pause ourselves (quick_send's idiom)
        // rather than demanding it as a `SyncPauseGuard` parameter the FFI
        // boundary cannot express. The guard owns an `Arc` clone of the
        // sync-mode handle, not a borrow of `self`, so it lives across the
        // `&mut self` migration call, and `?` refuses rather than migrating under a
        // state the pause could not stabilize.
        let guard = self.pause_sync_scoped()?;
        let result = self.migrate_immediately_presynced(account, &guard).await;
        if !resume_sync {
            guard.disarm();
        }
        result
    }

    /// Previews Phase 1 note splitting from the wallet's current confirmed
    /// notes: the [`MigrationPlan`] whose `split_rounds` are the Orchard
    /// self-sends that will run, alongside the resulting part denominations,
    /// the fees, and any residual dust. Pure and deterministic (nothing is
    /// signed or sent), so a client can show it before calling
    /// [`Self::quick_split`]. `plan.is_split()` (empty `split_rounds`) means
    /// nothing needs splitting.
    ///
    /// It is the same projection as [`Self::plan_ironwood_migration`], named
    /// for the Phase 1 mental model of the fused, stateless splitting flow.
    pub async fn plan_note_split(
        &self,
        account: zip32::AccountId,
    ) -> Result<MigrationPlan, LightClientError> {
        self.plan_ironwood_migration(account).await
    }

    /// Executes one round of Phase 1 note splitting as a send-shaped call,
    /// the mobile-facing entry point for the *private* migration path's
    /// splitting, the counterpart to [`Self::quick_immediate_migration`] for the immediate
    /// path.
    ///
    /// Like [`Self::quick_send`] it pauses sync internally, plans against the
    /// wallet's *current* confirmed notes without synchronizing, and restores
    /// the prior sync mode on return unless `resume_sync` is `false`. It
    /// persists no migration state: each call re-plans, and "a round is still
    /// in flight" is derived from the wallet's pending transactions rather than
    /// a stored phase.
    ///
    /// **One call does one round.** Loop it: after [`SplitOutcome::Round`],
    /// sync until its `txids` confirm, then call again. Stop at
    /// [`SplitOutcome::Complete`]. [`SplitOutcome::AwaitingConfirmation`] means
    /// a previously transmitted round has not confirmed yet, so sync and retry.
    /// Preview with [`Self::plan_note_split`]. Observe per-transaction progress
    /// through [`Self::split_progress_handle`].
    ///
    /// Refuses with [`MigrationError::AlreadyInProgress`] while a *scheduled*
    /// migration is active: that flow drives its own splitting and reserves
    /// notes for its parts, which the fused path must not race.
    pub async fn quick_split(
        &mut self,
        account: zip32::AccountId,
        resume_sync: bool,
    ) -> Result<SplitOutcome, LightClientError> {
        let guard = self.pause_sync_scoped()?;
        let result = self.split_next_round(account, &guard).await;
        if !resume_sync {
            guard.disarm();
        }
        result
    }

    /// One round of [`Self::quick_split`] under a caller-held pause: plan,
    /// classify, and, when a round is due, build and transmit it.
    async fn split_next_round(
        &mut self,
        account: zip32::AccountId,
        sync: &SyncPauseGuard,
    ) -> Result<SplitOutcome, LightClientError> {
        if self.wallet().read().await.migration.is_some() {
            return Err(MigrationError::AlreadyInProgress.into());
        }

        // A round already transmitted must confirm before the next is planned:
        // its self-outputs are unconfirmed and its spent inputs are not yet
        // marked spent (a not-yet-mined self-send carries no spend marks), so
        // a replan would re-select those inputs and re-transmit the round.
        // Check this first, from the wallet's pending transactions, and defer.
        if self.wallet().read().await.note_split_in_flight(account) {
            return Ok(SplitOutcome::AwaitingConfirmation);
        }

        // Mining ends the pending state some blocks before the anchor reaches
        // the round's outputs. In that gap the planner sees neither the spent
        // inputs nor the new outputs, so it plans nothing and the split reads
        // finished. Defer until the outputs are selectable, the stateless form
        // of `continue_note_splitting`'s `unanchored` check.
        if self.wallet().read().await.unanchored_v2_outputs(account)? {
            return Ok(SplitOutcome::AwaitingConfirmation);
        }

        let plan = self.plan_ironwood_migration(account).await?;

        // Nothing pending and nothing to split: every note is part-ready.
        if plan.is_split() {
            return Ok(SplitOutcome::Complete);
        }

        let round = plan
            .split_rounds
            .into_iter()
            .next()
            .expect("unsplit plan has at least one round");

        // Arm the per-transaction side channel for the poll handle. The scope
        // guard owns an `Arc` clone, so it survives the `&mut self`
        // `build_and_transmit` call and clears the snapshot on every exit.
        self.split_progress.begin(round.len() as u32);
        let _scope = SplitProgressScope(self.split_progress.clone());
        let progress = self.split_progress.clone();

        let txids = self
            .build_and_transmit(&round, sync, &progress, |wallet, planned| {
                wallet.build_note_split_transaction(account, planned)
            })
            .await?;

        Ok(SplitOutcome::Round { txids })
    }

    /// Builds and transmits one batch of planned migration transactions under
    /// a caller-held [`SyncPauseGuard`], enforcing the shared cleanup
    /// contract: a build failure fails the transactions already built, and a
    /// transmit failure fails every transaction still unsent, so no note
    /// stays spent by a transaction that will never reach the network. Both
    /// the immediate migration flow and the note-splitting rounds send through here. The
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
        // unless the caller (an immediate migration or a note-splitting round)
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
                        zl_common::status::ConfirmationStatus::Calculated(_)
                    )
                })
            })
            .collect();
        if !unsent.is_empty() {
            pepper_sync::set_transactions_failed(&mut wallet.wallet_transactions, unsent);
            wallet.save_required = true;
        }
    }

    /// Runs a full Orchard→Ironwood migration in one call: executes
    /// note-splitting rounds (waiting for each round to confirm), then
    /// materializes and transmits every part immediately through the
    /// [`TransmissionClient`].
    ///
    /// This is the interactive path (CLI, testing, or a user who chose
    /// immediate migration over the scheduled flow): sends coincide with
    /// synchronization and with each other, which the caller must disclose.
    /// Replans from wallet state before every round, so a migration
    /// interrupted by external spends, expiry or restart picks up where the
    /// notes actually are.
    ///
    /// Syncs to the chain tip before each round and while awaiting
    /// confirmations, via [`Self::sync_to_tip_and_await`], so it returns even
    /// when the wallet is configured for continuous sync
    /// (`shutdown_on_completion == false`). A running sync is
    /// stopped first and is not relaunched; the caller relaunches it with
    /// [`Self::sync`] when this returns.
    pub async fn migrate_to_ironwood(
        &mut self,
        account: zip32::AccountId,
    ) -> Result<MigrationSummary, LightClientError> {
        {
            let mut wallet = self.wallet().write().await;
            // Plan before gating: a failed precondition must not erase the
            // completed-migration history the gate would otherwise clear.
            let _ = wallet.plan_ironwood_migration_now(account)?;
            immediate_migration_entry_gate(&mut wallet, account)?;
        }

        let mut split_txids = Vec::new();
        let mut part_txids = Vec::new();

        for _ in 0..MAX_ROUNDS {
            self.sync_to_tip_and_await().await?;
            // Plan and (when the plan is split) bind under one write guard,
            // the same single-borrow bracket as `start_ironwood_migration`:
            // the notes hashed into the recorded consent are the notes
            // bound (issue #2493, finding 11).
            let plan = {
                let mut wallet = self.wallet().write().await;
                let plan = wallet.plan_ironwood_migration_now(account)?;
                if plan.is_split() {
                    // The entry gate ran once; the per-round resume
                    // re-verifies the state it is about to drive, so the
                    // consent guarantee lives in the state machine rather
                    // than in receiver discipline at the API surface. A
                    // future scheduled-flow split driver or a second
                    // client handle must not reopen the consent collapse
                    // through this path.
                    if let Some(state) = &wallet.migration {
                        if state.mode != MigrationMode::Immediate {
                            return Err(MigrationError::ScheduledMigrationExists.into());
                        }
                        if state.account != account {
                            return Err(MigrationError::DifferentAccount.into());
                        }
                    }
                    // An immediate part transmits in the current bucket and
                    // anchors in a lower one. Until the current bucket sits
                    // two buckets above the activation's, no legal anchor
                    // exists and every transmission pass would skip every part,
                    // previously a MAX_ROUNDS spin ending in a misleading
                    // SplitDidNotConverge.
                    let grid = wallet
                        .migration
                        .as_ref()
                        .map_or_else(
                            || MigrationParams::provisional(wallet.chain_type()),
                            |state| state.params.clone(),
                        )
                        .grid(wallet.chain_type());
                    let activation = wallet.ironwood_activation()?;
                    let now_height = wallet
                        .sync_state
                        .last_known_chain_height()
                        .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
                    let first_window =
                        schedule::first_ironwood_era_window_boundary(activation, &grid);
                    if now_height < first_window {
                        return Err(MigrationError::IronwoodEraTooYoung {
                            retry_after: first_window,
                        }
                        .into());
                    }
                    // Invoking the one-call constitutes consent to the
                    // current plan. Record the binding if this is a fresh
                    // migration.
                    if wallet.migration.is_none() {
                        let params = MigrationParams::provisional(wallet.chain_type());
                        wallet.migration = Some(MigrationState {
                            consent: ConsentBinding {
                                params_hash: params.params_hash(),
                                plan_hash: plan_hash(&plan),
                                consented_at: u64::from(crate::utils::now()),
                            },
                            params,
                            strategy: SigningStrategy::LazyAtBoundary,
                            mode: MigrationMode::Immediate,
                            account,
                            phase: MigrationPhase::Planned,
                            parts: Vec::new(),
                        });
                    }
                    let mut state = wallet
                        .migration
                        .take()
                        .expect("migration state exists here");
                    let bind = (|| -> Result<(), crate::wallet::error::WalletError> {
                        if state.parts.is_empty() {
                            wallet.bind_parts_to_notes(&mut state, account)?;
                        }
                        let now_height = wallet
                            .sync_state
                            .last_known_chain_height()
                            .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
                        let current_bucket =
                            state.params.grid(wallet.chain_type()).index_of(now_height);
                        // Immediate mode: everything sends now, in the
                        // current bucket, explicitly immediate. The anchor is
                        // still drawn at age one or more; immediacy is about
                        // the send time, not the anchor (ADR 0018).
                        for index in 0..state.parts.len() {
                            match state.parts[index].state {
                                PartState::Bound | PartState::Assigned => {
                                    let floor = schedule::AnchorFloor::new(
                                        activation,
                                        wallet.bound_note_confirmed_at(&state.parts[index]),
                                    );
                                    schedule::place_immediate(
                                        &mut state.parts[index],
                                        current_bucket,
                                        &floor,
                                        &mut crate::utils::system_rng(),
                                        &state.params.grid(wallet.chain_type()),
                                    )?;
                                }
                                _ => (),
                            }
                        }
                        state.phase = MigrationPhase::PartsScheduled;
                        Ok(())
                    })();
                    wallet.migration = Some(state);
                    wallet.save_required = true;
                    bind?;
                    wallet.refresh_part_witnesses()?;
                }
                plan
            };

            if plan.is_split() {
                let residual = plan.residual;
                let client = self.migration_transmission_client()?;
                let sent = self.transmit_due_parts_with(&client).await?;
                self.await_migration_confirmations(&sent).await?;
                part_txids.extend(sent);
                let _ = self.reconcile_migration().await?;

                let pending = {
                    let wallet = self.wallet().read().await;
                    wallet
                        .migration
                        .as_ref()
                        .map(|state| {
                            state.parts.iter().any(|part| {
                                !matches!(
                                    part.state,
                                    PartState::Confirmed { .. } | PartState::Invalidated
                                )
                            })
                        })
                        .unwrap_or(false)
                };
                if !pending {
                    let _ = self.reconcile_migration().await?;
                    return Ok(MigrationSummary {
                        split_txids,
                        part_txids,
                        residual,
                    });
                }
                // Some parts skipped (their boundary's tree state was not
                // capturable): sync forward and retry next round.
                continue;
            }

            let round = plan
                .split_rounds
                .into_iter()
                .next()
                .expect("unsplit plan has at least one round");
            let sync = self.pause_sync_scoped()?;
            let round_txids = self
                .build_and_transmit(&round, &sync, &(), |wallet, planned| {
                    wallet.build_note_split_transaction(account, planned)
                })
                .await?;
            // The confirmation wait syncs; release the pause first.
            drop(sync);
            split_txids.extend(round_txids.iter().copied());

            self.await_migration_confirmations(&round_txids).await?;
        }

        Err(MigrationError::SplitDidNotConverge(MAX_ROUNDS).into())
    }

    /// Syncs until every transaction in `txids` is confirmed, erroring if one
    /// fails or the wait times out.
    async fn await_migration_confirmations(
        &mut self,
        txids: &[TxId],
    ) -> Result<(), LightClientError> {
        for _ in 0..MAX_CONFIRMATION_POLLS {
            self.sync_to_tip_and_await().await?;
            let (all_confirmed, failed) = {
                let wallet = self.wallet().read().await;
                let statuses: Vec<_> = txids
                    .iter()
                    .map(|txid| {
                        wallet
                            .wallet_transactions
                            .get(txid)
                            .map(|transaction| transaction.status())
                    })
                    .collect();
                (
                    statuses
                        .iter()
                        .all(|status| status.is_some_and(|s| s.is_confirmed())),
                    statuses.iter().enumerate().find_map(|(i, status)| {
                        matches!(
                            status,
                            Some(zl_common::status::ConfirmationStatus::Failed(_)) | None
                        )
                        .then_some(txids[i])
                    }),
                )
            };
            if let Some(txid) = failed {
                return Err(MigrationError::SplitTransactionFailed(txid).into());
            }
            if all_confirmed {
                return Ok(());
            }
            tokio::time::sleep(CONFIRMATION_POLL_INTERVAL).await;
        }
        Err(MigrationError::SplitConfirmationTimeout.into())
    }
}

impl crate::wallet::LightWallet {
    /// Plans the migration from the wallet's current state: pure over the
    /// wallet, no lock management of its own. The read-only public planner
    /// calls it under a read guard. The consent brackets of
    /// `start_ironwood_migration` and the immediate path call it under the
    /// same write guard that binds, so the plan hashed and the notes bound
    /// come from one uninterrupted wallet view (issue #2493, finding 11).
    #[allow(clippy::result_large_err)]
    pub(crate) fn plan_ironwood_migration_now(
        &self,
        account: zip32::AccountId,
    ) -> Result<MigrationPlan, crate::wallet::error::WalletError> {
        let params = MigrationParams::provisional(self.chain_type());
        Ok(plan_migration(
            &self.migration_note_values(account)?,
            self.splits_confirm_post_activation(),
            &params,
        ))
    }

    /// Whether a transaction built now confirms at or after NU6.3
    /// activation. Note-splitting fees depend on it (the Orchard bundle's
    /// cross-address rules change the action count).
    pub(crate) fn splits_confirm_post_activation(&self) -> bool {
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

/// Decides what an existing migration state means for a new immediate run.
/// This is the entry gate of the one-call immediate path.
///
/// A consented scheduled migration must not be collapsed into an immediate
/// one (its bucket windows are what the user confirmed), and a different
/// account's migration must not be disturbed. Both refuse. A completed
/// migration is history: the slot clears so the rerun migrates newly
/// received funds instead of skipping binding against stale confirmed
/// parts. An interrupted immediate migration passes through and resumes.
fn immediate_migration_entry_gate(
    wallet: &mut crate::wallet::LightWallet,
    account: zip32::AccountId,
) -> Result<(), MigrationError> {
    match &wallet.migration {
        None => Ok(()),
        // Completed state clears before the account is compared: it is
        // terminal history for whichever account finished it, and must not
        // block another account's migration forever.
        Some(state) if matches!(state.phase, MigrationPhase::Complete { .. }) => {
            wallet.migration = None;
            wallet.save_required = true;
            Ok(())
        }
        Some(state) if state.account != account => Err(MigrationError::DifferentAccount),
        Some(state) if state.mode == MigrationMode::Scheduled => {
            Err(MigrationError::ScheduledMigrationExists)
        }
        Some(_) => Ok(()),
    }
}

/// The separator between two layers of a rendered cause chain, matching the
/// rendering `zl_netutils::diag` gives its own failure records.
pub(crate) const CAUSE_CHAIN_SEPARATOR: &str = ": ";

/// Renders every layer of a failure's cause chain into the one text a batch
/// report carries across serde.
fn render_cause_chain(error: &LightClientError) -> String {
    zl_netutils::diag::chain_texts(error).join(CAUSE_CHAIN_SEPARATOR)
}

/// Records one part submission's own route evidence in the cross-session
/// indexer history, so an audit reads the wire each part actually traveled
/// rather than inferring it from the session's policy afterwards. The
/// evidence never enters the wallet file: the history is its home, and the
/// wallet's persisted grammar is untouched.
fn record_part_route(
    history: &crate::lightclient::indexer_history::IndexerHistoryHandle,
    route: &crate::wallet::migration::TransmissionRoute,
    started: std::time::Instant,
    outcome: Result<(), crate::lightclient::indexer_history::FailureKind>,
) {
    use crate::lightclient::indexer_history::{
        AttemptKind, AttemptRoute, IndexerAttempt, now_unix_secs,
    };
    use crate::wallet::migration::TransmissionRoute;

    let (host, attempt_route) = match route {
        TransmissionRoute::Mixnet { destination, .. } => (
            crate::destination::Host::of_host_str(destination),
            AttemptRoute::Mixnet,
        ),
        TransmissionRoute::Nakednet { endpoint } => (
            crate::destination::Host::of_host_str(endpoint),
            AttemptRoute::Nakednet,
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

#[cfg(test)]
mod tests;
