use zcash_protocol::{PoolType, ShieldedPool};

use super::{FUNDING, HOSPITAL_MUSEUM, external_address, fund};
use crate::testutils::lightclient::{from_inputs, get_base_address};
use crate::testutils::mock_indexer::MockNet;
use crate::testutils::synthetic_wallet;

const PAYMENT: u64 = 10_000;

#[tokio::test]
async fn check_list_value_transfers_across_rescan() {
    let mut net = MockNet::launch().await;
    let mut recipient = net.client(HOSPITAL_MUSEUM, None).await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&recipient_ua, FUNDING, None)], 1).await;
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs its funding");

    let faucet_sapling = synthetic_wallet::external_sapling_address();
    let faucet_unified = external_address(PoolType::ORCHARD);
    from_inputs::quick_send(
        &mut recipient,
        vec![
            (faucet_sapling.as_str(), PAYMENT, Some("sapling with memo")),
            (faucet_sapling.as_str(), PAYMENT, None),
            (faucet_unified.as_str(), PAYMENT, Some("orchard with memo")),
            (faucet_unified.as_str(), PAYMENT, None),
        ],
    )
    .await
    .expect("the four-output send builds and transmits");
    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs the mined send");

    let pre_rescan_transactions = recipient
        .transaction_summaries(false)
        .await
        .expect("a synced wallet summarizes its transactions");
    let pre_rescan_summaries = recipient
        .value_transfers(true)
        .await
        .expect("a synced wallet lists its value transfers");
    recipient
        .rescan_and_await()
        .await
        .expect("a rescan from the birthday completes");
    let post_rescan_transactions = recipient
        .transaction_summaries(false)
        .await
        .expect("a rescanned wallet summarizes its transactions");
    let post_rescan_summaries = recipient
        .value_transfers(true)
        .await
        .expect("a rescanned wallet lists its value transfers");
    assert_eq!(pre_rescan_transactions, post_rescan_transactions);
    assert_eq!(pre_rescan_summaries, post_rescan_summaries);
}
