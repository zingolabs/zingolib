use zcash_protocol::{PoolType, ShieldedPool};
use zingolib::lightclient::LightClient;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::NetworkedTestEnvironment;
use zingolib::testutils::chain_generics::with_assertions;
use zingolib::testutils::lightclient::get_base_address;

use super::SELF_SEND_VALUE;

const TRANSPARENT_SELF_SEND_THEN_SHIELD: u64 = 100_001;
const ALL_POOLS_ORCHARD: u64 = 14_000;
const ALL_POOLS_SAPLING: u64 = 15_000;
const ALL_POOLS_TRANSPARENT: u64 = 100_000;
const TEST_MEMPOOL: bool = true;

async fn self_send(
    environment: &mut NetworkedTestEnvironment,
    client: &mut LightClient,
    pool: PoolType,
    value: u64,
) {
    let client_addr = get_base_address(client, pool).await;
    with_assertions::assure_propose_send_bump_sync_all_recipients(
        environment,
        client,
        vec![(&client_addr, value, None)],
        vec![],
        TEST_MEMPOOL,
    )
    .await
    .expect("the self-send proposes, transmits and confirms on testnet");
}

async fn shield(environment: &mut NetworkedTestEnvironment, client: &mut LightClient) {
    with_assertions::assure_propose_shield_bump_sync(environment, client, TEST_MEMPOOL)
        .await
        .expect("the shield proposes, transmits and confirms on testnet");
}

async fn faucet_self_send(pool: PoolType, value: u64) -> (NetworkedTestEnvironment, LightClient) {
    let mut environment = NetworkedTestEnvironment::setup().await;
    let mut client = environment.create_faucet().await;
    self_send(&mut environment, &mut client, pool, value).await;
    (environment, client)
}

macro_rules! shielded_self_send_test {
    ($name:ident, $pool:expr) => {
        #[tokio::test]
        async fn $name() {
            faucet_self_send(PoolType::Shielded($pool), SELF_SEND_VALUE).await;
        }
    };
}

shielded_self_send_test!(
    testnet_send_to_self_orchard_glory_goddess,
    ShieldedPool::Orchard
);
shielded_self_send_test!(
    testnet_send_to_self_sapling_glory_goddess,
    ShieldedPool::Sapling
);

#[tokio::test]
async fn testnet_send_to_self_transparent_and_then_shield_glory_goddess() {
    let (mut environment, mut client) =
        faucet_self_send(PoolType::Transparent, TRANSPARENT_SELF_SEND_THEN_SHIELD).await;
    shield(&mut environment, &mut client).await;
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
        self_send(&mut environment, &mut client, pool, value).await;
    }
    shield(&mut environment, &mut client).await;
}
