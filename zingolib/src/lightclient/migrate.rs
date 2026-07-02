//! Orchard→Ironwood migration orchestration.
//!
//! [`LightClient::migrate_to_ironwood`] drives a full migration in one call:
//! conditioning rounds (each awaiting confirmation) followed by the drain.
//! It is a thin loop over the same public pieces a mobile client uses to
//! coordinate the steps itself — [`LightClient::plan_ironwood_migration`],
//! [`crate::wallet::LightWallet::build_conditioning_transaction`], and
//! [`crate::wallet::LightWallet::build_drain_transaction`] — so behavior
//! cannot diverge between the one-call and the caller-driven paths.
//!
//! Per-transfer scheduling (the migration ZIP's anchor-height buckets and
//! de-correlated broadcast windows) stays caller-side: this loop broadcasts
//! each drain transaction as soon as it is built.

use std::time::Duration;

use nonempty::NonEmpty;
use zcash_primitives::transaction::TxId;

use crate::lightclient::LightClient;
use crate::lightclient::error::LightClientError;
use crate::wallet::ironwood_migration::{MigrationPlan, plan_migration};

/// How long to wait between sync polls while a conditioning round confirms.
const CONFIRMATION_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Give up waiting for a conditioning round after this many polls.
const MAX_CONFIRMATION_POLLS: usize = 720;
/// A migration replans after every round; a real plan converges in
/// `~log_K(N)` rounds, so far more than this means something is wrong.
const MAX_ROUNDS: usize = 64;

/// The transactions of a completed migration.
#[derive(Debug, Clone)]
pub struct MigrationSummary {
    /// Conditioning (Orchard→Orchard) transactions, in broadcast order.
    pub conditioning_txids: Vec<TxId>,
    /// Drain (Orchard→Ironwood) transactions, one per denomination.
    pub drain_txids: Vec<TxId>,
    /// Dust value (zatoshis) left unmigrated in the Orchard pool.
    pub stranded: u64,
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
        // Conditioning fees depend on whether the transactions confirm at or
        // after NU6.3 activation (the Orchard bundle's cross-address rules
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
        ))
    }

    /// Runs a full Orchard→Ironwood migration: executes conditioning rounds
    /// (waiting for each round to confirm), then broadcasts one canonical
    /// drain transaction per denomination.
    ///
    /// Replans from wallet state before every round, so a migration
    /// interrupted by external spends, expiry or restart picks up where the
    /// notes actually are.
    pub async fn migrate_to_ironwood(
        &mut self,
        account: zip32::AccountId,
    ) -> Result<MigrationSummary, LightClientError> {
        let mut conditioning_txids = Vec::new();

        for _ in 0..MAX_ROUNDS {
            self.sync_and_await().await?;
            let plan = self.plan_ironwood_migration(account).await?;

            if plan.is_conditioned() {
                let mut drain_txids = Vec::new();
                let _ignore_error = self.pause_sync();
                for denomination in &plan.drain {
                    let txid = self
                        .wallet()
                        .write()
                        .await
                        .build_drain_transaction(account, *denomination)?;
                    self.transmit_transactions(NonEmpty::new(txid)).await?;
                    drain_txids.push(txid);
                }
                let _ignore_error = self.resume_sync();
                return Ok(MigrationSummary {
                    conditioning_txids,
                    drain_txids,
                    stranded: plan.stranded,
                });
            }

            let round = plan
                .conditioning_rounds
                .into_iter()
                .next()
                .expect("unconditioned plan has at least one round");
            let _ignore_error = self.pause_sync();
            let mut round_txids = Vec::new();
            for planned in &round {
                round_txids.push(
                    self.wallet()
                        .write()
                        .await
                        .build_conditioning_transaction(account, planned)?,
                );
            }
            self.transmit_transactions(
                NonEmpty::from_vec(round_txids.clone())
                    .expect("conditioning round has at least one transaction"),
            )
            .await?;
            let _ignore_error = self.resume_sync();
            conditioning_txids.extend(round_txids.iter().copied());

            self.await_migration_confirmations(&round_txids).await?;
        }

        Err(LightClientError::MigrationError(format!(
            "conditioning did not converge within {MAX_ROUNDS} rounds"
        )))
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
                return Err(LightClientError::MigrationError(format!(
                    "conditioning transaction {txid} failed or disappeared"
                )));
            }
            if all_confirmed {
                return Ok(());
            }
            tokio::time::sleep(CONFIRMATION_POLL_INTERVAL).await;
        }
        Err(LightClientError::MigrationError(
            "timed out waiting for conditioning transactions to confirm".to_string(),
        ))
    }
}
