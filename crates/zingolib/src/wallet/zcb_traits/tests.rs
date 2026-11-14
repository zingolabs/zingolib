use zcash_client_backend::data_api::MaxSpendMode;
use zcash_protocol::PoolType;
use zcash_protocol::value::Zatoshis;

use super::*;
use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

/// One spendable note per shielded pool, plus an orchard note at the
/// dust line (`MARGINAL_FEE`) that budgeted selection would also
/// refuse to spend.
fn funded_wallet() -> LightWallet {
    SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .tip(20)
        .sapling_note(50_000)
        .orchard_note(100_000)
        .ironwood_note(70_000)
        .orchard_note(5_000)
        .build()
}

fn select_all_funds(
    wallet: &LightWallet,
    mode: MaxSpendMode,
) -> Result<ReceivedNotes<OutputRef>, WalletError> {
    wallet.select_spendable_notes(
        zip32::AccountId::ZERO,
        TargetValue::AllFunds(mode),
        &[
            ShieldedPool::Ironwood,
            ShieldedPool::Orchard,
            ShieldedPool::Sapling,
        ],
        TargetHeight::from(21),
        ConfirmationsPolicy::new_symmetrical(NonZeroU32::new(1).expect("nonzero"), false),
        &[],
        LockFilter::Unfiltered,
    )
}

/// A funded wallet whose sapling shard tree holds no checkpoint.
fn wallet_without_sapling_checkpoint() -> LightWallet {
    use pepper_sync::sync::SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE;
    use shardtree::store::memory::MemoryShardStore;

    let mut wallet = funded_wallet();
    wallet.shard_trees.sapling = ShardTree::new(
        MemoryShardStore::empty(),
        SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE as usize,
    );
    assert!(wallet.sync_state.last_known_chain_height().is_some());
    wallet
}

#[test]
fn missing_sapling_checkpoint_means_no_anchor_and_zero_balance() {
    let wallet = wallet_without_sapling_checkpoint();

    assert_eq!(
        wallet
            .get_target_and_anchor_heights(wallet.wallet_settings.min_confirmations)
            .expect("infallible"),
        None
    );
    assert_eq!(
        wallet
            .shielded_spendable_balance(zip32::AccountId::ZERO, false)
            .expect("a missing anchor is a zero balance, not an error"),
        Zatoshis::ZERO
    );
}

#[test]
fn reload_restores_missing_sapling_checkpoint() {
    let mut wallet = wallet_without_sapling_checkpoint();

    wallet.save_required = true;
    let bytes = wallet.save().unwrap().expect("save required");
    let reloaded = LightWallet::read(bytes.as_slice(), wallet.chain_type()).unwrap();

    assert_eq!(
        reloaded
            .shard_trees
            .sapling
            .store()
            .max_checkpoint_id()
            .expect("infallible"),
        Some(BlockHeight::from_u32(0))
    );
    assert!(
        reloaded
            .get_target_and_anchor_heights(wallet.wallet_settings.min_confirmations)
            .expect("infallible")
            .is_some()
    );
}

#[test]
fn all_funds_max_spendable_selects_every_spend_worthy_note() {
    let notes = select_all_funds(&funded_wallet(), MaxSpendMode::MaxSpendable)
        .expect("all-funds selection must succeed on a funded, synced wallet");
    assert_eq!(notes.sapling().len(), 1);
    assert_eq!(
        notes.orchard().len(),
        1,
        "the dust orchard note must be left behind"
    );
    assert_eq!(notes.ironwood().len(), 1);
    let total: u64 = notes
        .sapling()
        .iter()
        .map(|note| note.note().value().inner())
        .chain(
            notes
                .orchard()
                .iter()
                .map(|note| note.note().value().inner()),
        )
        .chain(
            notes
                .ironwood()
                .iter()
                .map(|note| note.note().value().inner()),
        )
        .sum();
    assert_eq!(total, 220_000);
}

#[test]
fn all_funds_everything_is_a_typed_error_not_an_abort() {
    let error = select_all_funds(&funded_wallet(), MaxSpendMode::Everything)
        .expect_err("Everything mode is unimplemented and must refuse, not abort");
    assert!(
        matches!(error, WalletError::AllFundsEverythingUnsupported),
        "the refusal must be the dedicated typed error, observed: {error}"
    );
}

// Tests that selection returns the requested pools' notes alone when they fall short. Upstream
// input selection rejects a note from a pool it did not ask for
// (GreedyInputSelectorError::UnrequestedPoolSelected) and reports the shortfall itself.
#[test]
fn at_least_selection_stays_in_the_requested_pools_when_they_fall_short() {
    let notes = funded_wallet()
        .select_spendable_notes(
            zip32::AccountId::ZERO,
            TargetValue::AtLeast(Zatoshis::const_from_u64(200_000)),
            &[ShieldedPool::Ironwood, ShieldedPool::Orchard],
            TargetHeight::from(21),
            ConfirmationsPolicy::new_symmetrical(
                NonZeroU32::new(1).expect("one is nonzero"),
                false,
            ),
            &[],
            LockFilter::Unfiltered,
        )
        .expect("selection over synthetic funds succeeds");
    assert!(notes.sapling().is_empty());
    assert_eq!(notes.ironwood().len(), 1);
    assert_eq!(notes.orchard().len(), 1);
}

// Tests that all-funds selection gathers the requested pools alone.
#[test]
fn all_funds_selection_gathers_only_the_requested_pools() {
    let notes = funded_wallet()
        .select_spendable_notes(
            zip32::AccountId::ZERO,
            TargetValue::AllFunds(MaxSpendMode::MaxSpendable),
            &[ShieldedPool::Orchard],
            TargetHeight::from(21),
            ConfirmationsPolicy::new_symmetrical(
                NonZeroU32::new(1).expect("one is nonzero"),
                false,
            ),
            &[],
            LockFilter::Unfiltered,
        )
        .expect("all-funds selection succeeds on a funded, synced wallet");
    assert!(notes.sapling().is_empty());
    assert!(notes.ironwood().is_empty());
    assert_eq!(notes.orchard().len(), 1);
}

/// Pins the resolution of the FIXME at the `PoolType::IRONWOOD` label in
/// `InputSource::select_spendable_notes`: the code is right and the comment
/// above it is stale.
///
/// The `OutputRef` pool label is never read by the upstream proposal
/// engine, since pool-involvement accounting, bundle attribution, and fee
/// calculation all dispatch on the embedded `Note` enum (stamped by
/// `ReceivedNotes::into_vec` from the vector the note occupies), not on
/// the `NoteRef`. The label's one consumer is this wallet's own `exclude`
/// split at the top of `select_spendable_notes`: upstream's
/// `GreedyInputSelector` hands previously selected refs back verbatim as
/// `exclude` when the change strategy reports `ChangeError::DustInputs`,
/// and the split routes each ref to a pool bucket by `pool_type()`. An
/// ironwood note labeled `ORCHARD` (as the stale comment mandates) would
/// land in the orchard bucket, never be excluded from ironwood selection,
/// and be re-selected forever, so the selector would then abort with
/// `InsufficientFunds` instead of pruning the dust.
#[test]
fn ironwood_noteref_pool_label_round_trips_through_exclude() {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .ironwood_note(100_000)
        .ironwood_note(100_000)
        .build();

    let confirmations_policy =
        ConfirmationsPolicy::new_symmetrical(1.try_into().expect("nonzero"), false);
    let target_height = TargetHeight::from(BlockHeight::from_u32(21));
    let sources = [
        ShieldedPool::Ironwood,
        ShieldedPool::Orchard,
        ShieldedPool::Sapling,
    ];

    // Phase 1: selection over ironwood-only funds labels every selected
    // ref IRONWOOD, matching the vector (and hence the Note-enum pool)
    // the note is returned in.
    let selected = wallet
        .select_spendable_notes(
            zip32::AccountId::ZERO,
            TargetValue::AtLeast(Zatoshis::const_from_u64(150_000)),
            &sources,
            target_height,
            confirmations_policy,
            &[],
            LockFilter::Unfiltered,
        )
        .expect("selection over synthetic funds succeeds");
    assert_eq!(
        selected.ironwood().len(),
        2,
        "both fabricated ironwood notes are selected"
    );
    let refs: Vec<_> = selected
        .ironwood()
        .iter()
        .map(|note| *note.internal_note_id())
        .collect();
    for output_ref in &refs {
        assert_eq!(
            output_ref.pool_type(),
            PoolType::IRONWOOD,
            "an ironwood note's ref must carry the IRONWOOD label so the \
                 exclude split routes it back to the ironwood bucket"
        );
    }

    // Phase 2: the refs round-trip through `exclude` exactly as the
    // greedy input selector's dust-pruning path replays them. Correctly
    // labeled refs suppress re-selection; ORCHARD-labeled refs would fall
    // into the wrong bucket and the same notes would come back.
    let reselected = wallet
        .select_spendable_notes(
            zip32::AccountId::ZERO,
            TargetValue::AtLeast(Zatoshis::const_from_u64(150_000)),
            &sources,
            target_height,
            confirmations_policy,
            &refs,
            LockFilter::Unfiltered,
        )
        .expect("selection with exclusions succeeds");
    assert!(
        reselected.ironwood().is_empty(),
        "excluded ironwood refs must not be re-selected; re-selection \
             means the exclude split routed them to the wrong pool bucket"
    );
}
