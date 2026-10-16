use zcash_protocol::{PoolType, ShieldedPool};
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::NetworkedTestEnvironment;
use zingolib::testutils::chain_generics::with_assertions;
use zingolib::testutils::lightclient::get_base_address;

const ORCHARD_SELF_SEND: u64 = 20_000;
const SAPLING_SELF_SEND: u64 = 20_000;
const TRANSPARENT_SELF_SEND_THEN_SHIELD: u64 = 100_001;
const ALL_POOLS_ORCHARD: u64 = 14_000;
const ALL_POOLS_SAPLING: u64 = 15_000;
const ALL_POOLS_TRANSPARENT: u64 = 100_000;
const TEST_MEMPOOL: bool = true;

async fn self_send(environment: &mut NetworkedTestEnvironment, pool: PoolType, value: u64) {
    let mut client = environment.create_faucet().await;
    let client_addr = get_base_address(&client, pool).await;
    with_assertions::assure_propose_send_bump_sync_all_recipients(
        environment,
        &mut client,
        vec![(&client_addr, value, None)],
        vec![],
        TEST_MEMPOOL,
    )
    .await
    .expect("the self-send proposes, transmits and confirms on testnet");
}

#[tokio::test]
async fn testnet_send_to_self_orchard_glory_goddess() {
    let mut environment = NetworkedTestEnvironment::setup().await;
    self_send(
        &mut environment,
        PoolType::Shielded(ShieldedPool::Orchard),
        ORCHARD_SELF_SEND,
    )
    .await;
}

#[tokio::test]
async fn testnet_send_to_self_sapling_glory_goddess() {
    let mut environment = NetworkedTestEnvironment::setup().await;
    self_send(
        &mut environment,
        PoolType::Shielded(ShieldedPool::Sapling),
        SAPLING_SELF_SEND,
    )
    .await;
}

#[tokio::test]
async fn testnet_send_to_self_transparent_and_then_shield_glory_goddess() {
    let mut environment = NetworkedTestEnvironment::setup().await;
    let mut client = environment.create_faucet().await;
    let client_addr = get_base_address(&client, PoolType::Transparent).await;
    with_assertions::assure_propose_send_bump_sync_all_recipients(
        &mut environment,
        &mut client,
        vec![(&client_addr, TRANSPARENT_SELF_SEND_THEN_SHIELD, None)],
        vec![],
        TEST_MEMPOOL,
    )
    .await
    .expect("the transparent self-send proposes, transmits and confirms on testnet");
    with_assertions::assure_propose_shield_bump_sync(&mut environment, &mut client, TEST_MEMPOOL)
        .await
        .expect("the shield proposes, transmits and confirms on testnet");
}

#[tokio::test]
async fn testnet_send_to_self_all_pools_glory_goddess() {
    let mut environment = NetworkedTestEnvironment::setup().await;
    let mut client = environment.create_faucet().await;
    for (pool, value) in [
        (PoolType::Shielded(ShieldedPool::Orchard), ALL_POOLS_ORCHARD),
        (PoolType::Shielded(ShieldedPool::Sapling), ALL_POOLS_SAPLING),
        (PoolType::Transparent, ALL_POOLS_TRANSPARENT),
    ] {
        let client_addr = get_base_address(&client, pool).await;
        with_assertions::assure_propose_send_bump_sync_all_recipients(
            &mut environment,
            &mut client,
            vec![(&client_addr, value, None)],
            vec![],
            TEST_MEMPOOL,
        )
        .await
        .expect("each pool's self-send proposes, transmits and confirms on testnet");
    }
}
