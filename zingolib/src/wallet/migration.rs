//! Orchard→Ironwood migration backend, implementing the two-phase design of
//! ZIP 318 (`draft-schell-ironwood-migration`).
//!
//! **Phase 1, note splitting**: Orchard→Orchard self-sends (both ends
//! shielded, no value revealed) resize the wallet's Orchard notes to exactly
//! `denomination + part fee`. Fragmented wallets are folded together with a
//! k-ary batched reduction (at most [`MigrationParams::max_actions_per_split_tx`]
//! notes merged per transaction, `~log_K(N)` rounds). Large notes are divided.
//! Note splitting may run before the NU6.3 activation.
//!
//! **Phase 2, migration transactions**: one canonical pool-crossing transfer
//! per split note, called a *part*. Each part has exactly one Ironwood output
//! whose value is a single power-of-ten denomination, **no change output**,
//! and the canonical ZIP-317 fee. Because every split note is pre-sized, no
//! part waits on change from an earlier one, and none emits a
//! wallet-fingerprinting output.
//!
//! The denominations are canonical so that migrated amounts collide across the
//! whole migrating population instead of fingerprinting a wallet:
//!
//! * Denominations are {0.001, 0.01, 0.1, 1, 10, 100} ZEC (powers of ten,
//!   capped at `DENOM_CAP`, floored at `DUST_FLOOR`).
//! * A balance is decomposed by decimal digit expansion, largest first.
//! * Residue below the dust floor folds into fees. Notes worth at most
//!   [`MigrationParams::sweep_min`] are stranded (moving them costs more than
//!   they are worth).
//!
//! [`plan_migration`] is the pure planning entry point. It is deterministic
//! and never touches the network, so callers (the one-call
//! `migrate_to_ironwood` orchestration, or a mobile client scheduling steps
//! itself) can present the whole plan for consent before anything is sent.
//!
//! ZIP 318 also permits an **immediate** migration, a single transfer with no
//! delay and minimal privacy, as an explicit alternative the user may choose
//! over the private path above. That is [`immediate`], which shares nothing with
//! this two-phase design but the transaction builder.

pub mod broadcast;
pub mod immediate;
pub mod params;
pub mod parts;
pub mod quantize;
pub mod reconcile;
pub mod schedule;
pub mod split;
pub mod store;

pub use broadcast::{BroadcastClient, BroadcastError};
pub use immediate::{ImmediateMigrationPlan, ImmediateMigrationTx, immediate_migration_fee};
pub use params::MigrationParams;
pub use parts::{
    BoundNote, BoundaryWitness, MaterializeOutcome, PartId, PartRecord, PartState, PrepareResult,
    SigningStrategy, SkipReason,
};
pub use quantize::{Denominations, decompose};
pub use reconcile::{
    ChainView, PartClass, RecommendedAction, ReconcileReport, due_now_parts, reconcile,
};
pub use schedule::{
    BroadcastWindow, WindowReport, estimated_unix_at, part_in_current_bucket, plan_schedule,
    upcoming_windows, window_timeline,
};
pub use split::{
    CANONICAL_PART_FEE, MigrationPlan, NoteSplitTx, note_split_fee, part_denomination, plan_hash,
    plan_migration,
};

use zcash_primitives::transaction::TxId;

/// The consent captured when the user confirmed the migration schedule
/// (ZIP 318 requires the whole schedule to be confirmed before any transfer
/// is sent). A replan under different parameters or a different plan produces
/// different hashes and therefore requires fresh consent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsentBinding {
    /// [`MigrationParams::params_hash`] at consent time.
    pub params_hash: [u8; 32],
    /// [`plan_hash`] of the consented plan.
    pub plan_hash: [u8; 32],
    /// Unix time the user confirmed, in seconds.
    pub consented_at: u64,
}

/// Which flow created and drives this migration.
///
/// The distinction is consent-bearing: a [`Scheduled`](Self::Scheduled)
/// migration's bucket windows are what the user confirmed, so the
/// immediate path must refuse to collapse them, while an
/// [`Immediate`](Self::Immediate) migration may be resumed and re-driven
/// by another immediate call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationMode {
    /// The consent-scheduled flow of `start_ironwood_migration`: parts
    /// broadcast inside their consented bucket windows.
    Scheduled,
    /// The one-call interactive flow of `migrate_to_ironwood`: everything
    /// sends now, with the disclosed correlation.
    Immediate,
}

/// The coarse stage the migration is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationPhase {
    /// Consent captured, nothing sent yet.
    Planned,
    /// Executing note-splitting rounds.
    NoteSplitting {
        /// The round currently awaiting confirmation, counted from zero.
        round: u32,
        /// Txids of that round's transactions.
        pending_txids: Vec<TxId>,
    },
    /// Splitting complete, parts bound and scheduled.
    PartsScheduled,
    /// Every part confirmed. Only the disclosed residual remains in Orchard.
    Complete {
        /// Unmigrated value left in the Orchard pool, in zatoshis.
        residual: u64,
    },
}

/// Everything the migration persists in the wallet file. Deliberately
/// wallet-file-local: restore-from-seed sees only the remaining Orchard
/// balance and offers a fresh migration, the reinstall path ZIP 318
/// prescribes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationState {
    /// The parameter set the plan and schedule were computed under.
    pub params: MigrationParams,
    /// The consent binding (FR7).
    pub consent: ConsentBinding,
    /// How Phase 2 transactions are signed.
    pub strategy: SigningStrategy,
    /// Which flow created and drives this migration.
    pub mode: MigrationMode,
    /// The account being migrated.
    pub account: zip32::AccountId,
    /// The coarse stage.
    pub phase: MigrationPhase,
    /// The part records. Empty until note splitting completes and
    /// `bind_parts_to_notes` runs.
    pub parts: Vec<PartRecord>,
}

impl MigrationState {
    /// The notes soft-reserved for pending parts: excluded from default input
    /// selection for ordinary sends, without ever blocking a spend the user
    /// insists on (an insistent spend consumes them and the affected parts
    /// are invalidated on the next reconciliation).
    pub fn reserved_output_ids(&self) -> Vec<pepper_sync::wallet::OutputId> {
        self.parts
            .iter()
            .filter(|part| {
                matches!(
                    part.state,
                    PartState::Bound
                        | PartState::Assigned
                        | PartState::Signed
                        | PartState::Broadcast
                )
            })
            .filter_map(|part| part.note.map(|note| note.output_id))
            .collect()
    }
}

impl crate::wallet::LightWallet {
    /// The take/use/restore bracket for the wallet's [`MigrationState`],
    /// made total. `f` receives the wallet and the state as two independent
    /// `&mut` borrows — the reason the state must leave the wallet at all —
    /// and the state is restored on every exit path before `f`'s result is
    /// returned: an early `?`-return inside `f` cannot skip the restore,
    /// and because `f` is synchronous the state is never out of the wallet
    /// across an `.await` point, so a cancelled future cannot strand it.
    /// The bracket itself is unobservable — the migration slot is `Some`
    /// after exactly when it was `Some` before, and every mutation belongs
    /// to `f`. Returns `None`, without calling `f`, when no migration is
    /// active; the caller chooses what a missing migration means.
    pub(crate) fn with_migration_state<R>(
        &mut self,
        f: impl FnOnce(&mut Self, &mut MigrationState) -> R,
    ) -> Option<R> {
        let mut state = self.migration.take()?;
        let result = f(self, &mut state);
        self.migration = Some(state);
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    /// Structural decoupling lint (ZIP 318: a broadcast session must never
    /// synchronize). The migration modules that run on the broadcast path
    /// must have no dependency edge to the sync engine or the network
    /// client. The only permitted pepper-sync surface is wallet data types.
    #[test]
    fn broadcast_path_has_no_sync_dependency() {
        let forbidden = [
            "pepper_sync::sync",
            "sync_and_await",
            "pause_sync",
            "resume_sync",
            "GrpcIndexer",
            "zingo_netutils",
        ];
        for module in ["parts", "broadcast", "schedule", "store", "reconcile"] {
            let path = format!(
                "{}/src/wallet/migration/{module}.rs",
                env!("CARGO_MANIFEST_DIR")
            );
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue; // modules land tranche by tranche
            };
            for needle in forbidden {
                assert!(
                    !source.contains(needle),
                    "{path} references `{needle}`: the migration broadcast path must remain \
                     decoupled from the sync engine"
                );
            }
        }
    }
}
