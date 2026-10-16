use super::{external_address, fund};
use crate::testutils::mock_indexer::{MockNet, faucet_funding_transaction_on};

mod received_mempool_status;
mod rescan_self_send;
mod rescan_value_transfers;
mod send_pre_ironwood;
mod untracked_history;
mod watch_only;
mod sync_all_expressible_epochs;
mod unified_address_discovery;

const HOSPITAL_MUSEUM: &str = zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
const FUNDING: u64 = 100_000;
const DEFERRED_NU6_3: u32 = 130;

/// - Mines one block carrying a funding transaction built for the chain's own era at its next height.
async fn fund_on_chain(net: &MockNet, receivers: Vec<(&str, u64, Option<&str>)>) {
    let (activation_heights, height) = {
        let chain = net.chain.read().await;
        (chain.activation_heights(), chain.next_height())
    };
    let funding = faucet_funding_transaction_on(activation_heights, height, receivers).await;
    net.chain.write().await.mine_block(vec![funding]);
}
