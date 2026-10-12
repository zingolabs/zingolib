//! Integration tests for the Orchard→Ironwood migration backend (ZIP 318).
//!
//! The full two-phase run needs Ironwood support along the whole node path
//! (pepper-sync V3 note scanning, the lightwalletd Ironwood parser and
//! zebra witness serving), so it is ignored until those land. The other
//! tests exercise the migration state machine against a live regtest chain
//! with today's stack: bound-note reservation, external-spend invalidation
//! and the no-sync transmit path.

use pepper_sync::wallet::{NoteInterface, OrchardNote, OutputId, OutputInterface};
use zcash_local_net::validator::Validator;
use zcash_primitives::transaction::TxId;
use zcash_protocol::PoolType;
use zingolib::get_base_address_macro;
use zingolib::lightclient::LightClient;
use zingolib::perspective::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransferKind, ValueTransfers,
};
use zingolib::testutils::lightclient::from_inputs;
use zingolib::wallet::migration::{
    BoundNote, ConsentBinding, MigrationParams, MigrationPhase, MigrationState, PartId, PartRecord,
    PartState, RecommendedAction, SigningStrategy,
};
use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};
use zingolib_testutils::setup_metrics::MeteredNet;
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

/// - Panics when a txid of `txids` has no migration value transfer.
fn migrated_value(value_transfers: &ValueTransfers, txids: &[TxId]) -> u64 {
    txids
        .iter()
        .map(|txid| {
            value_transfers
                .iter()
                .find(|vt| {
                    vt.txid == *txid
                        && vt.kind
                            == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                SelfSendValueTransfer::Migration,
                            ))
                })
                .unwrap_or_else(|| {
                    panic!("{txid:?} must be classified as a migration value transfer")
                })
                .value
        })
        .sum()
}

/// Persists a hand-built [`MigrationState`] whose parts are bound to real
/// wallet notes, standing in for the completed note-splitting phase so the
/// Phase 2 machinery can be exercised before Ironwood lands on the node
/// path. `bucket_index` assigns every part to that bucket. `None` leaves
/// them bound but unscheduled. `bucket_modulus` overrides the provisional
/// bucket geometry: every consumer of the schedule reads it from the
/// injected state, so a test can shrink the chain it must mine.
async fn inject_scheduled_migration(
    client: &LightClient,
    bound: Vec<(u64, OutputId, [u8; 32])>,
    bucket_index: Option<u64>,
    bucket_modulus: Option<u32>,
) {
    let mut wallet = client.wallet().write().await;
    let mut params = MigrationParams::provisional(wallet.chain_type());
    if let Some(bucket_modulus) = bucket_modulus {
        params.bucket_modulus = bucket_modulus;
    }
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
        mode: zingolib::wallet::migration::MigrationMode::Scheduled,
        account: AccountId::ZERO,
        phase: MigrationPhase::PartsScheduled,
        parts,
    });
}

/// A part's bound note is excluded from ordinary input selection while
/// another note can satisfy the request, the fallback pass consumes it when
/// nothing else can, and the external spend then invalidates the part on
/// reconciliation. The remainder left behind sits exactly at the Sweep
/// Minimum, so under the ratified completion rule (#2493 finding 8) no
/// replan is offered (a replan would strand everything it planned), and
/// the next reconciliation concludes the migration with the remainder
/// disclosed as its residual.
#[tokio::test]
async fn bound_note_reservation_and_external_spend_invalidation() {
    // Two V2 Orchard notes: the 100_000 note plays a bound split note, the
    // 50_000 note is free. The whole test then stays below the activation
    // height: reservation and reconciliation are era-independent, and
    // pre-activation orchard-only sends keep the note values' fee
    // arithmetic balanced.
    let (local_net, faucet, mut recipient) =
        pre_ironwood_funded_recipient(|_| vec![100_000, 50_000]).await;
    let faucet_address = get_base_address_macro!(faucet, "unified");

    let notes = orchard_note_records(&recipient).await;
    let reserved = note_by_value(&notes, 100_000);
    inject_scheduled_migration(
        &recipient,
        vec![(100_000, reserved.output_id, reserved.nullifier)],
        None,
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
    // The surviving change note is exactly the Sweep Minimum (10_000):
    // not worth replanning, so no replan is offered (ratified completion
    // rule; the prior behavior offered a replan that could only strand).
    assert!(
        !report
            .actions
            .iter()
            .any(|action| matches!(action, RecommendedAction::ReplanRemainder)),
        "a remainder at the Sweep Minimum must not prompt a replan: {:?}",
        report.actions
    );
    {
        let wallet = recipient.wallet().read().await;
        assert_eq!(
            wallet.migration.as_ref().unwrap().parts[0].state,
            PartState::Invalidated
        );
    }

    // With every part terminal and nothing worth replanning, the next
    // reconciliation concludes the migration, disclosing the stranded
    // remainder as its residual.
    let report = recipient.reconcile_migration().await.unwrap();
    assert!(
        report
            .actions
            .iter()
            .any(|action| matches!(action, RecommendedAction::MarkComplete { residual: 10_000 })),
        "an all-terminal migration with a Sweep-Minimum remainder must \
         complete: {:?}",
        report.actions
    );
    let wallet = recipient.wallet().read().await;
    assert_eq!(
        wallet.migration.as_ref().unwrap().phase,
        MigrationPhase::Complete { residual: 10_000 }
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

    // The state machine's own accounting of the migrated value...
    assert_eq!(status.value_migrated, expected_migrated);
    // ...and the same value scanned back out of the Ironwood pool.
    let confirmed_ironwood = recipient
        .wallet()
        .read()
        .await
        .account_balance(AccountId::ZERO)
        .unwrap()
        .confirmed_ironwood_balance
        .map(|zats| zats.into_u64())
        .unwrap_or(0);
    assert_eq!(
        confirmed_ironwood, expected_migrated,
        "the migrated value must appear in the Ironwood pool"
    );

    // Every confirmed part spends Orchard into the wallet's own Ironwood pool,
    // so it must be classified as a migration value transfer, not a plain
    // send-to-self, and those transfers carry the migrated value.
    let value_transfers = recipient.value_transfers(true).await.unwrap();
    assert_eq!(
        migrated_value(&value_transfers, &summary.part_txids),
        status.value_migrated,
        "the migration value transfers must carry the migrated value"
    );
}

/// The deferred NU6.3 activation height for scenarios that must fund the
/// recipient with pre-Ironwood notes first: far enough past the
/// funded-faucet setup tip ([`scenarios::FUNDED_FAUCET_SETUP_HEIGHT`]) that
/// the funding confirms before activation, close enough that crossing the
/// boundary stays cheap.
const DEFERRED_NU6_3_HEIGHT: u32 = 16;

/// The regtest activation-height fixture with NU6.3 deferred to `nu6_3`,
/// so a test can fund pre-Ironwood notes before the boundary and cross it
/// mid-test.
fn deferred_activation_heights(nu6_3: u32) -> zingolib::ActivationHeights {
    let fixture = scenarios::wallet_activation_heights(
        &zcash_local_net::validator::regtest_test_activation_heights(),
    );
    zingolib::ActivationHeights::builder()
        .set_overwinter(fixture.overwinter())
        .set_sapling(fixture.sapling())
        .set_blossom(fixture.blossom())
        .set_heartwood(fixture.heartwood())
        .set_canopy(fixture.canopy())
        .set_nu5(fixture.nu5())
        .set_nu6(fixture.nu6())
        .set_nu6_1(fixture.nu6_1())
        .set_nu6_2(fixture.nu6_2())
        .set_nu6_3(Some(nu6_3))
        .set_nu7(None)
        .build()
}

/// Launches a chain whose NU6.3 activation still lies ahead
/// ([`DEFERRED_NU6_3_HEIGHT`]) and funds the recipient with one
/// multi-output send, one pre-Ironwood (V2) Orchard note per value
/// produced by `values`. The chain is left below the activation height, so
/// tests that need the Ironwood pool live call
/// [`cross_ironwood_activation`] afterwards.
///
/// Every migration test needs this shape: the Turnstile forbids ordinary
/// payments into the Orchard pool from activation onward, so a recipient
/// funded on the default (already-activated) chain holds Ironwood notes
/// and never the V2 Orchard notes the migration machinery operates on.
/// The funding is a single transaction whatever the note count: a loop of
/// single-output sends exhausts the faucet's few confirmed notes, because
/// each send's change stays unconfirmed until a block is mined.
async fn pre_ironwood_funded_recipient(
    values: impl FnOnce(&MigrationParams) -> Vec<u64>,
) -> (MeteredNet, LightClient, LightClient) {
    let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient(
        PoolType::IRONWOOD,
        deferred_activation_heights(DEFERRED_NU6_3_HEIGHT),
        scenarios::ChainCachePolicy::PerTest,
    )
    .await;

    let note_values = {
        let wallet = recipient.wallet().read().await;
        values(&MigrationParams::provisional(wallet.chain_type()))
    };
    let recipient_address = get_base_address_macro!(recipient, "unified");
    let payments: Vec<(&str, u64, Option<&str>)> = note_values
        .iter()
        .map(|&value| (recipient_address.as_str(), value, None))
        .collect();
    from_inputs::quick_send(&mut faucet, payments)
        .await
        .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    let funded_tip = u32::from(
        recipient
            .wallet()
            .read()
            .await
            .sync_state
            .last_known_chain_height()
            .expect("the recipient has synced"),
    );
    assert!(
        funded_tip < DEFERRED_NU6_3_HEIGHT,
        "the funding must confirm before NU6.3 activates, but the chain is at {funded_tip}"
    );

    (local_net, faucet, recipient)
}

/// Mines the chain across the deferred NU6.3 activation boundary and syncs
/// the recipient past it, so the Ironwood pool is live. The immediate migrations need
/// this before building: an immediate migration sends into Ironwood, which does not exist
/// below the activation height.
async fn cross_ironwood_activation(local_net: &MeteredNet, recipient: &mut LightClient) {
    let tip = u32::from(
        recipient
            .wallet()
            .read()
            .await
            .sync_state
            .last_known_chain_height()
            .expect("the recipient has synced"),
    );
    assert!(
        tip < DEFERRED_NU6_3_HEIGHT,
        "the chain has already crossed the activation boundary (tip {tip})"
    );
    increase_height_and_wait_for_client(local_net, recipient, DEFERRED_NU6_3_HEIGHT - tip + 1)
        .await
        .unwrap();
}

/// The immediate migration: every spendable Orchard note is spent into Ironwood in
/// one pass, with no note splitting and no schedule. The Orchard pool empties
/// down to the disclosed dust, and the same value less fees appears in the
/// Ironwood pool. That second half is what proves the funds actually crossed.
#[tokio::test]
async fn migrate_all_orchard_to_ironwood() {
    // Several notes of unequal value: an immediate migration must sweep all of them, and
    // nothing here is a canonical denomination.
    let (local_net, _faucet, mut recipient) =
        pre_ironwood_funded_recipient(|_| vec![317_000, 1_250_000, 88_000]).await;
    cross_ironwood_activation(&local_net, &mut recipient).await;

    let orchard_before = recipient
        .wallet()
        .read()
        .await
        .account_balance(AccountId::ZERO)
        .unwrap()
        .confirmed_orchard_balance
        .unwrap()
        .into_u64();
    assert_eq!(orchard_before, 317_000 + 1_250_000 + 88_000);

    let plan = recipient
        .plan_immediate_migration(AccountId::ZERO)
        .await
        .unwrap();
    // Three modest notes fit one transaction: no chunking here.
    assert_eq!(plan.transactions.len(), 1);
    assert_eq!(
        plan.migrated + plan.fee + plan.residual,
        orchard_before,
        "the plan must account for every zatoshi"
    );

    let summary = recipient
        .migrate_immediately(AccountId::ZERO)
        .await
        .unwrap();
    assert_eq!(summary.txids.len(), plan.transactions.len());
    assert_eq!(summary.migrated, plan.migrated);

    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    let balance = recipient
        .wallet()
        .read()
        .await
        .account_balance(AccountId::ZERO)
        .unwrap();
    assert_eq!(
        balance
            .confirmed_orchard_balance
            .map(|zats| zats.into_u64())
            .unwrap_or(0),
        summary.residual,
        "the Orchard pool must be empty but for the disclosed dust"
    );
    assert_eq!(
        balance
            .confirmed_ironwood_balance
            .map(|zats| zats.into_u64())
            .unwrap_or(0),
        summary.migrated,
        "the immediate migrationed value must appear in the Ironwood pool"
    );

    // The immediate migration moves Orchard funds into the wallet's own Ironwood pool, so
    // each immediate migration transaction must be classified as a migration value transfer,
    // not a plain send-to-self, and those transfers carry the migrated value.
    let value_transfers = recipient.value_transfers(true).await.unwrap();
    assert_eq!(
        migrated_value(&value_transfers, &summary.txids),
        summary.migrated,
        "the migration value transfers must carry the migrated value"
    );
}

/// An immediate migration of a fragmented wallet chunks into several independent transactions,
/// all built and transmitted in the same pass, and still empties the pool.
#[tokio::test]
async fn immediate_migration_chunks_a_fragmented_wallet() {
    // More notes than fit one transaction's action budget, funded in one
    // transaction. Distinct values keep every payment in the ZIP-321
    // request unique.
    let (local_net, _faucet, mut recipient) = pre_ironwood_funded_recipient(|params| {
        (0..params.max_actions_per_split_tx as u64 + 3)
            .map(|index| 100_000 + index)
            .collect()
    })
    .await;
    cross_ironwood_activation(&local_net, &mut recipient).await;

    let plan = recipient
        .plan_immediate_migration(AccountId::ZERO)
        .await
        .unwrap();
    assert_eq!(
        plan.transactions.len(),
        2,
        "the notes must chunk into two transactions"
    );

    let summary = recipient
        .migrate_immediately(AccountId::ZERO)
        .await
        .unwrap();
    assert_eq!(summary.txids.len(), 2);

    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    let balance = recipient
        .wallet()
        .read()
        .await
        .account_balance(AccountId::ZERO)
        .unwrap();
    assert_eq!(
        balance
            .confirmed_orchard_balance
            .map(|zats| zats.into_u64())
            .unwrap_or(0),
        summary.residual
    );
    assert_eq!(
        balance
            .confirmed_ironwood_balance
            .map(|zats| zats.into_u64())
            .unwrap_or(0),
        summary.migrated
    );
}
