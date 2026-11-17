use zcash_protocol::PoolType;

use super::{FUNDING, funded_client};
use crate::testutils::lightclient::from_inputs;
use crate::testutils::mock_indexer::MockNet;
use crate::testutils::synthetic_wallet;

const PAYMENT: u64 = 10_000;

#[tokio::test]
async fn check_list_value_transfers_across_rescan() {
    let mut net = MockNet::launch().await;
    let mut recipient = funded_client(&mut net, &[(PoolType::ORCHARD, FUNDING)], 1).await;

    let faucet_sapling = synthetic_wallet::external_address(PoolType::SAPLING);
    let faucet_unified = synthetic_wallet::external_address(PoolType::ORCHARD);
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
