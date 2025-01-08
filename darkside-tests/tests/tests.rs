use darkside_tests::darkside_connector::DarksideConnector;
use darkside_tests::utils::prepare_darksidewalletd;
use darkside_tests::utils::scenarios::DarksideEnvironment;
use darkside_tests::utils::update_tree_states_for_transaction;
use darkside_tests::utils::DarksideHandler;
use std::future::Future;
use std::pin::Pin;
use testvectors::seeds::DARKSIDE_SEED;
use tokio::time::sleep;
use zcash_client_backend::PoolType::Shielded;
use zcash_client_backend::ShieldedProtocol::Orchard;
use zcash_primitives::consensus::BlockHeight;
use zingo_status::confirmation_status::ConfirmationStatus;
use zingolib::config::RegtestNetwork;
use zingolib::get_base_address_macro;
use zingolib::grpc_connector;
use zingolib::lightclient::LightClient;
use zingolib::lightclient::PoolBalances;
use zingolib::testutils;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::scenarios::setup::ClientBuilder;
use zingolib::wallet::notes::query::OutputQuery;

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
async fn reorg_away_receipt_blaze() {
    reorg_receipt_sync_generic(|lc| Box::pin(async { lc.do_sync(true).await.map(|_| ()) })).await;
}

#[ignore = "attempts to unwrap failed checked_sub on sapling output count"]
#[tokio::test]
async fn reorg_away_receipt_pepper() {
    reorg_receipt_sync_generic(|lc| {
        Box::pin(async {
            let uri = lc.config().lightwalletd_uri.read().unwrap().clone();
            let client = zingo_netutils::GrpcConnector::new(uri)
                .get_client()
                .await
                .unwrap();
            zingo_sync::sync::sync(client, &lc.config().chain.clone(), &mut lc.wallet)
                .await
                .map_err(|e| e.to_string())
        })
    })
    .await;
}
async fn reorg_receipt_sync_generic<F>(sync_fn: F)
where
    F: for<'a> Fn(&'a mut LightClient) -> Pin<Box<dyn Future<Output = Result<(), String>> + 'a>>,
{
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

    sync_fn(&mut light_client).await.unwrap();
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
    sync_fn(&mut light_client).await.unwrap();
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
    let light_client = client_manager
        .build_client(DARKSIDE_SEED.to_string(), 0, true, regtest_network)
        .await;
    let recipient = client_manager
        .build_client(
            testvectors::seeds::HOSPITAL_MUSEUM_SEED.to_string(),
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
    let one_txid = testutils::lightclient::from_inputs::quick_send(
        &light_client,
        vec![(&get_base_address_macro!(recipient, "unified"), 10_000, None)],
    )
    .await
    .unwrap();
    println!("{}", one_txid.first());
    recipient.do_sync(false).await.unwrap();
    dbg!(recipient.list_outputs().await);

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
    dbg!("Recipient pre-reorg: {}", recipient.list_outputs().await);
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
    connector.stage_blocks_create(4, 102, 0).await.unwrap();
    connector.apply_staged(105).await.unwrap();
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
    dbg!("Sender post-reorg: {}", light_client.list_outputs().await);
    let loaded_client =
        zingolib::testutils::lightclient::new_client_from_save_buffer(&light_client)
            .await
            .unwrap();
    loaded_client.do_sync(false).await.unwrap();
    dbg!("Sender post-load: {}", loaded_client.list_outputs().await);
    assert_eq!(
        loaded_client.do_balance().await.orchard_balance,
        Some(100000000)
    );
}

/// this test demonstrates that
/// when a transaction is dropped by lightwallet, zingolib does not proactively respond
/// the test is set up to have a different result if proactive rebroadcast happens
/// there is a further detail to understand: zingolib currently considers all recorded transactions to be un-contradictable. it cant imagine an alternate history without a transaction until the transaction is deleted. this can cause it to become deeply confused until it forgets about the contradicted transaction VIA a reboot.
/// despite this test, there may be unexpected failure modes in network conditions
#[tokio::test]
async fn transaction_disappears_before_mempool() {
    std::env::set_var("RUST_BACKTRACE", "1");

    let mut environment = <DarksideEnvironment as ConductChain>::setup().await;
    <DarksideEnvironment as ConductChain>::bump_chain(&mut environment).await;

    let primary = environment.fund_client_orchard(1_000_000).await;
    let secondary = environment.create_client().await;
    primary.do_sync(false).await.unwrap();

    let proposal = testutils::chain_generics::with_assertions::to_clients_proposal(
        &primary,
        &[(&secondary, Shielded(Orchard), 100_000, None)],
    )
    .await;
    println!("following proposal, preparing to unwind if an assertion fails.");

    let txids = primary
        .complete_and_broadcast_stored_proposal()
        .await
        .unwrap();

    // a modified zingolib::testutils::chain_generics::with_assertions::follow_proposal block
    {
        let sender = &primary;
        let proposal = &proposal;

        let server_height_at_send = BlockHeight::from(
            grpc_connector::get_latest_block(environment.lightserver_uri().unwrap())
                .await
                .unwrap()
                .height as u32,
        );

        // check that each record has the expected fee and status, returning the fee
        let (sender_recorded_fees, (sender_recorded_outputs, sender_recorded_statuses)): (
            Vec<u64>,
            (Vec<u64>, Vec<ConfirmationStatus>),
        ) = testutils::assertions::for_each_proposed_record(
            sender,
            proposal,
            &txids,
            |records, record, step| {
                (
                    testutils::assertions::compare_fee(records, record, step),
                    (record.query_sum_value(OutputQuery::any()), record.status),
                )
            },
        )
        .await
        .into_iter()
        .map(|stepwise_result| {
            stepwise_result
                .map(|(fee_comparison_result, others)| (fee_comparison_result.unwrap(), others))
                .unwrap()
        })
        .unzip();

        for status in sender_recorded_statuses {
            assert_eq!(
                status,
                ConfirmationStatus::Transmitted(server_height_at_send + 1)
            );
        }

        environment
            .get_connector()
            .clear_incoming_transactions()
            .await
            .unwrap();

        environment.bump_chain().await;
        // chain scan shows the same
        sender.do_sync(false).await.unwrap();

        sleep(std::time::Duration::from_millis(10_000)).await;

        environment.bump_chain().await;
        // chain scan shows the same
        sender.do_sync(false).await.unwrap();

        // check that each record has the expected fee and status, returning the fee and outputs
        let (sender_confirmed_fees, (sender_confirmed_outputs, sender_confirmed_statuses)): (
            Vec<u64>,
            (Vec<u64>, Vec<ConfirmationStatus>),
        ) = testutils::assertions::for_each_proposed_record(
            sender,
            proposal,
            &txids,
            |records, record, step| {
                (
                    testutils::assertions::compare_fee(records, record, step),
                    (record.query_sum_value(OutputQuery::any()), record.status),
                )
            },
        )
        .await
        .into_iter()
        .map(|stepwise_result| {
            stepwise_result
                .map(|(fee_comparison_result, others)| (fee_comparison_result.unwrap(), others))
                .unwrap()
        })
        .unzip();

        assert_eq!(sender_confirmed_fees, sender_recorded_fees);
        assert_eq!(sender_confirmed_outputs, sender_recorded_outputs);
        for status in sender_confirmed_statuses {
            assert_eq!(
                status,
                ConfirmationStatus::Transmitted(server_height_at_send + 1)
            );
        }
    }
}
