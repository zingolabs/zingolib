//! Integration tests for the Orchard→Ironwood migration backend (ZIP 318).
//!
//! The full two-phase run needs Ironwood support along the whole node path
//! (pepper-sync V3 note scanning, the lightwalletd Ironwood parser and
//! zebra witness serving), so it is ignored until those land. The other
//! tests exercise the migration state machine against a live regtest chain
//! with today's stack: the funding-note reservation, external-spend
//! invalidation, the release of a transfer and the no-sync broadcast path.

use std::time::Duration;

use pepper_sync::wallet::{NoteInterface, OrchardNote, OutputId, OutputInterface};
use zcash_primitives::transaction::TxId;
use zcash_protocol::PoolType;
use zingolib::config::{ClientConfig, WalletConfig};
use zingolib::get_base_address_macro;
use zingolib::lightclient::LightClient;
use zingolib::lightclient::error::{LightClientError, MigrationError};
use zingolib::lightclient::migrate::{MigrationPlan, TransferBroadcastResult, TransferProgress};
use zingolib::perspective::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransferKind,
};
use zingolib::testutils::default_test_wallet_settings;
use zingolib::testutils::lightclient::from_inputs;
use zingolib::wallet::error::ProposeSendError;
use zingolib::wallet::migration::{
    BoundNote, MigrationMode, MigrationParams, MigrationPhase, MigrationState, TransferId,
    TransferRecord, TransferState, bucket_index,
};
use zingolib_testutils::scenarios::{
    self, generate_n_blocks_return_new_height, increase_height_and_wait_for_client,
};
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

fn note_by_id(notes: &[NoteRecord], output_id: OutputId) -> &NoteRecord {
    notes
        .iter()
        .find(|note| note.output_id == output_id)
        .expect("the note is still in the wallet")
}

/// Persists a hand-built [`MigrationState`] whose transfers are bound to
/// real wallet notes, standing in for a committed schedule so the broadcast
/// machinery can be exercised before Ironwood lands on the node path.
/// `bucket_index` assigns every transfer to that window. `None` leaves them
/// bound but unscheduled. `bucket_modulus` overrides the provisional bucket
/// geometry: every consumer of the schedule reads it from the injected
/// state, so a test can shrink the chain it must mine.
async fn inject_scheduled_migration(
    client: &LightClient,
    bound: Vec<(u64, OutputId, [u8; 32])>,
    bucket_index: Option<u64>,
    bucket_modulus: Option<u32>,
) {
    let mut wallet = client.wallet().write().await;
    let mut params = MigrationParams::provisional(wallet.chain_type());
    if let Some(bucket_modulus) = bucket_modulus {
        params = params.with_bucket_modulus(bucket_modulus);
    }
    let transfers = bound
        .into_iter()
        .enumerate()
        .map(|(index, (denomination, output_id, nullifier))| {
            let mut transfer = TransferRecord::new(
                TransferId(u32::try_from(index).expect("test transfer count fits u32")),
                denomination,
                BoundNote {
                    output_id,
                    nullifier,
                    commitment: [0; 32],
                },
            );
            if let Some(bucket_index) = bucket_index {
                transfer
                    .assign(bucket_index)
                    .expect("fresh transfers are bound");
            }
            transfer
        })
        .collect();
    wallet.set_migration(Some(MigrationState::scheduled_for_tests(
        params,
        transfers,
        AccountId::ZERO,
    )));
}

async fn stamp_newest_wallet_block_now(client: &LightClient) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs() as u32;
    client
        .wallet()
        .write()
        .await
        .wallet_blocks
        .values_mut()
        .next_back()
        .expect("a synced wallet holds blocks")
        .set_time_for_test(now);
}

async fn orchard_balances(client: &LightClient) -> (u64, u64) {
    let balance = client
        .wallet()
        .read()
        .await
        .account_balance(AccountId::ZERO)
        .unwrap();
    (
        balance
            .confirmed_orchard_balance
            .map_or(0, |zats| zats.into_u64()),
        balance
            .reserved_orchard_balance
            .map_or(0, |zats| zats.into_u64()),
    )
}

/// A second client restored from the recipient's seed into its own data
/// directory: the same keys, no migration state, so its sends are external
/// to the migration.
async fn twin_of(recipient: &LightClient) -> LightClient {
    let chain_type = recipient.wallet().read().await.chain_type();
    let wallet_dir = std::env::temp_dir().join(format!(
        "zingo-migration-twin-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&wallet_dir).unwrap();
    let config = ClientConfig::builder()
        .set_indexer_uri(
            recipient
                .indexer_uri()
                .expect("the recipient is connected to the local net"),
        )
        .set_chain_type(chain_type)
        .set_wallet_dir(wallet_dir)
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED.to_string(),
            no_of_accounts: 1.try_into().unwrap(),
            birthday: 1,
            wallet_settings: default_test_wallet_settings(),
        })
        .build()
        .unwrap();
    let mut twin = LightClient::new_clearnet_consented(config, true)
        .await
        .unwrap();
    twin.sync_and_await().await.unwrap();
    twin
}

/// The funding note of a scheduled transfer is reserved: an ordinary send
/// never selects it while a free note can pay, and a send only the reserved
/// note could pay is refused with the typed reservation error instead of
/// consuming it. Releasing the transfer frees the note again, and the
/// released transfer counts as terminal, so the migration completes with
/// the remaining Orchard balance disclosed as its residual.
#[tokio::test]
async fn a_reserved_funding_note_is_refused_to_ordinary_sends_until_released() {
    let (local_net, faucet, mut recipient) =
        pre_ironwood_funded_recipient(|_| vec![100_000, 50_000]).await;
    let faucet_address = get_base_address_macro!(faucet, "unified");

    let notes = orchard_note_records(&recipient).await;
    let reserved = note_by_value(&notes, 100_000);
    let reserved_id = reserved.output_id;
    inject_scheduled_migration(
        &recipient,
        vec![(100_000, reserved.output_id, reserved.nullifier)],
        None,
        None,
    )
    .await;
    assert_eq!(
        recipient.wallet().read().await.reserved_output_ids(),
        vec![reserved_id],
        "the scheduled transfer reserves exactly its funding note"
    );
    assert_eq!(
        orchard_balances(&recipient).await,
        (50_000, 100_000),
        "the confirmed Orchard balance excludes the reserved note, reported on its own"
    );

    from_inputs::quick_send(&mut recipient, vec![(&faucet_address, 20_000, None)])
        .await
        .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    let notes = orchard_note_records(&recipient).await;
    assert!(
        note_by_id(&notes, reserved_id)
            .spending_transaction
            .is_none(),
        "an ordinary send must not consume a reserved note while another note suffices"
    );
    assert!(
        note_by_value(&notes, 50_000).spending_transaction.is_some(),
        "the free note covers the ordinary send"
    );

    let refused = from_inputs::propose(&mut recipient, vec![(&faucet_address, 100_000, None)])
        .await
        .expect_err("a send only the reserved note could pay is refused");
    assert!(
        matches!(
            refused,
            ProposeSendError::ReservedForMigration { reserved: 100_000 }
        ),
        "the refusal carries the reserved value: {refused:?}"
    );
    let notes = orchard_note_records(&recipient).await;
    assert!(
        note_by_id(&notes, reserved_id)
            .spending_transaction
            .is_none(),
        "the refusal leaves the reserved note untouched"
    );

    recipient.release_transfer(TransferId(0)).await.unwrap();
    assert!(
        recipient
            .wallet()
            .read()
            .await
            .reserved_output_ids()
            .is_empty(),
        "a released transfer reserves nothing"
    );
    let status = recipient.migration_status().await.unwrap();
    assert_eq!(status.transfers[0].progress, TransferProgress::Released);

    from_inputs::quick_send(&mut recipient, vec![(&faucet_address, 100_000, None)])
        .await
        .expect("the released note pays the same send");
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    let notes = orchard_note_records(&recipient).await;
    assert!(
        note_by_id(&notes, reserved_id)
            .spending_transaction
            .is_some(),
        "the released note is spent by the ordinary send"
    );

    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    let (confirmed, reserved) = orchard_balances(&recipient).await;
    assert_eq!(reserved, 0);
    let wallet = recipient.wallet().read().await;
    assert_eq!(
        *wallet.migration().unwrap().phase(),
        MigrationPhase::Complete {
            residual: confirmed
        },
        "with its only transfer released the migration completes, disclosing the remaining \
         Orchard balance as its residual"
    );
}

/// A spend of a funding note from outside the migration (a second wallet on
/// the same seed, which holds no migration state) invalidates its transfer
/// on the next reconciliation, which runs after the synchronization that
/// sees the spend. With every transfer terminal the migration completes.
#[tokio::test]
async fn an_external_spend_of_a_funding_note_invalidates_its_transfer() {
    let (local_net, faucet, mut recipient) =
        pre_ironwood_funded_recipient(|_| vec![100_000, 50_000]).await;
    let faucet_address = get_base_address_macro!(faucet, "unified");

    let notes = orchard_note_records(&recipient).await;
    let reserved = note_by_value(&notes, 100_000);
    let reserved_id = reserved.output_id;
    inject_scheduled_migration(
        &recipient,
        vec![(100_000, reserved.output_id, reserved.nullifier)],
        None,
        None,
    )
    .await;

    let mut twin = twin_of(&recipient).await;
    assert_eq!(
        orchard_balances(&twin).await,
        (150_000, 0),
        "the twin holds the same notes and reserves nothing"
    );
    from_inputs::quick_send(&mut twin, vec![(&faucet_address, 100_000, None)])
        .await
        .expect("the twin spends both notes: the amount plus the fee exceeds either one");
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    let notes = orchard_note_records(&recipient).await;
    assert!(
        note_by_id(&notes, reserved_id)
            .spending_transaction
            .is_some(),
        "the recipient sees the external spend of its funding note"
    );
    let status = recipient.migration_status().await.unwrap();
    assert_eq!(
        status.transfers[0].progress,
        TransferProgress::Invalid,
        "the external spend invalidates the transfer: {status:?}"
    );
    {
        let wallet = recipient.wallet().read().await;
        assert_eq!(
            wallet.migration().unwrap().transfers()[0].state,
            TransferState::Invalidated,
            "reconciliation after the sync applies the invalidation"
        );
        assert!(
            wallet.reserved_output_ids().is_empty(),
            "an invalidated transfer reserves nothing"
        );
    }

    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    let (confirmed, _) = orchard_balances(&recipient).await;
    let wallet = recipient.wallet().read().await;
    assert_eq!(
        *wallet.migration().unwrap().phase(),
        MigrationPhase::Complete {
            residual: confirmed
        },
        "with its only transfer invalidated the migration completes, disclosing the change \
         the twin's send left behind as its residual"
    );
}

/// A due transfer whose boundary tree state is unavailable is skipped with no
/// writes and no synchronization (the ZIP 318 decoupling requirement). The
/// wallet leaps past the boundary in a single sync, so the boundary
/// checkpoint falls outside shardtree's retention window and the witness
/// was never captured.
#[tokio::test]
async fn unavailable_boundary_tree_state_skips_without_sync() {
    use pepper_sync::sync::MAX_REORG_ALLOWANCE;

    // The blocks mined behind the wallet's back before the broadcast
    // attempt: enough to prove the skip performs no hidden sync, few
    // enough to stay inside the bucket.
    const HIDDEN_BLOCKS: u32 = 10;
    // The smallest bucket modulus of the provisional value's power-of-two
    // family that exceeds shardtree's checkpoint retention
    // (`MAX_REORG_ALLOWANCE`, 100 blocks, mirroring zebra's finalization
    // boundary `zebra_state::MAX_BLOCK_REORG_HEIGHT`). The premise needs
    // the boundary checkpoint pruned while the tip is still inside the
    // bucket. Shrinking the modulus shrinks the chain: under the
    // provisional 256 this test leapt 450 blocks, and mining that many
    // blocks to a halo2 miner address outran even a 1200-second container
    // budget.
    const PRUNED_BUCKET_MODULUS: u32 = (MAX_REORG_ALLOWANCE + 1).next_power_of_two();
    // The tip to leap to, centered in the window that satisfies both
    // constraints: past the SECOND bucket boundary by more than the
    // retention, so that boundary's checkpoint is pruned, and far enough
    // below the third boundary that the hidden blocks stay inside the
    // bucket. The second boundary rather than the first, because the
    // broadcast path skips a transfer whose boundary lies below the NU6.3
    // activation, and the deferred activation below sits past the first.
    const TARGET_TIP: u32 = 2 * PRUNED_BUCKET_MODULUS
        + MAX_REORG_ALLOWANCE
        + (PRUNED_BUCKET_MODULUS - MAX_REORG_ALLOWANCE - HIDDEN_BLOCKS) / 2;

    use zcash_protocol::consensus::COINBASE_MATURITY_BLOCKS;

    // Transparent coinbase becomes spendable only after
    // [`COINBASE_MATURITY_BLOCKS`] confirmations (100 blocks, ZIP 213,
    // enforced by the validator), so shielding and funding can complete no
    // earlier; the deferred activation leaves margin beyond that, and
    // still lies below [`TARGET_TIP`] so the leap crosses it.
    const TRANSPARENT_DEFERRED_NU6_3: u32 = COINBASE_MATURITY_BLOCKS + 30;
    // The transfer is scheduled at the second bucket boundary; the broadcast
    // path requires that boundary to sit at or above the activation.
    const _: () = assert!(2 * PRUNED_BUCKET_MODULUS >= TRANSPARENT_DEFERRED_NU6_3);

    // A transparent miner keeps this test's long chain cheap: transparent
    // coinbase carries no halo2 proof, where a shielded miner pool costs
    // roughly 2.7 seconds of block assembly per block, the cost that
    // previously pushed this test past even a 1200-second budget.
    let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient(
        PoolType::Transparent,
        deferred_activation_heights(TRANSPARENT_DEFERRED_NU6_3),
        scenarios::ChainCachePolicy::PerTest,
    )
    .await;

    // Mature the faucet's coinbase, shield it into pre-Ironwood Orchard,
    // and fund the recipient with the note the transfer will bind, all
    // below the activation height.
    increase_height_and_wait_for_client(&local_net, &mut faucet, COINBASE_MATURITY_BLOCKS)
        .await
        .unwrap();
    faucet.quick_shield(AccountId::ZERO).await.unwrap();
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();
    let recipient_address = get_base_address_macro!(recipient, "unified");
    from_inputs::quick_send(&mut faucet, vec![(&recipient_address, 100_000, None)])
        .await
        .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    // One leap to the target tip, crossing the deferred NU6.3 activation
    // on the way.
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
        funded_tip < TRANSPARENT_DEFERRED_NU6_3,
        "the funding must confirm before NU6.3 activates, but the chain is at {funded_tip}"
    );
    increase_height_and_wait_for_client(&local_net, &mut recipient, TARGET_TIP - funded_tip)
        .await
        .unwrap();

    let known_height = {
        let wallet = recipient.wallet().read().await;
        wallet
            .sync_state
            .last_known_chain_height()
            .expect("the wallet has synced")
    };
    let current_bucket = bucket_index(known_height, PRUNED_BUCKET_MODULUS);
    assert_eq!(
        current_bucket, 2,
        "the chain must sit inside the second bucket, past its boundary"
    );

    let notes = orchard_note_records(&recipient).await;
    let bound = note_by_value(&notes, 100_000);
    inject_scheduled_migration(
        &recipient,
        vec![(100_000, bound.output_id, bound.nullifier)],
        Some(current_bucket),
        Some(PRUNED_BUCKET_MODULUS),
    )
    .await;

    // New blocks the wallet has not seen: a hidden sync inside the
    // broadcast path would advance the wallet's known height.
    generate_n_blocks_return_new_height(&local_net, HIDDEN_BLOCKS).await;

    stamp_newest_wallet_block_now(&recipient).await;
    let report = recipient
        .broadcast_due_transfers(Duration::ZERO)
        .await
        .unwrap();
    assert!(
        report.sent_txids().is_empty(),
        "nothing must be transmitted: {report:?}"
    );
    assert!(
        report.halted.is_none(),
        "a skip is not a failure: {report:?}"
    );
    assert!(
        report
            .outcomes
            .iter()
            .all(|outcome| outcome.result == TransferBroadcastResult::Slid),
        "an unwitnessable transfer slides to a coming window: {report:?}"
    );

    let wallet = recipient.wallet().read().await;
    let transfer = &wallet.migration().unwrap().transfers()[0];
    assert_eq!(
        transfer.state,
        TransferState::Assigned,
        "a skip writes nothing"
    );
    assert_eq!(transfer.attempts, 0, "a skip records no attempt");
    assert!(transfer.anchor_witness.is_none());
    assert_eq!(
        wallet.sync_state.last_known_chain_height(),
        Some(known_height),
        "the broadcast path must never synchronize"
    );
}

/// The full two-phase run through the explicit commands: commit the plan,
/// broadcast the note-preparation rounds until every note is a funding
/// note, commit a schedule, then broadcast each window's transfers, ending
/// with every transfer confirmed and the migrated value equal to the sum of
/// the planned denominations.
#[ignore = "pending Ironwood node support: pepper-sync V3 scanning, lightwalletd Ironwood \
            parser and zebra witness serving"]
#[tokio::test]
async fn two_phase_migration_end_to_end() {
    const BLOCK_BUDGET: u32 = 2_000;

    // An amount that quantizes into several denominations plus dust.
    let (local_net, _faucet, mut recipient) =
        pre_ironwood_funded_recipient(|_| vec![1_250_000]).await;
    cross_ironwood_activation(&local_net, &mut recipient).await;

    let plan = recipient
        .plan_migration(AccountId::ZERO, MigrationMode::Scheduled)
        .await
        .unwrap();
    let MigrationPlan::Scheduled(scheduled) = &plan else {
        panic!("a scheduled plan request yields a scheduled plan");
    };
    let expected_transfers = scheduled.transfers.clone();
    let expected_migrated: u64 = expected_transfers.iter().sum();
    assert!(!expected_transfers.is_empty());
    assert!(
        !scheduled.preparation_rounds.is_empty(),
        "a single large note needs note preparation"
    );

    recipient
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .unwrap();
    assert_eq!(
        recipient.migration_status().await.unwrap().phase,
        Some(MigrationPhase::Committed)
    );

    let mut preparation_txids = Vec::new();
    let mut blocks_mined = 0;
    loop {
        match recipient.broadcast_preparation_round().await {
            Ok(round) => preparation_txids.extend(round.txids),
            Err(LightClientError::MigrationError(MigrationError::AlreadyPrepared)) => break,
            Err(LightClientError::MigrationError(MigrationError::RoundPending { .. })) => {
                blocks_mined += 1;
                assert!(
                    blocks_mined < BLOCK_BUDGET,
                    "note preparation did not confirm within {BLOCK_BUDGET} blocks"
                );
                increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
                    .await
                    .unwrap();
            }
            Err(other) => panic!("note preparation failed: {other:?}"),
        }
    }
    assert!(!preparation_txids.is_empty());
    assert_eq!(
        recipient.migration_status().await.unwrap().phase,
        Some(MigrationPhase::Prepared)
    );

    let proposed = recipient.propose_schedule(1).await.unwrap();
    assert_eq!(proposed.transfers.len(), expected_transfers.len());
    recipient.commit_schedule(&proposed).await.unwrap();
    assert_eq!(
        recipient.migration_status().await.unwrap().phase,
        Some(MigrationPhase::Scheduled)
    );
    assert_eq!(
        recipient.wallet().read().await.reserved_output_ids().len(),
        expected_transfers.len(),
        "the committed schedule reserves one funding note per transfer"
    );

    let mut transfer_txids = Vec::new();
    let mut blocks_mined = 0;
    loop {
        stamp_newest_wallet_block_now(&recipient).await;
        let report = recipient
            .broadcast_due_transfers(Duration::ZERO)
            .await
            .unwrap();
        assert!(report.halted.is_none(), "{report:?}");
        transfer_txids.extend(report.sent_txids());
        if matches!(
            recipient.migration_status().await.unwrap().phase,
            Some(MigrationPhase::Complete { .. })
        ) {
            break;
        }
        blocks_mined += 1;
        assert!(
            blocks_mined < BLOCK_BUDGET,
            "the schedule did not complete within {BLOCK_BUDGET} blocks"
        );
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
    }
    assert_eq!(transfer_txids.len(), expected_transfers.len());

    let status = recipient.migration_status().await.unwrap();
    assert_eq!(status.transfers_confirmed, status.transfers_total);
    assert!(
        status
            .transfers
            .iter()
            .all(|transfer| transfer.progress == TransferProgress::Confirmed),
        "{status:?}"
    );

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

    // Every confirmed transfer spends Orchard into the wallet's own Ironwood
    // pool, so it must be classified as a migration value transfer, not a
    // plain send-to-self.
    let value_transfers = recipient.value_transfers(true).await.unwrap();
    for transfer_txid in &transfer_txids {
        assert!(
            value_transfers.iter().any(|vt| vt.txid == *transfer_txid
                && vt.kind
                    == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                        SelfSendValueTransfer::Migration,
                    ))),
            "transfer {transfer_txid:?} must be classified as a migration value transfer"
        );
    }
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
/// one pass, with no note preparation and no schedule. The Orchard pool empties
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

    recipient.sync_and_await().await.unwrap();
    let plan = recipient
        .plan_migration(AccountId::ZERO, MigrationMode::Immediate)
        .await
        .unwrap();
    let MigrationPlan::Immediate(immediate) = &plan else {
        panic!("an immediate plan request yields an immediate plan");
    };
    // Three modest notes fit one transaction: no chunking here.
    assert_eq!(immediate.transactions.len(), 1);
    assert_eq!(
        immediate.migrated + immediate.fee + immediate.residual,
        orchard_before,
        "the plan must account for every zatoshi"
    );
    let (expected_transactions, expected_migrated) =
        (immediate.transactions.len(), immediate.migrated);

    let summary = recipient
        .migrate_immediately(AccountId::ZERO, &plan)
        .await
        .unwrap();
    assert_eq!(summary.txids.len(), expected_transactions);
    assert_eq!(summary.migrated, expected_migrated);
    assert!(
        recipient.wallet().read().await.migration().is_none(),
        "an immediate migration stores no migration state"
    );

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
        "the immediately migrated value must appear in the Ironwood pool"
    );

    // The immediate migration moves Orchard funds into the wallet's own Ironwood pool, so
    // each immediate migration transaction must be classified as a migration value transfer,
    // not a plain send-to-self.
    let value_transfers = recipient.value_transfers(true).await.unwrap();
    for migration_txid in &summary.txids {
        assert!(
            value_transfers.iter().any(|vt| vt.txid == *migration_txid
                && vt.kind
                    == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                        SelfSendValueTransfer::Migration,
                    ))),
            "immediate migration {migration_txid:?} must be classified as a migration value transfer"
        );
    }
}

/// An immediate migration of a fragmented wallet chunks into several independent transactions,
/// all built and transmitted in the same pass, and still empties the pool.
#[tokio::test]
async fn immediate_migration_chunks_a_fragmented_wallet() {
    // More notes than fit one transaction's action budget, funded in one
    // transaction. Distinct values keep every payment in the ZIP-321
    // request unique.
    let (local_net, _faucet, mut recipient) = pre_ironwood_funded_recipient(|params| {
        (0..params.max_actions_per_split_tx() as u64 + 3)
            .map(|index| 100_000 + index)
            .collect()
    })
    .await;
    cross_ironwood_activation(&local_net, &mut recipient).await;

    recipient.sync_and_await().await.unwrap();
    let plan = recipient
        .plan_migration(AccountId::ZERO, MigrationMode::Immediate)
        .await
        .unwrap();
    let MigrationPlan::Immediate(immediate) = &plan else {
        panic!("an immediate plan request yields an immediate plan");
    };
    assert_eq!(
        immediate.transactions.len(),
        2,
        "the notes must chunk into two transactions"
    );

    let summary = recipient
        .migrate_immediately(AccountId::ZERO, &plan)
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
