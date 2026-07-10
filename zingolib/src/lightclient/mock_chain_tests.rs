//! Offline twins of chain-bound libtonode tests, on the stateful mock
//! indexer ([`crate::testutils::mock_indexer`]): the real wallet
//! pipeline — `GrpcIndexer`, pepper-sync scanning, record building,
//! spend bookkeeping — driven against a fabricated in-process chain,
//! with no zebrad or zainod.
//!
//! Each twin's body lives ONCE in
//! [`crate::testutils::twin_fixtures`], generic over the environment;
//! this module instantiates the fixtures against
//! [`MockTwinChain`], and libtonode's `unit_test_twins` instantiates
//! the same fixtures live (gated, as the control group — user
//! direction, 2026-07-08). Environment divergences (funding height,
//! faucet-fee economics) are trait hooks documented on
//! [`crate::testutils::twin_fixtures::TwinChain`].

use zcash_protocol::PoolType;
use zcash_protocol::ShieldedPool;

use crate::check_client_balances;
use crate::testutils::lightclient::{from_inputs, get_base_address};
use crate::testutils::mock_indexer::{MockNet, MockTwinChain, faucet_funding_transaction};
use crate::testutils::synthetic_wallet::external_address;
use crate::testutils::twin_fixtures;

/// Funds `client` with one faucet-built transaction mined into the next
/// mock block, followed by `extra_blocks` empty blocks.
async fn fund(net: &MockNet, receivers: Vec<(&str, u64, Option<&str>)>, extra_blocks: u32) {
    let funding = faucet_funding_transaction(receivers).await;
    let mut chain = net.chain.write().await;
    chain.mine_block(vec![funding]);
    chain.mine_empty_blocks(extra_blocks);
}

/// The mock-net proof: a funding block scans to a confirmed balance,
/// a real quick_send round-trips through the mock's mempool into the
/// next block, and the post-confirmation balance carries the exact
/// ZIP-317 arithmetic.
#[tokio::test]
async fn funded_send_confirms_on_the_mock_chain() {
    let mut net = MockNet::launch().await;
    let mut recipient = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .await;
    let recipient_ua =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;

    net.chain.write().await.mine_empty_blocks(1);
    fund(&net, vec![(&recipient_ua, 100_000, None)], 1).await;

    recipient.sync_and_await().await.unwrap();
    check_client_balances!(recipient, o: 100_000 s: 0 t: 0);

    from_inputs::quick_send(
        &mut recipient,
        vec![(&external_address(PoolType::ORCHARD), 20_000, None)],
    )
    .await
    .unwrap();
    net.chain.write().await.mine_mempool();
    recipient.sync_and_await().await.unwrap();

    // 100_000 funding minus the 20_000 payment and its 10_000 one-orchard-
    // spend, two-logical-action ZIP-317 fee.
    check_client_balances!(recipient, o: 70_000 s: 0 t: 0);
}

#[tokio::test]
async fn zero_value_receipts() {
    twin_fixtures::zero_value_receipts::<MockTwinChain>().await;
}

#[tokio::test]
async fn list_value_transfers_check_fees() {
    twin_fixtures::list_value_transfers_check_fees::<MockTwinChain>().await;
}

#[tokio::test]
async fn self_send_to_t_displays_as_one_transaction() {
    twin_fixtures::self_send_to_t_displays_as_one_transaction::<MockTwinChain>().await;
}

#[tokio::test]
async fn send_to_transparent_and_sapling_maintain_balance() {
    twin_fixtures::send_to_transparent_and_sapling_maintain_balance::<MockTwinChain>().await;
}

/// Ignored, not broken: pepper-sync's SUBTRACTIVE `darkside_test`
/// feature deletes transparent-address discovery at compile time, and
/// cargo feature unification enables it for every crate co-built with
/// darkside-tests (`makers test packages`, `--workspace`), so this
/// test's purely-transparent step-1 funding is silently never detected
/// in those invocation shapes while passing in `-p zingolib` ones. Runs
/// green via `--run-ignored` in a single-package invocation. Un-ignore
/// when the feature becomes runtime configuration.
#[ignore = "zingolabs/zingolib#2447: pepper-sync's subtractive darkside_test feature, when \
            unified into multi-package builds, compiles out the transparent-address discovery \
            this test's funding depends on"]
#[tokio::test]
async fn from_t_z_o_tz_to_zo_tzo_to_orchard() {
    twin_fixtures::from_t_z_o_tz_to_zo_tzo_to_orchard::<MockTwinChain>().await;
}
