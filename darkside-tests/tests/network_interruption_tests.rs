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
use json::JsonValue;
use tokio::time::sleep;
use zingo_testutils::start_proxy_and_connect_lightclient;
use zingolib::{
    get_base_address,
    lightclient::PoolBalances,
    testvectors::seeds,
    wallet::{data::summaries::ValueTransferKind, Pool},
};

// Verifies that shielded transactions correctly mark notes as change
// Also verifies:
// - send-to-self value transfer is created
// - fees have correct value
// - balance is correct
#[ignore]
#[tokio::test]
async fn shielded_note_marked_as_change_chainbuild() {
    const BLOCKCHAIN_HEIGHT: u64 = 20_000;
    let chainbuild_file = create_chainbuild_file("shielded_note_marked_as_change");
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
            .shield_and_write_transaction(
                DarksideSender::IndexedClient(0),
                Pool::Sapling,
                &chainbuild_file,
            )
            .await;
    }

    // DEBUG
    // // stage and apply final blocks
    // scenario.stage_and_apply_blocks(BLOCKCHAIN_HEIGHT, 0).await;
    // scenario.get_lightclient(0).do_sync(false).await.unwrap();

    // println!("do balance:");
    // dbg!(scenario.get_lightclient(0).do_balance().await);
    // println!("do list_notes:");
    // println!(
    //     "{}",
    //     json::stringify_pretty(scenario.get_lightclient(0).do_list_notes(true).await, 4)
    // );
}
#[tokio::test]
async fn shielded_note_marked_as_change_test() {
    const BLOCKCHAIN_HEIGHT: u64 = 20_000;
    let transaction_set = load_chainbuild_file("shielded_note_marked_as_change");
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

    // setup gRPC network interrupt conditions
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

    // start test
    scenario.get_lightclient(0).do_sync(false).await.unwrap();

    // debug info
    println!("do list_notes:");
    println!(
        "{}",
        json::stringify_pretty(scenario.get_lightclient(0).do_list_notes(true).await, 4)
    );
    println!("do list tx summaries:");
    dbg!(scenario.get_lightclient(0).do_list_txsummaries().await);

    // assert the balance is correct
    assert_eq!(
        scenario.get_lightclient(0).do_balance().await,
        PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(760_000),
            verified_orchard_balance: Some(760_000),
            unverified_orchard_balance: Some(0),
            spendable_orchard_balance: Some(760_000),
            transparent_balance: Some(0),
        }
    );
    // assert all unspent orchard notes (shielded notes) are marked as change
    let notes = scenario.get_lightclient(0).do_list_notes(true).await;
    if let JsonValue::Array(unspent_orchard_notes) = &notes["unspent_orchard_notes"] {
        for notes in unspent_orchard_notes {
            assert_eq!(notes["is_change"].as_bool().unwrap(), true);
        }
    }
    // assert all fees are 10000 zats
    let value_transfers = scenario.get_lightclient(0).do_list_txsummaries().await;
    for value_transfer in &value_transfers {
        if let ValueTransferKind::Fee { amount } = value_transfer.kind {
            assert_eq!(amount, 10_000)
        }
    }
    // assert that every shield has a send-to-self value transfer
    assert_eq!(
        value_transfers
            .iter()
            .filter(|vt| vt.kind == ValueTransferKind::SendToSelf)
            .count(),
        (BLOCKCHAIN_HEIGHT / 1000 - 1) as usize
    );
}
