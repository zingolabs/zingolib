use darkside_tests::{
    constants,
    utils::{
        init_darksidewalletd, read_dataset,
        scenarios::{self, DarksideScenario},
        send_and_stage_transaction, stage_transaction,
    },
};
use zingo_testutils::{data::seeds, scenarios::setup::ClientBuilder};
use zingoconfig::RegtestNetwork;
use zingolib::{get_base_address, wallet::Pool};

// Temporary test to showcase new darkside helpers
#[tokio::test]
async fn darkside_scenario_test() {
    const BLOCKCHAIN_HEIGHT: i32 = 100;

    let mut scenario = DarksideScenario::default().await;
    scenario
        .build_faucet(Pool::Sapling)
        .await
        .build_client(seeds::HOSPITAL_MUSEUM_SEED.to_string(), 0)
        .await
        .generate_blocks(5, 1)
        .await;

    let faucet = scenario.get_faucet();
    faucet.do_sync(false).await.unwrap();
    dbg!(faucet.do_balance().await);

    let recipient = scenario.get_lightclient(0);
    recipient.do_sync(false).await.unwrap();
    dbg!(recipient.do_balance().await);

    scenario.generate_blocks(12, 2).await;

    dbg!(scenario.get_lightclient(0).do_sync(false).await.unwrap());
}
