use zcash_protocol::{PoolType, ShieldedPool};

use super::{HOSPITAL_MUSEUM, fund};
use crate::testutils::lightclient::{from_inputs, get_base_address};
use crate::testutils::mock_indexer::MockNet;

const SENDER_FUNDING: u64 = 1_000_000;
const SELF_SEND: u64 = 100_000;

#[tokio::test]
async fn self_send() {
    let mut net = MockNet::launch().await;
    let mut sender = net.client(HOSPITAL_MUSEUM, None).await;
    let sender_ua = get_base_address(&sender, PoolType::Shielded(ShieldedPool::Orchard)).await;
    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&sender_ua, SENDER_FUNDING, None)], 1).await;
    sender
        .sync_and_await()
        .await
        .expect("the sender syncs its funding");

    let sender_sapling = get_base_address(&sender, PoolType::Shielded(ShieldedPool::Sapling)).await;
    for memo in [None, Some("Second Transaction")] {
        from_inputs::quick_send(
            &mut sender,
            vec![(sender_sapling.as_str(), SELF_SEND, memo)],
        )
        .await
        .expect("a self-send to the sender's sapling receiver builds and transmits");
        net.chain.write().await.mine_mempool();
        sender
            .sync_and_await()
            .await
            .expect("the sender syncs the mined self-send");
    }

    let pre_rescan_summaries = sender
        .transaction_summaries(false)
        .await
        .expect("a synced wallet summarizes its transactions");
    sender
        .rescan_and_await()
        .await
        .expect("a rescan from the birthday completes");
    let post_rescan_summaries = sender
        .transaction_summaries(false)
        .await
        .expect("a rescanned wallet summarizes its transactions");
    assert_eq!(pre_rescan_summaries, post_rescan_summaries);
}
