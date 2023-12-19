use std::{sync::Arc, time::Duration};

use darkside_tests::utils::{
    create_chainbuild_file, load_chainbuild_file,
    scenarios::{DarksideScenario, DarksideSender},
};
use tokio::time::sleep;
use zingo_testutils::{grpc_proxy::ProxyServer, start_proxy_and_connect_lightclient};
use zingolib::{get_base_address, lightclient::PoolBalances, wallet::Pool};

// Test not finished, requires gRPC network interrupter
#[ignore]
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

    let proxy_status = start_proxy_and_connect_lightclient(scenario.get_faucet());
    tokio::task::spawn(async move {
        let mut online = false;
        loop {
            sleep(Duration::from_secs(5)).await;
            online = proxy_status.swap(online, std::sync::atomic::Ordering::Relaxed);
            println!("set proxy status to {}", !online);
        }
    });

    scenario.get_faucet().do_sync(false).await.unwrap();

    println!("do balance:");
    dbg!(scenario.get_faucet().do_balance().await);
    assert_eq!(
        scenario.get_faucet().do_balance().await,
        PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(8510000),
            verified_orchard_balance: Some(8510000),
            unverified_orchard_balance: Some(0),
            spendable_orchard_balance: Some(8510000),
            transparent_balance: Some(0),
        }
    );
    // println!("do list_notes:");
    // println!(
    //     "{}",
    //     json::stringify_pretty(scenario.get_faucet().do_list_notes(true).await, 4)
    // );
}
