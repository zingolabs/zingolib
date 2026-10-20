use zcash_protocol::{PoolType, ShieldedPool};
use zip32::AccountId;

use super::{DEFERRED_NU6_3, confirmed_balance, funded_client, migrated_value};
use crate::lightclient::LightClient;
use crate::testutils::mock_activation_heights_deferring_nu6_3;
use crate::testutils::mock_indexer::{MockChain, MockNet};
use crate::wallet::migration::MigrationParams;

const UNEQUAL_NOTES: [u64; 3] = [317_000, 1_250_000, 88_000];
const FRAGMENT_BASE: u64 = 100_000;
const FRAGMENTS_BEYOND_ONE_TRANSACTION: u64 = 3;
const CHUNKED_TRANSACTIONS: usize = 2;
const BLOCKS_PAST_ACTIVATION: u32 = 1;

fn runway_past_activation(tip: u32, activation: u32) -> Option<u32> {
    super::blocks_to_reach(tip, activation)
        .map(|to_activation| to_activation + BLOCKS_PAST_ACTIVATION)
}

#[test]
fn the_runway_lands_the_tip_one_block_past_the_activation() {
    for (tip, activation) in [(2, 130), (101, 200), (5, 6)] {
        let runway = runway_past_activation(tip, activation)
            .expect("each tip in the table sits below its activation");
        assert_eq!(
            tip + runway,
            activation + BLOCKS_PAST_ACTIVATION,
            "tip {tip}, activation {activation}"
        );
    }
}

#[test]
fn a_tip_at_or_past_the_activation_has_no_runway() {
    assert_eq!(runway_past_activation(130, 130), None);
    assert_eq!(runway_past_activation(131, 130), None);
}

/// - Launches a chain whose NU6.3 activation still lies ahead and mines one funding block carrying
///   one pre-Ironwood Orchard note per value, then syncs the recipient below the activation.
async fn pre_ironwood_funded_recipient(
    values: impl FnOnce(&MigrationParams) -> Vec<u64>,
) -> (MockNet, LightClient) {
    let mut net = MockNet::launch_with(MockChain::with_activation_heights(
        mock_activation_heights_deferring_nu6_3(DEFERRED_NU6_3),
    ))
    .await;
    let receivers: Vec<(PoolType, u64)> = values(&MigrationParams::provisional(net.chain_type()))
        .into_iter()
        .map(|value| (PoolType::ORCHARD, value))
        .collect();
    let recipient = funded_client(&mut net, &receivers, 0).await;
    (net, recipient)
}

/// - Mines the chain across the deferred NU6.3 activation boundary and syncs the recipient past it.
async fn cross_ironwood_activation(net: &MockNet, recipient: &mut LightClient) {
    let tip = net.chain.read().await.tip();
    let activation = u32::from(super::ironwood_activation(net).height());
    let runway = runway_past_activation(tip, activation).unwrap_or_else(|| {
        panic!(
            "the funding must confirm below the activation at {activation}, but the chain is at \
             {tip}"
        )
    });
    net.chain.write().await.mine_empty_blocks(runway);
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs across the activation");
}

#[tokio::test]
async fn migrate_all_orchard_to_ironwood() {
    // The immediate migration: every spendable Orchard note is spent into Ironwood in
    // one pass, with no note splitting and no schedule. The Orchard pool empties
    // down to the disclosed dust, and the same value less fees appears in the
    // Ironwood pool. That second half is what proves the funds actually crossed.
    // Several notes of unequal value: an immediate migration must sweep all of them, and
    // nothing here is a canonical denomination.
    let (net, mut recipient) = pre_ironwood_funded_recipient(|_| UNEQUAL_NOTES.to_vec()).await;
    cross_ironwood_activation(&net, &mut recipient).await;

    let orchard_before = confirmed_balance(&recipient, ShieldedPool::Orchard).await;
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

    let orchard_after = confirmed_balance(&recipient, ShieldedPool::Orchard).await;
    let ironwood_after = confirmed_balance(&recipient, ShieldedPool::Ironwood).await;
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

#[tokio::test]
async fn immediate_migration_chunks_a_fragmented_wallet() {
    // An immediate migration of a fragmented wallet chunks into several independent
    // transactions, all built and transmitted in the same pass, and still empties the pool.
    // More notes than fit one transaction's action budget, funded in one
    // transaction. Distinct values keep every payment in the ZIP-321
    // request unique.
    let (net, mut recipient) = pre_ironwood_funded_recipient(|params| {
        (0..params.max_actions_per_split_tx as u64 + FRAGMENTS_BEYOND_ONE_TRANSACTION)
            .map(|index| FRAGMENT_BASE + index)
            .collect()
    })
    .await;
    cross_ironwood_activation(&net, &mut recipient).await;

    let plan = recipient
        .plan_immediate_migration(AccountId::ZERO)
        .await
        .expect("the immediate migration plans over the synced notes");
    assert_eq!(
        plan.transactions.len(),
        CHUNKED_TRANSACTIONS,
        "the notes must chunk into two transactions"
    );

    let summary = recipient
        .migrate_immediately(AccountId::ZERO)
        .await
        .expect("the chunked immediate migration builds and transmits");
    assert_eq!(summary.txids.len(), CHUNKED_TRANSACTIONS);

    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs the mined migration chunks");

    let orchard_after = confirmed_balance(&recipient, ShieldedPool::Orchard).await;
    let ironwood_after = confirmed_balance(&recipient, ShieldedPool::Ironwood).await;
    assert_eq!(orchard_after, summary.residual);
    assert_eq!(ironwood_after, summary.migrated);
}
