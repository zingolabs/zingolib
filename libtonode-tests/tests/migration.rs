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
use zingolib::get_base_address_macro;
use zingolib::perspective::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransferKind, ValueTransfers,
};
use zingolib::testutils::lightclient::from_inputs;
use zingolib::wallet::migration::MigrationPhase;
use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};
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
