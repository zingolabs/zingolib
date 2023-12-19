use darkside_tests::utils::{
    create_chainbuild_file, load_chainbuild_file,
    scenarios::{DarksideScenario, DarksideSender},
};
use zingolib::{get_base_address, wallet::Pool};

// Test not finished, requires gRPC network interrupter
// #[ignore]
#[tokio::test]
async fn network_interrupt_chainbuild() {
    const BLOCKCHAIN_HEIGHT: u64 = 150_000;
    let chainbuild_file = create_chainbuild_file("network_interrupt");
    let mut scenario = DarksideScenario::default().await;

    scenario.build_faucet(Pool::Sapling).await;

    // stage a send to self every thousand blocks
    for thousands_blocks_count in 1..BLOCKCHAIN_HEIGHT / 1000 {
        scenario
            .generate_blocks(thousands_blocks_count * 1000 - 1, thousands_blocks_count)
            .await;
        scenario.get_faucet().do_sync(false).await.unwrap();
        scenario
            .send_and_write_transaction(
                DarksideSender::Faucet,
                &get_base_address!(scenario.get_faucet(), "unified"),
                40_000,
                &chainbuild_file,
            )
            .await;
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
#[tokio::test]
async fn network_interrupt_test() {
    const BLOCKCHAIN_HEIGHT: u64 = 150_000;
    let transaction_set = load_chainbuild_file("network_interrupt");
    let mut scenario = DarksideScenario::default().await;

    // stage a send to self every thousand blocks
    for thousands_blocks_count in 1..BLOCKCHAIN_HEIGHT / 1000 {
        scenario
            .generate_blocks(thousands_blocks_count * 1000 - 1, thousands_blocks_count)
            .await;
        scenario
            .stage_transaction(&transaction_set[(thousands_blocks_count - 1) as usize])
            .await;
    }
    // stage and apply final blocks
    scenario.generate_blocks(BLOCKCHAIN_HEIGHT, 150).await;

    scenario.build_faucet(Pool::Sapling).await;
    scenario.get_faucet().do_sync(false).await.unwrap();

    println!("do balance:");
    dbg!(scenario.get_faucet().do_balance().await);
    println!("do list_notes:");
    println!(
        "{}",
        json::stringify_pretty(scenario.get_faucet().do_list_notes(true).await, 4)
    );
}
