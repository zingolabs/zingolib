//! Integration tests for the Orchard→Ironwood migration backend (ZIP 318).
//!
//! The full two-phase run needs Ironwood support along the whole node path
//! (pepper-sync V3 note scanning, the lightwalletd Ironwood parser and
//! zebra witness serving), so it is ignored until those land. The other
//! tests exercise the migration state machine against a live regtest chain
//! with today's stack: bound-note reservation, external-spend invalidation
//! and the no-sync broadcast path.

use pepper_sync::wallet::{NoteInterface, OrchardNote, OutputId, OutputInterface};
use zcash_local_net::validator::Validator;
use zcash_primitives::transaction::TxId;
use zingolib::get_base_address_macro;
use zingolib::lightclient::LightClient;
use zingolib::testutils::lightclient::from_inputs;
use zingolib::wallet::migration::{
    BoundNote, ConsentBinding, MigrationParams, MigrationPhase, MigrationState, PartId, PartRecord,
    PartState, RecommendedAction, SigningStrategy, schedule,
};
use zingolib_testutils::scenarios::{
    self, generate_n_blocks_return_new_height, increase_height_and_wait_for_client,
};
use zip32::AccountId;

/// A snapshot of one of the wallet's Orchard notes.
struct NoteRecord {
    output_id: OutputId,
    nullifier: [u8; 32],
    value: u64,
    spending_transaction: Option<TxId>,
}

async fn orchard_note_records(client: &LightClient) -> Vec<NoteRecord> {
    let wallet = client.wallet().read().await;
    wallet
        .wallet_transactions
        .values()
        .flat_map(|transaction| {
            OrchardNote::transaction_outputs(transaction)
                .iter()
                .map(|note| NoteRecord {
                    output_id: note.output_id(),
                    nullifier: note
                        .nullifier()
                        .expect("scanned wallet notes carry nullifiers")
                        .to_bytes(),
                    value: note.value(),
                    spending_transaction: note.spending_transaction(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn note_by_value(notes: &[NoteRecord], value: u64) -> &NoteRecord {
    notes
        .iter()
        .find(|note| note.value == value)
        .unwrap_or_else(|| panic!("no note of {value} zatoshis in the wallet"))
}

/// Persists a hand-built [`MigrationState`] whose parts are bound to real
/// wallet notes, standing in for the completed note-splitting phase so the
/// Phase 2 machinery can be exercised before Ironwood lands on the node
/// path. `bucket_index` assigns every part to that bucket; `None` leaves
/// them bound but unscheduled.
async fn inject_scheduled_migration(
    client: &LightClient,
    bound: Vec<(u64, OutputId, [u8; 32])>,
    bucket_index: Option<u64>,
) {
    let mut wallet = client.wallet().write().await;
    let params = MigrationParams::provisional(wallet.chain_type());
    let parts = bound
        .into_iter()
        .enumerate()
        .map(|(index, (denomination, output_id, nullifier))| {
            let mut part = PartRecord::new(
                PartId(u32::try_from(index).expect("test part count fits u32")),
                denomination,
                BoundNote {
                    output_id,
                    nullifier,
                    // Never read on these paths: materialization revalidates
                    // it, but these tests stop before materializing.
                    commitment: [0; 32],
                },
            );
            if let Some(bucket_index) = bucket_index {
                part.assign(bucket_index).expect("fresh parts are bound");
            }
            part
        })
        .collect();
    wallet.migration = Some(MigrationState {
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
    });
}

/// A part's bound note is excluded from ordinary input selection while
/// another note can satisfy the request, the fallback pass consumes it when
/// nothing else can, and the external spend then invalidates the part on
/// reconciliation with a remainder replan recommended (the ZIP 318
/// invalidation predicate).
#[tokio::test]
async fn bound_note_reservation_and_external_spend_invalidation() {
    let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
    let faucet_address = get_base_address_macro!(faucet, "unified");
    let recipient_address = get_base_address_macro!(recipient, "unified");

    // Two Orchard notes: the 100_000 note plays a bound split note, the
    // 50_000 note is free.
    from_inputs::quick_send(&mut faucet, vec![(&recipient_address, 100_000, None)])
        .await
        .unwrap();
    from_inputs::quick_send(&mut faucet, vec![(&recipient_address, 50_000, None)])
        .await
        .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    let notes = orchard_note_records(&recipient).await;
    let reserved = note_by_value(&notes, 100_000);
    inject_scheduled_migration(
        &recipient,
        vec![(100_000, reserved.output_id, reserved.nullifier)],
        None,
    )
    .await;
    let reserved_id = reserved.output_id;

    // An ordinary send the free note can cover must leave the bound note
    // alone.
    from_inputs::quick_send(&mut recipient, vec![(&faucet_address, 20_000, None)])
        .await
        .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    let notes = orchard_note_records(&recipient).await;
    assert!(
        notes
            .iter()
            .find(|note| note.output_id == reserved_id)
            .expect("the bound note is still in the wallet")
            .spending_transaction
            .is_none(),
        "an ordinary send must not consume a bound note while another note suffices"
    );
    assert!(
        note_by_value(&notes, 50_000).spending_transaction.is_some(),
        "the free note covers the ordinary send"
    );

    // A send only the bound note can cover goes through anyway: the
    // reservation biases selection, it never blocks a spend.
    from_inputs::quick_send(&mut recipient, vec![(&faucet_address, 100_000, None)])
        .await
        .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    let notes = orchard_note_records(&recipient).await;
    assert!(
        notes
            .iter()
            .find(|note| note.output_id == reserved_id)
            .expect("the bound note is still in the wallet")
            .spending_transaction
            .is_some(),
        "the fallback pass consumes the bound note when nothing else can pay"
    );

    // Reconciliation sees the external spend, invalidates the part and
    // recommends replanning the remainder.
    let report = recipient.reconcile_migration().await.unwrap();
    assert!(
        report.actions.iter().any(|action| matches!(
            action,
            RecommendedAction::MarkInvalidated { part } if *part == PartId(0)
        )),
        "the external spend must invalidate the part: {:?}",
        report.actions
    );
    assert!(
        report
            .actions
            .iter()
            .any(|action| matches!(action, RecommendedAction::ReplanRemainder)),
        "spendable Orchard value remains, so a remainder replan is required: {:?}",
        report.actions
    );
    let wallet = recipient.wallet().read().await;
    assert_eq!(
        wallet.migration.as_ref().unwrap().parts[0].state,
        PartState::Invalidated
    );
}

/// A due part whose boundary tree state is unavailable is skipped with no
/// writes and no synchronization (the ZIP 318 decoupling requirement). The
/// wallet leaps past the boundary in a single sync, so the boundary
/// checkpoint falls outside shardtree's retention window and the witness
/// was never captured.
#[tokio::test]
async fn unavailable_boundary_tree_state_skips_without_sync() {
    let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
    let recipient_address = get_base_address_macro!(recipient, "unified");

    from_inputs::quick_send(&mut faucet, vec![(&recipient_address, 100_000, None)])
        .await
        .unwrap();
    // One leap past the first bucket boundary: far enough that the boundary
    // checkpoint is pruned, short of the second boundary.
    increase_height_and_wait_for_client(&local_net, &mut recipient, 450)
        .await
        .unwrap();

    let (known_height, bucket_modulus) = {
        let wallet = recipient.wallet().read().await;
        (
            wallet
                .sync_state
                .last_known_chain_height()
                .expect("the wallet has synced"),
            MigrationParams::provisional(wallet.chain_type()).bucket_modulus,
        )
    };
    let current_bucket = schedule::bucket_index(known_height, bucket_modulus);
    assert!(
        current_bucket >= 1,
        "the chain must have crossed at least one bucket boundary"
    );

    let notes = orchard_note_records(&recipient).await;
    let bound = note_by_value(&notes, 100_000);
    inject_scheduled_migration(
        &recipient,
        vec![(100_000, bound.output_id, bound.nullifier)],
        Some(current_bucket),
    )
    .await;

    // New blocks the wallet has not seen: a hidden sync inside the
    // broadcast path would advance the wallet's known height.
    generate_n_blocks_return_new_height(&local_net, 10).await;

    let sent = recipient.broadcast_due_parts().await.unwrap();
    assert!(sent.is_empty(), "nothing must be broadcast: {sent:?}");

    let wallet = recipient.wallet().read().await;
    let part = &wallet.migration.as_ref().unwrap().parts[0];
    assert_eq!(part.state, PartState::Assigned, "a skip writes nothing");
    assert_eq!(part.attempts, 0, "a skip records no attempt");
    assert!(part.anchor_witness.is_none());
    assert_eq!(
        wallet.sync_state.last_known_chain_height(),
        Some(known_height),
        "the broadcast path must never synchronize"
    );
}

/// The full two-phase run: note splitting to denomination-sized notes, then
/// one canonical part per note, ending with every part confirmed and the
/// migrated value equal to the sum of the planned denominations.
#[ignore = "pending Ironwood node support: pepper-sync V3 scanning, lightwalletd Ironwood \
            parser and zebra witness serving"]
#[tokio::test]
async fn two_phase_migration_end_to_end() {
    let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
    let recipient_address = get_base_address_macro!(recipient, "unified");

    // An amount that quantizes into several denominations plus dust.
    from_inputs::quick_send(&mut faucet, vec![(&recipient_address, 1_250_000, None)])
        .await
        .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    let plan = recipient
        .plan_ironwood_migration(AccountId::ZERO)
        .await
        .unwrap();
    let expected_parts = plan.parts.clone();
    let expected_migrated: u64 = expected_parts.iter().sum();
    assert!(!expected_parts.is_empty());

    // The one-call waits on confirmations, so blocks are mined alongside it
    // until it returns.
    let summary = {
        let migrate = recipient.migrate_to_ironwood(AccountId::ZERO);
        tokio::pin!(migrate);
        loop {
            tokio::select! {
                result = &mut migrate => break result.unwrap(),
                () = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                    local_net.validator().generate_blocks(1).await.unwrap();
                }
            }
        }
    };
    assert_eq!(summary.part_txids.len(), expected_parts.len());

    let status = recipient.migration_status().await.unwrap();
    assert!(matches!(
        status.phase,
        Some(MigrationPhase::Complete { .. })
    ));
    assert_eq!(status.parts_confirmed, status.parts_total);
    // FIXME: assert the Ironwood pool balance directly once pepper-sync
    // scans Ironwood notes. Until then the confirmed part denominations
    // are the migrated value.
    assert_eq!(status.value_migrated, expected_migrated);
}
