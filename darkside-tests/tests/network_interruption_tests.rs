use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use darkside_tests::{
    constants::DARKSIDE_SEED,
    utils::{
        create_chainbuild_file, load_chainbuild_file, prepare_darksidewalletd,
        scenarios::{DarksideEnvironment, DarksideSender},
        DarksideHandler,
    },
};
use tokio::time::sleep;
use zcash_client_backend::{PoolType, ShieldedProtocol};
use zingolib::config::RegtestNetwork;
use zingolib::get_base_address_macro;
use zingolib::testutils::{scenarios::setup::ClientBuilder, start_proxy_and_connect_lightclient};
use zingolib::{
    lightclient::PoolBalances,
    wallet::{
        data::summaries::TransactionSummaryInterface as _,
        transaction_record::{SendType, TransactionKind},
    },
};

#[ignore]
#[tokio::test]
async fn interrupt_initial_tree_fetch() {
    let darkside_handler = DarksideHandler::new(None);

    let server_id = zingolib::config::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));
    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();
    let regtest_network = RegtestNetwork::all_upgrades_active();
    let light_client = ClientBuilder::new(server_id, darkside_handler.darkside_dir.clone())
        .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
        .await;
    let mut cond_log =
        HashMap::<&'static str, Box<dyn Fn(Arc<AtomicBool>) + Send + Sync + 'static>>::new();
    let (sender, receiver) = std::sync::mpsc::channel();
    let sender = Arc::new(Mutex::new(sender));
    cond_log.insert(
        "get_tree_state",
        Box::new(move |_online| {
            println!("acquiring lcok");
            match sender.clone().lock() {
                Ok(mutguard) => {
                    println!("acquired lock");
                    mutguard.send(()).unwrap();
                    println!("Ending proxy");
                }
                Err(poisoned_thread) => panic!("{}", poisoned_thread),
            };
        }),
    );
    let (proxy_handle, _proxy_status) =
        start_proxy_and_connect_lightclient(&light_client, cond_log);

    let receiver = Arc::new(Mutex::new(receiver));
    println!("made receiver");
    std::thread::spawn(move || {
        receiver.lock().unwrap().recv().unwrap();
        proxy_handle.abort();
        println!("aborted proxy");
    });
    println!("spawned abortion task");
    let result = light_client.do_sync(true).await;
    assert_eq!(result.unwrap_err(),"status: Unavailable, message: \"error trying to connect: tcp connect error: Connection refused (os error 111)\", details: [], metadata: MetadataMap { headers: {} }");
}

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
    let mut scenario = DarksideEnvironment::default_faucet_recipient(PoolType::Shielded(
        ShieldedProtocol::Sapling,
    ))
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
                &get_base_address_macro!(scenario.get_lightclient(0), "sapling"),
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
#[ignore]
#[tokio::test]
async fn shielded_note_marked_as_change_test() {
    const BLOCKCHAIN_HEIGHT: u64 = 20_000;
    let transaction_set = load_chainbuild_file("shielded_note_marked_as_change");
    let mut scenario = DarksideEnvironment::default_faucet_recipient(PoolType::Shielded(
        ShieldedProtocol::Sapling,
    ))
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
        HashMap::<&'static str, Box<dyn Fn(Arc<AtomicBool>) + Send + Sync>>::new();
    // conditional_logic.insert(
    //     "get_block_range",
    //     Box::new(|online: &Arc<AtomicBool>| {
    //         println!("Turning off, as we received get_block_range call");
    //         online.store(false, Ordering::Relaxed);
    //     }),
    // );
    conditional_logic.insert(
        "get_tree_state",
        Box::new(|online: Arc<AtomicBool>| {
            println!("Turning off, as we received get_tree_state call");
            online.store(false, Ordering::Relaxed);
        }),
    );
    conditional_logic.insert(
        "get_transaction",
        Box::new(|online: Arc<AtomicBool>| {
            println!("Turning off, as we received get_transaction call");
            online.store(false, Ordering::Relaxed);
        }),
    );

    let (_proxy_handle, proxy_status) =
        start_proxy_and_connect_lightclient(scenario.get_lightclient(0), conditional_logic);
    tokio::task::spawn(async move {
        loop {
            sleep(Duration::from_secs(5)).await;
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
    dbg!(scenario.get_lightclient(0).value_transfers().await);

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
    // assert all fees are 10000 zats
    let transaction_summaries = scenario.get_lightclient(0).transaction_summaries().await;
    for summary in transaction_summaries.iter() {
        if let Some(fee) = summary.fee() {
            assert_eq!(fee, 10_000);
        }
    }
    // assert the number of shields are correct
    assert_eq!(
        transaction_summaries
            .iter()
            .filter(|summary| summary.kind() == TransactionKind::Sent(SendType::Shield))
            .count(),
        (BLOCKCHAIN_HEIGHT / 1000 - 1) as usize
    );
}
