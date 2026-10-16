use zcash_protocol::{PoolType, ShieldedPool};

use super::{DEFERRED_NU6_3, FUNDING, HOSPITAL_MUSEUM, fund_on_chain};
use crate::check_client_balances;
use crate::testutils::lightclient::get_base_address;
use crate::testutils::mock_activation_heights_deferring_nu6_3;
use crate::testutils::mock_indexer::{MockChain, MockNet};

#[tokio::test]
async fn send_pre_ironwood() {
    let mut net = MockNet::launch_with(MockChain::with_activation_heights(
        mock_activation_heights_deferring_nu6_3(DEFERRED_NU6_3),
    ))
    .await;
    let mut recipient = net.client(HOSPITAL_MUSEUM, None).await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    net.chain.write().await.mine_empty_blocks(1);
    fund_on_chain(&net, vec![(&recipient_ua, FUNDING, None)]).await;
    net.chain.write().await.mine_empty_blocks(1);
    recipient
        .sync_and_await()
        .await
        .expect("the recipient syncs its pre-Ironwood funding");
    check_client_balances!(recipient, i: 0 o: FUNDING s: 0 t: 0);
}
