use std::path::Path;
use std::time::Duration;

use zcash_local_net::validator::regtest_test_activation_heights;
use zcash_protocol::{PoolType, ShieldedPool};
use zingo_common_components::protocol::ActivationHeights;
use zingo_test_vectors::seeds;
use zingolib::lightclient::LightClient;
use zingolib::lightclient::migrate::{MigrationPlan, TransferBroadcastResult};
use zingolib::testutils::lightclient::get_base_address;
use zingolib::testutils::mock_indexer::{MockChain, MockNet, pre_ironwood_funding_transaction};
use zingolib::wallet::migration::{MigrationMode, MigrationState, TransferState};
use zip32::AccountId;

use crate::scenarios;

pub const MIGRATION_FIXTURE_RELATIVE_PATH: &str = "zingolib/src/wallet/disk/testing/examples/regtest/hmvasmuvwmssvichcarbpoct/v42_migration/zingo-wallet.dat";

pub const MIGRATION_FIXTURE_NU6_3_HEIGHT: u32 = 16;

pub const MIGRATION_FIXTURE_NOTE_VALUES: [u64; 3] = [1_020_000, 2_020_000, 5_020_000];

pub const MIGRATION_FIXTURE_FUNDING_HEIGHT: u32 = 4;

const STEP_BUDGET: usize = 80;

const BATCHES_TO_SEND: usize = 2;

pub struct MigrationFixture {
    pub bytes: Vec<u8>,
    pub state: MigrationState,
    pub tip: u32,
}

pub fn deferred_activation_heights(nu6_3: u32) -> ActivationHeights {
    let fixture = scenarios::wallet_activation_heights(&regtest_test_activation_heights());
    ActivationHeights::builder()
        .set_overwinter(fixture.overwinter())
        .set_sapling(fixture.sapling())
        .set_blossom(fixture.blossom())
        .set_heartwood(fixture.heartwood())
        .set_canopy(fixture.canopy())
        .set_nu5(fixture.nu5())
        .set_nu6(fixture.nu6())
        .set_nu6_1(fixture.nu6_1())
        .set_nu6_2(fixture.nu6_2())
        .set_nu6_3(Some(nu6_3))
        .set_nu7(None)
        .build()
}

pub async fn generate_migration_fixture() -> MigrationFixture {
    let activation_heights = deferred_activation_heights(MIGRATION_FIXTURE_NU6_3_HEIGHT);
    let mut net =
        MockNet::launch_with(MockChain::with_activation_heights(activation_heights)).await;
    let mut client = net.client(seeds::HOSPITAL_MUSEUM_SEED).await;
    client.consent_to_clearnet_for_tests().await;

    fund_pre_ironwood(&net, &client, activation_heights).await;
    client.sync_and_await().await.unwrap();

    let plan = client
        .plan_migration(AccountId::ZERO, MigrationMode::Scheduled)
        .await
        .unwrap();
    let MigrationPlan::Scheduled(split) = &plan else {
        panic!("a scheduled plan request yields a scheduled plan");
    };
    assert!(split.preparation_rounds.is_empty(), "{split:?}");
    assert_eq!(split.transfers.len(), MIGRATION_FIXTURE_NOTE_VALUES.len());
    client
        .commit_migration(AccountId::ZERO, &plan)
        .await
        .unwrap();
    let proposed = client.propose_schedule(1).await.unwrap();
    client.commit_schedule(&proposed).await.unwrap();

    drive_batches(&net, &mut client, BATCHES_TO_SEND).await;

    let mut wallet = client.wallet().write().await;
    let chain_type = wallet.chain_type();
    let mut bytes = Vec::new();
    wallet.write(&mut bytes, &chain_type).unwrap();
    let state = wallet.migration().unwrap().clone();
    let tip = u32::from(wallet.sync_state.last_known_chain_height().unwrap());
    assert_eq!(state_shape(&state), (1, 1, 1), "{state:#?}");
    MigrationFixture { bytes, state, tip }
}

pub fn write_migration_fixture(fixture: &MigrationFixture, output: &Path) {
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    std::fs::write(output, &fixture.bytes).unwrap();
}

async fn fund_pre_ironwood(
    net: &MockNet,
    client: &LightClient,
    activation_heights: ActivationHeights,
) {
    let address = get_base_address(client, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let receivers = MIGRATION_FIXTURE_NOTE_VALUES
        .iter()
        .map(|&value| (address.as_str(), value, None))
        .collect();
    let funding = pre_ironwood_funding_transaction(activation_heights, receivers).await;
    let mut chain = net.chain.write().await;
    let to_funding = MIGRATION_FIXTURE_FUNDING_HEIGHT - chain.tip() - 1;
    chain.mine_empty_blocks(to_funding);
    chain.mine_block(vec![funding]);
    let past_activation = MIGRATION_FIXTURE_NU6_3_HEIGHT + 1 - chain.tip();
    chain.mine_empty_blocks(past_activation);
}

async fn drive_batches(net: &MockNet, client: &mut LightClient, batches: usize) {
    let mut sent = 0;
    for _ in 0..STEP_BUDGET {
        let report = client
            .broadcast_due_transfers(Duration::ZERO)
            .await
            .unwrap();
        assert!(report.halted.is_none(), "{report:?}");
        if report
            .outcomes
            .iter()
            .any(|outcome| matches!(outcome.result, TransferBroadcastResult::Sent(_)))
        {
            sent += 1;
            if sent == batches {
                return;
            }
            net.chain.write().await.mine_mempool();
        } else {
            let tip = chain_tip(client).await;
            let blocks = next_send_height(client)
                .await
                .map_or(1, |height| height.saturating_sub(tip).max(1));
            net.chain.write().await.mine_empty_blocks(blocks);
        }
        client.sync_and_await().await.unwrap();
    }
    panic!("{sent} of {batches} batches sent within {STEP_BUDGET} steps");
}

async fn next_send_height(client: &LightClient) -> Option<u32> {
    let wallet = client.wallet().read().await;
    let state = wallet.migration().unwrap();
    let modulus = state.params().bucket_modulus();
    state
        .transfers()
        .iter()
        .filter(|transfer| {
            matches!(
                transfer.state,
                TransferState::Assigned | TransferState::Signed
            )
        })
        .filter_map(|transfer| {
            transfer
                .bucket_index
                .map(|bucket| (bucket, transfer.target_height))
        })
        .min()
        .map(|(bucket, target)| {
            let bucket = u32::try_from(bucket).unwrap();
            let opens = bucket * modulus;
            let closes = opens + modulus - 1;
            target.map_or(opens, |target| u32::from(target).clamp(opens, closes))
        })
}

async fn chain_tip(client: &LightClient) -> u32 {
    u32::from(
        client
            .wallet()
            .read()
            .await
            .sync_state
            .last_known_chain_height()
            .unwrap(),
    )
}

fn state_shape(state: &MigrationState) -> (usize, usize, usize) {
    let count = |predicate: fn(&TransferState) -> bool| {
        state
            .transfers()
            .iter()
            .filter(|transfer| predicate(&transfer.state))
            .count()
    };
    (
        count(|state| matches!(state, TransferState::Confirmed { .. })),
        count(|state| matches!(state, TransferState::Broadcast)),
        count(|state| matches!(state, TransferState::Assigned)),
    )
}
