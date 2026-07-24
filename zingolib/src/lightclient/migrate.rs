//! Orchard→Ironwood migration orchestration.
//!
//! The scheduled flow a mobile client drives:
//! [`LightClient::plan_ironwood_migration`] →
//! [`LightClient::start_ironwood_migration`] (consent) → note-splitting
//! rounds → [`LightClient::reconcile_migration`] on every launch →
//! [`LightClient::broadcast_due_parts`] from background wakes →
//! [`LightClient::catch_up_migration`] when windows were missed.
//!
//! [`LightClient::migrate_to_ironwood`] composes the same pieces into an
//! interactive one-call for CLI use, testing, and the user who prefers the
//! immediate migration ZIP 318 permits with a disclosed privacy trade-off.
//!
//! [`LightClient::drain_orchard_to_ironwood`] is the other option ZIP 318
//! offers the user: move everything at once, no note splitting and no
//! schedule, accepting that the transfers are correlated and the amounts are
//! the wallet's own.
//!
//! Part broadcasts obey the Mixnet Mode policy (ADR 0011, amendment
//! 2026-07-23): while the mode is on they travel only over the mixnet, fail
//! closed while it is not ready, and never target the synchronization
//! endpoint's host. See [`broadcast_route`].

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
    BroadcastClient, ChainView, ConsentBinding, DrainPlan, MigrationMode, MigrationParams,
    MigrationPhase, MigrationPlan, MigrationState, PartId, PartState, PrepareResult,
    RecommendedAction, ReconcileReport, SigningStrategy, WakePoint, plan_hash, plan_migration,
    plan_schedule, reconcile, schedule,
};

pub mod broadcast_grpc;
pub mod broadcast_route;

/// How long to wait between sync polls while a note-splitting round confirms.
const CONFIRMATION_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Give up waiting for a note-splitting round after this many polls.
const MAX_CONFIRMATION_POLLS: usize = 720;
/// A migration replans after every round. A real plan converges in
/// `~log_K(N)` rounds, so far more than this means something is wrong.
const MAX_ROUNDS: usize = 64;
/// How many buckets ahead [`LightClient::migration_status`] reports wakes
/// for.
const WAKE_HORIZON_BUCKETS: u64 = 32;

/// The transactions of a completed drain
/// ([`LightClient::drain_orchard_to_ironwood`]).
#[derive(Debug, Clone)]
pub struct DrainSummary {
    /// The drain transactions, in broadcast order. More than one only when the
    /// account held more notes than fit in a single transaction.
    pub txids: Vec<TxId>,
    /// Value (zatoshis) sent into the Ironwood pool.
    pub migrated: u64,
    /// Total fees paid, in zatoshis.
    pub fee: u64,
    /// Dust value (zatoshis) left unmigrated in the Orchard pool.
    pub stranded: u64,
}

/// The coarse stage an in-progress immediate drain is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainPhase {
    /// Proving and signing the planned transactions.
    Building,
    /// Broadcasting the built transactions.
    Transmitting,
}

/// A snapshot of an in-progress immediate Orchard→Ironwood drain, for rendering
/// "built i/N, sent i/N". The immediate-drain counterpart to [`MigrationStatus`].
/// `None` from [`LightClient::drain_status`] means no drain is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainStatus {
    /// Total transactions in the plan (N), fixed when the drain begins.
    pub total: u32,
    /// Transactions built (proved + signed) so far, `0..=total`.
    pub built: u32,
    /// Transactions broadcast so far, `0..=total`.
    pub sent: u32,
    /// Which phase the drain is in.
    pub phase: DrainPhase,
}

/// A cloneable handle to an immediate drain's live progress, readable without
/// touching the wallet lock. A consumer that runs the drain (which borrows the
/// client `&mut self`) grabs this via [`LightClient::drain_progress_handle`]
/// *before* starting the drain, then polls [`Self::status`] concurrently.
///
/// The drain holds the wallet write lock across its whole build and transmit
/// loops, so progress lives in this side channel instead of in wallet state:
/// a poll never contends with the drain for the wallet lock.
#[derive(Debug, Clone, Default)]
pub struct DrainProgressHandle(Arc<Mutex<Option<DrainStatus>>>);

impl DrainProgressHandle {
    /// The current drain snapshot, or `None` when no drain is running.
    pub fn status(&self) -> Option<DrainStatus> {
        self.0
            .lock()
            .expect("drain progress mutex poisoned")
            .clone()
    }

    /// Arms a fresh drain of `total` transactions. Every other mutator is a
    /// no-op until this has been called, which is what scopes progress to the
    /// immediate drain and leaves the shared build/transmit primitives
    /// untouched for every other caller.
    pub(crate) fn begin(&self, total: u32) {
        *self.0.lock().expect("drain progress mutex poisoned") = Some(DrainStatus {
            total,
            built: 0,
            sent: 0,
            phase: DrainPhase::Building,
        });
    }

    /// Publishes the number of transactions built so far. No-op when idle.
    pub(crate) fn set_built(&self, built: u32) {
        if let Some(status) = self
            .0
            .lock()
            .expect("drain progress mutex poisoned")
            .as_mut()
        {
            status.built = built;
        }
    }

    /// Advances the phase to [`DrainPhase::Transmitting`]. No-op when idle.
    pub(crate) fn enter_transmit(&self) {
        if let Some(status) = self
            .0
            .lock()
            .expect("drain progress mutex poisoned")
            .as_mut()
        {
            status.phase = DrainPhase::Transmitting;
        }
    }

    /// Publishes the number of transactions broadcast so far. No-op when idle.
    pub(crate) fn set_sent(&self, sent: u32) {
        if let Some(status) = self
            .0
            .lock()
            .expect("drain progress mutex poisoned")
            .as_mut()
        {
            status.sent = sent;
        }
    }

    /// Returns to the idle state, so a poll reports `None` once more.
    pub(crate) fn clear(&self) {
        *self.0.lock().expect("drain progress mutex poisoned") = None;
    }
}

/// Clears the drain progress on drop, so a failed or early-returning drain never
/// leaves a stale snapshot behind. Owns an `Arc` clone (not a borrow of the
/// client) so it can live across the `&mut self` [`LightClient::build_and_transmit`]
/// call.
struct DrainProgressScope(DrainProgressHandle);

impl Drop for DrainProgressScope {
    fn drop(&mut self) {
        self.0.clear();
    }
}

/// The transactions of a completed migration.
#[derive(Debug, Clone)]
pub struct MigrationSummary {
    /// Note-splitting (Orchard→Orchard) transactions, in broadcast order.
    pub split_txids: Vec<TxId>,
    /// Parts (Orchard→Ironwood), one per denomination.
    pub part_txids: Vec<TxId>,
    /// Dust value (zatoshis) left unmigrated in the Orchard pool.
    pub stranded: u64,
}

/// The migration's progress, arranged for direct rendering.
#[derive(Debug, Clone)]
pub struct MigrationStatus {
    /// Confirmed-spendable balance left in the *Orchard pool* specifically.
    /// ZIP 318 requires displaying this figure, never only a unified total.
    pub orchard_confirmed_spendable: u64,
    /// Where the migration is, `None` when none is in progress.
    pub phase: Option<MigrationPhase>,
    /// Scheduled parts in total.
    pub parts_total: u32,
    /// Parts confirmed so far.
    pub parts_confirmed: u32,
    /// Total value across all parts, in zatoshis.
    pub value_total: u64,
    /// Value already confirmed into the Ironwood pool, in zatoshis.
    pub value_migrated: u64,
    /// Coming broadcast windows, what a platform scheduler feeds into its
    /// earliest-begin requests.
    pub next_wakes: Vec<WakePoint>,
}

impl LightClient {
    /// Plans a migration from the wallet's current spendable Orchard notes.
    ///
    /// Pure and deterministic: nothing is signed or sent, so the plan (its
    /// transaction count, fees and stranded dust) can be shown to the user
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
    /// the client re-plans. The note set must already be fully split: parts
    /// are bound to their notes and scheduled immediately. A plan that
    /// still needs note splitting is refused with
    /// [`MigrationError::NoteSplittingRequired`], because no driver for
    /// scheduled-flow splitting exists yet — persisting such a state would
    /// strand the consent in [`MigrationPhase::Planned`] forever while
    /// blocking the drain path.
    /// `per_bucket` overrides `k_max` in the migration params, capping how
    /// many parts share each broadcast window. Lower values spread parts
    /// across more sessions (better privacy, slower completion); higher values
    /// concentrate them (faster, more correlated). `None` keeps the default.
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
        // — including a sync commit — needs this same lock, so the notes
        // hashed are the notes bound; no await point sits inside the
        // bracket, so a cancelled future cannot abandon it midway (issue
        // #2493, finding 11).
        let mut wallet = self.wallet().write().await;
        let plan = wallet.plan_ironwood_migration_now(account)?;
        let hash = plan_hash(&plan);
        if hash != consented_plan_hash {
            return Err(MigrationError::ConsentStale.into());
        }
        if !plan.is_split() {
            return Err(MigrationError::NoteSplittingRequired.into());
        }
        if wallet.migration.is_some() {
            return Err(MigrationError::AlreadyInProgress.into());
        }
        let activation = wallet.ironwood_activation()?;

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
        wallet.bind_parts_to_notes(&mut state, account)?;
        let now_height = wallet
            .sync_state
            .last_known_chain_height()
            .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
        plan_schedule(
            &mut state.parts,
            now_height,
            activation,
            &state.params,
            &mut rand::rngs::OsRng,
        )?;
        state.phase = MigrationPhase::PartsScheduled;
        wallet.migration = Some(state);
        wallet.save_required = true;
        Ok(())
    }

    /// The broadcast-only client parts are submitted through, resolved by the
    /// Mixnet Mode policy (ADR 0011, amendment 2026-07-23) like every other
    /// transmitting surface.
    ///
    /// While the mode is on, parts travel ONLY over the mixnet — this fails
    /// closed ([`MixnetNotReady`](crate::nym::MixnetNotReady)) while the
    /// proxy bootstraps or after it dies — to one Broadcast Indexer drawn at
    /// random per submission, with the synchronization endpoint's host
    /// forbidden as a target (a `migration_broadcast_uri` on that host is
    /// refused). Clearnet carries parts only when the user deliberately
    /// toggled the mode off, or in a build without the `nym` feature: then
    /// the dedicated `migration_broadcast_uri` when configured, else the
    /// synchronization endpoint with a logged correlation warning, else
    /// [`LightClientError::Offline`] with no traffic emitted.
    fn migration_broadcast_client(
        &self,
    ) -> Result<broadcast_route::RoutedBroadcastClient, LightClientError> {
        #[cfg(feature = "nym")]
        if let crate::nym::MixnetRoute::Mixnet(socks5_addr) = self.mixnet_route()? {
            let sync_host = self
                .indexer_uri()
                .and_then(|uri| uri.host().map(str::to_string));
            let candidates = broadcast_route::eligible_candidates(
                self.migration_broadcast_uri.clone(),
                sync_host.as_deref(),
                crate::nym::broadcast_indexers::broadcast_indexers(),
            )?;
            return Ok(broadcast_route::RoutedBroadcastClient::Mixnet(
                broadcast_route::MixnetBroadcastClient::new(socks5_addr, candidates),
            ));
        }

        let clearnet = match &self.migration_broadcast_uri {
            Some(uri) => broadcast_grpc::GrpcBroadcastClient::new(uri.clone()),
            None => {
                let indexer_uri = self.indexer_uri().ok_or(LightClientError::Offline)?;
                log::warn!(
                    "no dedicated migration broadcast endpoint configured; parts will be \
                     broadcast to the synchronization endpoint, which lets that server \
                     correlate synchronization with migration activity"
                );
                broadcast_grpc::GrpcBroadcastClient::new(indexer_uri)
            }
        };
        Ok(broadcast_route::RoutedBroadcastClient::Clearnet(clearnet))
    }

    /// Materializes and broadcasts every part whose bucket window is open.
    ///
    /// Works from persisted state and the local shard tree only: this path
    /// never synchronizes and never touches the synchronization client
    /// (ZIP 318's decoupling requirement). Parts whose tree state is
    /// unavailable are skipped and fall to reconciliation. Parts in earlier,
    /// missed buckets are catch-up's business, because sending them needs
    /// the user-facing disclosure.
    pub async fn broadcast_due_parts(&mut self) -> Result<Vec<TxId>, LightClientError> {
        let client = self.migration_broadcast_client()?;
        self.broadcast_due_parts_with(&client).await
    }

    /// [`Self::broadcast_due_parts`] with an injectable client, for tests
    /// and the one-call path.
    pub(crate) async fn broadcast_due_parts_with(
        &mut self,
        client: &impl BroadcastClient,
    ) -> Result<Vec<TxId>, LightClientError> {
        self.broadcast_due_parts_selected(client, None).await
    }

    /// The due-part broadcast loop, optionally narrowed to a single part so
    /// catch-up can sequence sends with spacing.
    ///
    /// Proving is parallelised across all due parts via
    /// [`tokio::task::spawn_blocking`]: wallet reads happen under the write
    /// lock (Phase A), all Halo2/Groth16 work runs concurrently on the
    /// blocking thread pool without holding the lock (Phase B), and wallet
    /// writes + submission happen sequentially under the lock again (Phase C).
    /// The due-part broadcast loop, optionally narrowed to a single part so
    /// catch-up can sequence sends with spacing.
    ///
    /// Proving is parallelised across all due parts via
    /// [`tokio::task::spawn_blocking`]: wallet reads happen under the write
    /// lock (Phase A), all Halo2/Groth16 work runs concurrently on the
    /// blocking thread pool without holding the lock (Phase B), and wallet
    /// writes + submission happen sequentially under the lock again (Phase C).
    async fn broadcast_due_parts_selected(
        &mut self,
        client: &impl BroadcastClient,
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
                            matches!(part.state, PartState::Assigned | PartState::Signed)
                                && part.bucket_index == Some(current_bucket)
                                && part.target_height.is_none_or(|t| now_height >= t)
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
                                        prove().map(|(txid, raw_tx)| (index, txid, raw_tx))
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
        }; // wallet write lock released — Phase B runs without the lock

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
                    let expiry_height = boundary + state.params.expiry_delta;
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
                        .chain(pre_proven.into_iter())
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
            match client.submit(raw_tx, txid, expiry_height).await {
                Ok(_) => {
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
    /// binding and scheduling parts once splitting confirms, and marking
    /// completion. Actions needing consent or a user-facing disclosure
    /// (catch-up, replanning the remainder) are returned untouched in the
    /// report.
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
                            schedule::place(
                                part,
                                schedule::first_permitted_bucket(
                                    now_height,
                                    activation,
                                    &state.params,
                                ),
                                &mut rand::rngs::OsRng,
                                &state.params,
                            )?;
                        }
                        RecommendedAction::BindAndSchedule => {
                            let account = state.account;
                            wallet.bind_parts_to_notes(state, account)?;
                            let now_height = wallet
                                .sync_state
                                .last_known_chain_height()
                                .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
                            let activation = wallet.ironwood_activation()?;
                            plan_schedule(
                                &mut state.parts,
                                now_height,
                                activation,
                                &state.params,
                                &mut rand::rngs::OsRng,
                            )?;
                            state.phase = MigrationPhase::PartsScheduled;
                        }
                        RecommendedAction::MarkComplete { residual } => {
                            state.phase = MigrationPhase::Complete {
                                residual: *residual,
                            };
                        }
                        // Left to the caller: user-facing disclosure or fresh
                        // consent required, or nothing to apply.
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
    /// broadcasts (never simultaneously), after the caller has shown the
    /// ZIP 318 disclosure that sending at application-open time correlates
    /// the broadcasts with the user's activity.
    ///
    /// Each overdue part is shifted into the current bucket (its old anchor
    /// is stale) before materializing and broadcasting.
    pub async fn catch_up_migration(
        &mut self,
        spacing: Duration,
    ) -> Result<Vec<TxId>, LightClientError> {
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
            return Ok(Vec::new());
        }

        {
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
                    for part_id in &overdue {
                        let part = &mut state.parts[part_id.0 as usize];
                        if part.state == PartState::Assigned {
                            // Catch-up fires now by disclosed intent:
                            // explicitly immediate placement. Overdue
                            // signed parts never reach here — reconcile
                            // classifies them AwaitingExpiry, outside the
                            // catch-up cohort.
                            schedule::place_immediate(part, current_bucket)?;
                        }
                    }
                    Ok::<_, crate::wallet::error::WalletError>(())
                })
                .ok_or(MigrationError::NoMigration)??;
        }
        self.wallet().write().await.refresh_part_witnesses()?;

        let client = self.migration_broadcast_client()?;
        let mut sent = Vec::new();
        for part_id in overdue {
            let txids = self
                .broadcast_due_parts_selected(&client, Some(part_id))
                .await?;
            if !txids.is_empty() {
                sent.extend(txids);
                tokio::time::sleep(spacing).await;
            }
        }
        Ok(sent)
    }

    /// Broadcasts any parts whose bucket window and random target height are
    /// both reached, without synchronizing. Call this after each sync to drive
    /// the scheduled migration automatically.
    ///
    /// No-op when no migration is active or no parts are due.
    pub async fn auto_broadcast_if_due(
        &mut self,
    ) -> Result<Vec<zcash_primitives::transaction::TxId>, LightClientError> {
        {
            let wallet = self.wallet().read().await;
            if wallet.migration.is_none() {
                return Ok(Vec::new());
            }
        }
        self.wallet().write().await.refresh_part_witnesses()?;
        let client = self.migration_broadcast_client()?;
        self.broadcast_due_parts_with(&client).await
    }

    /// The migration's progress, everything a progress UI renders. Includes
    /// the Orchard-pool-specific confirmed-spendable figure, which ZIP 318
    /// requires displaying instead of a unified total.
    pub async fn migration_status(&self) -> Result<MigrationStatus, LightClientError> {
        let wallet = self.wallet().read().await;
        let (phase, parts_total, parts_confirmed, value_total, value_migrated, wakes, account) =
            match &wallet.migration {
                Some(state) => {
                    let confirmed: Vec<_> = state
                        .parts
                        .iter()
                        .filter(|part| matches!(part.state, PartState::Confirmed { .. }))
                        .collect();
                    let now_height = wallet.sync_state.last_known_chain_height();
                    let wakes = now_height.map_or_else(Vec::new, |height| {
                        crate::wallet::migration::next_wakes(
                            &state.parts,
                            height,
                            u64::from(crate::utils::now()),
                            WAKE_HORIZON_BUCKETS,
                            &state.params,
                        )
                    });
                    // What this migration moved: the confirmed part
                    // denominations, and nothing else. The account's whole
                    // Ironwood balance also holds shields and ordinary
                    // receives, which are not migration progress (issue
                    // #2493, finding 10).
                    let value_migrated = confirmed.iter().map(|part| part.denomination).sum();
                    (
                        Some(state.phase.clone()),
                        state.parts.len() as u32,
                        confirmed.len() as u32,
                        state.parts.iter().map(|part| part.denomination).sum(),
                        value_migrated,
                        wakes,
                        state.account,
                    )
                }
                None => (None, 0, 0, 0, 0, Vec::new(), zip32::AccountId::ZERO),
            };

        Ok(MigrationStatus {
            orchard_confirmed_spendable: ChainView::orchard_confirmed_spendable(&*wallet, account),
            phase,
            parts_total,
            parts_confirmed,
            value_total,
            value_migrated,
            next_wakes: wakes,
        })
    }

    /// Plans an immediate drain of the account's Orchard pool into Ironwood.
    ///
    /// Pure and deterministic, nothing is signed or sent, so the plan (its
    /// transaction count, fees and stranded dust) can be shown to the user for
    /// consent before [`Self::drain_orchard_to_ironwood`] executes it.
    pub async fn plan_orchard_drain(
        &self,
        account: zip32::AccountId,
    ) -> Result<DrainPlan, LightClientError> {
        let wallet = self.wallet().read().await;
        let params = MigrationParams::provisional(wallet.chain_type());
        Ok(wallet.plan_drain(account, &params)?)
    }

    /// Spends every spendable Orchard note in `account` into the Ironwood pool,
    /// in one round of independent transactions.
    ///
    /// This is the *migrate immediately* path ZIP 318 offers alongside the
    /// private one. All the transfers are broadcast at once, so they correlate with each other and
    /// with the user's activity. Every one of those identifies the wallet on-chain.
    /// **The caller must disclose this.** For the private path, use
    /// [`Self::migrate_to_ironwood`].
    ///
    /// Notes worth at most [`MigrationParams::sweep_min`] are left behind.
    /// Spending one costs more than it carries, and their total is reported as
    /// [`DrainSummary::stranded`].
    ///
    /// This function is idempotent over wallet state: a call that fails partway leaves the notes
    /// of every unsent transaction spendable. Calling it again re-plans and
    /// sends the remainder.
    ///
    /// Syncs the wallet before draining. Consumers that own the sync
    /// lifecycle and keep a background sync running should call
    /// [`Self::drain_orchard_to_ironwood_presynced`] instead, which drains
    /// against current wallet state without launching its own sync.
    pub async fn drain_orchard_to_ironwood(
        &mut self,
        account: zip32::AccountId,
    ) -> Result<DrainSummary, LightClientError> {
        // A scheduled migration rejects the drain regardless of chain state,
        // so check before paying for a sync. The presynced body re-checks
        // after the sync lands.
        if self.wallet().read().await.migration.is_some() {
            return Err(MigrationError::AlreadyInProgress.into());
        }

        self.sync_and_await().await?;
        let sync = self.pause_sync_scoped()?;
        self.drain_orchard_to_ironwood_presynced(account, &sync)
            .await
    }

    /// Broadcasts the immediate Orchard→Ironwood drain against the wallet's
    /// *current* state, without syncing first.
    ///
    /// This is [`Self::drain_orchard_to_ironwood`] minus the leading
    /// `sync_and_await`, for consumers that own the sync lifecycle and keep a
    /// background sync running continuously (e.g. zingo-mobile). Calling the
    /// syncing variant from such a consumer collides with the running sync
    /// and fails with [`pepper_sync::error::SyncModeError::SyncAlreadyRunning`];
    /// this entry point lets the caller drive sync itself.
    ///
    /// The caller is responsible for keeping the wallet synced before
    /// calling, and proves it has paused its sync by presenting the
    /// [`SyncPauseGuard`] — [`Self::pause_sync_scoped`] pauses a running
    /// engine and resumes it when the guard drops. Planning and building
    /// therefore observe one stable wallet state, the same
    /// pause-before-proposing invariant the `send`/`shield` mutation paths
    /// establish. Everything else — the plan, the chunked broadcast, and the
    /// idempotent cleanup on partial failure — is identical to the syncing
    /// variant.
    ///
    /// Calling without the guard does not compile — a stable wallet state
    /// across plan and build is a compile-time precondition, not a runtime
    /// courtesy:
    ///
    /// ```compile_fail
    /// # async fn caller(client: &mut zingolib::lightclient::LightClient) {
    /// let _ = client
    ///     .drain_orchard_to_ironwood_presynced(zip32::AccountId::ZERO)
    ///     .await;
    /// # }
    /// ```
    pub async fn drain_orchard_to_ironwood_presynced(
        &mut self,
        account: zip32::AccountId,
        sync: &SyncPauseGuard,
    ) -> Result<DrainSummary, LightClientError> {
        // A scheduled migration soft-reserves the notes its parts are bound to.
        // Draining them would invalidate those parts behind its back.
        if self.wallet().read().await.migration.is_some() {
            return Err(MigrationError::AlreadyInProgress.into());
        }

        let plan = self.plan_orchard_drain(account).await?;
        if plan.is_empty() {
            return Err(crate::wallet::error::WalletError::NothingToMigrate.into());
        }

        // Arm per-transaction progress for the poll side channel. The scope
        // guard owns an `Arc` clone (not a borrow of `self`), so it survives the
        // `&mut self` `build_and_transmit` call and clears the snapshot on every
        // exit — success, `?`-propagated error, or panic.
        self.drain_progress.begin(plan.transactions.len() as u32);
        let _scope = DrainProgressScope(self.drain_progress.clone());

        let txids = self
            .build_and_transmit(&plan.transactions, sync, |wallet, planned| {
                wallet.build_drain_transaction(account, planned)
            })
            .await?;

        Ok(DrainSummary {
            txids,
            migrated: plan.migrated,
            fee: plan.fee,
            stranded: plan.stranded,
        })
    }

    /// Builds and transmits one batch of planned migration transactions under
    /// a caller-held [`SyncPauseGuard`], enforcing the shared cleanup
    /// contract: a build failure fails the transactions already built, and a
    /// transmit failure fails every transaction still unsent, so no note
    /// stays spent by a transaction that will never reach the network. Both
    /// the drain flow and the note-splitting rounds send through here. The
    /// guard parameter is pure proof — the caller's guard performs the
    /// pause and its drop the resume, on every exit path.
    async fn build_and_transmit<T>(
        &mut self,
        planned: &[T],
        _sync: &SyncPauseGuard,
        build: impl Fn(&mut LightWallet, &T) -> Result<TxId, WalletError>,
    ) -> Result<Vec<TxId>, LightClientError> {
        let txids = self.build_transactions(planned, build).await?;

        // Build is done; the transmit loop below publishes "sent i/N". No-op
        // unless an immediate drain armed the side channel.
        self.drain_progress.enter_transmit();

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
        build: impl Fn(&mut LightWallet, &T) -> Result<TxId, WalletError>,
    ) -> Result<Vec<TxId>, LightClientError> {
        let mut wallet = self.wallet().write().await;
        let mut txids = Vec::with_capacity(planned.len());

        for item in planned {
            match build(&mut wallet, item) {
                Ok(txid) => {
                    txids.push(txid);
                    // Publish "built i/N". No-op unless a drain armed the side
                    // channel, so ordinary sends and note-splitting are untouched.
                    self.drain_progress.set_built(txids.len() as u32);
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
    /// materializes and broadcasts every part immediately through the
    /// [`BroadcastClient`].
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
                    // than in receiver discipline at the API surface — a
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
                    // An immediate part anchors at the current bucket's
                    // boundary. Until the first post-activation boundary
                    // opens, no anchor exists and every broadcast pass
                    // would skip every part — previously a MAX_ROUNDS
                    // spin ending in a misleading SplitDidNotConverge.
                    let bucket_modulus = wallet.migration.as_ref().map_or_else(
                        || MigrationParams::provisional(wallet.chain_type()).bucket_modulus,
                        |state| state.params.bucket_modulus,
                    );
                    let activation = wallet.ironwood_activation()?;
                    let now_height = wallet
                        .sync_state
                        .last_known_chain_height()
                        .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
                    let current_boundary = schedule::boundary_of(
                        schedule::bucket_index(now_height, bucket_modulus),
                        bucket_modulus,
                    );
                    if current_boundary < activation.height() {
                        return Err(MigrationError::ActivationBoundaryPending {
                            retry_after: schedule::first_anchorable_boundary(
                                activation,
                                bucket_modulus,
                            ),
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
                        // current bucket, explicitly immediate.
                        for part in state.parts.iter_mut() {
                            match part.state {
                                PartState::Bound | PartState::Assigned => {
                                    schedule::place_immediate(part, current_bucket)?;
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
                let stranded = plan.stranded;
                let client = self.migration_broadcast_client()?;
                let sent = self.broadcast_due_parts_with(&client).await?;
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
                        stranded,
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
                .build_and_transmit(&round, &sync, |wallet, planned| {
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
    /// calls it under a read guard; the consent brackets of
    /// `start_ironwood_migration` and the immediate path call it under the
    /// same write guard that binds, so the plan hashed and the notes bound
    /// come from one uninterrupted wallet view (issue #2493, finding 11).
    #[allow(clippy::result_large_err)]
    pub(crate) fn plan_ironwood_migration_now(
        &self,
        account: zip32::AccountId,
    ) -> Result<MigrationPlan, crate::wallet::error::WalletError> {
        let params = MigrationParams::provisional(self.chain_type());
        // Note-splitting fees depend on whether the transactions confirm at
        // or after NU6.3 activation (the Orchard bundle's cross-address rules
        // change the action count).
        let post_activation = match (
            self.sync_state.last_known_chain_height(),
            pepper_sync::wallet::PoolActivation::of(
                &self.chain_type(),
                zcash_protocol::ShieldedPool::Ironwood,
            ),
        ) {
            (Some(chain_height), Some(activation)) => chain_height + 1 >= activation.height(),
            _ => false,
        };
        Ok(plan_migration(
            &self.migration_note_values(account)?,
            post_activation,
            &params,
        ))
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

/// The entry gate of the one-call immediate path: decides what an existing
/// migration state means for a new immediate run.
///
/// A consented scheduled migration must not be collapsed into an immediate
/// one — its bucket windows are what the user confirmed — and a different
/// account's migration must not be disturbed; both refuse. A completed
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

#[cfg(test)]
mod tests {
    use pepper_sync::wallet::{NoteInterface as _, OrchardNote, OutputInterface as _};
    use zip32::AccountId;

    use crate::lightclient::LightClient;
    use crate::mocks::broadcast::MockBroadcastClient;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;
    use crate::wallet::migration::{
        BoundNote, ConsentBinding, MigrationMode, MigrationParams, MigrationPhase, MigrationState,
        PartId, PartRecord, PartState, SigningStrategy, schedule,
    };

    use super::{DrainPhase, DrainProgressHandle};

    /// The value of the one fabricated note every scenario here binds a
    /// migration part to.
    const NOTE_VALUE: u64 = 100_000;

    /// The drain-progress side channel: a fresh handle is idle, `begin` arms it,
    /// the per-transaction mutators advance a clone the same way a mobile poll
    /// thread would observe, and every mutator is a no-op once idle — the
    /// property that scopes progress to the immediate drain and leaves the
    /// shared build/transmit primitives untouched for every other caller.
    #[test]
    fn drain_progress_handle_tracks_a_drain() {
        let handle = DrainProgressHandle::default();
        assert_eq!(handle.status(), None, "idle until a drain arms it");

        // A consumer grabs its own clone before the drain starts and must see
        // the same updates (mobile polls this clone on another thread).
        let observer = handle.clone();

        handle.begin(4);
        let armed = observer.status().expect("armed by begin");
        assert_eq!(armed.total, 4);
        assert_eq!(armed.built, 0);
        assert_eq!(armed.sent, 0);
        assert_eq!(armed.phase, DrainPhase::Building);

        handle.set_built(2);
        assert_eq!(observer.status().expect("armed").built, 2);

        handle.enter_transmit();
        assert_eq!(
            observer.status().expect("armed").phase,
            DrainPhase::Transmitting
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

    /// An error raised after the migration state is taken out of the wallet
    /// must not destroy the state. [`SyncState::last_known_chain_height`] is
    /// the end of the last scan range, so it is `None` exactly when the scan
    /// ranges are empty; the two tests here are identical except for the
    /// route into that state, and the pair triangulates. `via_clear_all`
    /// pins that a production path — a rescan — really produces the
    /// dangerous combination of a live migration and no height, and its
    /// guard assertions fail loudly if [`LightWallet::clear_all`] ever
    /// cancels migrations or rebuilds scan ranges eagerly.
    /// `via_empty_sync_state` pins the broadcast path's contract on the
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
        /// [`WalletError::NoSyncData`] is correct and expected; the
        /// consented migration schedule surviving it is what the assertions
        /// pin, because the broadcast path's early `?`-return between take
        /// and restore silently discarded the state, and any later save
        /// persisted the loss.
        async fn broadcast_error_must_preserve_the_state(
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
            let broadcast_client = MockBroadcastClient::default();
            let result = client.broadcast_due_parts_with(&broadcast_client).await;
            assert!(
                matches!(
                    result,
                    Err(LightClientError::WalletError(WalletError::NoSyncData))
                ),
                "the broadcast must fail with NoSyncData, got {result:?}"
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
            broadcast_error_must_preserve_the_state(LightWallet::clear_all).await;
        }

        /// The fabricated route: the state contract alone, independent of
        /// any particular path into it.
        #[tokio::test]
        async fn via_empty_sync_state() {
            broadcast_error_must_preserve_the_state(|wallet| {
                wallet.sync_state = SyncState::new();
            })
            .await;
        }
    }

    /// Offline twin of the libtonode
    /// `unavailable_boundary_tree_state_skips_without_sync` scenario: a due
    /// part whose bucket-boundary checkpoint is absent from the shard tree
    /// is skipped with no writes, no attempt recorded, and nothing
    /// broadcast.
    ///
    /// Limitation: the synthetic wallet FABRICATES the pruned-checkpoint
    /// state — the builder checkpoints the shard trees only at the tip —
    /// so this twin proves the skip logic alone. It cannot prove that
    /// pepper-sync's real pruning produces the state, nor that the
    /// broadcast path performs no hidden synchronization while a reachable
    /// Indexer exists; both belong to the live libtonode twin. The tip
    /// still sits more than the checkpoint retention past the boundary, so
    /// the fabricated state matches one a synced wallet can genuinely
    /// reach.
    #[tokio::test]
    async fn boundary_tree_state_unavailable_skips_the_part() {
        // Past the provisional first bucket boundary (256) by more than
        // pepper-sync's 100-block checkpoint retention.
        const TIP: u32 = 360;

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

        let broadcast_client = MockBroadcastClient::default();
        let sent = client
            .broadcast_due_parts_with(&broadcast_client)
            .await
            .unwrap();
        assert!(sent.is_empty(), "nothing must be broadcast: {sent:?}");
        assert!(
            broadcast_client.submissions.lock().unwrap().is_empty(),
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

    /// The entry gate of the one-call immediate path (issue #2493,
    /// findings 3 and 4): a consented scheduled migration is refused
    /// rather than collapsed into an immediate broadcast, a different
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
    /// whole broadcast pass with a hard error (issue #2493, finding 2):
    /// one bad part previously left every other due part unsent until a
    /// reconcile happened to run.
    mod per_part_conditions_skip {
        use zcash_primitives::transaction::TxId;

        use super::*;
        use crate::wallet::migration::{BoundaryWitness, PrepareResult, SkipReason};

        /// Past the provisional first bucket boundary, as in
        /// [`super::boundary_tree_state_unavailable_skips_the_part`].
        const TIP: u32 = 360;

        /// An assigned part carrying a fabricated boundary witness, so
        /// `prepare_part` reaches the bound-note revalidation instead of
        /// skipping earlier on the missing checkpoint. The revalidation
        /// runs before the witness bytes are parsed, so garbage suffices.
        fn assigned_part_with_witness(bound_note: BoundNote, bucket: u64) -> PartRecord {
            let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
            part.assign(bucket).expect("fresh parts are bound");
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

            let mut part = assigned_part_with_witness(bound_note, bucket);
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

            let mut part = assigned_part_with_witness(diverged, bucket);
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
        fn pre_activation_boundary_skips() {
            let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
            let params = MigrationParams::provisional(wallet.chain_type());

            // Bucket zero's boundary is height zero, below any activation.
            let mut part = assigned_part_with_witness(bound_note, 0);
            let result = wallet
                .prepare_part(AccountId::ZERO, &mut part, &params)
                .expect("a pre-activation boundary is a skip, not an error");
            assert!(matches!(
                result,
                PrepareResult::Skip(SkipReason::BoundaryBeforeActivation { .. })
            ));
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
    async fn spend_evidence_lag_must_not_rebuild_a_broadcast_part() {
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
    /// not catch-up material — broadcasting its stale signature would mine
    /// a permanent lateness fingerprint (cleartext expiry, old anchor)
    /// into its denomination cohort — and it is never silently skipped
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
        // Bucket 1's window [256, 512) closed at tip 700, beyond the slip
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
        // untouched, with no broadcast attempted.
        let sent = client
            .catch_up_migration(std::time::Duration::ZERO)
            .await
            .expect("catch-up runs");
        assert!(sent.is_empty());
        let wallet = client.wallet().read().await;
        let part = &wallet.migration.as_ref().unwrap().parts[0];
        assert_eq!(part.state, PartState::Signed, "the signature is kept");
        assert_eq!(part.attempts, 0, "no lateness fingerprint is broadcast");
    }

    /// Issue #2493, finding 10: `value_migrated` reports the account's
    /// whole confirmed ironwood balance, so ironwood funds from any other
    /// source — shields, ordinary receives — inflate migration progress,
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

    /// The scheduled flow refuses a plan that still needs note splitting
    /// (issue #2493, finding 1): no production code drives the
    /// [`MigrationPhase::NoteSplitting`] phase, so persisting the state
    /// would strand the consent in `Planned` forever while blocking the
    /// drain path with `AlreadyInProgress`.
    #[tokio::test]
    async fn unsplit_plan_is_refused_without_stranding_consent() {
        use crate::lightclient::error::{LightClientError, MigrationError};
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

        let result = client
            .start_ironwood_migration(
                AccountId::ZERO,
                SigningStrategy::LazyAtBoundary,
                plan_hash(&plan),
                None,
            )
            .await;
        assert!(
            matches!(
                result,
                Err(LightClientError::MigrationError(
                    MigrationError::NoteSplittingRequired
                ))
            ),
            "an unsplit plan must be refused: {result:?}"
        );
        let wallet = client.wallet().read().await;
        assert!(
            wallet.migration.is_none(),
            "the refusal must not strand a Planned state"
        );
    }

    /// Open windows at relaunch: a part whose bucket window is *currently
    /// open* (opened before now, not yet closed) is reachable immediately
    /// after a process relaunch, without waiting to become Overdue.
    /// `next_wakes` lists only future buckets and `reconcile` classifies the
    /// open window as OnTrack with no action; the open window belongs to the
    /// third leg, `broadcast_due_parts` (driven at startup or after sync by
    /// `auto_broadcast_if_due`), whose due predicate selects
    /// `bucket_index == current_bucket`.
    #[tokio::test]
    async fn open_window_part_is_broadcast_at_relaunch() {
        use zcash_primitives::transaction::TxId;

        use crate::wallet::migration::{PartClass, next_wakes, reconcile};

        // Tip 300 sits mid-window in bucket 1 of the provisional M = 256:
        // the window [256, 512) opened before "now" and has not closed.
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

        // The two true premises of #2419 review finding 5: the wake listing
        // omits the open window and reconciliation classifies it OnTrack
        // with no action.
        {
            let state = wallet.migration.as_ref().expect("just set");
            assert!(
                next_wakes(&state.parts, now_height, 0, u64::MAX, &params).is_empty(),
                "next_wakes lists future buckets only"
            );
            let report = reconcile(state, &wallet);
            assert_eq!(report.assessments[0].class, PartClass::OnTrack);
            assert!(report.actions.is_empty());
        }

        // The finding's conclusion is nevertheless false: the due-part
        // broadcast path covers the open window at relaunch.
        let mut client = LightClient::new_for_test(wallet).await;
        let broadcast_client = MockBroadcastClient::default();
        let sent = client
            .broadcast_due_parts_with(&broadcast_client)
            .await
            .unwrap();

        assert_eq!(
            sent,
            vec![own_txid],
            "the open-window part broadcasts now instead of slipping to Overdue"
        );
        {
            let submissions = broadcast_client.submissions.lock().unwrap();
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

    /// A scheduled migration rejects the syncing drain *before* the drain
    /// pays for a sync. The client here is offline, so any attempt to sync
    /// first surfaces as [`LightClientError::Offline`] instead of the
    /// pre-condition's [`MigrationError::AlreadyInProgress`] — which is
    /// exactly how this test stays red while the wrapper syncs before
    /// checking, and green once the check runs first. The presynced body
    /// keeps its own post-sync check; this pins the wrapper's early one.
    #[tokio::test]
    async fn scheduled_migration_rejects_drain_before_syncing() {
        let (mut wallet, bound_note) = wallet_with_migration_note(360);
        let params = MigrationParams::provisional(wallet.chain_type());
        let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        part.assign(0).expect("fresh parts are bound");
        wallet.migration = Some(scheduled_state(params, vec![part]));

        let mut client = LightClient::new_for_test(wallet).await;
        let result = client.drain_orchard_to_ironwood(AccountId::ZERO).await;
        assert!(
            matches!(
                result,
                Err(crate::lightclient::error::LightClientError::MigrationError(
                    crate::lightclient::error::MigrationError::AlreadyInProgress
                ))
            ),
            "the drain must reject a scheduled migration without syncing, got {result:?}"
        );
    }
}
