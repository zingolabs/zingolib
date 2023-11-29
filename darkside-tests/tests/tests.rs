use darkside_tests::{
    constants,
    utils::{
        generate_blocks, init_darksidewalletd, prepare_darksidewalletd, send_and_stage_transaction,
        stage_transaction, update_tree_states_for_transaction, DarksideConnector, DarksideHandler,
    },
};
use tokio::time::sleep;
use zingo_testutils::{data::seeds::DARKSIDE_SEED, scenarios::setup::ClientBuilder};
use zingoconfig::RegtestNetwork;
use zingolib::{get_base_address, lightclient::PoolBalances};

#[tokio::test]
async fn simple_sync() {
    let darkside_handler = DarksideHandler::new(None);

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
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

    let result = light_client.do_sync(true).await.unwrap();

    println!("{}", result);

    assert!(result.success);
    assert_eq!(result.latest_block, 3);
    assert_eq!(result.total_blocks_synced, 3);
    assert_eq!(
        light_client.do_balance().await,
        PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(100000000),
            verified_orchard_balance: Some(100000000),
            spendable_orchard_balance: Some(100000000),
            unverified_orchard_balance: Some(0),
            transparent_balance: Some(0)
        }
    );
}

#[tokio::test]
async fn reorg_away_receipt() {
    let darkside_handler = DarksideHandler::new(None);

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));
    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();

    let regtest_network = RegtestNetwork::all_upgrades_active();
    let light_client = ClientBuilder::new(server_id.clone(), darkside_handler.darkside_dir.clone())
        .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
        .await;

    light_client.do_sync(true).await.unwrap();
    assert_eq!(
        light_client.do_balance().await,
        PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(100000000),
            verified_orchard_balance: Some(100000000),
            spendable_orchard_balance: Some(100000000),
            unverified_orchard_balance: Some(0),
            transparent_balance: Some(0)
        }
    );
    prepare_darksidewalletd(server_id.clone(), false)
        .await
        .unwrap();
    light_client.do_sync(true).await.unwrap();
    assert_eq!(
        light_client.do_balance().await,
        PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(0),
            verified_orchard_balance: Some(0),
            spendable_orchard_balance: Some(0),
            unverified_orchard_balance: Some(0),
            transparent_balance: Some(0)
        }
    );
}

#[tokio::test]
async fn sent_transaction_reorged_into_mempool() {
    let darkside_handler = DarksideHandler::new(None);

    let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));
    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();

    let mut client_manager =
        ClientBuilder::new(server_id.clone(), darkside_handler.darkside_dir.clone());
    let regtest_network = RegtestNetwork::all_upgrades_active();
    let light_client = client_manager
        .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
        .await;
    let recipient = client_manager
        .build_client(
            zingo_testutils::data::seeds::HOSPITAL_MUSEUM_SEED.to_string(),
            1,
            true,
            regtest_network,
        )
        .await;

    light_client.do_sync(true).await.unwrap();
    assert_eq!(
        light_client.do_balance().await,
        PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(100000000),
            verified_orchard_balance: Some(100000000),
            spendable_orchard_balance: Some(100000000),
            unverified_orchard_balance: Some(0),
            transparent_balance: Some(0)
        }
    );
    let txid = light_client
        .do_send(vec![(
            &get_base_address!(recipient, "unified"),
            10_000,
            None,
        )])
        .await
        .unwrap();
    println!("{}", txid);
    recipient.do_sync(false).await.unwrap();
    println!("{}", recipient.do_list_transactions().await.pretty(2));

    let connector = DarksideConnector(server_id.clone());
    let mut streamed_raw_txns = connector.get_incoming_transactions().await.unwrap();
    let raw_tx = streamed_raw_txns.message().await.unwrap().unwrap();
    // There should only be one transaction incoming
    assert!(streamed_raw_txns.message().await.unwrap().is_none());
    connector
        .stage_transactions_stream(vec![(raw_tx.data.clone(), 4)])
        .await
        .unwrap();
    connector.stage_blocks_create(4, 1, 0).await.unwrap();
    update_tree_states_for_transaction(&server_id, raw_tx.clone(), 4).await;
    connector.apply_staged(4).await.unwrap();
    sleep(std::time::Duration::from_secs(1)).await;

    recipient.do_sync(false).await.unwrap();
    //  light_client.do_sync(false).await.unwrap();
    println!(
        "Recipient pre-reorg: {}",
        recipient.do_list_transactions().await.pretty(2)
    );
    println!(
        "Recipient pre-reorg: {}",
        serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
    );
    println!(
        "Sender pre-reorg (unsynced): {}",
        serde_json::to_string_pretty(&light_client.do_balance().await).unwrap()
    );

    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();
    let connector = DarksideConnector(server_id.clone());
    connector.stage_blocks_create(4, 1, 0).await.unwrap();
    connector.apply_staged(4).await.unwrap();
    sleep(std::time::Duration::from_secs(1)).await;

    recipient.do_sync(false).await.unwrap();
    light_client.do_sync(false).await.unwrap();
    println!(
        "Recipient post-reorg: {}",
        serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
    );
    println!(
        "Sender post-reorg: {}",
        serde_json::to_string_pretty(&light_client.do_balance().await).unwrap()
    );
    println!(
        "Sender post-reorg: {}",
        light_client.do_list_transactions().await.pretty(2)
    );
    let loaded_client = light_client.new_client_from_save_buffer().await.unwrap();
    loaded_client.do_sync(false).await.unwrap();
    println!(
        "Sender post-load: {}",
        loaded_client.do_list_transactions().await.pretty(2)
    );
    assert_eq!(
        loaded_client.do_balance().await.orchard_balance,
        Some(100000000)
    );
}
#[tokio::test]
async fn interrupt_sync_chainbuild() {
    const BLOCKCHAIN_HEIGHT: i32 = 150_000;
    let mut current_blockheight: i32;
    let mut target_blockheight: i32;

    // initialise darksidewalletd and stage initial funds
    let (handler, connector) = init_darksidewalletd(None).await.unwrap();
    target_blockheight = 2;
    let mut current_tree_state = stage_transaction(
        &connector,
        target_blockheight as u64,
        constants::ABANDON_TO_DARKSIDE_SAP_10_000_000_ZAT,
    )
    .await;
    current_blockheight = target_blockheight;

    // build clients
    let mut client_builder = ClientBuilder::new(connector.0.clone(), handler.darkside_dir.clone());
    let regtest_network = RegtestNetwork::all_upgrades_active();
    let darkside_client = client_builder
        .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
        .await;

    // stage a send to self every thousand blocks
    for thousands_blocks_count in 1..(BLOCKCHAIN_HEIGHT / 1000) as u64 {
        target_blockheight = (thousands_blocks_count * 1000 - 1) as i32;
        generate_blocks(
            &connector,
            current_tree_state,
            current_blockheight,
            target_blockheight,
            thousands_blocks_count as i32,
        )
        .await;
        darkside_client.do_sync(false).await.unwrap();
        target_blockheight += 1;
        current_tree_state = send_and_stage_transaction(
            &connector,
            &darkside_client,
            &get_base_address!(darkside_client, "unified"),
            40_000,
            target_blockheight as u64,
        )
        .await;
        current_blockheight = target_blockheight;
    }

    // stage and apply final blocks
    generate_blocks(
        &connector,
        current_tree_state,
        current_blockheight,
        BLOCKCHAIN_HEIGHT,
        150,
    )
    .await;
    darkside_client.do_sync(false).await.unwrap();

    println!("do balance:");
    dbg!(darkside_client.do_balance().await);
    println!("do list_notes:");
    println!(
        "{}",
        json::stringify_pretty(darkside_client.do_list_notes(true).await, 4)
    );
}
// #[tokio::test]
// async fn interrupt_sync_e2e_test() {
//     // initialise darksidewalletd and stage first part of blockchain
//     let (handler, connector) = init_darksidewalletd(None).await.unwrap();
//     const BLOCKCHAIN_HEIGHT: i32 = 150_000;
//     connector
//         .stage_blocks_create(2, BLOCKCHAIN_HEIGHT - 1, 0)
//         .await
//         .unwrap();
//     stage_transaction(
//         &connector,
//         2,
//         constants::ABANDON_TO_DARKSIDE_SAP_10_000_000_ZAT,
//     )
//     .await;

//     // build clients
//     let mut client_builder = ClientBuilder::new(connector.0.clone(), handler.darkside_dir.clone());
//     let regtest_network = RegtestNetwork::all_upgrades_active();
//     let darkside_client = client_builder
//         .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
//         .await;

//     let tx_set_path = format!(
//         "{}/{}",
//         get_cargo_manifest_dir().to_string_lossy(),
//         INTERRUPT_SYNC_TX_SET
//     );
//     let tx_set = read_block_dataset(tx_set_path);

//     // stage a send to self every thousand blocks
//     for thousands_blocks_count in 1..(BLOCKCHAIN_HEIGHT / 1000) as u64 {
//         generate_blocks(&connector, thousands_blocks_count * 1000 - 1).await;
//         stage_transaction(
//             &connector,
//             thousands_blocks_count * 1000,
//             &tx_set[(thousands_blocks_count - 1) as usize],
//         )
//         .await;
//     }

//     // apply last part of the blockchain
//     connector.apply_staged(BLOCKCHAIN_HEIGHT).await.unwrap();

//     darkside_client.do_sync(true).await.unwrap();
//     println!("do list transactions:");
//     println!("{}", darkside_client.do_list_transactions().await.pretty(2));
//     println!("do balance:");
//     dbg!(darkside_client.do_balance().await);
//     println!("do list_notes:");
//     println!(
//         "{}",
//         json::stringify_pretty(darkside_client.do_list_notes(true).await, 4)
//     );
// }
