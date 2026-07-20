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

use std::time::Duration;

use nonempty::NonEmpty;
use zcash_primitives::transaction::TxId;

use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, MigrationError};
use zcash_protocol::consensus::BlockHeight;

use crate::wallet::LightWallet;
use crate::wallet::error::WalletError;
use crate::wallet::migration::{
    BroadcastClient, ChainView, ConsentBinding, DrainPlan, MigrationParams, MigrationPhase,
    MigrationPlan, MigrationState, PartId, PartState, PrepareResult, RecommendedAction,
    ReconcileReport, SigningStrategy, WakePoint, plan_hash, plan_migration, plan_schedule,
    reconcile, schedule,
};

pub mod broadcast_grpc;

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
        use zcash_protocol::consensus::{NetworkUpgrade, Parameters as _};

        let wallet = self.wallet().read().await;
        let params = MigrationParams::provisional(wallet.chain_type());
        // Note-splitting fees depend on whether the transactions confirm at
        // or after NU6.3 activation (the Orchard bundle's cross-address rules
        // change the action count).
        let post_activation = match (
            wallet.sync_state.last_known_chain_height(),
            wallet.chain_type().activation_height(NetworkUpgrade::Nu6_3),
        ) {
            (Some(chain_height), Some(activation)) => chain_height + 1 >= activation,
            _ => false,
        };
        Ok(plan_migration(
            &wallet.migration_note_values(account)?,
            post_activation,
            &params,
        ))
    }

    /// Records the user's consent to a proposed migration plan and persists
    /// the migration state (ZIP 318 requires the whole schedule confirmed
    /// before any transfer is sent).
    ///
    /// `consented_plan_hash` is the [`plan_hash`] of the plan the user was
    /// shown. If the wallet's notes changed in between, the call fails and
    /// the client re-plans. When the note set is already fully split, parts
    /// are bound to their notes and scheduled immediately. Otherwise the
    /// migration starts in the [`MigrationPhase::Planned`] phase and note
    /// splitting proceeds from there.
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

        let plan = self.plan_ironwood_migration(account).await?;
        let hash = plan_hash(&plan);
        if hash != consented_plan_hash {
            return Err(MigrationError::ConsentStale.into());
        }

        let mut wallet = self.wallet().write().await;
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
            account,
            phase: MigrationPhase::Planned,
            parts: Vec::new(),
        };
        if plan.is_split() {
            wallet.bind_parts_to_notes(&mut state, account)?;
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
            plan_schedule(
                &mut state.parts,
                now_height,
                &state.params,
                &mut rand::rngs::OsRng,
            )?;
            state.phase = MigrationPhase::PartsScheduled;
        }
        wallet.migration = Some(state);
        wallet.save_required = true;
        Ok(())
    }

    /// The broadcast-only client parts are submitted through: the dedicated
    /// `migration_broadcast_uri` when configured (independent of the Indexer
    /// connection), the synchronization endpoint (with a logged correlation
    /// warning) otherwise. With neither endpoint configured the client emits
    /// no network traffic and this returns [`LightClientError::Offline`].
    fn migration_broadcast_client(
        &self,
    ) -> Result<broadcast_grpc::GrpcBroadcastClient, LightClientError> {
        match &self.migration_broadcast_uri {
            Some(uri) => Ok(broadcast_grpc::GrpcBroadcastClient::new(uri.clone())),
            None => {
                let indexer_uri = self.indexer_uri().ok_or(LightClientError::Offline)?;
                log::warn!(
                    "no dedicated migration broadcast endpoint configured; parts will be \
                     broadcast to the synchronization endpoint, which lets that server \
                     correlate synchronization with migration activity"
                );
                Ok(broadcast_grpc::GrpcBroadcastClient::new(indexer_uri))
            }
        }
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
            match client.submit(raw_tx, expiry_height).await {
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
                            let next_bucket =
                                schedule::bucket_index(now_height, state.params.bucket_modulus) + 1;
                            part.reassign(next_bucket)?;
                        }
                        RecommendedAction::BindAndSchedule => {
                            let account = state.account;
                            wallet.bind_parts_to_notes(state, account)?;
                            let now_height = wallet
                                .sync_state
                                .last_known_chain_height()
                                .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
                            plan_schedule(
                                &mut state.parts,
                                now_height,
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
                            part.shift(current_bucket)?;
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
                    let value_migrated = wallet
                        .account_balance(state.account)
                        .ok()
                        .and_then(|b| b.confirmed_ironwood_balance)
                        .map(zcash_protocol::value::Zatoshis::into_u64)
                        .unwrap_or(0);
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
        self.sync_and_await().await?;
        self.drain_orchard_to_ironwood_presynced(account).await
    }

    /// Broadcasts the immediate Orchard→Ironwood drain against the wallet's
    /// *current* state, without syncing first.
    ///
    /// This is [`Self::drain_orchard_to_ironwood`] minus the leading
    /// `sync_and_await`, for consumers that own the sync lifecycle and keep a
    /// background sync running continuously (e.g. zingo-mobile). Calling the
    /// syncing variant from such a consumer collides with the running sync
    /// and fails with [`pepper_sync::error::SyncModeError::SyncAlreadyRunning`];
    /// this entry point lets the caller drive sync itself, matching the
    /// `send`/`shield` mutation paths.
    ///
    /// The caller is responsible for keeping the wallet synced before calling.
    /// Everything else — the plan, the `pause_sync`/`resume_sync` bracketing
    /// around build and transmit, the chunked broadcast, and the idempotent
    /// cleanup on partial failure — is identical to the syncing variant.
    pub async fn drain_orchard_to_ironwood_presynced(
        &mut self,
        account: zip32::AccountId,
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

        let txids = self
            .build_and_transmit(&plan.transactions, |wallet, planned| {
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
    /// a paused sync, enforcing the shared cleanup contract: a build failure
    /// fails the transactions already built, and a transmit failure fails
    /// every transaction still unsent, so no note stays spent by a
    /// transaction that will never reach the network. Both the drain flow and
    /// the note-splitting rounds send through here.
    async fn build_and_transmit<T>(
        &mut self,
        planned: &[T],
        build: impl Fn(&mut LightWallet, &T) -> Result<TxId, WalletError>,
    ) -> Result<Vec<TxId>, LightClientError> {
        let _ignore_error = self.pause_sync();
        let built = self.build_transactions(planned, build).await;
        let txids = match built {
            Ok(txids) => txids,
            Err(e) => {
                let _ignore_error = self.resume_sync();
                return Err(e);
            }
        };

        let transmitted = self
            .transmit_transactions(
                NonEmpty::from_vec(txids.clone()).expect("planned batches are never empty"),
            )
            .await;
        let _ignore_error = self.resume_sync();

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
                Ok(txid) => txids.push(txid),
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
        let mut split_txids = Vec::new();
        let mut part_txids = Vec::new();

        for _ in 0..MAX_ROUNDS {
            self.sync_and_await().await?;
            let plan = self.plan_ironwood_migration(account).await?;

            if plan.is_split() {
                let stranded = plan.stranded;
                // Invoking the one-call constitutes consent to the current
                // plan. Record the binding if this is a fresh migration.
                {
                    let mut wallet = self.wallet().write().await;
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
                        // current bucket.
                        for part in state.parts.iter_mut() {
                            match part.state {
                                PartState::Bound => part.assign(current_bucket)?,
                                PartState::Assigned => part.shift(current_bucket)?,
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
            let round_txids = self
                .build_and_transmit(&round, |wallet, planned| {
                    wallet.build_note_split_transaction(account, planned)
                })
                .await?;
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

#[cfg(test)]
mod tests {
    use pepper_sync::wallet::{NoteInterface as _, OrchardNote, OutputInterface as _};
    use zip32::AccountId;

    use crate::lightclient::LightClient;
    use crate::mocks::broadcast::MockBroadcastClient;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
    use crate::wallet::LightWallet;
    use crate::wallet::migration::{
        BoundNote, ConsentBinding, MigrationParams, MigrationPhase, MigrationState, PartId,
        PartRecord, PartState, SigningStrategy, schedule,
    };

    /// The value of the one fabricated note every scenario here binds a
    /// migration part to.
    const NOTE_VALUE: u64 = 100_000;

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
}
