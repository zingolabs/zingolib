use zcash_protocol::PoolType;
use zingo_status::confirmation_status::ConfirmationStatus;

use super::{FUNDING, funded_client, funding_at_next_height};
use crate::lightclient::LightClient;
use crate::testutils::lightclient::get_base_address;
use crate::testutils::mock_indexer::MockNet;

const RECEIVED: u64 = 20_000;

async fn status_of_received(client: &LightClient) -> ConfirmationStatus {
    client
        .transaction_summaries(false)
        .await
        .expect("a synced wallet summarizes its transactions")
        .0
        .iter()
        .find(|tx| tx.value == RECEIVED)
        .expect("the received transaction is recorded")
        .status
}

#[tokio::test]
async fn received_tx_status_pending_to_confirmed_with_mempool_monitor() {
    let mut net = MockNet::launch().await;
    let mut recipient = funded_client(&mut net, &[(PoolType::ORCHARD, FUNDING)], 1).await;
    let recipient_ua = get_base_address(&recipient, PoolType::ORCHARD).await;

    // The transaction is built for the very height it is then pinned to,
    // under the one lock, so no mining can slip between the two.
    let mempool_target_height = {
        let mut chain = net.chain.write().await;
        let target = chain.next_height();
        let incoming = funding_at_next_height(&chain, vec![(&recipient_ua, RECEIVED, None)]).await;
        chain.enter_mempool(incoming);
        target
    };

    recipient
        .sync_and_await()
        .await
        .expect("the mempool monitor records the incoming transaction");
    assert_eq!(
        status_of_received(&recipient).await,
        ConfirmationStatus::Mempool(mempool_target_height)
    );

    net.chain.write().await.mine_mempool();
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs the mined block");
    assert_eq!(
        status_of_received(&recipient).await,
        ConfirmationStatus::Confirmed(mempool_target_height)
    );
}
