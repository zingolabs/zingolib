use darkside_tests::{
    constants::{self, BRANCH_ID, GENESIS_BLOCK},
    darkside_types::{Empty, RawTransaction, TreeState},
    utils::{
        init_darksidewalletd, prepare_darksidewalletd, stage_transaction,
        update_tree_states_for_transaction, DarksideConnector, DarksideHandler,
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
async fn interrupt_sync() {
    let (handler, connector) = init_darksidewalletd().await.unwrap();

    // generate blockchain
    connector.stage_blocks_create(2, 1, 0).await.unwrap();
    stage_transaction(&connector, 2, constants::O_TO_O_SEND_TO_SELF).await;
    connector.apply_staged(2).await.unwrap();

    let regtest_network = RegtestNetwork::all_upgrades_active();
    let light_client = ClientBuilder::new(connector.0, handler.darkside_dir.clone())
        .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
        .await;

    let result = light_client.do_sync(true).await.unwrap();

    println!("{}", result);

    assert!(result.success);
    assert_eq!(result.latest_block, 2);
    assert_eq!(result.total_blocks_synced, 2);
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

// #[tokio::test]
// async fn interrupt_sync() {
//     let darkside_handler = DarksideHandler::new(None);

//     let server_id = zingoconfig::construct_lightwalletd_uri(Some(format!(
//         "http://127.0.0.1:{}",
//         darkside_handler.grpc_port
//     )));
//     prepare_darksidewalletd(server_id.clone(), false)
//         .await
//         .unwrap();

//     let connector = DarksideConnector(server_id.clone());

//     connector.stage_blocks_create(4, 1, 0).await.unwrap();

//     connector
//         .stage_transactions_stream(vec![(
//             hex::decode(constants::O_TO_O_SEND_TO_SELF).unwrap(),
//             4,
//         )])
//         .await
//         .unwrap();
//     let tree_height_4 = update_tree_states_for_transaction(
//         &server_id.clone(),
//         RawTransaction {
//             data: hex::decode(constants::O_TO_O_SEND_TO_SELF).unwrap(),
//             height: 4,
//         },
//         4,
//     )
//     .await;
//     connector
//         .add_tree_state(TreeState {
//             height: 4,
//             ..tree_height_4
//         })
//         .await
//         .unwrap();

//     sleep(std::time::Duration::new(2, 0)).await;

//     connector.apply_staged(4).await.unwrap();

//     let regtest_network = RegtestNetwork::all_upgrades_active();
//     let light_client = ClientBuilder::new(server_id, darkside_handler.darkside_dir.clone())
//         .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
//         .await;

//     let result = light_client.do_sync(true).await.unwrap();

//     println!("{}", result);

//     assert!(result.success);
//     assert_eq!(result.latest_block, 4);
//     assert_eq!(result.total_blocks_synced, 4);
//     assert_eq!(
//         light_client.do_balance().await,
//         PoolBalances {
//             sapling_balance: Some(0),
//             verified_sapling_balance: Some(0),
//             spendable_sapling_balance: Some(0),
//             unverified_sapling_balance: Some(0),
//             orchard_balance: Some(100000000),
//             verified_orchard_balance: Some(100000000),
//             spendable_orchard_balance: Some(100000000),
//             unverified_orchard_balance: Some(0),
//             transparent_balance: Some(0)
//         }
//     );
// }

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
async fn decode_tx() {
    use darkside_tests::constants;

    let tx = hex::decode(constants::O_TO_O_SEND_TO_SELF).unwrap();

    dbg!(zcash_primitives::transaction::Transaction::read(
        &tx[..],
        zcash_primitives::consensus::BranchId::Nu5
    )
    .unwrap());
}
