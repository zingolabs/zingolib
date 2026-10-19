use crate::testutils::mock_activation_heights_with;
use crate::testutils::mock_indexer::{MockChain, MockNet};

const NU5_HEIGHT: u32 = 2;
const NU6_X_HEIGHT: u32 = 5;
const EPOCH_RUNWAY: u32 = 12;

#[tokio::test]
async fn sync_all_expressible_epochs() {
    let activation_heights = mock_activation_heights_with(|era| {
        era.set_nu5(Some(NU5_HEIGHT))
            .set_nu6(Some(NU5_HEIGHT))
            .set_nu6_1(Some(NU6_X_HEIGHT))
            .set_nu6_2(Some(NU6_X_HEIGHT))
            .set_nu6_3(Some(NU6_X_HEIGHT))
            .set_nu7(None)
    });
    let mut net =
        MockNet::launch_with(MockChain::with_activation_heights(activation_heights)).await;
    let mut client = net.client(super::HOSPITAL_MUSEUM, None).await;
    net.chain.write().await.mine_empty_blocks(EPOCH_RUNWAY);
    client
        .sync_and_await()
        .await
        .expect("a wallet syncs across every expressible epoch boundary");
}
