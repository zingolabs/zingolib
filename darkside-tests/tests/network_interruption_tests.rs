use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use darkside_tests::utils::{
    create_chainbuild_file, load_chainbuild_file,
    scenarios::{DarksideScenario, DarksideSender},
};
use tokio::time::sleep;
use zingo_testutils::start_proxy_and_connect_lightclient;
use zingolib::{get_base_address, lightclient::PoolBalances, testvectors::seeds, wallet::Pool};

// Test not finished, not failing
#[ignore]
#[tokio::test]
async fn network_interrupt_chainbuild() {
    const BLOCKCHAIN_HEIGHT: u64 = 5_000;
    let chainbuild_file = create_chainbuild_file("network_interrupt");
    let mut scenario = DarksideScenario::default().await;
    scenario.build_faucet(Pool::Sapling).await;
    scenario
        .build_client(seeds::HOSPITAL_MUSEUM_SEED.to_string(), 4)
        .await;

    // stage a sapling to orchard send-to-self every thousand blocks
    for thousands_blocks_count in 1..BLOCKCHAIN_HEIGHT / 1000 {
        scenario
            .stage_and_apply_blocks(thousands_blocks_count * 1000 - 2, 0)
            .await;
        scenario.get_faucet().do_sync(false).await.unwrap();
        scenario
            .send_and_write_transaction(
                DarksideSender::Faucet,
                &get_base_address!(scenario.get_lightclient(0), "sapling"),
                50_000,
                &chainbuild_file,
            )
            .await;
        scenario
            .apply_blocks(thousands_blocks_count * 1000 - 1)
            .await;
        scenario.get_lightclient(0).do_sync(false).await.unwrap();
        scenario
            .shield_and_write_transaction(DarksideSender::IndexedClient(0), &chainbuild_file)
            .await;
    }
    // stage and apply final blocks
    scenario.stage_and_apply_blocks(BLOCKCHAIN_HEIGHT, 0).await;
    scenario.get_lightclient(0).do_sync(false).await.unwrap();

    println!("do balance:");
    dbg!(scenario.get_lightclient(0).do_balance().await);
    println!("do list_notes:");
    println!(
        "{}",
        json::stringify_pretty(scenario.get_lightclient(0).do_list_notes(true).await, 4)
    );
}
#[tokio::test]
async fn network_interrupt_test() {
    const BLOCKCHAIN_HEIGHT: u64 = 5_000;
    // const BLOCKCHAIN_HEIGHT: u64 = 100_000;
    let transaction_set = load_chainbuild_file("network_interrupt");
    let mut scenario = DarksideScenario::default().await;
    scenario.build_faucet(Pool::Sapling).await;
    scenario
        .build_client(seeds::HOSPITAL_MUSEUM_SEED.to_string(), 4)
        .await;

    // stage a send to self every thousand blocks
    for thousands_blocks_count in 1..BLOCKCHAIN_HEIGHT / 1000 {
        scenario
            .stage_and_apply_blocks(thousands_blocks_count * 1000 - 2, 0)
            .await;
        scenario.stage_next_transaction(&transaction_set).await;
        scenario
            .apply_blocks(thousands_blocks_count * 1000 - 1)
            .await;
        scenario.stage_next_transaction(&transaction_set).await;
    }
    // stage and apply final blocks
    scenario.stage_and_apply_blocks(BLOCKCHAIN_HEIGHT, 0).await;

    let mut conditional_logic =
        HashMap::<&'static str, Box<dyn Fn(&Arc<AtomicBool>) + Send + Sync>>::new();
    conditional_logic.insert(
        "get_tree_state",
        Box::new(|online: &Arc<AtomicBool>| {
            println!("Turning off, as we received get_tree_state call");
            online.store(false, Ordering::Relaxed);
        }),
    );
    conditional_logic.insert(
        "get_transaction",
        Box::new(|online: &Arc<AtomicBool>| {
            println!("Turning off, as we received get_transaction call");
            online.store(false, Ordering::Relaxed);
        }),
    );

    let proxy_status =
        start_proxy_and_connect_lightclient(scenario.get_lightclient(0), conditional_logic);
    tokio::task::spawn(async move {
        loop {
            sleep(Duration::from_secs(3)).await;
            proxy_status.store(true, std::sync::atomic::Ordering::Relaxed);
            println!("Set proxy status to true");
        }
    });

    scenario.get_lightclient(0).do_sync(false).await.unwrap();

    // println!("do balance:");
    // dbg!(scenario.get_lightclient(0).do_balance().await);
    println!("do list_notes:");
    println!(
        "{}",
        json::stringify_pretty(scenario.get_lightclient(0).do_list_notes(true).await, 4)
    );
    println!("do list tx summaries:");
    dbg!(scenario.get_lightclient(0).do_list_txsummaries().await);
    // println!("do list transactions:");
    // dbg!(scenario.get_lightclient(0).do_list_transactions().await);
    assert_eq!(
        scenario.get_lightclient(0).do_balance().await,
        PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(160_000),
            verified_orchard_balance: Some(160_000),
            unverified_orchard_balance: Some(0),
            spendable_orchard_balance: Some(160_000),
            transparent_balance: Some(0),
        }
    );
}

mod assorted_interrupt_attempts {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn network_interrupt_5_on_5_off() {
        const BLOCKCHAIN_HEIGHT: u64 = 150_000;
        let transaction_set = load_chainbuild_file("network_interrupt");
        let mut scenario = DarksideScenario::default().await;

        // stage a send to self every thousand blocks
        for thousands_blocks_count in 1..BLOCKCHAIN_HEIGHT / 1000 {
            scenario
                .stage_and_apply_blocks(thousands_blocks_count * 1000 - 1, thousands_blocks_count)
                .await;
            scenario
                .stage_transaction(&transaction_set[(thousands_blocks_count - 1) as usize])
                .await;
        }
        // stage and apply final blocks
        scenario
            .stage_and_apply_blocks(BLOCKCHAIN_HEIGHT, 150)
            .await;

        scenario.build_faucet(Pool::Sapling).await;

        let proxy_status =
            start_proxy_and_connect_lightclient(scenario.get_faucet(), HashMap::new());
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
}
