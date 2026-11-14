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
        &TransmissionRoute::Nakednet {
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
/// migration part, on a chain where NU7 never activates: the bucket grid
/// is uniform at `tip`'s scale, one window per 144 blocks, which these
/// tests' bucket arithmetic assumes. The grid's NU7 scaling has its own
/// tests in `wallet::migration::schedule` and on the mock chain.
fn wallet_with_migration_note(tip: u32) -> (LightWallet, BoundNote) {
    wallet_with_migration_note_on(
        tip,
        crate::testutils::mock_activation_heights_with(|era| era.set_nu7(None)),
    )
}

/// [`wallet_with_migration_note`] on a chain with the given activation
/// schedule.
fn wallet_with_migration_note_on(
    tip: u32,
    heights: zingo_common_components::protocol::ActivationHeights,
) -> (LightWallet, BoundNote) {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .activation_heights(heights)
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
/// Indexer exists. Both belong to the live libtonode twin. The
/// fabricated state stands in for a wallet whose boundary checkpoint is
/// gone, whatever removed it.
#[tokio::test]
async fn boundary_tree_state_unavailable_skips_the_part() {
    // Past the current bucket's boundary (288 under the provisional
    // M = 144).
    const TIP: u32 = 400;

    let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
    let params = MigrationParams::provisional(wallet.chain_type());
    let known_height = wallet
        .sync_state
        .last_known_chain_height()
        .expect("the synthetic wallet is fully synced");
    let current_bucket = params.grid(wallet.chain_type()).index_of(known_height);
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
    let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
    let anchor_bucket = current_bucket - 1;
    let boundary = params.grid(wallet.chain_type()).boundary_of(anchor_bucket);

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
            let mut wallet = wallet_with_state(mode, MigrationPhase::Complete { residual: 5_000 });
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

    /// A part whose window straddles the NU7 activation is built against
    /// the activation once the chain has crossed it, so its transaction
    /// commits to the branch the network validates; built below the
    /// activation it targets the window's boundary.
    #[test]
    fn a_straddling_window_builds_against_the_activation_once_crossed() {
        use shardtree::store::ShardStore as _;
        use zcash_protocol::consensus::BlockHeight;
        const ACTIVATION: u32 = 1_000;
        // Boundary 864 opens the last bucket below the activation, and the
        // grid's next boundary is 1_296.
        let heights =
            || crate::testutils::mock_activation_heights_with(|era| era.set_nu7(Some(ACTIVATION)));
        for (tip, expected_target) in [(999u32, 865u32), (1_000, ACTIVATION), (1_200, ACTIVATION)] {
            let (mut wallet, bound_note) = wallet_with_migration_note_on(tip, heights());
            let params = MigrationParams::provisional(wallet.chain_type());
            let grid = params.grid(wallet.chain_type());
            let window = grid.index_of(BlockHeight::from_u32(tip));
            assert_eq!(grid.boundary_of(window), BlockHeight::from_u32(864));
            // A checkpoint the anchor boundary can reach, holding the
            // bound note, so the build captures a real witness; the
            // builder checkpoints at the tip alone.
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
                .add_checkpoint(
                    grid.boundary_of(window - 1),
                    shardtree::store::Checkpoint::at_position(position),
                )
                .expect("infallible on the memory store");
            let mut part = assigned_part_with_witness(bound_note, window, window - 1);
            part.anchor_witness = None;
            // The bound note is sized denomination plus fee, as a split
            // note is, so the canonical-shape check passes.
            part.denomination = NOTE_VALUE - params.part_fee;
            match wallet
                .prepare_part(AccountId::ZERO, &mut part, &params)
                .expect("the part prepares")
            {
                PrepareResult::Ready { prove, .. } => assert_eq!(
                    prove.target_height(),
                    BlockHeight::from_u32(expected_target),
                    "tip {tip}"
                ),
                PrepareResult::Skip(reason) => {
                    panic!("tip {tip}: expected a ready part, got a skip: {reason:?}")
                }
            }
        }
    }

    #[test]
    fn spent_bound_note_skips() {
        let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
        let params = MigrationParams::provisional(wallet.chain_type());
        let bucket = params
            .grid(wallet.chain_type())
            .index_of(wallet.sync_state.last_known_chain_height().unwrap());
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
        let bucket = params
            .grid(wallet.chain_type())
            .index_of(wallet.sync_state.last_known_chain_height().unwrap());
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
        let bucket = params
            .grid(wallet.chain_type())
            .index_of(wallet.sync_state.last_known_chain_height().unwrap());

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
    use zl_common::status::ConfirmationStatus;

    use crate::mocks::orchard_note::OrchardCryptoNoteBuilder;

    // Every confirmed note is gone (spent into the round); the round's
    // outputs live only in a transmitted, unconfirmed transaction.
    let mut wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
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
    let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
    let window_start = params.grid(wallet.chain_type()).boundary_of(current_bucket);
    let window_end = params
        .grid(wallet.chain_type())
        .boundary_of(current_bucket + 1);
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
            upcoming_windows(
                &state.parts,
                now_height,
                0,
                u64::MAX,
                &params.grid(wallet.chain_type())
            )
            .is_empty(),
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
        let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
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
        let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
        let window_end = params
            .grid(wallet.chain_type())
            .boundary_of(current_bucket + 1);
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

    /// HYPOTHESIS: a part whose submission the endpoint refuses is
    /// reported `Failed` and halts the batch, rather than `Slid`: its
    /// transaction was built and recorded, it just never left, and it
    /// stays signed so the next batch resubmits it. Falsified if the
    /// report reads `Slid` with no halt, or the part leaves `Signed`.
    #[tokio::test]
    async fn a_refused_submission_is_reported_failed_not_slid() {
        const TIP: u32 = 300;
        let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
        let params = MigrationParams::provisional(wallet.chain_type());
        let now_height = wallet
            .sync_state
            .last_known_chain_height()
            .expect("synced synthetic wallet");
        let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
        let window_end = params
            .grid(wallet.chain_type())
            .boundary_of(current_bucket + 1);
        let own_txid = TxId::from_bytes([7; 32]);
        let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        part.assign(current_bucket).expect("fresh parts are bound");
        part.mark_signed(own_txid, window_end, Some(vec![0xAB; 64]))
            .expect("assigned parts sign");
        wallet.migration = Some(scheduled_state(params, vec![part]));

        let mut client = LightClient::new_for_test(wallet).await;
        let transmission_client = MockTransmissionClient::default();
        transmission_client
            .fail
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let report = client
            .execute_due_parts_with(&transmission_client, Duration::ZERO)
            .await
            .unwrap();

        let halted = report
            .halted
            .as_deref()
            .expect("a refused submission halts the batch");
        assert!(
            halted.contains("mock transport failure"),
            "the halt carries the submission error, got {halted:?}"
        );
        assert_eq!(
            report.outcomes,
            vec![PartOutcome {
                part: PartId(0),
                denomination: NOTE_VALUE,
                result: PartSendResult::Failed {
                    error: halted.to_string(),
                },
            }]
        );
        assert_eq!(
            client
                .wallet()
                .read()
                .await
                .migration
                .as_ref()
                .unwrap()
                .parts[0]
                .state,
            PartState::Signed,
            "the refused part stays signed, due for a resubmission"
        );

        // The endpoint recovers: the next batch resubmits the same part.
        transmission_client
            .fail
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let retry = client
            .execute_due_parts_with(&transmission_client, Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(
            retry.outcomes,
            vec![PartOutcome {
                part: PartId(0),
                denomination: NOTE_VALUE,
                result: PartSendResult::Sent(own_txid),
            }]
        );
        assert!(retry.halted.is_none());
    }

    /// HYPOTHESIS: on the automatic path, a refusal in the middle of a
    /// pass returns the txids accepted before it and leaves the refused
    /// part signed, so the next pass resubmits only that part. Falsified
    /// if the accepted txid is dropped, the refused part leaves `Signed`,
    /// or the accepted part is submitted twice.
    #[tokio::test]
    async fn a_mid_pass_refusal_keeps_the_accepted_txids_and_resubmits_the_rest() {
        const TIP: u32 = 300;
        let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
        let params = MigrationParams::provisional(wallet.chain_type());
        let now_height = wallet
            .sync_state
            .last_known_chain_height()
            .expect("synced synthetic wallet");
        let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
        let window_end = params
            .grid(wallet.chain_type())
            .boundary_of(current_bucket + 1);
        let first_txid = TxId::from_bytes([1; 32]);
        let second_txid = TxId::from_bytes([2; 32]);
        let mut first = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        first.assign(current_bucket).expect("fresh parts are bound");
        first
            .mark_signed(first_txid, window_end, Some(vec![0x01; 64]))
            .expect("assigned parts sign");
        let mut second = PartRecord::new(PartId(1), NOTE_VALUE, bound_note);
        second
            .assign(current_bucket)
            .expect("fresh parts are bound");
        second
            .mark_signed(second_txid, window_end, Some(vec![0x02; 64]))
            .expect("assigned parts sign");
        wallet.migration = Some(scheduled_state(params, vec![first, second]));

        let mut client = LightClient::new_for_test(wallet).await;
        let transmission_client = MockTransmissionClient::default();
        // The endpoint takes one submission, then refuses.
        transmission_client
            .fail_from
            .store(1, std::sync::atomic::Ordering::Relaxed);
        let sent = client
            .transmit_due_parts_with(&transmission_client)
            .await
            .unwrap();
        assert_eq!(sent, vec![first_txid], "the accepted txid is kept");

        {
            let wallet = client.wallet().read().await;
            let parts = &wallet.migration.as_ref().unwrap().parts;
            assert_eq!(parts[0].state, PartState::Broadcast);
            assert_eq!(
                parts[1].state,
                PartState::Signed,
                "the refused part stays signed"
            );
            assert_eq!(parts[1].attempts, 1, "the refusal counts as an attempt");
        }

        // The endpoint recovers: only the refused part is resubmitted.
        transmission_client
            .fail_from
            .store(usize::MAX, std::sync::atomic::Ordering::Relaxed);
        let retry = client
            .transmit_due_parts_with(&transmission_client)
            .await
            .unwrap();
        assert_eq!(retry, vec![second_txid]);
        assert_eq!(
            transmission_client.submissions.lock().unwrap().len(),
            2,
            "each part is accepted exactly once"
        );
        let wallet = client.wallet().read().await;
        let parts = &wallet.migration.as_ref().unwrap().parts;
        assert_eq!(parts[0].state, PartState::Broadcast);
        assert_eq!(parts[1].state, PartState::Broadcast);
    }

    /// HYPOTHESIS: in a batch, a refusal reports the parts accepted
    /// before it as `Sent`, the refused part as `Failed` under its own
    /// id, and stops: the parts after it get no outcome and no attempt.
    /// Falsified if a later part is attempted, the failed outcome names
    /// the wrong part, or the halt is missing.
    #[tokio::test]
    async fn a_refusal_stops_the_batch_after_the_accepted_parts() {
        const TIP: u32 = 300;
        let (mut wallet, bound_note) = wallet_with_migration_note(TIP);
        let params = MigrationParams::provisional(wallet.chain_type());
        let now_height = wallet
            .sync_state
            .last_known_chain_height()
            .expect("synced synthetic wallet");
        let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
        let window_end = params
            .grid(wallet.chain_type())
            .boundary_of(current_bucket + 1);
        let txids = [
            TxId::from_bytes([1; 32]),
            TxId::from_bytes([2; 32]),
            TxId::from_bytes([3; 32]),
        ];
        let parts = txids
            .iter()
            .enumerate()
            .map(|(i, txid)| {
                let mut part = PartRecord::new(PartId(i as u32), NOTE_VALUE, bound_note);
                part.assign(current_bucket).expect("fresh parts are bound");
                part.mark_signed(*txid, window_end, Some(vec![i as u8; 64]))
                    .expect("assigned parts sign");
                part
            })
            .collect();
        wallet.migration = Some(scheduled_state(params, parts));

        let mut client = LightClient::new_for_test(wallet).await;
        let transmission_client = MockTransmissionClient::default();
        // The endpoint takes one submission, then refuses.
        transmission_client
            .fail_from
            .store(1, std::sync::atomic::Ordering::Relaxed);
        let report = client
            .execute_due_parts_with(&transmission_client, Duration::ZERO)
            .await
            .unwrap();

        let halted = report
            .halted
            .as_deref()
            .expect("the refusal halts the batch");
        assert_eq!(
            report.outcomes,
            vec![
                PartOutcome {
                    part: PartId(0),
                    denomination: NOTE_VALUE,
                    result: PartSendResult::Sent(txids[0]),
                },
                PartOutcome {
                    part: PartId(1),
                    denomination: NOTE_VALUE,
                    result: PartSendResult::Failed {
                        error: halted.to_string(),
                    },
                },
            ],
            "the accepted part is Sent, the refused part is Failed, the rest has no outcome"
        );

        let wallet = client.wallet().read().await;
        let parts = &wallet.migration.as_ref().unwrap().parts;
        assert_eq!(parts[0].state, PartState::Broadcast);
        assert_eq!(
            parts[1].state,
            PartState::Signed,
            "the refused part stays signed"
        );
        assert_eq!(parts[1].attempts, 1);
        assert_eq!(
            parts[2].state,
            PartState::Signed,
            "the part after the halt is untouched"
        );
        assert_eq!(
            parts[2].attempts, 0,
            "the part after the halt is not attempted"
        );
        assert_eq!(transmission_client.submissions.lock().unwrap().len(), 1);
    }

    /// HYPOTHESIS: once the window of a refused part has passed, the
    /// part is not catch-up material: reconciliation classes it
    /// `AwaitingExpiry`, status does not advertise it, and the batch
    /// leaves it signed in its old bucket. The part the halt blocked,
    /// still assigned, is `Overdue`, advertised, and folds into the
    /// batch. Falsified if the refused part is folded or resubmitted,
    /// or the blocked part is left behind.
    #[tokio::test]
    async fn a_refused_part_past_its_window_awaits_expiry_while_the_blocked_part_folds_in() {
        use crate::wallet::migration::{PartClass, RecommendedAction};
        use zcash_protocol::consensus::BlockHeight;

        // Tip 360 sits beyond bucket 0's slip tolerance, as in
        // `overdue_part_folds_into_the_batch`.
        let (mut wallet, bound_note) = wallet_with_migration_note(360);
        let params = MigrationParams::provisional(wallet.chain_type());
        let tip = wallet
            .sync_state
            .last_known_chain_height()
            .expect("synced synthetic wallet");
        let current_bucket = params.grid(wallet.chain_type()).index_of(tip);
        assert!(current_bucket > 0, "bucket 0 must be in the past");
        let expiry = BlockHeight::from_u32(1_000);
        assert!(expiry > tip, "the refused transaction is still valid");

        // The refused part: signed in bucket 0, attempted once, its
        // transaction still valid.
        let refused_txid = TxId::from_bytes([1; 32]);
        let mut refused = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        refused.assign(0).expect("fresh parts are bound");
        refused
            .mark_signed(refused_txid, expiry, Some(vec![0x01; 64]))
            .expect("assigned parts sign");
        refused.record_attempt();
        // The part the halt blocked: still assigned in bucket 0.
        let mut blocked = PartRecord::new(PartId(1), NOTE_VALUE, bound_note);
        blocked.assign(0).expect("fresh parts are bound");
        wallet.migration = Some(scheduled_state(params, vec![refused, blocked]));

        let mut client = LightClient::new_for_test(wallet).await;

        let report = client.reconcile_migration().await.unwrap();
        let class_of = |id: PartId| {
            report
                .assessments
                .iter()
                .find(|assessment| assessment.id == id)
                .map(|assessment| assessment.class)
        };
        assert_eq!(
            class_of(PartId(0)),
            Some(PartClass::AwaitingExpiry { expiry }),
            "the refused part waits out its expiry"
        );
        assert_eq!(class_of(PartId(1)), Some(PartClass::Overdue));
        assert!(
            report.actions.iter().any(|action| matches!(
                action,
                RecommendedAction::PromptCatchUp { parts, .. } if parts == &vec![PartId(1)]
            )),
            "only the blocked part is catch-up material: {:?}",
            report.actions
        );
        assert!(
            !report
                .actions
                .iter()
                .any(|action| matches!(action, RecommendedAction::Rebuild { .. })),
            "a still-valid transaction is not rebuilt"
        );

        let status = client.migration_status().await.unwrap();
        let due = status.due_now.expect("the blocked part is due");
        assert_eq!(
            due.part_ids,
            vec![PartId(1)],
            "status advertises the blocked part only"
        );

        let transmission_client = MockTransmissionClient::default();
        let batch = client
            .execute_due_parts_with(&transmission_client, Duration::ZERO)
            .await
            .unwrap();
        // The blocked part folds in and is attempted; the synthetic wallet
        // cannot witness the boundary, so it slides rather than sends.
        assert!(
            matches!(
                batch.outcomes[..],
                [PartOutcome {
                    part: PartId(1),
                    result: PartSendResult::Slid,
                    ..
                }]
            ),
            "only the blocked part is in the batch: {:?}",
            batch.outcomes
        );
        assert!(batch.halted.is_none());
        assert!(
            transmission_client.submissions.lock().unwrap().is_empty(),
            "the refused part is not resubmitted late"
        );

        let wallet = client.wallet().read().await;
        let parts = &wallet.migration.as_ref().unwrap().parts;
        assert_eq!(parts[0].state, PartState::Signed);
        assert_eq!(parts[0].attempts, 1);
        assert_eq!(
            parts[0].bucket_index,
            Some(0),
            "the refused part is not shifted"
        );
        assert_eq!(parts[0].txid, Some(refused_txid));
        assert_eq!(
            parts[1].bucket_index,
            Some(current_bucket),
            "the blocked part folded into the current window"
        );
    }

    /// HYPOTHESIS: once the refused part's transaction expires,
    /// reconciliation rebuilds it: the part returns to `Assigned` with
    /// no transaction, placed in a coming window, never the current one.
    /// Falsified if the part stays signed, keeps its txid, or lands in
    /// the current or a past bucket.
    #[tokio::test]
    async fn a_refused_part_is_rebuilt_once_its_transaction_expires() {
        use crate::wallet::migration::{PartClass, RecommendedAction};
        use zcash_protocol::consensus::BlockHeight;

        let (mut wallet, bound_note) = wallet_with_migration_note(360);
        let params = MigrationParams::provisional(wallet.chain_type());
        let tip = wallet
            .sync_state
            .last_known_chain_height()
            .expect("synced synthetic wallet");
        let current_bucket = params.grid(wallet.chain_type()).index_of(tip);
        let expiry = BlockHeight::from_u32(300);
        assert!(expiry <= tip, "the refused transaction has expired");

        let mut refused = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        refused.assign(0).expect("fresh parts are bound");
        refused
            .mark_signed(TxId::from_bytes([1; 32]), expiry, Some(vec![0x01; 64]))
            .expect("assigned parts sign");
        refused.record_attempt();
        wallet.migration = Some(scheduled_state(params, vec![refused]));

        let mut client = LightClient::new_for_test(wallet).await;
        let report = client.reconcile_migration().await.unwrap();
        assert_eq!(report.assessments[0].class, PartClass::Expired);
        assert!(
            report
                .actions
                .contains(&RecommendedAction::Rebuild { part: PartId(0) }),
            "the expired part is rebuilt: {:?}",
            report.actions
        );

        let wallet = client.wallet().read().await;
        let part = &wallet.migration.as_ref().unwrap().parts[0];
        assert_eq!(part.state, PartState::Assigned, "rebuilt fresh");
        assert_eq!(part.txid, None, "the dead transaction is dropped");
        assert!(part.signed_blob.is_none());
        assert!(
            part.bucket_index
                .is_some_and(|bucket| bucket > current_bucket),
            "placed in a coming window, got {:?} with current {current_bucket}",
            part.bucket_index
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
        let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
        let window_end = params
            .grid(wallet.chain_type())
            .boundary_of(current_bucket + 1);
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
        let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
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
        let current_bucket = state.params.grid(wallet.chain_type()).index_of(now_height);
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
        wallet.wallet_settings.min_confirmations = NonZeroU32::new(360).expect("non-zero literal");
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
        wallet.wallet_settings.min_confirmations = NonZeroU32::new(360).expect("non-zero literal");
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
        let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
        let boundary = params.grid(wallet.chain_type()).boundary_of(current_bucket);
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
        let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);

        let mut part = PartRecord::new(PartId(0), NOTE_VALUE, bound_note);
        schedule::plan_schedule(
            std::slice::from_mut(&mut part),
            now_height,
            activation,
            |part| wallet.bound_note_confirmed_at(part),
            &params,
            &params.grid(wallet.chain_type()),
            &mut crate::utils::system_rng(),
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
        let grid = params.grid(wallet.chain_type());
        let now_height = wallet
            .sync_state
            .last_known_chain_height()
            .expect("synced synthetic wallet");
        let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
        let window_end = params
            .grid(wallet.chain_type())
            .boundary_of(current_bucket + 1);
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
        assert_eq!(batch.boundary, grid.boundary_of(current_bucket),);

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
        let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
        let window_end = params
            .grid(wallet.chain_type())
            .boundary_of(current_bucket + 1);

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
        let current_bucket = params
            .grid(wallet.chain_type())
            .index_of(BlockHeight::from_u32(360));
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

    /// A client whose Mixnet Mode is Ready at the mock tunnel endpoint, with a
    /// remote sync indexer that is never dialed.
    #[cfg(feature = "nym")]
    async fn ready_client(tip: u32) -> (LightClient, BoundNote) {
        let (wallet, bound_note) = wallet_with_migration_note(tip);
        let mut client = LightClient::new_for_test(wallet).await;
        client
            .set_indexer_uri_lazy("https://indexer.example:443".parse().expect("a static uri"))
            .expect("a lazy indexer needs no connection");
        client
            .switch_on_mixnet_for_tests(crate::mocks::transmission::MOCK_SOCKS5_ADDR)
            .await;
        (client, bound_note)
    }

    /// HYPOTHESIS: the resolved transmission client is the mixnet
    /// variant whenever Mixnet Mode is ready, so no migration part can
    /// reach a nakednet wire without the deliberate opt-out. Falsified
    /// if a ready session resolves anything else.
    #[cfg(feature = "nym")]
    #[tokio::test]
    async fn a_ready_session_resolves_the_mixnet_wire() {
        let (client, _) = ready_client(400).await;
        let resolved = client
            .migration_transmission_client()
            .expect("a ready session resolves a wire");
        assert!(
            resolved.is_mixnet(),
            "a ready session must resolve the mixnet wire"
        );
    }

    /// HYPOTHESIS: while the mixnet is unavailable and the user has not
    /// consented to nakednet, the seam refuses instead of resolving any
    /// wire, so no part is emitted. Falsified if an unattached session
    /// resolves a client at all.
    #[cfg(feature = "nym")]
    #[tokio::test]
    async fn an_unattached_session_refuses_rather_than_resolving_nakednet() {
        let (wallet, _) = wallet_with_migration_note(400);
        let client = LightClient::new_for_test(wallet).await;
        assert!(
            client.migration_transmission_client().is_err(),
            "absence of a mixnet is never consent to nakednet"
        );
    }

    /// HYPOTHESIS: every part the lifecycle transmits carries a mixnet
    /// route receipt, and the count of receipts equals the count of
    /// parts the schedule sent — no part reaches a wire outside the
    /// seam, and none travels nakednet. Falsified if any receipt names
    /// a nakednet route, or if the wire saw a different number of
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
        let current_bucket = params.grid(wallet.chain_type()).index_of(now_height);
        let window_end = params
            .grid(wallet.chain_type())
            .boundary_of(current_bucket + 1);

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

    /// HYPOTHESIS: the validation is not vacuous — a nakednet receipt is
    /// visibly nakednet, so a future path that leaks would be caught
    /// rather than silently passing. Falsified if the nakednet route
    /// reports itself as mixnet.
    #[test]
    fn the_detector_can_see_a_nakednet_leak() {
        let mixnet = TransmissionRoute::Mixnet {
            destination: "destination.example".to_string(),
            via_socks5: "127.0.0.1:1".to_string(),
        };
        let nakednet = TransmissionRoute::Nakednet {
            endpoint: "nakednet.example".to_string(),
        };
        assert!(mixnet.is_mixnet());
        assert!(
            !nakednet.is_mixnet(),
            "a nakednet route must never read as mixnet"
        );
    }
}
