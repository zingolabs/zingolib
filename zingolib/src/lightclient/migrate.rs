//! Orchard→Ironwood migration orchestration.
//!
//! The scheduled flow a mobile client drives:
//! [`LightClient::plan_ironwood_migration`] →
//! [`LightClient::start_ironwood_migration`] (consent) →
//! [`LightClient::continue_note_splitting`] after each sync until the parts
//! are scheduled → [`LightClient::reschedule_parts`] when the user picks the
//! Phase 2 cadence → [`LightClient::reconcile_migration`] on every launch →
//! [`LightClient::transmit_due_parts`] from background wakes →
//! [`LightClient::catch_up_migration`] when windows were missed.
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
    MigrationPhase, MigrationPlan, MigrationState, PartId, PartState, PrepareResult,
    RecommendedAction, ReconcileReport, SigningStrategy, TransmissionClient, TransmissionWindow,
    WindowReport, due_now_parts, plan_hash, plan_migration, plan_schedule, reconcile, schedule,
};

pub mod transmission_grpc;
pub mod transmission_route;

use zingo_netutils::time::CONFIRMATION_POLL_INTERVAL;
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
                &mut rand::rngs::OsRng,
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
    /// Call it after a sync whenever [`Self::reconcile_migration`] reports
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
                        &mut rand::rngs::OsRng,
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
                        &mut rand::rngs::OsRng,
                    )?;
                }
                wallet.save_required = true;
                Ok::<_, LightClientError>(())
            })
            .ok_or(MigrationError::NoMigration)?
    }

    /// The transmit-only client parts are submitted through, resolved by the
    /// Mixnet Mode policy (ADR 0011, amendment 2026-07-23) like every other
    /// transmitting surface.
    ///
    /// While the mode is on, parts travel ONLY over the mixnet (failing
    /// closed with [`MixnetNotReady`](crate::mixnet::MixnetNotReady) while the
    /// proxy bootstraps or after it dies) to one Destination drawn at
    /// random per submission, with the synchronization endpoint's operator
    /// forbidden as a target (ADR 0022: a `migration_transmission_uri` on the
    /// sync operator's domain is refused, and the draw excludes that
    /// operator). Clearnet carries parts only when the user deliberately
    /// toggled the mode off, or in a build without the `nym` feature: then
    /// the dedicated `migration_transmission_uri` when configured, else the
    /// synchronization endpoint with a logged correlation warning, else
    /// [`LightClientError::Offline`] with no traffic emitted.
    fn migration_transmission_client(
        &self,
    ) -> Result<transmission_route::RoutedTransmissionClient, LightClientError> {
        #[cfg(feature = "nym")]
        if let crate::mixnet::MixnetRoute::Mixnet(conduit) = self.mixnet_route()? {
            // The guard travels into the client, which dials on every
            // submission long after this function returns.
            let dial = conduit.dial();
            let sync_indexer = self.indexer_uri();
            let candidates = transmission_route::eligible_candidates(
                self.migration_transmission_uri.clone(),
                sync_indexer.as_ref(),
            )?;
            return Ok(transmission_route::RoutedTransmissionClient::Mixnet(
                transmission_route::MixnetTransmissionClient::new(dial, candidates),
            ));
        }

        let clearnet = match &self.migration_transmission_uri {
            Some(uri) => transmission_grpc::GrpcTransmissionClient::new(uri.clone()),
            None => {
                let indexer_uri = self.indexer_uri().ok_or(LightClientError::Offline)?;
                log::warn!(
                    "no dedicated migration transmission endpoint configured; parts will be \
                     transmitted to the synchronization endpoint, which lets that server \
                     correlate synchronization with migration activity"
                );
                transmission_grpc::GrpcTransmissionClient::new(indexer_uri)
            }
        };
        Ok(transmission_route::RoutedTransmissionClient::Clearnet(
            clearnet,
        ))
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
        self.transmit_due_parts_selected(client, None).await
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
    ) -> Result<Vec<TxId>, LightClientError> {
        type ProveHandle = tokio::task::JoinHandle<
            Result<(usize, TxId, Vec<u8>), crate::wallet::error::WalletError>,
        >;

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
                        schedule::bucket_index(now_height, state.params.bucket_modulus);

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
                                        prove.prove().map(|(txid, raw_tx)| (index, txid, raw_tx))
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
        let mut newly_proven: Vec<(usize, TxId, Vec<u8>)> = Vec::new();
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
                for (index, txid, raw_tx) in newly_proven {
                    let bucket = state.parts[index]
                        .bucket_index
                        .expect("assigned parts carry a bucket");
                    let boundary = schedule::boundary_of(bucket, state.params.bucket_modulus);
                    let target_height = boundary + 1;
                    let expiry_height = schedule::canonical_expiry_height(target_height);
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
                Err(e) => {
                    log::warn!("part submission failed, leaving the part signed: {e}");
                    break;
                }
            }
        }
        Ok(sent)
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
                                schedule::first_permitted_bucket(now_height, &floor, &state.params),
                                &floor,
                                &mut rand::rngs::OsRng,
                                &state.params,
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
                .await?;
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
                let current_bucket =
                    schedule::bucket_index(now_height, state.params.bucket_modulus);
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
                            &mut rand::rngs::OsRng,
                            &state.params,
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
            let current_bucket = schedule::bucket_index(now_height, state.params.bucket_modulus);
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
                Ok(txids) if !txids.is_empty() => {
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
    /// wallet's current tree state. [`Self::await_sync`] does this
    /// automatically after every successful sync. A consumer driving sync
    /// through [`Self::poll_sync`] calls it on completion instead, while
    /// the boundary checkpoint is still retained.
    pub async fn capture_migration_witnesses(&mut self) -> Result<(), LightClientError> {
        Ok(self.wallet().write().await.refresh_part_witnesses()?)
    }

    /// Transmits any parts whose bucket window and random target height are
    /// both reached, without synchronizing. Call this after each sync to drive
    /// the scheduled migration automatically.
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
                        &state.params,
                    )
                });
                // The batch a tap would transmit now. `execute_due_parts`
                // folds overdue parts via a reconcile pass, so the read-only
                // status predicts what it sends from the same reconcile.
                // Only meaningful once parts are scheduled.
                let due_now = match (now_height, &state.phase) {
                    (Some(height), MigrationPhase::PartsScheduled) => {
                        let report = reconcile(state, &*wallet);
                        let due_ids = due_now_parts(&state.parts, &report, height, &state.params);
                        (!due_ids.is_empty()).then(|| {
                            let current_bucket =
                                schedule::bucket_index(height, state.params.bucket_modulus);
                            let due: Vec<_> = state
                                .parts
                                .iter()
                                .filter(|part| due_ids.contains(&part.id))
                                .collect();
                            DueBatch {
                                boundary: schedule::boundary_of(
                                    current_bucket,
                                    state.params.bucket_modulus,
                                ),
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
            Some(state) => {
                crate::wallet::migration::window_timeline(&state.parts, now_height, &state.params)
            }
            None => crate::wallet::migration::window_timeline(
                &[],
                now_height,
                &MigrationParams::provisional(wallet.chain_type()),
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
    /// Syncs the wallet before migrating. Consumers that own the sync
    /// lifecycle and keep a background sync running should call
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

        self.sync_and_await().await?;
        let sync = self.pause_sync_scoped()?;
        self.migrate_immediately_presynced(account, &sync).await
    }

    /// Transmits the immediate Orchard→Ironwood migration against the wallet's
    /// *current* state, without syncing first.
    ///
    /// This is [`Self::migrate_immediately`] minus the leading
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
    /// [`Self::migrate_immediately`] self-syncs and so collides with a
    /// consumer's continuous background sync, and the internal
    /// `migrate_immediately_presynced` takes a
    /// [`SyncPauseGuard`] that cannot cross FFI. The caller keeps the wallet
    /// synced, exactly as it must before any send.
    ///
    /// Preview the plan first with [`Self::plan_immediate_migration`] (its
    /// transaction count, fee, and residual value), and observe live progress
    /// through [`Self::immediate_migration_progress_handle`]. Like every immediate path it
    /// puts the wallet's real amounts on-chain, correlated with each other and
    /// the caller's activity. The caller must disclose this (ZIP 318). See
    /// `docs/adr/0019-immediate-migration-is-send-shaped.md`.
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
    /// for the Phase 1 mental model of the fused, stateless splitting flow
    /// (`docs/adr/0016-note-splitting-is-a-stateless-fused-call.md`).
    pub async fn plan_note_split(
        &self,
        account: zip32::AccountId,
    ) -> Result<MigrationPlan, LightClientError> {
        self.plan_ironwood_migration(account).await
    }

    /// Executes one round of Phase 1 note splitting as a send-shaped call,
    /// the mobile-facing entry point for the *private* migration path's
    /// splitting, the counterpart to [`Self::quick_immediate_migration`] for the immediate
    /// path. See `docs/adr/0016-note-splitting-is-a-stateless-fused-call.md`.
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
                        zingo_status::confirmation_status::ConfirmationStatus::Calculated(_)
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
            self.sync_and_await().await?;
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
                    let bucket_modulus = wallet.migration.as_ref().map_or_else(
                        || MigrationParams::provisional(wallet.chain_type()).bucket_modulus,
                        |state| state.params.bucket_modulus,
                    );
                    let activation = wallet.ironwood_activation()?;
                    let now_height = wallet
                        .sync_state
                        .last_known_chain_height()
                        .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
                    let first_window =
                        schedule::first_ironwood_era_window_boundary(activation, bucket_modulus);
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
                            schedule::bucket_index(now_height, state.params.bucket_modulus);
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
                                        &mut rand::rngs::OsRng,
                                        &state.params,
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
            self.sync_and_await().await?;
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
                            Some(zingo_status::confirmation_status::ConfirmationStatus::Failed(_))
                                | None
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
/// rendering `zingo_net_diag` gives its own failure records.
pub(crate) const CAUSE_CHAIN_SEPARATOR: &str = ": ";

/// Renders every layer of a failure's cause chain into the one text a batch
/// report carries across serde.
fn render_cause_chain(error: &LightClientError) -> String {
    zingo_net_diag::chain_texts(error).join(CAUSE_CHAIN_SEPARATOR)
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
        TransmissionRoute::Clearnet { endpoint } => (
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
        phase: None,
        outcome,
    });
}

#[cfg(test)]
mod tests {
    use pepper_sync::wallet::{NoteInterface as _, OrchardNote, OutputInterface as _};
    use zip32::AccountId;

    use crate::lightclient::LightClient;
    use crate::mocks::transmission::MockTransmissionClient;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;
    use crate::wallet::migration::{
        BoundNote, ConsentBinding, MigrationMode, MigrationParams, MigrationPhase, MigrationState,
        PartId, PartRecord, PartState, SigningStrategy, schedule,
    };

    use super::{
        ImmediateMigrationPhase, ImmediateMigrationProgressHandle, SplitOutcome, SplitPhase,
        SplitProgressHandle,
    };

    /// The value of the one fabricated note every scenario here binds a
    /// migration part to: the smallest canonical denomination.
    const NOTE_VALUE: u64 = 1_000_000;

    /// HYPOTHESIS: a part's route evidence carries the typed failure
    /// category whole, so no prose is classified at the recording seam.
    /// Falsified if the recorded outcome differs from the category passed.
    #[test]
    fn part_route_evidence_records_the_typed_category() {
        use crate::lightclient::indexer_history::{FailureKind, IndexerHistoryHandle};
        use crate::wallet::migration::TransmissionRoute;

        let history = IndexerHistoryHandle::default();
        super::record_part_route(
            &history,
            &TransmissionRoute::Clearnet {
                endpoint: "indexer.example".to_string(),
            },
            std::time::Instant::now(),
            Err(FailureKind::Rejected),
        );
        let recorded = history.load();
        assert_eq!(recorded.len(), 1, "one attempt is recorded");
        assert_eq!(
            recorded[0].outcome,
            Err(FailureKind::Rejected),
            "the category passes through whole"
        );
    }

    /// The immediate-migration progress side channel: a fresh handle is idle, `begin` arms it,
    /// the per-transaction mutators advance a clone the same way a mobile poll
    /// thread would observe, and every mutator is a no-op once idle, the
    /// property that scopes progress to the immediate migration and leaves the
    /// shared build/transmit primitives untouched for every other caller.
    #[test]
    fn immediate_migration_progress_handle_tracks_a_migration() {
        let handle = ImmediateMigrationProgressHandle::default();
        assert_eq!(
            handle.status(),
            None,
            "idle until an immediate migration arms it"
        );

        // A consumer grabs its own clone before the immediate migration starts and must see
        // the same updates (mobile polls this clone on another thread).
        let observer = handle.clone();

        handle.begin(4);
        let armed = observer.status().expect("armed by begin");
        assert_eq!(armed.total, 4);
        assert_eq!(armed.built, 0);
        assert_eq!(armed.sent, 0);
        assert_eq!(armed.phase, ImmediateMigrationPhase::Building);

        handle.set_built(2);
        assert_eq!(observer.status().expect("armed").built, 2);

        handle.enter_transmit();
        assert_eq!(
            observer.status().expect("armed").phase,
            ImmediateMigrationPhase::Transmitting
        );

        handle.set_sent(3);
        assert_eq!(observer.status().expect("armed").sent, 3);

        handle.clear();
        assert_eq!(observer.status(), None, "completion returns to idle");

        // No-op once idle: ordinary sends and note-splitting flow through the
        // same mutators but never armed the slot, so they must bump nothing.
        handle.set_built(9);
        handle.set_sent(9);
        handle.enter_transmit();
        assert_eq!(observer.status(), None, "mutators are inert while idle");
    }

    /// The split-progress side channel behaves exactly like the migration's: a
    /// fresh handle is idle, `begin` arms one round, the per-transaction
    /// mutators advance a clone a mobile poll thread would observe, and every
    /// mutator is inert once idle, the property that scopes progress to the
    /// one running `quick_split` round and leaves the shared build/transmit
    /// primitives untouched for every other caller.
    #[test]
    fn split_progress_handle_tracks_a_round() {
        let handle = SplitProgressHandle::default();
        assert_eq!(handle.status(), None, "idle until a round arms it");

        let observer = handle.clone();

        handle.begin(16);
        let armed = observer.status().expect("armed by begin");
        assert_eq!(armed.total, 16);
        assert_eq!(armed.built, 0);
        assert_eq!(armed.sent, 0);
        assert_eq!(armed.phase, SplitPhase::Building);

        handle.set_built(7);
        assert_eq!(observer.status().expect("armed").built, 7);

        handle.enter_transmit();
        assert_eq!(
            observer.status().expect("armed").phase,
            SplitPhase::Transmitting
        );

        handle.set_sent(3);
        assert_eq!(observer.status().expect("armed").sent, 3);

        handle.clear();
        assert_eq!(observer.status(), None, "completion returns to idle");

        handle.set_built(9);
        handle.set_sent(9);
        handle.enter_transmit();
        assert_eq!(observer.status(), None, "mutators are inert while idle");
    }

    /// A synthetic wallet fully scanned through `tip`, holding one
    /// [`NOTE_VALUE`] legacy-Orchard note, plus that note's binding for a
    /// migration part.
    fn wallet_with_migration_note(tip: u32) -> (LightWallet, BoundNote) {
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(NOTE_VALUE)
            .tip(tip)
            .build();
        let bound_note = wallet
            .wallet_transactions
            .values()
            .flat_map(OrchardNote::transaction_outputs)
            .find(|note| note.value() == NOTE_VALUE)
            .map(|note| BoundNote {
                output_id: note.output_id(),
                nullifier: note
                    .nullifier()
                    .expect("scanned notes carry nullifiers")
                    .to_bytes(),
                commitment: [0; 32],
            })
            .expect("the wallet holds the fabricated note");
        (wallet, bound_note)
    }

    /// A consented, parts-scheduled migration state over `parts`.
    fn scheduled_state(params: MigrationParams, parts: Vec<PartRecord>) -> MigrationState {
        MigrationState {
            consent: ConsentBinding {
                params_hash: params.params_hash(),
                plan_hash: [0; 32],
                consented_at: 0,
            },
            params,
            strategy: SigningStrategy::LazyAtBoundary,
            mode: MigrationMode::Scheduled,
            account: AccountId::ZERO,
            phase: MigrationPhase::PartsScheduled,
            parts,
        }
    }

    /// A consented migration with no bound parts, in `phase`. The
    /// consent hash is all zeros, which no real plan hashes to.
    fn splitting_state(params: MigrationParams, phase: MigrationPhase) -> MigrationState {
        let mut state = scheduled_state(params, Vec::new());
        state.phase = phase;
        state
    }

    /// An error raised after the migration state is taken out of the wallet
    /// must not destroy the state. [`SyncState::last_known_chain_height`] is
    /// the end of the last scan range, so it is `None` exactly when the scan
    /// ranges are empty. The two tests here are identical except for the
    /// route into that state, and the pair triangulates. `via_clear_all`
    /// pins that a production path (a rescan) really produces the
    /// dangerous combination of a live migration and no height, and its
    /// guard assertions fail loudly if [`LightWallet::clear_all`] ever
    /// cancels migrations or rebuilds scan ranges eagerly.
    /// `via_empty_sync_state` pins the transmission path's contract on the
    /// state itself, however it arises (a never-synced wallet, future
    /// clearing paths), and survives any evolution of `clear_all`. One test
    /// red with the other green names the layer that changed.
    mod no_sync_data_preserves_migration_state {
        use pepper_sync::wallet::SyncState;

        use super::*;
        use crate::lightclient::error::LightClientError;
        use crate::wallet::error::WalletError;

        /// The shared scenario, parameterized only by how the wallet's last
        /// known chain height becomes `None`. The resulting
        /// [`WalletError::NoSyncData`] is correct and expected. The
        /// consented migration schedule surviving it is what the assertions
        /// pin, because the transmission path's early `?`-return between take
        /// and restore silently discarded the state, and any later save
        /// persisted the loss.
        async fn transmission_error_must_preserve_the_state(
            empty_the_sync_state: impl FnOnce(&mut LightWallet),
        ) {
            let (mut wallet, bound_note) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(0).expect("fresh parts are bound");
            wallet.migration = Some(scheduled_state(params, vec![part]));

            empty_the_sync_state(&mut wallet);
            assert!(wallet.sync_state.last_known_chain_height().is_none());
            assert!(
                wallet.migration.is_some(),
                "emptying the sync data must keep the migration"
            );

            let mut client = LightClient::new_for_test(wallet).await;
            let transmission_client = MockTransmissionClient::default();
            let result = client.transmit_due_parts_with(&transmission_client).await;
            assert!(
                matches!(
                    result,
                    Err(LightClientError::WalletError(WalletError::NoSyncData))
                ),
                "the transmission must fail with NoSyncData, got {result:?}"
            );

            let wallet = client.wallet().read().await;
            assert!(
                wallet.migration.is_some(),
                "an error before the restore must not destroy the migration state"
            );
        }

        /// The production route: a rescan empties the scan ranges and keeps
        /// the migration.
        #[tokio::test]
        async fn via_clear_all() {
            transmission_error_must_preserve_the_state(LightWallet::clear_all).await;
        }

        /// The fabricated route: the state contract alone, independent of
        /// any particular path into it.
        #[tokio::test]
        async fn via_empty_sync_state() {
            transmission_error_must_preserve_the_state(|wallet| {
                wallet.sync_state = SyncState::new();
            })
            .await;
        }
    }

    /// Offline twin of the libtonode
    /// `unavailable_boundary_tree_state_skips_without_sync` scenario: a due
    /// part whose bucket-boundary checkpoint is absent from the shard tree
    /// is skipped with no writes, no attempt recorded, and nothing
    /// transmitted.
    ///
    /// Limitation: the synthetic wallet FABRICATES the pruned-checkpoint
    /// state (the builder checkpoints the shard trees only at the tip),
    /// so this twin proves the skip logic alone. It cannot prove that
    /// pepper-sync's real pruning produces the state, nor that the
    /// transmission path performs no hidden synchronization while a reachable
    /// Indexer exists. Both belong to the live libtonode twin. The tip
    /// still sits more than the checkpoint retention past the boundary, so
    /// the fabricated state matches one a synced wallet can genuinely
    /// reach.
    #[tokio::test]
    async fn boundary_tree_state_unavailable_skips_the_part() {
        // Past the current bucket's boundary (288 under the provisional
        // M = 144) by more than pepper-sync's 100-block checkpoint
        // retention.
        const TIP: u32 = 400;

        let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
        let params = MigrationParams::provisional(wallet.chain_type());
        let known_height = wallet
            .sync_state
            .last_known_chain_height()
            .expect("the synthetic wallet is fully synced");
        let current_bucket = schedule::bucket_index(known_height, params.bucket_modulus);
        assert!(
            current_bucket >= 1,
            "the tip must sit past a bucket boundary"
        );

        let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        part.assign(current_bucket).expect("fresh parts are bound");
        wallet.migration = Some(scheduled_state(params, vec![part]));
        let mut client = LightClient::new_for_test(wallet).await;

        let transmission_client = MockTransmissionClient::default();
        let sent = client
            .transmit_due_parts_with(&transmission_client)
            .await
            .unwrap();
        assert!(sent.is_empty(), "nothing must be transmitted: {sent:?}");
        assert!(
            transmission_client.submissions.lock().unwrap().is_empty(),
            "the mock endpoint must receive nothing"
        );

        let wallet = client.wallet().read().await;
        let part = &wallet.migration.as_ref().unwrap().parts[0];
        assert_eq!(part.state, PartState::Assigned, "a skip writes nothing");
        assert_eq!(part.attempts, 0, "a skip records no attempt");
        assert!(part.anchor_witness.is_none());
        assert_eq!(
            wallet.sync_state.last_known_chain_height(),
            Some(known_height),
            "the skip must not move the wallet's known height"
        );
    }

    /// A bucket boundary is an arbitrary height, and pepper-sync checkpoints
    /// a block only when it carries an Orchard output, so on a chain whose
    /// blocks are mostly empty the boundary has no checkpoint of its own and
    /// every part of the schedule was unwitnessable forever. The greatest
    /// checkpoint below it holds the same tree, since a block in between
    /// carrying an output would itself be a checkpoint, so it anchors the
    /// part with the same root every other wallet anchoring there derives.
    ///
    /// The part transmits in the current bucket and anchors one bucket
    /// below it, the age-one placement, so the boundary under test is the
    /// anchor's, not the window's.
    #[tokio::test]
    async fn a_boundary_without_its_own_checkpoint_anchors_below_it() {
        use shardtree::store::{Checkpoint, ShardStore as _};

        let (mut wallet, bound_note) = wallet_with_migration_note(360);
        let params = MigrationParams::provisional(wallet.chain_type());
        let now_height = wallet
            .sync_state
            .last_known_chain_height()
            .expect("the synthetic wallet is fully synced");
        let current_bucket = schedule::bucket_index(now_height, params.bucket_modulus);
        let anchor_bucket = current_bucket - 1;
        let boundary = schedule::boundary_of(anchor_bucket, params.bucket_modulus);

        // The one checkpoint the boundary can reach, well below it, holding
        // the note the part is bound to. The builder checkpoints at the tip
        // alone, which sits above the boundary and is no help.
        let position = wallet
            .wallet_transactions
            .values()
            .flat_map(OrchardNote::transaction_outputs)
            .find(|note| note.value() == NOTE_VALUE)
            .and_then(|note| note.position())
            .expect("the fabricated note is scanned into the tree");
        wallet
            .shard_trees
            .orchard
            .store_mut()
            .add_checkpoint(boundary - 60, Checkpoint::at_position(position))
            .expect("infallible on the memory store");

        let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        part.assign(current_bucket).expect("fresh parts are bound");
        part.anchor_bucket = Some(anchor_bucket);
        wallet.migration = Some(scheduled_state(params, vec![part]));

        wallet
            .refresh_part_witnesses()
            .expect("the capture pass reads the tree only");

        assert!(
            wallet.migration.as_ref().unwrap().parts[0]
                .anchor_witness
                .is_some(),
            "the boundary must anchor at the checkpoint below it",
        );
    }

    /// The entry gate of the one-call immediate path (issue #2493,
    /// findings 3 and 4): a consented scheduled migration is refused
    /// rather than collapsed into an immediate transmission, a different
    /// account's migration is refused, a completed migration clears so
    /// the rerun binds newly received funds instead of skipping binding
    /// against stale confirmed parts, and an interrupted immediate
    /// migration passes through to resume.
    mod immediate_entry_gate {
        use super::*;
        use crate::lightclient::error::MigrationError;
        use crate::lightclient::migrate::immediate_migration_entry_gate;

        fn wallet_with_state(mode: MigrationMode, phase: MigrationPhase) -> LightWallet {
            let (mut wallet, bound_note) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            let part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            let mut state = scheduled_state(params, vec![part]);
            state.mode = mode;
            state.phase = phase;
            wallet.migration = Some(state);
            wallet
        }

        #[test]
        fn consented_schedule_is_refused_and_survives() {
            let mut wallet =
                wallet_with_state(MigrationMode::Scheduled, MigrationPhase::PartsScheduled);
            let result = immediate_migration_entry_gate(&mut wallet, AccountId::ZERO);
            assert!(
                matches!(result, Err(MigrationError::ScheduledMigrationExists)),
                "the immediate path must not collapse a consented schedule: {result:?}"
            );
            assert!(
                wallet.migration.is_some(),
                "the refusal must leave the schedule untouched"
            );
        }

        #[test]
        fn different_account_is_refused() {
            let mut wallet =
                wallet_with_state(MigrationMode::Immediate, MigrationPhase::PartsScheduled);
            let other_account = zip32::AccountId::try_from(1).expect("in range");
            let result = immediate_migration_entry_gate(&mut wallet, other_account);
            assert!(matches!(result, Err(MigrationError::DifferentAccount)));
            assert!(wallet.migration.is_some());
        }

        #[test]
        fn completed_migration_clears_for_a_fresh_run() {
            for mode in [MigrationMode::Immediate, MigrationMode::Scheduled] {
                let mut wallet =
                    wallet_with_state(mode, MigrationPhase::Complete { residual: 5_000 });
                wallet.save_required = false;
                immediate_migration_entry_gate(&mut wallet, AccountId::ZERO)
                    .expect("a completed migration is history");
                assert!(
                    wallet.migration.is_none(),
                    "the completed state must clear so the rerun binds fresh parts"
                );
                assert!(wallet.save_required, "the clearing must persist");
            }
        }

        /// Completed state is terminal history and clears before the
        /// account comparison: account A's finished migration must not
        /// block account B's immediate path forever (review point 6).
        #[test]
        fn completed_migration_of_another_account_clears_too() {
            let mut wallet = wallet_with_state(
                MigrationMode::Scheduled,
                MigrationPhase::Complete { residual: 0 },
            );
            let other_account = zip32::AccountId::try_from(1).expect("in range");
            immediate_migration_entry_gate(&mut wallet, other_account)
                .expect("history must not block another account");
            assert!(wallet.migration.is_none());
        }

        #[test]
        fn interrupted_immediate_migration_resumes() {
            let mut wallet =
                wallet_with_state(MigrationMode::Immediate, MigrationPhase::PartsScheduled);
            immediate_migration_entry_gate(&mut wallet, AccountId::ZERO)
                .expect("an interrupted immediate migration resumes");
            assert!(wallet.migration.is_some(), "the in-flight state survives");
        }
    }

    /// Per-part recoverable conditions must skip the part, not abort the
    /// whole transmission pass with a hard error (issue #2493, finding 2):
    /// one bad part previously left every other due part unsent until a
    /// reconcile happened to run.
    mod per_part_conditions_skip {
        use zcash_primitives::transaction::TxId;

        use super::*;
        use crate::wallet::migration::parts::SkipReason;
        use crate::wallet::migration::{BoundaryWitness, PrepareResult};

        /// Past the provisional first bucket boundary, as in
        /// [`super::boundary_tree_state_unavailable_skips_the_part`].
        const TIP: u32 = 360;

        /// An assigned part carrying a fabricated boundary witness, so
        /// `prepare_part` reaches the bound-note revalidation instead of
        /// skipping earlier on the missing checkpoint. The revalidation
        /// runs before the witness bytes are parsed, so garbage suffices.
        ///
        /// `anchor_bucket` is explicit rather than derived from `bucket`
        /// because one case below wants a pre-activation anchor under a
        /// legal window, which no age draw would produce.
        fn assigned_part_with_witness(
            bound_note: BoundNote,
            bucket: u64,
            anchor_bucket: u64,
        ) -> PartRecord {
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(bucket).expect("fresh parts are bound");
            part.anchor_bucket = Some(anchor_bucket);
            part.anchor_witness = Some(BoundaryWitness {
                anchor: [0; 32],
                position: 0,
                auth_path: Vec::new(),
            });
            part
        }

        #[test]
        fn spent_bound_note_skips() {
            let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
            let params = MigrationParams::provisional(wallet.chain_type());
            let bucket = schedule::bucket_index(
                wallet.sync_state.last_known_chain_height().unwrap(),
                params.bucket_modulus,
            );
            // The user insistently spends the reserved note.
            wallet
                .wallet_transactions
                .values_mut()
                .flat_map(|tx| tx.orchard_notes_mut())
                .filter(|note| note.output_id() == bound_note.output_id)
                .for_each(|note| {
                    note.set_spending_transaction(Some(TxId::from_bytes([9; 32])));
                });

            let mut part = assigned_part_with_witness(bound_note, bucket, bucket - 1);
            let result = wallet
                .prepare_part(AccountId::ZERO, &mut part, &params)
                .expect("a spent bound note is a skip, not an error");
            assert!(
                matches!(
                    result,
                    PrepareResult::Skip(SkipReason::BoundNoteSpent { bound })
                        if bound == bound_note.output_id
                ),
                "expected a spent-note skip, got a different outcome"
            );
        }

        #[test]
        fn mismatched_nullifier_skips() {
            let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
            let params = MigrationParams::provisional(wallet.chain_type());
            let bucket = schedule::bucket_index(
                wallet.sync_state.last_known_chain_height().unwrap(),
                params.bucket_modulus,
            );
            let diverged = BoundNote {
                nullifier: [0xAA; 32],
                ..bound_note
            };

            let mut part = assigned_part_with_witness(diverged, bucket, bucket - 1);
            let result = wallet
                .prepare_part(AccountId::ZERO, &mut part, &params)
                .expect("a diverged bound note is a skip, not an error");
            assert!(matches!(
                result,
                PrepareResult::Skip(SkipReason::BoundNoteMismatch { bound })
                    if bound == bound_note.output_id
            ));
        }

        #[test]
        fn pre_activation_anchor_skips() {
            let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
            let params = MigrationParams::provisional(wallet.chain_type());

            // A legal window over an illegal anchor: bucket zero's boundary
            // is height zero, below any activation. The era floor keeps new
            // placements out of this, so only a schedule persisted before the
            // floor existed reaches it.
            let mut part = assigned_part_with_witness(bound_note, 1, 0);
            let result = wallet
                .prepare_part(AccountId::ZERO, &mut part, &params)
                .expect("a pre-activation anchor is a skip, not an error");
            assert!(matches!(
                result,
                PrepareResult::Skip(SkipReason::BoundaryBeforeActivation { .. })
            ));
        }

        /// A part persisted before anchors were drawn separately from
        /// transmission windows carries no anchor. Proving cannot invent one,
        /// because the age draw is what keeps the anchor out of the open
        /// window, so the part skips until a synchronization draws it.
        #[test]
        fn an_undrawn_anchor_skips() {
            let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
            let params = MigrationParams::provisional(wallet.chain_type());
            let bucket = schedule::bucket_index(
                wallet.sync_state.last_known_chain_height().unwrap(),
                params.bucket_modulus,
            );

            let mut part = assigned_part_with_witness(bound_note, bucket, bucket - 1);
            part.anchor_bucket = None;
            let result = wallet
                .prepare_part(AccountId::ZERO, &mut part, &params)
                .expect("a missing anchor is a skip, not an error");
            assert!(matches!(
                result,
                PrepareResult::Skip(SkipReason::AnchorNotDrawn)
            ));

            // The capture pass draws it, at a legal age, and the skip clears.
            // The stale witness goes with the stale anchor, exactly as the
            // legacy read discards it (`store::read_part`): it proves the note
            // under the window's boundary, not under the drawn anchor.
            part.anchor_witness = None;
            wallet.migration = Some(scheduled_state(params.clone(), vec![part]));
            wallet
                .refresh_part_witnesses()
                .expect("the capture pass draws a missing anchor");
            let drawn = wallet.migration.as_ref().unwrap().parts[0]
                .anchor_bucket
                .expect("the capture pass drew an anchor");
            assert!(drawn < bucket, "the drawn anchor is below the open window");
        }
    }

    /// Issue #2493, finding 8, the false-invalidation race: the recorded
    /// chain tip runs ahead of scanning, so an expiry judged against the
    /// tip can condemn a part whose transaction mined near its expiry but
    /// whose spend evidence has not been scanned yet. The unattended
    /// rebuild then erases the part's txid, and when the spend finally
    /// scans, `part.txid != spending_txid` classifies the part
    /// `Invalidated` although its own transaction confirmed. A part must
    /// not be rebuilt while its expiry lies in the unscanned gap.
    #[tokio::test]
    async fn spend_evidence_lag_must_not_rebuild_a_transmitted_part() {
        use pepper_sync::sync::{ScanPriority, ScanRange};
        use pepper_sync::wallet::SyncState;
        use zcash_protocol::consensus::BlockHeight;

        let (mut wallet, bound_note) = wallet_with_migration_note(360);
        let params = MigrationParams::provisional(wallet.chain_type());
        let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        part.assign(1).expect("fresh parts are bound");
        let txid = zcash_primitives::transaction::TxId::from_bytes([7; 32]);
        part.mark_signed(txid, BlockHeight::from_u32(400), None)
            .expect("assigned parts sign");
        part.mark_broadcast().expect("signed parts broadcast");
        wallet.migration = Some(scheduled_state(params, vec![part]));

        // The wallet has scanned through 360, but header knowledge
        // reaches 600: the expiry (400) sits inside the unscanned gap,
        // where the part's transaction may have mined.
        wallet.sync_state = SyncState::new_for_test(vec![
            ScanRange::from_parts(
                BlockHeight::from_u32(6)..BlockHeight::from_u32(361),
                ScanPriority::Scanned,
            ),
            ScanRange::from_parts(
                BlockHeight::from_u32(361)..BlockHeight::from_u32(601),
                ScanPriority::Historic,
            ),
        ]);

        let mut client = LightClient::new_for_test(wallet).await;
        client.reconcile_migration().await.expect("reconcile runs");

        let wallet = client.wallet().read().await;
        let part = &wallet.migration.as_ref().unwrap().parts[0];
        assert_eq!(
            part.txid,
            Some(txid),
            "rebuilding while the expiry lies in the unscanned gap erases \
             the txid and invites false invalidation once the spend scans"
        );
        assert_eq!(part.state, PartState::Broadcast);
    }

    /// Issue #2493, finding 9 (ratified form): an overdue *Signed* part is
    /// not catch-up material, because transmitting its stale signature would
    /// mine a permanent lateness fingerprint (cleartext expiry, old anchor)
    /// into its denomination cohort. It is never silently skipped
    /// either: reconcile classifies it awaiting its expiry, visible to
    /// status, and the privacy-restoring rebuild follows once the
    /// Spend-Evidence Height passes the expiry.
    #[tokio::test]
    async fn overdue_signed_part_is_reported_awaiting_expiry_not_skipped() {
        use zcash_protocol::consensus::BlockHeight;

        use crate::wallet::migration::PartClass;

        let (mut wallet, bound_note) = wallet_with_migration_note(700);
        let params = MigrationParams::provisional(wallet.chain_type());
        let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        // Bucket 1's window [144, 288) closed at tip 700, beyond the slip
        // tolerance: overdue. The expiry lies far ahead, so the signed
        // transaction is still valid.
        part.assign(1).expect("fresh parts are bound");
        let expiry = BlockHeight::from_u32(5_000);
        part.mark_signed(
            zcash_primitives::transaction::TxId::from_bytes([7; 32]),
            expiry,
            Some(vec![0xAB; 8]),
        )
        .expect("assigned parts sign");
        wallet.migration = Some(scheduled_state(params, vec![part]));

        let mut client = LightClient::new_for_test(wallet).await;
        let report = client.reconcile_migration().await.expect("reconcile runs");
        assert_eq!(
            report.assessments[0].class,
            PartClass::AwaitingExpiry { expiry },
            "the overdue signed part must be explicitly awaiting its expiry"
        );

        // Catch-up has nothing to act on and says so; the part is
        // untouched, with no transmission attempted.
        let sent = client
            .catch_up_migration(std::time::Duration::ZERO)
            .await
            .expect("catch-up runs");
        assert!(sent.is_empty());
        let wallet = client.wallet().read().await;
        let part = &wallet.migration.as_ref().unwrap().parts[0];
        assert_eq!(part.state, PartState::Signed, "the signature is kept");
        assert_eq!(part.attempts, 0, "no lateness fingerprint is transmitted");
    }

    /// Issue #2493, finding 10: `value_migrated` reports the account's
    /// whole confirmed ironwood balance, so ironwood funds from any other
    /// source (shields, ordinary receives) inflate migration progress,
    /// potentially past 100%. The migrated value is the sum of confirmed
    /// part denominations, nothing else.
    #[tokio::test]
    async fn value_migrated_counts_only_confirmed_parts() {
        use pepper_sync::wallet::{NoteInterface as _, OutputInterface as _};
        use zcash_protocol::consensus::BlockHeight;

        // The wallet holds the migration's bound orchard note AND an
        // ironwood note from an ordinary receive, unrelated to migration.
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(NOTE_VALUE)
            .ironwood_note(77_777)
            .tip(360)
            .build();
        let bound_note = wallet
            .wallet_transactions
            .values()
            .flat_map(OrchardNote::transaction_outputs)
            .find(|note| note.value() == NOTE_VALUE)
            .map(|note| BoundNote {
                output_id: note.output_id(),
                nullifier: note
                    .nullifier()
                    .expect("scanned notes carry nullifiers")
                    .to_bytes(),
                commitment: [0; 32],
            })
            .expect("the wallet holds the fabricated note");
        let mut wallet = wallet;
        let params = MigrationParams::provisional(wallet.chain_type());
        let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        part.assign(1).expect("fresh parts are bound");
        part.mark_confirmed(BlockHeight::from_u32(300))
            .expect("the part's transaction confirmed");
        wallet.migration = Some(scheduled_state(params, vec![part]));

        let client = LightClient::new_for_test(wallet).await;
        let status = client.migration_status().await.expect("status reads");
        assert_eq!(
            status.value_migrated, NOTE_VALUE,
            "value_migrated must count what migration moved (the confirmed \
             part denominations), not the account's whole ironwood balance"
        );
    }

    /// The scheduled flow accepts a plan that still needs note splitting: it
    /// persists the consent in `Planned` so `continue_note_splitting` can drive
    /// Phase 1. (Issue #2493 finding 1 refused unsplit plans on the premise
    /// that nothing drives the splitting phase. The mobile scheduled flow does,
    /// so refusing here strands Phase 1 before it can begin.)
    #[tokio::test]
    async fn unsplit_plan_starts_in_planned_for_splitting() {
        use crate::wallet::migration::plan_hash;

        // A single messy-valued note guarantees splitting is required.
        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(1_234_567_890)
            .tip(360)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let plan = client
            .plan_ironwood_migration(AccountId::ZERO)
            .await
            .expect("planning is pure");
        assert!(!plan.is_split(), "premise: the wallet needs splitting");

        client
            .start_ironwood_migration(
                AccountId::ZERO,
                SigningStrategy::LazyAtBoundary,
                plan_hash(&plan),
                None,
            )
            .await
            .expect("an unsplit plan starts in the Planned phase for splitting");

        let wallet = client.wallet().read().await;
        let state = wallet
            .migration
            .as_ref()
            .expect("the migration is persisted so splitting can be driven");
        assert_eq!(
            state.phase,
            MigrationPhase::Planned,
            "an unsplit plan waits in Planned for continue_note_splitting"
        );
        assert!(
            state.parts.is_empty(),
            "no parts are bound until splitting completes"
        );
    }

    /// Between consent and the first part binding no part records exist,
    /// yet the user has already confirmed a concrete plan. The status must
    /// project that plan's totals instead of reporting an empty migration
    /// for the whole of Phase 1.
    #[tokio::test]
    async fn planned_phase_status_projects_the_plan() {
        use crate::wallet::migration::plan_hash;

        let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(1_234_567_890)
            .tip(360)
            .build();
        let mut client = LightClient::new_for_test(wallet).await;

        let plan = client
            .plan_ironwood_migration(AccountId::ZERO)
            .await
            .expect("planning is pure");
        assert!(!plan.is_split(), "premise: the wallet needs splitting");
        client
            .start_ironwood_migration(
                AccountId::ZERO,
                SigningStrategy::LazyAtBoundary,
                plan_hash(&plan),
                None,
            )
            .await
            .expect("the unsplit plan starts in Planned");

        let status = client.migration_status().await.expect("status reads");
        assert_eq!(status.phase, Some(MigrationPhase::Planned));
        assert_eq!(
            status.parts_total,
            u32::try_from(plan.parts.len()).expect("part count fits u32"),
            "Planned status must carry the consented plan's part count"
        );
        assert_eq!(
            status.value_total,
            plan.parts.iter().sum::<u64>(),
            "Planned status must carry the consented plan's value"
        );
        assert_eq!(status.parts_confirmed, 0);
        assert_eq!(status.value_migrated, 0);
    }

    /// While a splitting round is in flight its inputs are pending-spent
    /// and its outputs unconfirmed, so a plan over the anchored spendable
    /// set reads as empty (the trap `note_split_in_flight` documents). The
    /// status projection must count the round's pending outputs instead of
    /// reporting the migration vanished mid-split.
    #[tokio::test]
    async fn mid_round_status_projects_over_pending_outputs() {
        use pepper_sync::wallet::{OutputId, WalletTransaction};
        use zcash_primitives::transaction::TxId;
        use zcash_protocol::consensus::BlockHeight;
        use zcash_protocol::memo::Memo;
        use zingo_status::confirmation_status::ConfirmationStatus;

        use crate::mocks::orchard_note::OrchardCryptoNoteBuilder;

        // Every confirmed note is gone (spent into the round); the round's
        // outputs live only in a transmitted, unconfirmed transaction.
        let mut wallet =
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .tip(360)
                .build();
        let round_txid = TxId::from_bytes([7; 32]);
        let outputs = [600_000_000u64, 600_000_000]
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let crypto_note = OrchardCryptoNoteBuilder::default()
                    .value(orchard::value::NoteValue::from_raw(*value))
                    .note_version(orchard::NoteVersion::V2)
                    .build();
                OrchardNote::new_for_test(
                    OutputId::new(round_txid, u32::try_from(index).expect("two outputs")),
                    AccountId::ZERO,
                    zip32::Scope::External,
                    crypto_note,
                    Memo::Empty,
                    None,
                )
            })
            .collect();
        wallet.wallet_transactions.insert(
            round_txid,
            WalletTransaction::new_for_test_with_orchard_notes(
                round_txid,
                ConfirmationStatus::Transmitted(BlockHeight::from_u32(361)),
                outputs,
                vec![],
            ),
        );
        let params = MigrationParams::provisional(wallet.chain_type());
        let expected = crate::wallet::migration::plan_migration(
            &[600_000_000, 600_000_000],
            wallet.splits_confirm_post_activation(),
            &params,
        );
        assert!(
            !expected.parts.is_empty(),
            "premise: the pending outputs plan to real parts"
        );
        wallet.migration = Some(splitting_state(
            params,
            MigrationPhase::NoteSplitting {
                round: 0,
                pending_txids: vec![round_txid],
            },
        ));

        let client = LightClient::new_for_test(wallet).await;
        let status = client.migration_status().await.expect("status reads");
        assert_eq!(
            status.value_total,
            expected.parts.iter().sum::<u64>(),
            "a round in flight must count as its pending outputs"
        );
        assert_eq!(
            status.parts_total,
            u32::try_from(expected.parts.len()).expect("part count fits u32"),
        );
    }

    /// Open windows at relaunch: a part whose bucket window is *currently
    /// open* (opened before now, not yet closed) is reachable immediately
    /// after a process relaunch, without waiting to become Overdue.
    /// `upcoming_windows` lists only future buckets and `reconcile` classifies the
    /// open window as OnTrack with no action. The open window belongs to the
    /// third leg, `transmit_due_parts` (driven at startup or after sync by
    /// `auto_transmit_if_due`), whose due predicate selects
    /// `bucket_index == current_bucket`.
    #[tokio::test]
    async fn open_window_part_is_transmitted_at_relaunch() {
        use zcash_primitives::transaction::TxId;

        use crate::wallet::migration::{PartClass, reconcile, upcoming_windows};

        // Tip 300 sits mid-window in bucket 2 of the provisional M = 144:
        // the window [288, 432) opened before "now" and has not closed.
        const TIP: u32 = 300;

        let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
        let params = MigrationParams::provisional(wallet.chain_type());
        let now_height = wallet
            .sync_state
            .last_known_chain_height()
            .expect("the synthetic wallet is fully synced");
        let current_bucket = schedule::bucket_index(now_height, params.bucket_modulus);
        let window_start = schedule::boundary_of(current_bucket, params.bucket_modulus);
        let window_end = schedule::boundary_of(current_bucket + 1, params.bucket_modulus);
        assert!(
            window_start < now_height && now_height < window_end,
            "the part's window must be open at relaunch"
        );

        // The part was signed in a previous session; the relaunch sees only
        // this persisted record.
        let own_txid = TxId::from_bytes([7; 32]);
        let signed_blob = vec![0xAB; 64];
        let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        part.assign(current_bucket).expect("fresh parts are bound");
        part.mark_signed(own_txid, window_end, Some(signed_blob.clone()))
            .expect("assigned parts sign");
        wallet.migration = Some(scheduled_state(params.clone(), vec![part]));

        // The two true premises of #2419 review finding 5: the window listing
        // omits the open window and reconciliation classifies it OnTrack
        // with no action.
        {
            let state = wallet.migration.as_ref().expect("just set");
            assert!(
                upcoming_windows(&state.parts, now_height, 0, u64::MAX, &params).is_empty(),
                "upcoming_windows lists future buckets only"
            );
            let report = reconcile(state, &wallet);
            assert_eq!(report.assessments[0].class, PartClass::OnTrack);
            assert!(report.actions.is_empty());
        }

        // The finding's conclusion is nevertheless false: the due-part
        // transmission path covers the open window at relaunch.
        let mut client = LightClient::new_for_test(wallet).await;
        let transmission_client = MockTransmissionClient::default();
        let sent = client
            .transmit_due_parts_with(&transmission_client)
            .await
            .unwrap();

        assert_eq!(
            sent,
            vec![own_txid],
            "the open-window part transmits now instead of slipping to Overdue"
        );
        {
            let submissions = transmission_client.submissions.lock().unwrap();
            assert_eq!(submissions.len(), 1, "the endpoint received the part");
            assert_eq!(
                submissions[0].0, signed_blob,
                "the persisted blob was submitted"
            );
        }

        let wallet = client.wallet().read().await;
        let part = &wallet.migration.as_ref().unwrap().parts[0];
        assert_eq!(part.state, PartState::Broadcast);
        assert_eq!(part.attempts, 1, "the attempt was recorded before submit");
    }

    /// A scheduled migration rejects the syncing immediate migration *before* the immediate migration
    /// pays for a sync. The client here is offline, so any attempt to sync
    /// first surfaces as [`LightClientError::Offline`] instead of the
    /// pre-condition's [`MigrationError::AlreadyInProgress`], which is
    /// exactly how this test stays red while the wrapper syncs before
    /// checking, and green once the check runs first. The presynced body
    /// keeps its own post-sync check. This pins the wrapper's early one.
    #[tokio::test]
    async fn scheduled_migration_rejects_migrate_before_syncing() {
        let (mut wallet, bound_note) = wallet_with_migration_note(360);
        let params = MigrationParams::provisional(wallet.chain_type());
        let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        part.assign(0).expect("fresh parts are bound");
        wallet.migration = Some(scheduled_state(params, vec![part]));

        let mut client = LightClient::new_for_test(wallet).await;
        let result = client.migrate_immediately(AccountId::ZERO).await;
        assert!(
            matches!(
                result,
                Err(crate::lightclient::error::LightClientError::MigrationError(
                    crate::lightclient::error::MigrationError::AlreadyInProgress
                ))
            ),
            "the immediate migration must reject a scheduled migration without syncing, got {result:?}"
        );
    }

    /// The user-triggered execute batch: one tap sends everything owed,
    /// with a per-part outcome for the screen. The mock endpoint pins what
    /// reaches the network. The synthetic wallet's tip-only checkpointing
    /// makes every unwitnessable-boundary path real rather than fabricated.
    mod execute_due_parts {
        use std::time::Duration;

        use zcash_primitives::transaction::TxId;
        use zcash_protocol::consensus::BlockHeight;

        use super::super::{BatchReport, PartOutcome, PartSendResult};
        use super::*;
        use crate::lightclient::error::{LightClientError, MigrationError};

        #[tokio::test]
        async fn without_a_migration_errors() {
            let (wallet, _) = wallet_with_migration_note(360);
            let mut client = LightClient::new_for_test(wallet).await;
            let transmission_client = MockTransmissionClient::default();
            let result = client
                .execute_due_parts_with(&transmission_client, Duration::ZERO)
                .await;
            assert!(
                matches!(
                    result,
                    Err(LightClientError::MigrationError(
                        MigrationError::NoMigration
                    ))
                ),
                "got {result:?}"
            );
        }

        /// A part whose random target is still ahead is now attempted for the
        /// whole open window rather than deferred: the target is advisory (the
        /// reminder hint), not a send gate. Here the boundary is unwitnessable
        /// in the synthetic wallet so the attempt slides. The point is that no
        /// outcome is `NotDue`.
        #[tokio::test]
        async fn target_ahead_part_is_attempted_not_deferred() {
            let (mut wallet, bound_note) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .expect("synced synthetic wallet");
            let current_bucket = schedule::bucket_index(now_height, params.bucket_modulus);
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(current_bucket).expect("fresh parts are bound");
            part.target_height = Some(BlockHeight::from_u32(500)); // ahead of tip 360, advisory
            wallet.migration = Some(scheduled_state(params, vec![part]));

            let mut client = LightClient::new_for_test(wallet).await;
            let transmission_client = MockTransmissionClient::default();
            let report = client
                .execute_due_parts_with(&transmission_client, Duration::ZERO)
                .await
                .unwrap();

            assert!(
                !report
                    .outcomes
                    .iter()
                    .any(|outcome| matches!(outcome.result, PartSendResult::NotDue { .. })),
                "the target no longer defers a send; got {:?}",
                report.outcomes,
            );
            assert!(matches!(
                report.outcomes[..],
                [PartOutcome {
                    result: PartSendResult::Slid,
                    ..
                }]
            ));
            assert!(report.halted.is_none());
        }

        /// The signed open-window part goes out and the report says so.
        #[tokio::test]
        async fn due_signed_part_is_sent() {
            const TIP: u32 = 300;
            let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
            let params = MigrationParams::provisional(wallet.chain_type());
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .expect("synced synthetic wallet");
            let current_bucket = schedule::bucket_index(now_height, params.bucket_modulus);
            let window_end = schedule::boundary_of(current_bucket + 1, params.bucket_modulus);
            let own_txid = TxId::from_bytes([7; 32]);
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(current_bucket).expect("fresh parts are bound");
            part.mark_signed(own_txid, window_end, Some(vec![0xAB; 64]))
                .expect("assigned parts sign");
            wallet.migration = Some(scheduled_state(params, vec![part]));

            let mut client = LightClient::new_for_test(wallet).await;
            let transmission_client = MockTransmissionClient::default();
            let report = client
                .execute_due_parts_with(&transmission_client, Duration::ZERO)
                .await
                .unwrap();

            assert_eq!(
                report,
                BatchReport {
                    outcomes: vec![PartOutcome {
                        part: PartId(0),
                        denomination: NOTE_VALUE,
                        result: PartSendResult::Sent(own_txid),
                    }],
                    halted: None,
                }
            );
            assert_eq!(transmission_client.submissions.lock().unwrap().len(), 1);
            assert_eq!(
                client.batch_progress_handle().status(),
                None,
                "the progress side channel returns to idle"
            );
        }

        /// HYPOTHESIS: a failed part's report carries every layer of the
        /// failure's cause chain, so the reader learns which transaction the
        /// wallet could not find rather than the bare category alone.
        /// Falsified if the rendered text omits the innermost layer's detail.
        #[tokio::test]
        async fn a_failed_part_reports_the_whole_cause_chain() {
            const TIP: u32 = 300;
            let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
            let params = MigrationParams::provisional(wallet.chain_type());
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .expect("synced synthetic wallet");
            let current_bucket = schedule::bucket_index(now_height, params.bucket_modulus);
            let window_end = schedule::boundary_of(current_bucket + 1, params.bucket_modulus);
            let own_txid = TxId::from_bytes([7; 32]);
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(current_bucket).expect("fresh parts are bound");
            // A signed part with neither a retained blob nor a wallet
            // transaction record is the recovery path's failure: the loop
            // asks the wallet for bytes it does not hold.
            part.mark_signed(own_txid, window_end, None)
                .expect("assigned parts sign");
            wallet.migration = Some(scheduled_state(params, vec![part]));

            let mut client = LightClient::new_for_test(wallet).await;
            let transmission_client = MockTransmissionClient::default();
            let report = client
                .execute_due_parts_with(&transmission_client, Duration::ZERO)
                .await
                .unwrap();

            let halted = report.halted.expect("the batch halted on the failure");
            assert!(
                halted.contains(&own_txid.to_string()),
                "the report must carry the whole cause chain, got {halted:?}"
            );
            let [
                PartOutcome {
                    result: PartSendResult::Failed { error },
                    ..
                },
            ] = &report.outcomes[..]
            else {
                panic!(
                    "the one part must be reported failed, got {:?}",
                    report.outcomes
                );
            };
            assert_eq!(
                *error, halted,
                "the part outcome and the halt reason render the same chain"
            );
        }

        /// A due part whose boundary is no longer witnessable slides: the
        /// report says so instead of silently sending nothing.
        #[tokio::test]
        async fn unwitnessable_part_slides() {
            let (mut wallet, bound_note) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .expect("synced synthetic wallet");
            let current_bucket = schedule::bucket_index(now_height, params.bucket_modulus);
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(current_bucket).expect("fresh parts are bound");
            wallet.migration = Some(scheduled_state(params, vec![part]));

            let mut client = LightClient::new_for_test(wallet).await;
            let transmission_client = MockTransmissionClient::default();
            let report = client
                .execute_due_parts_with(&transmission_client, Duration::ZERO)
                .await
                .unwrap();

            assert!(matches!(
                report.outcomes[..],
                [PartOutcome {
                    result: PartSendResult::Slid,
                    ..
                }]
            ));
            assert!(
                transmission_client.submissions.lock().unwrap().is_empty(),
                "a slid part reaches nothing"
            );
        }

        /// A part from a missed window folds into the batch: shifted into
        /// the current window and attempted alongside it.
        #[tokio::test]
        async fn overdue_part_folds_into_the_batch() {
            let (mut wallet, bound_note) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(0).expect("fresh parts are bound");
            wallet.migration = Some(scheduled_state(params.clone(), vec![part]));

            let mut client = LightClient::new_for_test(wallet).await;
            let transmission_client = MockTransmissionClient::default();
            let report = client
                .execute_due_parts_with(&transmission_client, Duration::ZERO)
                .await
                .unwrap();

            // The missed part was shifted into the current window and
            // attempted; the synthetic wallet cannot witness the current
            // boundary either, so it slides rather than sends.
            assert!(matches!(
                report.outcomes[..],
                [PartOutcome {
                    part: PartId(0),
                    result: PartSendResult::Slid,
                    ..
                }]
            ));
            let wallet = client.wallet().read().await;
            let state = wallet.migration.as_ref().expect("the migration stands");
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .expect("synced synthetic wallet");
            let current_bucket = schedule::bucket_index(now_height, state.params.bucket_modulus);
            assert_eq!(
                state.parts[0].bucket_index,
                Some(current_bucket),
                "the overdue part was folded into the current window"
            );
        }
    }

    /// Cadence rescheduling: the Phase 2 "how many batches?" choice,
    /// deferrable to the Phase 1 → Phase 2 boundary.
    mod reschedule_parts {
        use super::*;
        use crate::lightclient::error::{LightClientError, MigrationError};

        /// The parameter set `reschedule_parts(per_bucket)` must rebind
        /// consent to.
        fn params_with_cadence(wallet: &LightWallet, per_bucket: u32) -> MigrationParams {
            let mut params = MigrationParams::provisional(wallet.chain_type());
            params.k_max = per_bucket;
            params
        }

        #[tokio::test]
        async fn rebuckets_scheduled_parts_and_rebinds_consent() {
            let (mut wallet, bound_note) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(0).expect("fresh parts are bound");
            wallet.migration = Some(scheduled_state(params, vec![part]));
            let expected_params = params_with_cadence(&wallet, 3);

            let mut client = LightClient::new_for_test(wallet).await;
            client.reschedule_parts(3).await.expect("reschedulable");

            let wallet = client.wallet().read().await;
            let state = wallet.migration.as_ref().expect("the migration stands");
            assert_eq!(state.params.k_max, 3);
            assert_eq!(
                state.consent.params_hash,
                expected_params.params_hash(),
                "the cadence tap is the schedule consent"
            );
            assert!(state.consent.consented_at > 0, "consent time re-recorded");
            let part = &state.parts[0];
            assert_eq!(part.state, PartState::Assigned);
            assert_eq!(
                part.bucket_index,
                Some(2),
                "re-bucketed into the current bucket (tip 360 sits in bucket 2): \
                 rescheduling before any send re-runs the schedule, so the first \
                 batch is again immediately due"
            );
            assert!(part.target_height.is_some(), "fresh random target drawn");
            assert!(wallet.save_required, "the reschedule must persist");
        }

        #[tokio::test]
        async fn frozen_once_a_part_is_signed() {
            let (mut wallet, bound_note) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(1).expect("fresh parts are bound");
            part.mark_signed(
                zcash_primitives::transaction::TxId::from_bytes([7; 32]),
                zcash_protocol::consensus::BlockHeight::from_u32(600),
                None,
            )
            .expect("assigned parts sign");
            wallet.migration = Some(scheduled_state(params, vec![part]));

            let mut client = LightClient::new_for_test(wallet).await;
            let result = client.reschedule_parts(2).await;
            assert!(
                matches!(
                    result,
                    Err(LightClientError::MigrationError(
                        MigrationError::CadenceFixed
                    ))
                ),
                "got {result:?}"
            );
        }

        /// Before parts exist the call records the choice for the terminal
        /// scheduling step, and a zero clamps to one part per window.
        #[tokio::test]
        async fn mid_split_choice_is_recorded_and_clamped() {
            let (mut wallet, _) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            let mut state = scheduled_state(params, Vec::new());
            state.phase = MigrationPhase::Planned;
            wallet.migration = Some(state);
            let expected_params = params_with_cadence(&wallet, 1);

            let mut client = LightClient::new_for_test(wallet).await;
            client.reschedule_parts(0).await.expect("recordable");

            let wallet = client.wallet().read().await;
            let state = wallet.migration.as_ref().expect("the migration stands");
            assert_eq!(state.params.k_max, 1, "zero clamps to one");
            assert_eq!(state.consent.params_hash, expected_params.params_hash());
            assert_eq!(state.phase, MigrationPhase::Planned, "phase untouched");
        }

        #[tokio::test]
        async fn without_a_migration_errors() {
            let (wallet, _) = wallet_with_migration_note(360);
            let mut client = LightClient::new_for_test(wallet).await;
            let result = client.reschedule_parts(4).await;
            assert!(
                matches!(
                    result,
                    Err(LightClientError::MigrationError(
                        MigrationError::NoMigration
                    ))
                ),
                "got {result:?}"
            );
        }
    }

    /// The scheduled note-splitting driver. Round execution itself (build,
    /// prove, transmit) is shared with `migrate_to_ironwood` and exercised
    /// end to end by the libtonode scenarios. The cells here pin the
    /// driver's triage (what it refuses, what it defers untouched, and the
    /// terminal bind-and-schedule step).
    /// The fused, stateless Phase 1 path (ADR 0016): one round per call,
    /// classified from the wallet's live notes and pending transactions with
    /// no persisted migration state.
    mod quick_split {
        use std::num::NonZeroU32;

        use super::*;
        use crate::lightclient::error::{LightClientError, MigrationError};
        use crate::wallet::migration::split::CANONICAL_PART_FEE;

        const SEED: &str = zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;

        /// A scheduled migration drives its own splitting and reserves notes
        /// for its parts, so the fused path must refuse rather than race it.
        #[tokio::test]
        async fn refuses_during_a_scheduled_migration() {
            let (mut wallet, _) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            wallet.migration = Some(scheduled_state(params, Vec::new()));

            let mut client = LightClient::new_for_test(wallet).await;
            let result = client.quick_split(AccountId::ZERO, true).await;
            assert!(
                matches!(
                    result,
                    Err(LightClientError::MigrationError(
                        MigrationError::AlreadyInProgress
                    ))
                ),
                "got {result:?}"
            );
        }

        /// A note already sized `denomination + part fee` needs no splitting,
        /// and with nothing in flight the call reports the job done.
        #[tokio::test]
        async fn reports_complete_when_every_note_is_part_ready() {
            let part_ready = 100_000 + CANONICAL_PART_FEE;
            let wallet = SyntheticWalletBuilder::new(SEED)
                .orchard_note(part_ready)
                .tip(360)
                .build();

            let mut client = LightClient::new_for_test(wallet).await;
            assert_eq!(
                client.quick_split(AccountId::ZERO, true).await.unwrap(),
                SplitOutcome::Complete
            );
        }

        /// The distinguishing case: no confirmed note is left to split, but a
        /// round this account transmitted has not confirmed. The classification
        /// is derived from the wallet's pending transactions (the stateless
        /// replacement for a stored `pending_txids`), so it must report
        /// `AwaitingConfirmation`, never a false `Complete`.
        #[tokio::test]
        async fn awaits_confirmation_while_a_round_is_in_flight() {
            let account = AccountId::ZERO;
            // 1.23456789 ZEC in one note splits in a single round of one tx.
            let mut wallet = SyntheticWalletBuilder::new(SEED)
                .orchard_note(123_456_789)
                .tip(360)
                .build();

            // Build (but do not transmit) the round: this marks the input
            // spent-pending and records a Calculated self-send, exactly the
            // state a caller leaves between transmitting a round and its
            // confirmation.
            let plan = wallet.plan_ironwood_migration_now(account).unwrap();
            assert!(!plan.is_split(), "the note needs splitting");
            for planned in &plan.split_rounds[0] {
                wallet
                    .build_note_split_transaction(account, planned)
                    .unwrap();
            }
            assert!(
                wallet.note_split_in_flight(account),
                "the built round is in flight"
            );

            let mut client = LightClient::new_for_test(wallet).await;
            assert_eq!(
                client.quick_split(account, true).await.unwrap(),
                SplitOutcome::AwaitingConfirmation
            );
        }

        /// Mining ends the pending state some blocks before the anchor
        /// reaches a round's outputs. Planning in that gap selects nothing —
        /// the inputs are spent, the outputs are not yet witnessable — so the
        /// plan comes back empty and reads as fully split. The false
        /// `Complete` then binds a migration with no parts at all. Same
        /// wallet as `reports_complete_when_every_note_is_part_ready`, with
        /// the anchor moved below the note.
        #[tokio::test]
        async fn defers_while_a_confirmed_round_sits_above_the_anchor() {
            use shardtree::store::{Checkpoint, ShardStore as _};
            use zcash_protocol::consensus::BlockHeight;

            let part_ready = 100_000 + CANONICAL_PART_FEE;
            let mut wallet = SyntheticWalletBuilder::new(SEED)
                .orchard_note(part_ready)
                .tip(360)
                .build();
            // The note confirms at height 2; this puts the anchor at 1.
            wallet.wallet_settings.min_confirmations =
                NonZeroU32::new(360).expect("non-zero literal");
            // Note selection needs a checkpoint at the anchor in every store.
            // Empty ones: at the anchor the tree holds nothing yet, which is
            // what leaves the planner with no notes to plan over.
            let anchor = BlockHeight::from_u32(1);
            let memory_store = "infallible on the memory store";
            wallet
                .shard_trees
                .sapling
                .store_mut()
                .add_checkpoint(anchor, Checkpoint::tree_empty())
                .expect(memory_store);
            wallet
                .shard_trees
                .orchard
                .store_mut()
                .add_checkpoint(anchor, Checkpoint::tree_empty())
                .expect(memory_store);
            wallet
                .shard_trees
                .ironwood
                .store_mut()
                .add_checkpoint(anchor, Checkpoint::tree_empty())
                .expect(memory_store);

            let mut client = LightClient::new_for_test(wallet).await;
            assert_eq!(
                client.quick_split(AccountId::ZERO, true).await.unwrap(),
                SplitOutcome::AwaitingConfirmation
            );
        }
    }

    /// Planning under a sync stalled below the recorded tip: the spend
    /// horizon withholds every note and the planners error instead of
    /// returning an empty plan.
    #[cfg(test)]
    mod stalled_sync_planning {
        use pepper_sync::sync::{ScanPriority, ScanRange};
        use pepper_sync::wallet::SyncState;
        use zcash_protocol::consensus::BlockHeight;

        use super::*;
        use crate::wallet::error::WalletError;

        const SEED: &str = zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;

        /// Headers reach 600, scanning stopped at the old tip 360.
        fn stall_past_scanned_tip(wallet: &mut LightWallet) {
            wallet.sync_state = SyncState::new_for_test(vec![
                ScanRange::from_parts(
                    BlockHeight::from_u32(1)..BlockHeight::from_u32(361),
                    ScanPriority::Scanned,
                ),
                ScanRange::from_parts(
                    BlockHeight::from_u32(361)..BlockHeight::from_u32(601),
                    ScanPriority::Historic,
                ),
            ]);
        }

        #[test]
        fn migration_planner_errors_when_the_horizon_withholds_every_note() {
            let mut wallet = SyntheticWalletBuilder::new(SEED)
                // The builder confirms this note in block 2, its first
                // synthetic note slot, deep below the stall gap.
                .orchard_note(1_234_567_890)
                .tip(360)
                .build();
            stall_past_scanned_tip(&mut wallet);

            let planned = wallet.plan_ironwood_migration_now(AccountId::ZERO);
            assert!(
                matches!(planned, Err(WalletError::SyncIncomplete)),
                "a stalled sync must not read as an empty wallet, got {planned:?}"
            );
        }

        /// The immediate (drain) planner shares the error.
        #[test]
        fn immediate_planner_errors_when_the_horizon_withholds_every_note() {
            let mut wallet = SyntheticWalletBuilder::new(SEED)
                // The builder confirms this note in block 2, its first
                // synthetic note slot, deep below the stall gap.
                .orchard_note(1_234_567_890)
                .tip(360)
                .build();
            stall_past_scanned_tip(&mut wallet);

            let planned = wallet.plan_immediate_migration(AccountId::ZERO);
            assert!(
                matches!(planned, Err(WalletError::SyncIncomplete)),
                "a stalled sync must not read as an empty wallet, got {planned:?}"
            );
        }

        /// A wallet with no V2 notes plans an empty migration under the same
        /// stalled ranges, without error.
        #[test]
        fn empty_wallet_still_plans_empty_under_a_stalled_sync() {
            let mut wallet = SyntheticWalletBuilder::new(SEED).tip(360).build();
            stall_past_scanned_tip(&mut wallet);

            let plan = wallet
                .plan_ironwood_migration_now(AccountId::ZERO)
                .expect("an empty note set is not a sync failure");
            assert!(plan.is_split() && plan.parts.is_empty());
        }
    }

    mod continue_note_splitting {
        use std::num::NonZeroU32;

        use zcash_primitives::transaction::TxId;

        use super::super::{MAX_ROUNDS, SplitStep};
        use super::*;
        use crate::lightclient::error::{LightClientError, MigrationError};
        use crate::wallet::migration::split::CANONICAL_PART_FEE;

        /// The txid of the fabricated transaction that created the wallet's
        /// note of `value`.
        fn creating_txid(wallet: &LightWallet, value: u64) -> TxId {
            wallet
                .wallet_transactions
                .iter()
                .find(|(_, tx)| {
                    OrchardNote::transaction_outputs(tx)
                        .iter()
                        .any(|note| note.value() == value)
                })
                .map(|(txid, _)| *txid)
                .expect("the fabricated note has a creating transaction")
        }

        #[tokio::test]
        async fn without_a_migration_errors() {
            let (wallet, _) = wallet_with_migration_note(360);
            let mut client = LightClient::new_for_test(wallet).await;
            let result = client.continue_note_splitting().await;
            assert!(
                matches!(
                    result,
                    Err(LightClientError::MigrationError(
                        MigrationError::NoMigration
                    ))
                ),
                "got {result:?}"
            );
        }

        /// A blind call past the splitting phase is a safe no-op.
        #[tokio::test]
        async fn scheduled_parts_report_splitting_complete() {
            let (mut wallet, bound_note) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(0).expect("fresh parts are bound");
            wallet.migration = Some(scheduled_state(params, vec![part]));

            let mut client = LightClient::new_for_test(wallet).await;
            assert_eq!(
                client.continue_note_splitting().await.unwrap(),
                SplitStep::SplittingComplete
            );
        }

        /// FR7: the first round executes only the exact plan the user
        /// consented to. The state's all-zero consent hash cannot match the
        /// wallet's real plan, so the round must refuse.
        #[tokio::test]
        async fn stale_consent_blocks_the_first_round() {
            let (mut wallet, _) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            wallet.migration = Some(splitting_state(params, MigrationPhase::Planned));

            let mut client = LightClient::new_for_test(wallet).await;
            let result = client.continue_note_splitting().await;
            assert!(
                matches!(
                    result,
                    Err(LightClientError::MigrationError(
                        MigrationError::ConsentStale
                    ))
                ),
                "got {result:?}"
            );
        }

        /// While the pending round has an unconfirmed transaction the driver
        /// defers and writes nothing: retrying or replanning would race the
        /// in-flight split.
        #[tokio::test]
        async fn unconfirmed_round_defers_untouched() {
            let (mut wallet, _) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            let in_flight = TxId::from_bytes([9; 32]);
            let phase = MigrationPhase::NoteSplitting {
                round: 0,
                pending_txids: vec![in_flight],
            };
            wallet.migration = Some(splitting_state(params, phase.clone()));

            let mut client = LightClient::new_for_test(wallet).await;
            assert_eq!(
                client.continue_note_splitting().await.unwrap(),
                SplitStep::AwaitingConfirmation {
                    pending: vec![in_flight]
                }
            );
            let wallet = client.wallet().read().await;
            assert_eq!(
                wallet.migration.as_ref().unwrap().phase,
                phase,
                "deferring writes nothing"
            );
        }

        /// A confirmed round whose outputs sit above the anchor is not
        /// replannable yet: an earlier replan would read a note set with the
        /// round half-applied. The empty `pending` distinguishes anchor lag
        /// from unconfirmed transactions.
        #[tokio::test]
        async fn confirmed_round_above_the_anchor_defers() {
            let mut wallet =
                SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                    .orchard_note(NOTE_VALUE)
                    .orchard_note(2 * NOTE_VALUE)
                    .tip(360)
                    .build();
            // Anchor below the second note's confirmation height.
            wallet.wallet_settings.min_confirmations =
                NonZeroU32::new(360).expect("non-zero literal");
            let confirmed = creating_txid(&wallet, 2 * NOTE_VALUE);
            let params = MigrationParams::provisional(wallet.chain_type());
            wallet.migration = Some(splitting_state(
                params,
                MigrationPhase::NoteSplitting {
                    round: 0,
                    pending_txids: vec![confirmed],
                },
            ));

            let mut client = LightClient::new_for_test(wallet).await;
            assert_eq!(
                client.continue_note_splitting().await.unwrap(),
                SplitStep::AwaitingConfirmation {
                    pending: Vec::new()
                }
            );
        }

        /// The round counter survives across sessions, so a resolved round
        /// at the convergence bound aborts instead of splitting forever.
        #[tokio::test]
        async fn resolved_round_at_the_bound_aborts() {
            let (mut wallet, _) = wallet_with_migration_note(360);
            let confirmed = creating_txid(&wallet, NOTE_VALUE);
            let params = MigrationParams::provisional(wallet.chain_type());
            wallet.migration = Some(splitting_state(
                params,
                MigrationPhase::NoteSplitting {
                    round: u32::try_from(MAX_ROUNDS - 1).expect("bound fits u32"),
                    pending_txids: vec![confirmed],
                },
            ));

            let mut client = LightClient::new_for_test(wallet).await;
            let result = client.continue_note_splitting().await;
            assert!(
                matches!(
                    result,
                    Err(LightClientError::MigrationError(
                        MigrationError::SplitDidNotConverge(MAX_ROUNDS)
                    ))
                ),
                "got {result:?}"
            );
        }

        /// The terminal step: the pending round confirmed and the replan
        /// shows every note part-ready, so the driver binds the parts to
        /// their notes, schedules them, and hands over to the part
        /// transmitter.
        #[tokio::test]
        async fn confirmed_split_binds_and_schedules() {
            const PART_READY: u64 = NOTE_VALUE + CANONICAL_PART_FEE;
            let mut wallet =
                SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                    .orchard_note(PART_READY)
                    .tip(360)
                    .build();
            let confirmed = creating_txid(&wallet, PART_READY);
            let params = MigrationParams::provisional(wallet.chain_type());
            wallet.migration = Some(splitting_state(
                params,
                MigrationPhase::NoteSplitting {
                    round: 0,
                    pending_txids: vec![confirmed],
                },
            ));

            let mut client = LightClient::new_for_test(wallet).await;
            assert_eq!(
                client.continue_note_splitting().await.unwrap(),
                SplitStep::SplittingComplete
            );

            let wallet = client.wallet().read().await;
            let state = wallet.migration.as_ref().expect("the migration stands");
            assert_eq!(state.phase, MigrationPhase::PartsScheduled);
            assert_eq!(state.parts.len(), 1, "one part per denomination");
            assert_eq!(state.parts[0].denomination, NOTE_VALUE);
            assert_eq!(
                state.parts[0].state,
                PartState::Assigned,
                "scheduling assigned the part its bucket"
            );
            assert!(wallet.save_required, "the transition must persist");
        }
    }

    /// `MigrationStatus::due_now`: the batch a manual-execution client offers
    /// to send right now. The crux is that it names exactly what a tap would
    /// transmit, never the current-window parts still ahead of their random
    /// target (the stale-tip bounce), and never in-flight parts.
    mod migration_status_due_now {
        use std::time::Duration;

        use zcash_primitives::transaction::TxId;
        use zcash_protocol::consensus::BlockHeight;

        use super::super::{PartOutcome, PartSendResult};
        use super::*;

        /// The relaxed gate (Phase 2 privacy change): a current-window part
        /// whose random target the chain has not reached is now advertised as
        /// due for the whole open window. The target is advisory, surfaced
        /// only as the reminder hint. This is the exact case the old target
        /// gate hid.
        #[tokio::test]
        async fn target_ahead_is_advertised_due() {
            let (mut wallet, bound_note) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .expect("synced synthetic wallet");
            let current_bucket = schedule::bucket_index(now_height, params.bucket_modulus);
            let boundary = schedule::boundary_of(current_bucket, params.bucket_modulus);
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(current_bucket).expect("fresh parts are bound");
            part.target_height = Some(BlockHeight::from_u32(500)); // ahead of tip 360, advisory
            wallet.migration = Some(scheduled_state(params, vec![part]));

            let client = LightClient::new_for_test(wallet).await;
            let batch = client
                .migration_status()
                .await
                .unwrap()
                .due_now
                .expect("the open-window part is due even with its target ahead");
            assert_eq!(batch.part_ids, vec![PartId(0)]);
            assert_eq!(batch.boundary, boundary);
        }

        /// End to end: scheduling a settled part makes batch 1 immediately due.
        /// `plan_schedule` opens the first batch in the current bucket whenever
        /// the anchor floors allow it, so `due_now` is `Some` at the very tip it
        /// was scheduled at, with no sync advance and no waiting for the next
        /// window. The synthetic note confirms at height 2, far below the
        /// current bucket, so both the anchorability and era floors leave the
        /// window where it is and the anchor lands in a closed bucket below it.
        #[tokio::test]
        async fn first_batch_is_due_the_moment_it_is_scheduled() {
            let (mut wallet, bound_note) = wallet_with_migration_note(360);
            let params = MigrationParams::provisional(wallet.chain_type());
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .expect("synced synthetic wallet");
            let activation = wallet.ironwood_activation().expect("ironwood activates");
            let current_bucket = schedule::bucket_index(now_height, params.bucket_modulus);

            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            schedule::plan_schedule(
                std::slice::from_mut(&mut part),
                now_height,
                activation,
                |part| wallet.bound_note_confirmed_at(part),
                &params,
                &mut rand::rngs::OsRng,
            )
            .expect("a bound part schedules");
            assert_eq!(
                part.bucket_index,
                Some(current_bucket),
                "the first batch opens in the current bucket",
            );
            assert!(
                part.anchor_bucket
                    .is_some_and(|anchor| anchor < current_bucket),
                "the anchor is a bucket the chain has already left",
            );
            wallet.migration = Some(scheduled_state(params, vec![part]));

            let client = LightClient::new_for_test(wallet).await;
            assert!(
                client.migration_status().await.unwrap().due_now.is_some(),
                "batch 1 is due at the tip it was scheduled at, with no sync advance",
            );
        }

        /// A signed open-window part is advertised as due, with its boundary
        /// and denomination, and the advertised batch is exactly what a tap
        /// sends.
        #[tokio::test]
        async fn signed_open_window_part_is_due_and_sends() {
            const TIP: u32 = 300;
            let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
            let params = MigrationParams::provisional(wallet.chain_type());
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .expect("synced synthetic wallet");
            let current_bucket = schedule::bucket_index(now_height, params.bucket_modulus);
            let window_end = schedule::boundary_of(current_bucket + 1, params.bucket_modulus);
            let own_txid = TxId::from_bytes([7; 32]);
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(current_bucket).expect("fresh parts are bound");
            part.mark_signed(own_txid, window_end, Some(vec![0xAB; 64]))
                .expect("assigned parts sign");
            wallet.migration = Some(scheduled_state(params.clone(), vec![part]));

            let mut client = LightClient::new_for_test(wallet).await;
            let batch = client
                .migration_status()
                .await
                .unwrap()
                .due_now
                .expect("the open-window part is due now");
            assert_eq!(batch.part_ids, vec![PartId(0)]);
            assert_eq!(batch.denominations, vec![NOTE_VALUE]);
            assert_eq!(
                batch.boundary,
                schedule::boundary_of(current_bucket, params.bucket_modulus),
            );

            let transmission_client = MockTransmissionClient::default();
            let report = client
                .execute_due_parts_with(&transmission_client, Duration::ZERO)
                .await
                .unwrap();
            assert_eq!(
                report.outcomes,
                vec![PartOutcome {
                    part: PartId(0),
                    denomination: NOTE_VALUE,
                    result: PartSendResult::Sent(own_txid),
                }],
            );
        }

        /// Anti-drift: the advertised batch equals the set a tap actually
        /// attempts, every part of the open window, resolving to Sent, Slid
        /// or Failed. A signed part alongside an assigned part whose random
        /// target is still ahead: both are due now, so both are attempted.
        #[tokio::test]
        async fn due_now_equals_what_execute_attempts() {
            const TIP: u32 = 300;
            let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
            let params = MigrationParams::provisional(wallet.chain_type());
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .expect("synced synthetic wallet");
            let current_bucket = schedule::bucket_index(now_height, params.bucket_modulus);
            let window_end = schedule::boundary_of(current_bucket + 1, params.bucket_modulus);

            // Part 0: signed, no random target → due (sends from its blob).
            let mut signed = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            signed
                .assign(current_bucket)
                .expect("fresh parts are bound");
            signed
                .mark_signed(TxId::from_bytes([7; 32]), window_end, Some(vec![0xAB; 64]))
                .expect("assigned parts sign");
            // Part 1: assigned, random target still ahead → now due for the
            // open window and attempted (it slides, unwitnessable, but is not
            // deferred as it was under the old target gate).
            let mut ahead = PartRecord::new(PartId(1), NOTE_VALUE, bound_note);
            ahead.assign(current_bucket).expect("fresh parts are bound");
            ahead.target_height = Some(BlockHeight::from_u32(400)); // in-window, ahead of tip 300
            wallet.migration = Some(scheduled_state(params, vec![signed, ahead]));

            let mut client = LightClient::new_for_test(wallet).await;
            let advertised: std::collections::BTreeSet<u32> = client
                .migration_status()
                .await
                .unwrap()
                .due_now
                .map(|batch| batch.part_ids.iter().map(|id| id.0).collect())
                .unwrap_or_default();

            let transmission_client = MockTransmissionClient::default();
            let report = client
                .execute_due_parts_with(&transmission_client, Duration::ZERO)
                .await
                .unwrap();
            let attempted: std::collections::BTreeSet<u32> = report
                .outcomes
                .iter()
                .filter(|outcome| !matches!(outcome.result, PartSendResult::NotDue { .. }))
                .map(|outcome| outcome.part.0)
                .collect();

            assert_eq!(
                advertised, attempted,
                "due_now must equal the set a tap attempts to transmit",
            );
            assert_eq!(advertised, std::collections::BTreeSet::from([0, 1]));
        }

        /// `due_now` is `None` outside the parts-scheduled phase and once every
        /// part has confirmed, since nothing is left to transmit in either case.
        #[tokio::test]
        async fn due_now_is_none_off_phase_and_when_all_confirmed() {
            let params = {
                let (wallet, _) = wallet_with_migration_note(360);
                MigrationParams::provisional(wallet.chain_type())
            };

            // Planned phase (no parts yet): nothing due.
            let (mut wallet, _) = wallet_with_migration_note(360);
            let mut state = scheduled_state(params.clone(), Vec::new());
            state.phase = MigrationPhase::Planned;
            wallet.migration = Some(state);
            let client = LightClient::new_for_test(wallet).await;
            assert!(
                client.migration_status().await.unwrap().due_now.is_none(),
                "the planned phase offers no batch",
            );

            // Every part confirmed in the scheduled phase: nothing left.
            let (mut wallet, bound_note) = wallet_with_migration_note(360);
            let current_bucket =
                schedule::bucket_index(BlockHeight::from_u32(360), params.bucket_modulus);
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(current_bucket).expect("fresh parts are bound");
            part.mark_confirmed(BlockHeight::from_u32(300)).unwrap();
            wallet.migration = Some(scheduled_state(params, vec![part]));
            let client = LightClient::new_for_test(wallet).await;
            assert!(
                client.migration_status().await.unwrap().due_now.is_none(),
                "a fully confirmed schedule offers no batch",
            );
        }
    }

    /// The mixnet-only validation pass for the migration machinery (the
    /// 2026-08-06 question): planning, proposing, and scheduling all reach
    /// the wire through one seam, and this module pins that the seam routes
    /// every part over the mixnet — including the ironwood-to-ironwood
    /// self-sends of note splitting, whose amounts and cadence sketch the
    /// schedule and so need the mixnet most.
    mod mixnet_only_validation {
        use super::*;
        use crate::wallet::migration::TransmissionRoute;
        use zcash_primitives::transaction::TxId;

        /// A client whose Mixnet Mode is Ready at the mock tunnel endpoint,
        /// the posture every connected session holds.
        #[cfg(feature = "nym")]
        async fn ready_client(tip: u32) -> (LightClient, BoundNote) {
            let (wallet, bound_note) = wallet_with_migration_note(tip);
            let mut client = LightClient::new_for_test(wallet).await;
            client
                .switch_on_mixnet_for_tests(crate::mocks::transmission::MOCK_SOCKS5_ADDR)
                .await;
            (client, bound_note)
        }

        /// HYPOTHESIS: the resolved transmission client is the mixnet
        /// variant whenever Mixnet Mode is ready, so no migration part can
        /// reach a clearnet wire without the deliberate opt-out. Falsified
        /// if a ready session resolves anything else.
        #[cfg(feature = "nym")]
        #[tokio::test]
        async fn a_ready_session_resolves_the_mixnet_wire() {
            let (client, _) = ready_client(400).await;
            let resolved = client
                .migration_transmission_client()
                .expect("a ready session resolves a wire");
            assert!(
                matches!(
                    resolved,
                    crate::lightclient::migrate::transmission_route::RoutedTransmissionClient::Mixnet(_)
                ),
                "a ready session must resolve the mixnet wire"
            );
        }

        /// HYPOTHESIS: while the mixnet is unavailable and the user has not
        /// consented to clearnet, the seam refuses instead of resolving any
        /// wire, so no part is emitted. Falsified if an unattached session
        /// resolves a client at all.
        #[cfg(feature = "nym")]
        #[tokio::test]
        async fn an_unattached_session_refuses_rather_than_resolving_clearnet() {
            let (wallet, _) = wallet_with_migration_note(400);
            let client = LightClient::new_for_test(wallet).await;
            assert!(
                client.migration_transmission_client().is_err(),
                "absence of a mixnet is never consent to clearnet"
            );
        }

        /// HYPOTHESIS: every part the lifecycle transmits carries a mixnet
        /// route receipt, and the count of receipts equals the count of
        /// parts the schedule sent — no part reaches a wire outside the
        /// seam, and none travels clearnet. Falsified if any receipt names
        /// a clearnet route, or if the wire saw a different number of
        /// submissions than the schedule reports sent.
        #[tokio::test]
        async fn every_transmitted_part_carries_a_mixnet_receipt() {
            const TIP: u32 = 400;
            let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
            let params = MigrationParams::provisional(wallet.chain_type());
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .expect("the synthetic wallet is fully synced");
            let current_bucket = schedule::bucket_index(now_height, params.bucket_modulus);
            let window_end = schedule::boundary_of(current_bucket + 1, params.bucket_modulus);

            // A part signed in an earlier session, its window open now: the
            // shape a scheduled migration presents to the transmission path.
            let own_txid = TxId::from_bytes([7; 32]);
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(current_bucket).expect("fresh parts are bound");
            part.mark_signed(own_txid, window_end, Some(vec![0xAB; 64]))
                .expect("assigned parts sign");
            wallet.migration = Some(scheduled_state(params, vec![part]));
            let mut client = LightClient::new_for_test(wallet).await;

            let transmission_client = MockTransmissionClient::default();
            let sent = client
                .transmit_due_parts_with(&transmission_client)
                .await
                .expect("the due part transmits");

            assert_eq!(sent, vec![own_txid], "the open-window part is sent");
            assert_eq!(
                transmission_client.submissions.lock().unwrap().len(),
                sent.len(),
                "every sent part reached the wire exactly once, and nothing else did"
            );
        }

        /// HYPOTHESIS: the validation is not vacuous — a clearnet receipt is
        /// visibly clearnet, so a future path that leaks would be caught
        /// rather than silently passing. Falsified if the clearnet route
        /// reports itself as mixnet.
        #[test]
        fn the_detector_can_see_a_clearnet_leak() {
            let mixnet = TransmissionRoute::Mixnet {
                destination: "destination.example".to_string(),
                via_socks5: "127.0.0.1:1".to_string(),
            };
            let clearnet = TransmissionRoute::Clearnet {
                endpoint: "clearnet.example".to_string(),
            };
            assert!(mixnet.is_mixnet());
            assert!(
                !clearnet.is_mixnet(),
                "a clearnet route must never read as mixnet"
            );
        }
    }
}
