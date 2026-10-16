//! Integration tests for the Orchard→Ironwood migration backend (ZIP 318).
//!
//! The full two-phase run needs Ironwood support along the whole node path
//! (pepper-sync V3 note scanning, the lightwalletd Ironwood parser and
//! zebra witness serving), so it is ignored until those land. The other
//! tests exercise the migration state machine against a live regtest chain
//! with today's stack: bound-note reservation, external-spend invalidation
//! and the no-sync transmit path.

use zcash_local_net::validator::Validator;
use zcash_primitives::transaction::TxId;
use zcash_protocol::PoolType;
use zingolib::get_base_address_macro;
use zingolib::lightclient::LightClient;
use zingolib::perspective::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransferKind, ValueTransfers,
};
use zingolib::testutils::lightclient::from_inputs;
use zingolib::wallet::migration::{MigrationParams, MigrationPhase};
use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};
use zingolib_testutils::setup_metrics::MeteredNet;
use zip32::AccountId;

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
