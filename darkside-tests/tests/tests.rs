// use darkside_tests::darkside_connector::DarksideConnector;
use darkside_tests::utils::prepare_darksidewalletd;
// use darkside_tests::utils::scenarios::DarksideEnvironment;
// use darkside_tests::utils::update_tree_states_for_transaction;
use darkside_tests::utils::DarksideHandler;
use testvectors::seeds::DARKSIDE_SEED;
// use tokio::time::sleep;
// use zcash_client_backend::PoolType::Shielded;
// use zcash_client_backend::ShieldedProtocol::Orchard;
// use zingo_status::confirmation_status::ConfirmationStatus;
use zingolib::config::RegtestNetwork;
// use zingolib::get_base_address_macro;
use zingolib::lightclient::PoolBalances;
// use zingolib::testutils::chain_generics::conduct_chain::ConductChain as _;
// use zingolib::testutils::chain_generics::with_assertions::to_clients_proposal;
// use zingolib::testutils::lightclient::from_inputs;
use zingolib::testutils::scenarios::setup::ClientBuilder;

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
        .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
        .await;

    let result = light_client.sync_and_await().await.unwrap();

    println!("{}", result);

    assert_eq!(result.sync_end_height, 3.into());
    assert_eq!(result.scanned_blocks, 3);
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
    let mut light_client =
        ClientBuilder::new(server_id.clone(), darkside_handler.darkside_dir.clone())
            .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
            .await;
    light_client.sync_and_await().await.unwrap();

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
    light_client.sync_and_await().await.unwrap();
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

// FIXME: zingo2
// #[tokio::test]
// async fn sent_transaction_reorged_into_mempool() {
//     let darkside_handler = DarksideHandler::new(None);

//     let server_id = zingolib::config::construct_lightwalletd_uri(Some(format!(
//         "http://127.0.0.1:{}",
//         darkside_handler.grpc_port
//     )));
//     prepare_darksidewalletd(server_id.clone(), true)
//         .await
//         .unwrap();

//     let mut client_manager =
//         ClientBuilder::new(server_id.clone(), darkside_handler.darkside_dir.clone());
//     let regtest_network = RegtestNetwork::all_upgrades_active();
//     let light_client = client_manager
//         .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
//         .await;
//     let recipient = client_manager
//         .build_client(
//             testvectors::seeds::HOSPITAL_MUSEUM_SEED.to_string(),
//             1,
//             true,
//             regtest_network,
//         )
//         .await;

//     light_client.do_sync(true).await.unwrap();
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
//     let one_txid = from_inputs::quick_send(
//         &light_client,
//         vec![(&get_base_address_macro!(recipient, "unified"), 10_000, None)],
//     )
//     .await
//     .unwrap();
//     println!("{}", one_txid.first());
//     recipient.do_sync(false).await.unwrap();
//     dbg!(recipient.list_outputs().await);

//     let connector = DarksideConnector(server_id.clone());
//     let mut streamed_raw_txns = connector.get_incoming_transactions().await.unwrap();
//     let raw_tx = streamed_raw_txns.message().await.unwrap().unwrap();
//     // There should only be one transaction incoming
//     assert!(streamed_raw_txns.message().await.unwrap().is_none());
//     connector
//         .stage_transactions_stream(vec![(raw_tx.data.clone(), 4)])
//         .await
//         .unwrap();
//     connector.stage_blocks_create(4, 1, 0).await.unwrap();
//     update_tree_states_for_transaction(&server_id, raw_tx.clone(), 4).await;
//     connector.apply_staged(4).await.unwrap();
//     sleep(std::time::Duration::from_secs(1)).await;

//     recipient.do_sync(false).await.unwrap();
//     //  light_client.do_sync(false).await.unwrap();
//     dbg!("Recipient pre-reorg: {}", recipient.list_outputs().await);
//     println!(
//         "Recipient pre-reorg: {}",
//         serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
//     );
//     println!(
//         "Sender pre-reorg (unsynced): {}",
//         serde_json::to_string_pretty(&light_client.do_balance().await).unwrap()
//     );

//     prepare_darksidewalletd(server_id.clone(), true)
//         .await
//         .unwrap();
//     let connector = DarksideConnector(server_id.clone());
//     connector.stage_blocks_create(4, 102, 0).await.unwrap();
//     connector.apply_staged(105).await.unwrap();
//     sleep(std::time::Duration::from_secs(1)).await;

//     recipient.do_sync(false).await.unwrap();
//     light_client.do_sync(false).await.unwrap();
//     println!(
//         "Recipient post-reorg: {}",
//         serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
//     );
//     println!(
//         "Sender post-reorg: {}",
//         serde_json::to_string_pretty(&light_client.do_balance().await).unwrap()
//     );
//     dbg!("Sender post-reorg: {}", light_client.list_outputs().await);
//     let loaded_client =
//         zingolib::testutils::lightclient::new_client_from_save_buffer(&light_client)
//             .await
//             .unwrap();
//     loaded_client.do_sync(false).await.unwrap();
//     dbg!("Sender post-load: {}", loaded_client.list_outputs().await);
//     assert_eq!(
//         loaded_client.do_balance().await.orchard_balance,
//         Some(100000000)
//     );
// }

// FIXME: zingo2
// #[tokio::test]
// #[ignore = "incomplete"]
// async fn evicted_transaction_is_rebroadcast() {
//     std::env::set_var("RUST_BACKTRACE", "1");

//     let mut environment = DarksideEnvironment::setup().await;
//     environment.bump_chain().await;

//     let primary = environment.fund_client_orchard(1_000_000).await;
//     let secondary = environment.create_client().await;
//     primary.do_sync(false).await.unwrap();

//     let proposal =
//         to_clients_proposal(&primary, &[(&secondary, Shielded(Orchard), 100_000, None)]).await;

//     let mut send_height = 0;

//     let txids = &primary
//         .complete_and_broadcast_stored_proposal()
//         .await
//         .unwrap();

//     println!(
//         "{:?}",
//         primary
//             .wallet
//             .lock()
//             .await
//             .wallet_transactions
//             .keys()
//             .cloned()
//             .collect::<Vec<_>>()
//     );

//     let _recorded_fee = *zingolib::testutils::assertions::lookup_fees_with_proposal_check(
//         &primary, &proposal, txids,
//     )
//     .await
//     .first()
//     .expect("one transaction must have been proposed")
//     .as_ref()
//     .expect("record must exist");

//     zingolib::testutils::lightclient::lookup_statuses(&primary, txids.clone())
//         .await
//         .map(|status| {
//             assert_eq!(
//                 status,
//                 Some(ConfirmationStatus::Transmitted(send_height.into()))
//             );
//         });

//     zingolib::testutils::lightclient::lookup_statuses(&secondary, txids.clone())
//         .await
//         .map(|status| {
//             assert!(status.is_none());
//         });

//     environment
//         .get_connector()
//         .clear_incoming_transactions()
//         .await
//         .unwrap();
//     environment.bump_chain().await;

//     zingolib::testutils::lightclient::lookup_statuses(&primary, txids.clone())
//         .await
//         .map(|status| {
//             assert_eq!(
//                 status,
//                 Some(ConfirmationStatus::Transmitted(send_height.into()))
//             );
//         });

//     zingolib::testutils::lightclient::lookup_statuses(&secondary, txids.clone())
//         .await
//         .map(|status| {
//             assert!(status.is_none());
//         });

//     send_height = 0;

//     primary.do_sync(false).await.unwrap();

//     zingolib::testutils::lightclient::lookup_statuses(&primary, txids.clone())
//         .await
//         .map(|status| {
//             assert_eq!(
//                 status,
//                 Some(ConfirmationStatus::Transmitted(send_height.into()))
//             );
//         });

//     // FIXME:
//     // let ref_primary: Arc<LightClient> = Arc::new(primary);
//     // LightClient::start_mempool_monitor(ref_primary).unwrap();
// }
