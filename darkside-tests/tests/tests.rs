use darkside_tests::{
    constants::DARKSIDE_SEED,
    utils::{prepare_darksidewalletd, update_tree_states_for_transaction},
};
use tokio::time::sleep;
use zingo_testutils::{data::seeds, scenarios::setup::ClientBuilder};
use zingoconfig::RegtestNetwork;
use zingolib::{get_base_address, lightclient::PoolBalances};

#[tokio::test]
async fn simple_sync() {
    let darkside_handler = prepare_darksidewalletd(true).await.unwrap();
    let regtest_network = RegtestNetwork::all_upgrades_active();
    let light_client = ClientBuilder::new(
        darkside_handler.darkside_uri.clone(),
        darkside_handler.darkside_dir.clone(),
    )
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
    let darkside_handler = prepare_darksidewalletd(true).await.unwrap();

    let regtest_network = RegtestNetwork::all_upgrades_active();
    let light_client = ClientBuilder::new(
        darkside_handler.darkside_uri.clone(),
        darkside_handler.darkside_dir.clone(),
    )
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
    prepare_darksidewalletd(false).await.unwrap();
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
    let darkside_handler = prepare_darksidewalletd(true).await.unwrap();

    let mut client_manager = ClientBuilder::new(
        darkside_handler.darkside_uri.clone(),
        darkside_handler.darkside_dir.clone(),
    );
    let regtest_network = RegtestNetwork::all_upgrades_active();
    let light_client = client_manager
        .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
        .await;
    let recipient = client_manager
        .build_client(
            seeds::HOSPITAL_MUSEUM_SEED.to_string(),
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

    let mut streamed_raw_txns = darkside_handler
        .darkside_connector
        .get_incoming_transactions()
        .await
        .unwrap();
    let raw_tx = streamed_raw_txns.message().await.unwrap().unwrap();
    // There should only be one transaction incoming
    assert!(streamed_raw_txns.message().await.unwrap().is_none());
    darkside_handler
        .darkside_connector
        .stage_transactions_stream(vec![(raw_tx.data.clone(), 4)])
        .await
        .unwrap();
    darkside_handler
        .darkside_connector
        .stage_blocks_create(4, 1, 0)
        .await
        .unwrap();
    update_tree_states_for_transaction(&darkside_handler.darkside_uri, raw_tx.clone(), 4).await;
    darkside_handler
        .darkside_connector
        .apply_staged(4)
        .await
        .unwrap();
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

    let darkside_handler = prepare_darksidewalletd(true).await.unwrap();
    darkside_handler
        .darkside_connector
        .stage_blocks_create(4, 1, 0)
        .await
        .unwrap();
    darkside_handler
        .darkside_connector
        .apply_staged(4)
        .await
        .unwrap();
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
