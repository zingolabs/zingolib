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

use std::time::Duration;

use nonempty::NonEmpty;
use zcash_primitives::transaction::TxId;

use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, MigrationError};
use crate::wallet::migration::{
    BroadcastClient, ChainView, ConsentBinding, MaterializeOutcome, MigrationParams,
    MigrationPhase, MigrationPlan, MigrationState, PartId, PartState, RecommendedAction,
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
    pub async fn start_ironwood_migration(
        &mut self,
        account: zip32::AccountId,
        strategy: SigningStrategy,
        consented_plan_hash: [u8; 32],
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

        let params = MigrationParams::provisional(wallet.chain_type());
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
            plan_schedule(&mut state.parts, now_height, &state.params)?;
            state.phase = MigrationPhase::PartsScheduled;
        }
        wallet.migration = Some(state);
        wallet.save_required = true;
        Ok(())
    }

    /// The broadcast-only client parts are submitted through: the dedicated
    /// `migration_broadcast_uri` when configured, the synchronization
    /// endpoint (with a logged correlation warning) otherwise.
    fn migration_broadcast_client(&self) -> broadcast_grpc::GrpcBroadcastClient {
        match &self.migration_broadcast_uri {
            Some(uri) => broadcast_grpc::GrpcBroadcastClient::new(uri.clone()),
            None => {
                log::warn!(
                    "no dedicated migration broadcast endpoint configured; parts will be \
                     broadcast to the synchronization endpoint, which lets that server \
                     correlate synchronization with migration activity"
                );
                broadcast_grpc::GrpcBroadcastClient::new(self.indexer_uri().clone())
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
        let client = self.migration_broadcast_client();
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
    async fn broadcast_due_parts_selected(
        &mut self,
        client: &impl BroadcastClient,
        only: Option<PartId>,
    ) -> Result<Vec<TxId>, LightClientError> {
        let mut wallet = self.wallet().write().await;
        let Some(mut state) = wallet.migration.take() else {
            return Err(MigrationError::NoMigration.into());
        };

        let result = async {
            let now_height = wallet
                .sync_state
                .last_known_chain_height()
                .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
            let current_bucket = schedule::bucket_index(now_height, state.params.bucket_modulus);

            let mut sent = Vec::new();
            for index in 0..state.parts.len() {
                let due = {
                    let part = &state.parts[index];
                    matches!(part.state, PartState::Assigned | PartState::Signed)
                        && part.bucket_index == Some(current_bucket)
                        && only.is_none_or(|part_id| part.id == part_id)
                };
                if !due {
                    continue;
                }

                let raw_tx = if state.parts[index].state == PartState::Assigned {
                    let account = state.account;
                    let strategy = state.strategy;
                    let params = state.params.clone();
                    match wallet.materialize_part(
                        account,
                        &mut state.parts[index],
                        strategy,
                        &params,
                    )? {
                        MaterializeOutcome::Materialized { raw_tx, .. } => raw_tx,
                        MaterializeOutcome::Skip(reason) => {
                            log::info!(
                                "skipping part {index}: {reason:?}; it falls to reconciliation"
                            );
                            continue;
                        }
                    }
                } else {
                    // Signed already (an earlier submit failed): recover the
                    // bytes from the blob or the wallet's transaction record.
                    let part = &state.parts[index];
                    match &part.signed_blob {
                        Some(blob) => blob.clone(),
                        None => {
                            let txid = part.txid.expect("signed parts have txids");
                            let transaction = wallet.wallet_transactions.get(&txid).ok_or(
                                crate::wallet::error::WalletError::TransactionNotFound(txid),
                            )?;
                            let mut bytes = Vec::new();
                            transaction
                                .transaction()
                                .write(&mut bytes)
                                .map_err(crate::wallet::error::WalletError::TransactionWrite)?;
                            bytes
                        }
                    }
                };

                // The attempt is recorded before submission so a crash in
                // between is detectable (the mined part is then promoted via
                // its nullifier on reconciliation).
                state.parts[index].record_attempt();
                wallet.save_required = true;

                let expiry_height = state.parts[index]
                    .expiry_height
                    .expect("signed parts have expiry heights");
                match client.submit(raw_tx, expiry_height).await {
                    Ok(_) => {
                        state.parts[index].mark_broadcast()?;
                        wallet.save_required = true;
                        sent.push(state.parts[index].txid.expect("signed parts have txids"));
                    }
                    Err(e) => {
                        log::warn!("part submission failed, leaving the part signed: {e}");
                        break;
                    }
                }
            }
            Ok(sent)
        }
        .await;

        wallet.migration = Some(state);
        result
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
        let Some(mut state) = wallet.migration.take() else {
            return Err(MigrationError::NoMigration.into());
        };

        let result = (|| {
            let report = reconcile(&state, &*wallet);
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
                        wallet.bind_parts_to_notes(&mut state, account)?;
                        let now_height = wallet
                            .sync_state
                            .last_known_chain_height()
                            .ok_or(crate::wallet::error::WalletError::NoSyncData)?;
                        plan_schedule(&mut state.parts, now_height, &state.params)?;
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
            Ok(report)
        })();

        wallet.migration = Some(state);
        result
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
            let Some(mut state) = wallet.migration.take() else {
                return Err(MigrationError::NoMigration.into());
            };
            let shift = (|| -> Result<(), crate::wallet::error::WalletError> {
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
                Ok(())
            })();
            wallet.migration = Some(state);
            wallet.save_required = true;
            shift?;
        }
        self.wallet().write().await.refresh_part_witnesses()?;

        let client = self.migration_broadcast_client();
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
                    (
                        Some(state.phase.clone()),
                        state.parts.len() as u32,
                        confirmed.len() as u32,
                        state.parts.iter().map(|part| part.denomination).sum(),
                        confirmed.iter().map(|part| part.denomination).sum(),
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

                let client = self.migration_broadcast_client();
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
            let _ignore_error = self.pause_sync();
            let mut round_txids = Vec::new();
            for planned in &round {
                round_txids.push(
                    self.wallet()
                        .write()
                        .await
                        .build_note_split_transaction(account, planned)?,
                );
            }
            self.transmit_transactions(
                NonEmpty::from_vec(round_txids.clone())
                    .expect("note-splitting round has at least one transaction"),
            )
            .await?;
            let _ignore_error = self.resume_sync();
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
