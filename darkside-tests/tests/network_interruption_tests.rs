use darkside_tests::{
    constants,
    utils::{
        init_darksidewalletd, read_dataset,
        scenarios::{self, DarksideScenario},
    },
};
use zingo_testutils::{data::seeds, scenarios::setup::ClientBuilder};
use zingoconfig::RegtestNetwork;
use zingolib::{get_base_address, wallet::Pool};

// Temporary test to showcase new darkside helpers
#[tokio::test]
async fn interrupt_sync_chainbuild() {
    const BLOCKCHAIN_HEIGHT: u64 = 150_000;

    let mut scenario = DarksideScenario::default().await;
    scenario.build_faucet(Pool::Sapling).await;

    // stage a send to self every thousand blocks
    for thousands_blocks_count in 1..BLOCKCHAIN_HEIGHT / 1000 {
        scenario
            .generate_blocks(thousands_blocks_count * 1000 - 1, thousands_blocks_count)
            .await;
        scenario.get_faucet().do_sync(false).await.unwrap();
        // scenario
        //     .send_and_stage_transaction(
        //         scenario.get_faucet(),
        //         &get_base_address!(scenario.get_faucet(), "unified"),
        //         40_000,
        //     )
        //     .await;
    }
    // stage and apply final blocks
    scenario.generate_blocks(BLOCKCHAIN_HEIGHT, 150).await;
    scenario.get_faucet().do_sync(false).await.unwrap();

    println!("do balance:");
    dbg!(scenario.get_faucet().do_balance().await);
    println!("do list_notes:");
    println!(
        "{}",
        json::stringify_pretty(scenario.get_faucet().do_list_notes(true).await, 4)
    );
}
