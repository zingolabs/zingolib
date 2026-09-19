//! Orchard→Ironwood migration backend, implementing the two-phase design of
//! ZIP 318 (`draft-schell-ironwood-migration`).
//!
//! **Phase 1, note preparation**: Orchard→Orchard self-sends (both ends
//! shielded, no value revealed) resize the wallet's Orchard notes to exactly
//! `denomination + transfer fee`. Fragmented wallets are folded together with a
//! k-ary batched reduction (at most [`MigrationParams::max_actions_per_split_tx`]
//! notes merged per transaction, `~log_K(N)` rounds). Large notes are divided.
//! Note preparation may run before the NU6.3 activation.
//!
//! **Phase 2, migration transactions**: one canonical pool-crossing transfer
//! per funding note, called a *transfer*. Each transfer has exactly one Ironwood output
//! whose value is a single power-of-ten denomination, **no change output**,
//! and the canonical ZIP-317 fee. Because every funding note is pre-sized, no
//! transfer waits on change from an earlier one, and none emits a
//! wallet-fingerprinting output.
//!
//! The denominations are canonical so that migrated amounts collide across the
//! whole migrating population instead of fingerprinting a wallet
//! (<https://zips.z.cash/zip-0318#amountselectioncanonicalquantization>):
//!
//! * Denominations are the values `n x 10^k` ZEC with `n` in {1, 2, 5},
//!   from `MAX_RESIDUAL_VALUE` (0.01 ZEC) up to `DENOM_CAP` (10 000 ZEC),
//!   both imported from the `zcash_pool_migration` reference crate.
//! * A balance is decomposed greedily, largest denomination first.
//! * Balance below `MAX_RESIDUAL_VALUE` stays unmigrated as the residual.
//!   Notes worth at most [`MigrationParams::sweep_min`] are stranded
//!   (moving them costs more than they are worth; the ZIP leaves that
//!   economics to ZIP 317 and standardizes no sweep threshold).
//!
//! [`plan_migration`] is the pure planning entry point. It is deterministic
//! and never touches the network, so a consumer can present the whole plan
//! for consent before anything is sent.
//!
//! ZIP 318 also permits an **immediate** migration, a single transfer with no
//! delay and minimal privacy, as an explicit alternative the user may choose
//! over the private path above. That is the `immediate` module, which shares nothing with
//! this two-phase design but the transaction builder.

// The submodules carry a crate ceiling: the re-export block below is this
// module's whole public surface, so a new public name is a deliberate act
// here rather than a side effect of `pub` in a submodule. Two modules stay
// public because the mobile consumer imports through their paths
// (`transfers::TransferState`, `transfers::SigningStrategy`, `split::plan_hash`);
// their items are tightened individually instead.
pub(crate) mod broadcast;
pub(crate) mod immediate;
pub(crate) mod params;
pub mod preparation;
pub(crate) mod quantize;
pub(crate) mod reconcile;
pub(crate) mod schedule;
pub(crate) mod store;
pub mod transfers;

pub use broadcast::{BroadcastClient, BroadcastReceipt, BroadcastRoute, TransferBroadcastError};
pub use immediate::{ImmediateMigrationPlan, ImmediateMigrationTx};
pub use params::InvalidMigrationParams;
pub use params::MigrationParams;
pub use preparation::{
    NoteClass, PreparationTx, ScheduledMigrationPlan, classify_note, plan_hash, plan_migration,
};
pub use quantize::{Denominations, decompose};
pub use reconcile::{
    ChainView, RecommendedAction, ReconcileReport, TransferAssessment, TransferClass,
    due_now_transfers, reconcile,
};
pub use schedule::{BroadcastWindow, WindowReport, bucket_index};
pub(crate) use schedule::{plan_schedule, upcoming_windows, window_timeline};
pub use transfers::{
    BoundNote, BoundaryWitness, BuildResult, SigningStrategy, TransferId, TransferRecord,
    TransferState,
};

use zcash_primitives::transaction::TxId;

/// The plan and parameter hashes recorded when a migration starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanCommitment {
    /// [`MigrationParams::params_hash`] at start time.
    pub params_hash: [u8; 32],
    /// [`plan_hash`] of the committed plan.
    pub plan_hash: [u8; 32],
    /// Unix time of the start call, in seconds.
    pub committed_at: u64,
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
    /// The consent-scheduled flow of `commit_migration`: transfers
    /// broadcast inside their consented bucket windows.
    Scheduled,
    /// The immediate flow of `migrate_immediately`: everything sends now,
    /// with the disclosed correlation.
    Immediate,
}

/// The coarse stage the migration is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationPhase {
    /// The plan is committed. Nothing is sent yet.
    Committed,
    /// Executing note-preparation rounds.
    Preparing {
        /// The round currently awaiting confirmation, counted from zero.
        round: u32,
        /// Txids of that round's transactions.
        pending_txids: Vec<TxId>,
    },
    /// Note preparation is complete. The schedule is not committed yet.
    Prepared,
    /// The schedule is committed. The transfers are bound and scheduled.
    Scheduled,
    /// Every transfer is terminal. Only the disclosed residual remains in Orchard.
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
    pub(crate) params: MigrationParams,
    /// The consent binding (FR7).
    pub(crate) commitment: PlanCommitment,
    /// How Phase 2 transactions are signed.
    pub(crate) strategy: SigningStrategy,
    /// Which flow created and drives this migration.
    pub(crate) mode: MigrationMode,
    /// The account being migrated.
    pub(crate) account: zip32::AccountId,
    /// The coarse stage.
    pub(crate) phase: MigrationPhase,
    /// The transfer records. Empty until note preparation completes and
    /// `bind_transfers_to_notes` runs.
    pub(crate) transfers: Vec<TransferRecord>,
}

impl MigrationState {
    pub fn commitment(&self) -> &PlanCommitment {
        &self.commitment
    }

    pub fn params(&self) -> &MigrationParams {
        &self.params
    }

    pub fn strategy(&self) -> SigningStrategy {
        self.strategy
    }

    pub fn mode(&self) -> MigrationMode {
        self.mode
    }

    pub fn account(&self) -> zip32::AccountId {
        self.account
    }

    pub fn phase(&self) -> &MigrationPhase {
        &self.phase
    }

    pub fn transfers(&self) -> &[TransferRecord] {
        &self.transfers
    }

    #[cfg(any(test, feature = "testutils"))]
    pub fn scheduled_for_tests(
        params: MigrationParams,
        transfers: Vec<TransferRecord>,
        account: zip32::AccountId,
    ) -> Self {
        MigrationState {
            commitment: PlanCommitment {
                params_hash: params.params_hash(),
                plan_hash: [0; 32],
                committed_at: 0,
            },
            params,
            strategy: SigningStrategy::LazyAtBoundary,
            mode: MigrationMode::Scheduled,
            account,
            phase: MigrationPhase::Scheduled,
            transfers,
        }
    }
}

impl MigrationState {
    fn bound_output_ids(&self) -> Vec<pepper_sync::wallet::OutputId> {
        self.transfers
            .iter()
            .filter(|transfer| {
                matches!(
                    transfer.state,
                    TransferState::Bound
                        | TransferState::Assigned
                        | TransferState::Signed
                        | TransferState::Broadcast
                        | TransferState::Expired
                ) || (transfer.state == TransferState::Released && transfer.txid.is_some())
            })
            .filter_map(|transfer| transfer.note.map(|note| note.output_id))
            .collect()
    }
}

impl crate::wallet::LightWallet {
    /// The notes reserved for the migration. An ordinary send never selects
    /// them. While the plan is committed and the notes are not yet bound,
    /// every unspent pre-Ironwood Orchard note of the migration account is
    /// reserved. Once the schedule is committed, only the funding notes of
    /// the pending transfers are.
    pub fn reserved_output_ids(&self) -> Vec<pepper_sync::wallet::OutputId> {
        use pepper_sync::wallet::{KeyIdInterface as _, NoteInterface as _, OutputInterface as _};

        let Some(state) = &self.migration else {
            return Vec::new();
        };
        match state.phase {
            MigrationPhase::Committed
            | MigrationPhase::Preparing { .. }
            | MigrationPhase::Prepared => self
                .wallet_transactions
                .values()
                .filter(|transaction| !transaction.status().is_failed())
                .flat_map(|transaction| {
                    pepper_sync::wallet::OrchardNote::transaction_outputs(transaction)
                })
                .filter(|note| {
                    note.key_id().account_id() == state.account
                        && note.note().version() == orchard::note::NoteVersion::V2
                        && note.spending_transaction().is_none()
                })
                .map(|note| note.output_id())
                .collect(),
            MigrationPhase::Scheduled => state.bound_output_ids(),
            MigrationPhase::Complete { .. } => Vec::new(),
        }
    }

    /// The value of the reserved notes, in zatoshis.
    pub fn reserved_orchard_value(&self) -> u64 {
        use pepper_sync::wallet::OutputInterface as _;

        let reserved = self.reserved_output_ids();
        self.wallet_transactions
            .values()
            .flat_map(|transaction| {
                pepper_sync::wallet::OrchardNote::transaction_outputs(transaction)
            })
            .filter(|note| reserved.contains(&note.output_id()))
            .map(|note| note.value())
            .sum()
    }

    /// The take/use/restore bracket for the wallet's [`MigrationState`],
    /// made total. `f` receives the wallet and the state as two independent
    /// `&mut` borrows (the reason the state must leave the wallet at all),
    /// and the state is restored on every exit path before `f`'s result is
    /// returned: an early `?`-return inside `f` cannot skip the restore,
    /// and because `f` is synchronous the state is never out of the wallet
    /// across an `.await` point, so a cancelled future cannot strand it.
    /// The bracket itself is unobservable: the migration slot is `Some`
    /// after exactly when it was `Some` before, and every mutation belongs
    /// to `f`. Returns `None`, without calling `f`, when no migration is
    /// active. The caller chooses what a missing migration means.
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
    /// Structural decoupling lint (ZIP 318: a transfer-transmission session must
    /// never synchronize). The migration modules that run on the transmission
    /// path must have no dependency edge to the sync engine or the network
    /// client. The only permitted pepper-sync surface is wallet data types.
    #[test]
    fn transmission_path_has_no_sync_dependency() {
        let forbidden = [
            "pepper_sync::sync",
            "sync_and_await",
            "pause_sync",
            "resume_sync",
            "GrpcIndexer",
            "zingo_netutils",
        ];
        for module in ["transfers", "broadcast", "schedule", "store", "reconcile"] {
            let path = format!(
                "{}/src/wallet/migration/{module}.rs",
                env!("CARGO_MANIFEST_DIR")
            );
            let source = std::fs::read_to_string(&path).expect("the module exists");
            for needle in forbidden {
                assert!(
                    !source.contains(needle),
                    "{path} references `{needle}`: the migration transmission path must remain \
                     decoupled from the sync engine"
                );
            }
        }
    }

    mod reservation {
        use pepper_sync::wallet::{
            NoteInterface as _, OrchardNote, OutputId, OutputInterface as _,
        };
        use zcash_primitives::transaction::TxId;
        use zcash_protocol::consensus::BlockHeight;

        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
        use crate::wallet::LightWallet;
        use crate::wallet::migration::{
            BoundNote, MigrationMode, MigrationParams, MigrationPhase, MigrationState,
            PlanCommitment, SigningStrategy, TransferId, TransferRecord, TransferState,
        };

        const FUNDING_NOTE: u64 = 1_020_000;
        const RESIDUAL_NOTE: u64 = 50_000;
        const IRONWOOD_NOTE: u64 = 70_000;

        fn wallet() -> LightWallet {
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(FUNDING_NOTE)
                .orchard_note(RESIDUAL_NOTE)
                .ironwood_note(IRONWOOD_NOTE)
                .build()
        }

        fn orchard_note_of(wallet: &LightWallet, value: u64) -> BoundNote {
            wallet
                .wallet_transactions
                .values()
                .flat_map(OrchardNote::transaction_outputs)
                .find(|note| note.value() == value)
                .map(|note| BoundNote {
                    output_id: note.output_id(),
                    nullifier: note
                        .nullifier()
                        .expect("scanned notes carry nullifiers")
                        .to_bytes(),
                    commitment: [0; 32],
                })
                .expect("the wallet holds the fabricated note")
        }

        fn state(phase: MigrationPhase, transfers: Vec<TransferRecord>) -> MigrationState {
            let params = MigrationParams::provisional(crate::config::ChainType::Mainnet);
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
                phase,
                transfers,
            }
        }

        fn transfer_in(wallet: &LightWallet, value: u64, state: TransferState) -> TransferRecord {
            let mut transfer =
                TransferRecord::new(TransferId(0), value, orchard_note_of(wallet, value));
            transfer.state = state;
            transfer
        }

        fn sorted(mut ids: Vec<OutputId>) -> Vec<OutputId> {
            ids.sort();
            ids
        }

        fn every_orchard_output(wallet: &LightWallet) -> Vec<OutputId> {
            sorted(vec![
                orchard_note_of(wallet, FUNDING_NOTE).output_id,
                orchard_note_of(wallet, RESIDUAL_NOTE).output_id,
            ])
        }

        #[test]
        fn committed_preparing_and_prepared_reserve_every_unspent_v2_orchard_note() {
            for phase in [
                MigrationPhase::Committed,
                MigrationPhase::Preparing {
                    round: 0,
                    pending_txids: vec![TxId::from_bytes([3; 32])],
                },
                MigrationPhase::Prepared,
            ] {
                let mut wallet = wallet();
                wallet.migration = Some(state(phase.clone(), vec![]));

                assert_eq!(
                    sorted(wallet.reserved_output_ids()),
                    every_orchard_output(&wallet),
                    "{phase:?} reserves both pre-Ironwood notes and not the Ironwood note"
                );
                assert_eq!(
                    wallet.reserved_orchard_value(),
                    FUNDING_NOTE + RESIDUAL_NOTE,
                    "{phase:?}"
                );
            }
        }

        #[test]
        fn a_spent_note_is_not_reserved_while_the_plan_is_committed() {
            let mut wallet = wallet();
            wallet.migration = Some(state(MigrationPhase::Committed, vec![]));
            let spent = orchard_note_of(&wallet, RESIDUAL_NOTE);
            let transaction = wallet
                .wallet_transactions
                .get_mut(&spent.output_id.txid())
                .expect("the note's transaction is in the wallet");
            for note in transaction.orchard_notes_mut() {
                if note.output_id() == spent.output_id {
                    note.set_spending_transaction(Some(TxId::from_bytes([5; 32])));
                }
            }

            assert_eq!(
                wallet.reserved_output_ids(),
                vec![orchard_note_of(&wallet, FUNDING_NOTE).output_id]
            );
            assert_eq!(wallet.reserved_orchard_value(), FUNDING_NOTE);
        }

        #[test]
        fn scheduled_reserves_the_funding_notes_of_pending_transfers() {
            for transfer_state in [
                TransferState::Bound,
                TransferState::Assigned,
                TransferState::Signed,
                TransferState::Broadcast,
            ] {
                let mut wallet = wallet();
                let transfer = transfer_in(&wallet, FUNDING_NOTE, transfer_state);
                wallet.migration = Some(state(MigrationPhase::Scheduled, vec![transfer]));

                assert_eq!(
                    wallet.reserved_output_ids(),
                    vec![orchard_note_of(&wallet, FUNDING_NOTE).output_id],
                    "a {transfer_state:?} transfer reserves its funding note only"
                );
                assert_eq!(wallet.reserved_orchard_value(), FUNDING_NOTE);
            }
        }

        #[test]
        fn scheduled_frees_the_note_of_a_terminal_transfer() {
            for transfer_state in [
                TransferState::Confirmed {
                    height: BlockHeight::from_u32(10),
                },
                TransferState::Invalidated,
                TransferState::Released,
            ] {
                let mut wallet = wallet();
                let transfer = transfer_in(&wallet, FUNDING_NOTE, transfer_state);
                wallet.migration = Some(state(MigrationPhase::Scheduled, vec![transfer]));

                assert!(
                    wallet.reserved_output_ids().is_empty(),
                    "a {transfer_state:?} transfer reserves nothing"
                );
                assert_eq!(wallet.reserved_orchard_value(), 0);
            }
        }

        #[test]
        fn scheduled_keeps_an_expired_transfer_reserved_until_it_is_rebuilt() {
            let mut wallet = wallet();
            let transfer = transfer_in(&wallet, FUNDING_NOTE, TransferState::Expired);
            wallet.migration = Some(state(MigrationPhase::Scheduled, vec![transfer]));

            assert_eq!(
                wallet.reserved_output_ids(),
                vec![orchard_note_of(&wallet, FUNDING_NOTE).output_id],
                "an Expired transfer is reassigned with the same note, so the note stays reserved"
            );
        }

        #[test]
        fn scheduled_keeps_the_note_of_a_released_transfer_reserved_while_its_transaction_is_on_the_wire()
         {
            let mut wallet = wallet();
            let mut transfer = transfer_in(&wallet, FUNDING_NOTE, TransferState::Assigned);
            transfer
                .mark_signed(TxId::from_bytes([9; 32]), BlockHeight::from_u32(500), None)
                .expect("assigned transfers sign");
            transfer.record_attempt();
            transfer
                .mark_broadcast()
                .expect("signed transfers broadcast");
            transfer
                .mark_released()
                .expect("broadcast transfers release");
            assert!(
                transfer.state.is_terminal(),
                "Released is terminal by state"
            );
            assert!(transfer.is_on_the_wire());
            wallet.migration = Some(state(MigrationPhase::Scheduled, vec![transfer]));

            assert_eq!(
                wallet.reserved_output_ids(),
                vec![orchard_note_of(&wallet, FUNDING_NOTE).output_id],
                "a released transfer whose transaction may still mine keeps its funding note reserved"
            );
            assert_eq!(wallet.reserved_orchard_value(), FUNDING_NOTE);
        }

        #[test]
        fn scheduled_frees_the_note_of_a_released_transfer_once_its_wire_is_forgotten() {
            let mut wallet = wallet();
            let mut transfer = transfer_in(&wallet, FUNDING_NOTE, TransferState::Assigned);
            transfer
                .mark_signed(TxId::from_bytes([9; 32]), BlockHeight::from_u32(500), None)
                .expect("assigned transfers sign");
            transfer.record_attempt();
            transfer
                .mark_broadcast()
                .expect("signed transfers broadcast");
            transfer
                .mark_released()
                .expect("broadcast transfers release");
            transfer.forget_wire();
            assert!(!transfer.is_on_the_wire());
            wallet.migration = Some(state(MigrationPhase::Scheduled, vec![transfer]));

            assert!(wallet.reserved_output_ids().is_empty());
            assert_eq!(wallet.reserved_orchard_value(), 0);
        }

        #[test]
        fn scheduled_leaves_the_residual_note_free() {
            let mut wallet = wallet();
            let transfer = transfer_in(&wallet, FUNDING_NOTE, TransferState::Assigned);
            wallet.migration = Some(state(MigrationPhase::Scheduled, vec![transfer]));

            let residual = orchard_note_of(&wallet, RESIDUAL_NOTE).output_id;
            assert!(!wallet.reserved_output_ids().contains(&residual));
        }

        #[test]
        fn complete_reserves_nothing() {
            let mut wallet = wallet();
            let transfer = transfer_in(&wallet, FUNDING_NOTE, TransferState::Bound);
            wallet.migration = Some(state(
                MigrationPhase::Complete {
                    residual: RESIDUAL_NOTE,
                },
                vec![transfer],
            ));

            assert!(wallet.reserved_output_ids().is_empty());
            assert_eq!(wallet.reserved_orchard_value(), 0);
        }

        #[test]
        fn no_migration_reserves_nothing() {
            let wallet = wallet();
            assert!(wallet.migration.is_none());

            assert!(wallet.reserved_output_ids().is_empty());
            assert_eq!(wallet.reserved_orchard_value(), 0);
        }

        #[test]
        fn reserved_orchard_value_sums_every_reserved_note() {
            let mut wallet = wallet();
            let mut second = transfer_in(&wallet, RESIDUAL_NOTE, TransferState::Assigned);
            second.id = TransferId(1);
            let transfers = vec![
                transfer_in(&wallet, FUNDING_NOTE, TransferState::Bound),
                second,
            ];
            wallet.migration = Some(state(MigrationPhase::Scheduled, transfers));

            assert_eq!(
                sorted(wallet.reserved_output_ids()),
                every_orchard_output(&wallet)
            );
            assert_eq!(
                wallet.reserved_orchard_value(),
                FUNDING_NOTE + RESIDUAL_NOTE,
                "the reserved value is the sum over the reserved output ids"
            );
        }
    }
}
