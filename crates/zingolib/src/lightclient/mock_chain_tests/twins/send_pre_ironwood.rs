use zcash_protocol::PoolType;

use super::{DEFERRED_NU6_3, FUNDING, funded_client};
use crate::check_client_balances;
use crate::testutils::mock_activation_heights_deferring_nu6_3;
use crate::testutils::mock_indexer::{MockChain, MockNet};

#[tokio::test]
async fn send_pre_ironwood() {
    let mut net = MockNet::launch_with(MockChain::with_activation_heights(
        mock_activation_heights_deferring_nu6_3(DEFERRED_NU6_3),
    ))
    .await;
    let recipient = funded_client(&mut net, &[(PoolType::ORCHARD, FUNDING)], 1).await;
    check_client_balances!(recipient, i: 0 o: FUNDING s: 0 t: 0);
}
