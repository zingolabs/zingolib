use darkside_tests::darkside_connector::DarksideConnector;
use darkside_tests::utils::prepare_darksidewalletd;
// use darkside_tests::utils::scenarios::DarksideEnvironment;
use darkside_tests::utils::DarksideHandler;
use darkside_tests::utils::update_tree_states_for_transaction;
use testvectors::seeds::DARKSIDE_SEED;
// use zcash_client_backend::PoolType::Shielded;
// use zcash_client_backend::ShieldedProtocol::Orchard;
// use zingo_status::confirmation_status::ConfirmationStatus;
use zingolib::config::RegtestNetwork;
use zingolib::get_base_address_macro;
// use zingolib::testutils::chain_generics::conduct_chain::ConductChain as _;
// use zingolib::testutils::chain_generics::with_assertions::to_clients_proposal;
use zingolib::testutils::lightclient::from_inputs;
use zingolib::testutils::scenarios::setup::ClientBuilder;
use zingolib::wallet::balance::AccountBalance;

#[ignore = "darkside bug, invalid block hash length in tree states"]
#[tokio::test]
async fn simple_sync() {
    let darkside_handler = DarksideHandler::new(None);

    let server_id = zingolib::config::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));
    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();
    let regtest_network = RegtestNetwork::all_upgrades_active();
    let mut light_client = ClientBuilder::new(server_id, darkside_handler.darkside_dir.clone())
        .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network);

    let result = light_client.sync_and_await().await.unwrap();

    println!("{}", result);

    assert_eq!(result.sync_end_height, 3.into());
    assert_eq!(result.blocks_scanned, 3);
    assert_eq!(
        light_client
            .wallet
            .read()
            .await
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap(),
        AccountBalance {
            total_sapling_balance: Some(0.try_into().unwrap()),
            confirmed_sapling_balance: Some(0.try_into().unwrap()),
            unconfirmed_sapling_balance: Some(0.try_into().unwrap()),
            total_orchard_balance: Some(100_000_000.try_into().unwrap()),
            confirmed_orchard_balance: Some(100_000_000.try_into().unwrap()),
            unconfirmed_orchard_balance: Some(0.try_into().unwrap()),
            total_transparent_balance: Some(0.try_into().unwrap()),
            confirmed_transparent_balance: Some(0.try_into().unwrap()),
            unconfirmed_transparent_balance: Some(0.try_into().unwrap())
        }
    );
}

#[ignore = "investigate invalid block hash length"]
#[tokio::test]
async fn reorg_receipt_sync_generic() {
    let darkside_handler = DarksideHandler::new(None);

    let server_id = zingolib::config::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));
    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();

    let regtest_network = RegtestNetwork::all_upgrades_active();
    let mut light_client = ClientBuilder::new(
        server_id.clone(),
        darkside_handler.darkside_dir.clone(),
    )
    .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network);
    light_client.sync_and_await().await.unwrap();

    assert_eq!(
        light_client
            .wallet
            .read()
            .await
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap(),
        AccountBalance {
            total_sapling_balance: Some(0.try_into().unwrap()),
            confirmed_sapling_balance: Some(0.try_into().unwrap()),
            unconfirmed_sapling_balance: Some(0.try_into().unwrap()),
            total_orchard_balance: Some(100_000_000.try_into().unwrap()),
            confirmed_orchard_balance: Some(100_000_000.try_into().unwrap()),
            unconfirmed_orchard_balance: Some(0.try_into().unwrap()),
            total_transparent_balance: Some(0.try_into().unwrap()),
            confirmed_transparent_balance: Some(0.try_into().unwrap()),
            unconfirmed_transparent_balance: Some(0.try_into().unwrap())
        }
    );
    prepare_darksidewalletd(server_id.clone(), false)
        .await
        .unwrap();
    light_client.sync_and_await().await.unwrap();
    assert_eq!(
        light_client
            .wallet
            .read()
            .await
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap(),
        AccountBalance {
            total_sapling_balance: Some(0.try_into().unwrap()),
            confirmed_sapling_balance: Some(0.try_into().unwrap()),
            unconfirmed_sapling_balance: Some(0.try_into().unwrap()),
            total_orchard_balance: Some(0.try_into().unwrap()),
            confirmed_orchard_balance: Some(0.try_into().unwrap()),
            unconfirmed_orchard_balance: Some(0.try_into().unwrap()),
            total_transparent_balance: Some(0.try_into().unwrap()),
            confirmed_transparent_balance: Some(0.try_into().unwrap()),
            unconfirmed_transparent_balance: Some(0.try_into().unwrap())
        }
    );
}

#[ignore = "investigate invalid block hash length"]
#[tokio::test]
async fn sent_transaction_reorged_into_mempool() {
    let darkside_handler = DarksideHandler::new(None);

    let server_id = zingolib::config::construct_lightwalletd_uri(Some(format!(
        "http://127.0.0.1:{}",
        darkside_handler.grpc_port
    )));
    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();

    let mut client_manager =
        ClientBuilder::new(server_id.clone(), darkside_handler.darkside_dir.clone());
    let regtest_network = RegtestNetwork::all_upgrades_active();
    let mut light_client =
        client_manager.build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network);
    let mut recipient = client_manager.build_client(
        testvectors::seeds::HOSPITAL_MUSEUM_SEED.to_string(),
        1,
        true,
        regtest_network,
    );

    light_client.sync_and_await().await.unwrap();
    assert_eq!(
        light_client
            .wallet
            .read()
            .await
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap(),
        AccountBalance {
            total_sapling_balance: Some(0.try_into().unwrap()),
            confirmed_sapling_balance: Some(0.try_into().unwrap()),
            unconfirmed_sapling_balance: Some(0.try_into().unwrap()),
            total_orchard_balance: Some(100_000_000.try_into().unwrap()),
            confirmed_orchard_balance: Some(100_000_000.try_into().unwrap()),
            unconfirmed_orchard_balance: Some(0.try_into().unwrap()),
            total_transparent_balance: Some(0.try_into().unwrap()),
            confirmed_transparent_balance: Some(0.try_into().unwrap()),
            unconfirmed_transparent_balance: Some(0.try_into().unwrap())
        }
    );
    let one_txid = from_inputs::quick_send(
        &mut light_client,
        vec![(&get_base_address_macro!(recipient, "unified"), 10_000, None)],
    )
    .await
    .unwrap();
    println!("{}", one_txid.first());
    recipient.sync_and_await().await.unwrap();

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
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    recipient.sync_and_await().await.unwrap();
    //  light_client.do_sync(false).await.unwrap();
    println!(
        "Recipient pre-reorg: {}",
        &recipient
            .wallet
            .read()
            .await
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap()
    );
    println!(
        "Sender pre-reorg (unsynced): {}",
        &light_client
            .wallet
            .read()
            .await
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap()
    );

    prepare_darksidewalletd(server_id.clone(), true)
        .await
        .unwrap();
    let connector = DarksideConnector(server_id.clone());
    connector.stage_blocks_create(4, 102, 0).await.unwrap();
    connector.apply_staged(105).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    recipient.sync_and_await().await.unwrap();
    light_client.sync_and_await().await.unwrap();
    println!(
        "Recipient post-reorg: {}",
        &recipient
            .wallet
            .read()
            .await
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap()
    );
    println!(
        "Sender post-reorg: {}",
        &light_client
            .wallet
            .read()
            .await
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap()
    );
    let mut loaded_client =
        zingolib::testutils::lightclient::new_client_from_save_buffer(&mut light_client)
            .await
            .unwrap();
    loaded_client.sync_and_await().await.unwrap();
    assert_eq!(
        loaded_client
            .wallet
            .read()
            .await
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap()
            .total_orchard_balance
            .unwrap()
            .into_u64(),
        100000000
    );
}
