use std::time::{Duration, Instant};

use zcash_protocol::{PoolType, ShieldedPool};
use zip32::AccountId;

use super::{HOSPITAL_MUSEUM, continuous_sync_wallet_settings, fund_on_chain, migrated_value};
use crate::testutils::lightclient::get_base_address;
use crate::testutils::mock_activation_heights_deferring_nu6_3;
use crate::testutils::mock_indexer::{MockChain, MockNet};
use crate::wallet::migration::MigrationPhase;

/// The deferred NU6.3 activation height: past the funding, close enough that
/// crossing the boundary stays cheap.
const DEFERRED_NU6_3_HEIGHT: u32 = 16;
/// An amount that quantizes into several denominations plus dust.
const FUNDING: u64 = 1_250_000;
const MINING_INTERVAL: Duration = Duration::from_millis(500);
const MIGRATION_TIME_LIMIT: Duration = Duration::from_secs(300);

/// - Launches continuous sync on the recipient and mines the mock mempool every half second
///   until the two-phase migration returns.
#[tokio::test]
async fn two_phase_migration_end_to_end() {
    let mut net = MockNet::launch_with(MockChain::with_activation_heights(
        mock_activation_heights_deferring_nu6_3(DEFERRED_NU6_3_HEIGHT),
    ))
    .await;
    let mut recipient = net
        .client(HOSPITAL_MUSEUM, Some(continuous_sync_wallet_settings()))
        .await;
    recipient.set_transmit_retry_interval(Duration::ZERO);
    let recipient_address =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    net.chain.write().await.mine_empty_blocks(1);
    fund_on_chain(&net, vec![(&recipient_address, FUNDING, None)]).await;
    {
        let mut chain = net.chain.write().await;
        let tip = chain.tip();
        assert!(tip < DEFERRED_NU6_3_HEIGHT);
        chain.mine_empty_blocks(DEFERRED_NU6_3_HEIGHT - tip + 1);
    }
    recipient
        .sync_to_tip_and_await()
        .await
        .expect("the recipient syncs its Orchard note across the activation");

    let plan = recipient
        .plan_ironwood_migration(AccountId::ZERO)
        .await
        .expect("the two-phase migration plans over the synced note");
    let expected_parts = plan.parts.clone();
    let expected_migrated: u64 = expected_parts.iter().sum();
    assert!(!expected_parts.is_empty());

    // The one-call waits on confirmations, so blocks are mined alongside it
    // until it returns.
    recipient.sync().await.expect("continuous sync launches");
    let summary = {
        let migrate = recipient.migrate_to_ironwood(AccountId::ZERO);
        tokio::pin!(migrate);
        let deadline = Instant::now() + MIGRATION_TIME_LIMIT;
        loop {
            tokio::select! {
                result = &mut migrate => break result.expect("the two-phase migration completes"),
                () = tokio::time::sleep(MINING_INTERVAL) => {
                    assert!(
                        Instant::now() < deadline,
                        "the two-phase migration did not return within {MIGRATION_TIME_LIMIT:?}"
                    );
                    net.chain.write().await.mine_mempool();
                }
            }
        }
    };
    assert_eq!(summary.part_txids.len(), expected_parts.len());

    let status = recipient
        .migration_status()
        .await
        .expect("a completed migration reports its status");
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
        .expect("a synced wallet reports a balance")
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
    let value_transfers = recipient
        .value_transfers(true)
        .await
        .expect("a synced wallet lists its value transfers");
    assert_eq!(
        migrated_value(&value_transfers, &summary.part_txids),
        status.value_migrated,
        "the migration value transfers must carry the migrated value"
    );
}
