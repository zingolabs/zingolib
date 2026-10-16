use zcash_primitives::transaction::TxId;
use zcash_protocol::{PoolType, ShieldedPool};
use zip32::AccountId;

use super::{HOSPITAL_MUSEUM, fund_on_chain};
use crate::lightclient::LightClient;
use crate::perspective::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransferKind, ValueTransfers,
};
use crate::testutils::lightclient::get_base_address;
use crate::testutils::mock_activation_heights_deferring_nu6_3;
use crate::testutils::mock_indexer::{MockChain, MockNet};
use crate::wallet::migration::MigrationParams;

/// The deferred NU6.3 activation height: past the funding, close enough that
/// crossing the boundary stays cheap.
const DEFERRED_NU6_3_HEIGHT: u32 = 16;
const UNEQUAL_NOTES: [u64; 3] = [317_000, 1_250_000, 88_000];

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

/// - Launches a chain whose NU6.3 activation still lies ahead and mines one funding block carrying
///   one pre-Ironwood Orchard note per value, then syncs the recipient below the activation.
async fn pre_ironwood_funded_recipient(
    values: impl FnOnce(&MigrationParams) -> Vec<u64>,
) -> (MockNet, LightClient) {
    let mut net = MockNet::launch_with(MockChain::with_activation_heights(
        mock_activation_heights_deferring_nu6_3(DEFERRED_NU6_3_HEIGHT),
    ))
    .await;
    let mut recipient = net.client(HOSPITAL_MUSEUM, None).await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    net.chain.write().await.mine_empty_blocks(1);
    let note_values = values(&MigrationParams::provisional(recipient.chain_type()));
    let payments: Vec<(&str, u64, Option<&str>)> = note_values
        .iter()
        .map(|&value| (recipient_ua.as_str(), value, None))
        .collect();
    fund_on_chain(&net, payments).await;
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs its pre-Ironwood notes");
    let funded_tip = net.chain.read().await.tip();
    assert!(
        funded_tip < DEFERRED_NU6_3_HEIGHT,
        "the funding must confirm before NU6.3 activates, but the chain is at {funded_tip}"
    );
    (net, recipient)
}

/// - Mines the chain across the deferred NU6.3 activation boundary and syncs the recipient past it.
async fn cross_ironwood_activation(net: &MockNet, recipient: &mut LightClient) {
    let tip = net.chain.read().await.tip();
    assert!(
        tip < DEFERRED_NU6_3_HEIGHT,
        "the chain has already crossed the activation boundary (tip {tip})"
    );
    net.chain
        .write()
        .await
        .mine_empty_blocks(DEFERRED_NU6_3_HEIGHT - tip + 1);
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs across the activation");
}

async fn confirmed_orchard_and_ironwood(client: &LightClient) -> (u64, u64) {
    let balance = client
        .wallet()
        .read()
        .await
        .account_balance(AccountId::ZERO)
        .expect("a synced wallet reports a balance");
    (
        balance
            .confirmed_orchard_balance
            .map(|zats| zats.into_u64())
            .unwrap_or(0),
        balance
            .confirmed_ironwood_balance
            .map(|zats| zats.into_u64())
            .unwrap_or(0),
    )
}

/// The immediate migration: every spendable Orchard note is spent into Ironwood in
/// one pass, with no note splitting and no schedule. The Orchard pool empties
/// down to the disclosed dust, and the same value less fees appears in the
/// Ironwood pool. That second half is what proves the funds actually crossed.
#[tokio::test]
async fn migrate_all_orchard_to_ironwood() {
    // Several notes of unequal value: an immediate migration must sweep all of them, and
    // nothing here is a canonical denomination.
    let (net, mut recipient) = pre_ironwood_funded_recipient(|_| UNEQUAL_NOTES.to_vec()).await;
    cross_ironwood_activation(&net, &mut recipient).await;

    let (orchard_before, _) = confirmed_orchard_and_ironwood(&recipient).await;
    assert_eq!(orchard_before, UNEQUAL_NOTES.iter().sum::<u64>());

    let plan = recipient
        .plan_immediate_migration(AccountId::ZERO)
        .await
        .expect("the immediate migration plans over the synced notes");
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
        .expect("the immediate migration builds and transmits");
    assert_eq!(summary.txids.len(), plan.transactions.len());
    assert_eq!(summary.migrated, plan.migrated);

    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs the mined migration");

    let (orchard_after, ironwood_after) = confirmed_orchard_and_ironwood(&recipient).await;
    assert_eq!(
        orchard_after, summary.residual,
        "the Orchard pool must be empty but for the disclosed dust"
    );
    assert_eq!(
        ironwood_after, summary.migrated,
        "the immediately migrated value must appear in the Ironwood pool"
    );

    // The immediate migration moves Orchard funds into the wallet's own Ironwood pool, so
    // each immediate migration transaction must be classified as a migration value transfer,
    // not a plain send-to-self, and those transfers carry the migrated value.
    let value_transfers = recipient
        .value_transfers(true)
        .await
        .expect("a synced wallet lists its value transfers");
    assert_eq!(
        migrated_value(&value_transfers, &summary.txids),
        summary.migrated,
        "the migration value transfers must carry the migrated value"
    );
}

