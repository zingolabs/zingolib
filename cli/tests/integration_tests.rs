#![forbid(unsafe_code)]
use std::time::Duration;

mod data;
mod setup;
use setup::two_clients_a_coinbase_backed;
use tokio::runtime::Runtime;
use tokio::time::sleep;
#[test]
fn basic_connectivity_scenario_canary() {
    let (_, _child_process_handler_to_drop, _) = setup::coinbasebacked_spendcapable();
}

#[test]
fn create_network_disconnected_client() {
    let (_regtest_manager_1, _child_process_handler_1, _client_1) =
        setup::coinbasebacked_spendcapable();
}

#[test]
fn zcashd_sapling_commitment_tree() {
    let (regtest_manager, _child_process_handler, _client) = setup::coinbasebacked_spendcapable();
    let trees = regtest_manager
        .get_cli_handle()
        .args(["z_gettreestate", "1"])
        .output()
        .expect("Couldn't get the trees.");
    let trees = json::parse(&String::from_utf8_lossy(&trees.stdout));
    let pretty_trees = json::stringify_pretty(trees.unwrap(), 4);
    println!("{}", pretty_trees);
}

#[test]
fn actual_empty_zcashd_sapling_commitment_tree() {
    // Expectations:
    let sprout_commitments_finalroot =
        "59d2cde5e65c1414c32ba54f0fe4bdb3d67618125286e6a191317917c812c6d7";
    let sapling_commitments_finalroot =
        "3e49b5f954aa9d3545bc6c37744661eea48d7c34e3000d82b7f0010c30f4c2fb";
    let orchard_commitments_finalroot =
        "2fd8e51a03d9bbe2dd809831b1497aeb68a6e37ddf707ced4aa2d8dff13529ae";
    let finalstates = "000000";
    // Setup
    let (regtest_manager, _child_process_handler, _client) = setup::basic_no_spendable();
    // Execution:
    let trees = regtest_manager
        .get_cli_handle()
        .args(["z_gettreestate", "1"])
        .output()
        .expect("Couldn't get the trees.");
    let trees = json::parse(&String::from_utf8_lossy(&trees.stdout));
    // Assertions:
    assert_eq!(
        sprout_commitments_finalroot,
        trees.as_ref().unwrap()["sprout"]["commitments"]["finalRoot"]
    );
    assert_eq!(
        sapling_commitments_finalroot,
        trees.as_ref().unwrap()["sapling"]["commitments"]["finalRoot"]
    );
    assert_eq!(
        orchard_commitments_finalroot,
        trees.as_ref().unwrap()["orchard"]["commitments"]["finalRoot"]
    );
    assert_eq!(
        finalstates,
        trees.as_ref().unwrap()["sprout"]["commitments"]["finalState"]
    );
    assert_eq!(
        finalstates,
        trees.as_ref().unwrap()["sapling"]["commitments"]["finalState"]
    );
    assert_eq!(
        finalstates,
        trees.as_ref().unwrap()["orchard"]["commitments"]["finalState"]
    );
    dbg!(std::process::Command::new("grpcurl").args(["-plaintext", "127.0.0.1:9067"]));
}

#[test]
fn mine_sapling_to_self() {
    let (_regtest_manager, _child_process_handler, client) = setup::coinbasebacked_spendcapable();

    Runtime::new().unwrap().block_on(async {
        sleep(Duration::from_secs(2)).await;
        client.do_sync(true).await.unwrap();

        let balance = client.do_balance().await;
        assert_eq!(balance["sapling_balance"], 3_750_000_000u64);
    });
}

#[test]
fn send_mined_sapling_to_orchard() {
    let (regtest_manager, _child_process_handler, client) = setup::coinbasebacked_spendcapable();
    Runtime::new().unwrap().block_on(async {
        sleep(Duration::from_secs(2)).await;
        client.do_sync(true).await.unwrap();

        let o_addr = client.do_new_address("o").await.unwrap()[0].take();
        client
            .do_send(vec![(
                o_addr.to_string().as_str(),
                5000,
                Some("Scenario test: engage!".to_string()),
            )])
            .await
            .unwrap();

        increase_height_and_sync_client(&regtest_manager, &client, 2).await;
        let balance = client.do_balance().await;
        assert_eq!(balance["verified_orchard_balance"], 5000);
    });
}
use zingo_cli::regtest::RegtestManager;
use zingolib::lightclient::LightClient;
async fn increase_height_and_sync_client(manager: &RegtestManager, client: &LightClient, n: u32) {
    let start_height = client
        .do_wallet_last_scanned_height()
        .await
        .as_u32()
        .unwrap();
    let target = start_height + n;
    manager
        .generate_n_blocks(n)
        .expect("Called for side effect, failed!");
    while check_wallet_chainheight_value(&client, target).await {
        sleep(Duration::from_millis(50)).await;
    }
}
async fn check_wallet_chainheight_value(client: &LightClient, target: u32) -> bool {
    client.do_sync(true).await.unwrap();
    client
        .do_wallet_last_scanned_height()
        .await
        .as_u32()
        .unwrap()
        != target
}
/// This implements similar behavior to 'two_clients_a_coinbase_backed', but with the
/// advantage of starting client_b on a different server, thus testing the ability
/// to change servers after boot
#[test]
fn note_selection_order() {
    let (regtest_manager, client_1, client_2, child_process_handler) =
        two_clients_a_coinbase_backed();

    Runtime::new().unwrap().block_on(async {
        //check_client_blockchain_height_belief(&client_1, 0).await;
        sleep(Duration::from_secs(2)).await;
        client_1.do_sync(true).await.unwrap();
        //check_wallet_chainheight_value(&client_1, 6).await;
        client_2.set_server(client_1.get_server().clone());
        let address_of_2 = client_2.do_address().await["sapling_addresses"][0].clone();
        for n in 1..=5 {
            client_1
                .do_send(vec![(
                    &address_of_2.to_string(),
                    n * 1000,
                    Some(n.to_string()),
                )])
                .await
                .unwrap();
        }
        client_2.do_rescan().await.unwrap();
        increase_height_and_sync_client(&regtest_manager, &client_2, 5).await;
        let address_of_1 = client_1.do_address().await["sapling_addresses"][0].clone();
        client_2
            .do_send(vec![(
                &address_of_1.to_string(),
                5000,
                Some("Sending back, should have 2 inputs".to_string()),
            )])
            .await
            .unwrap();
        let notes = client_2.do_list_notes(false).await;
        assert_eq!(notes["pending_sapling_notes"].len(), 2);
        assert_eq!(notes["unspent_sapling_notes"].len(), 4);
        assert_eq!(
            notes["unspent_sapling_notes"]
                .members()
                .filter(|note| note["is_change"].as_bool().unwrap())
                .collect::<Vec<_>>()
                .len(),
            1
        );
    });

    // More explicit than ignoring the unused variable, we only care about this in order to drop it
    drop(child_process_handler);
}

#[test]
fn send_orchard_back_and_forth() {
    let (regtest_manager, client_a, client_b, child_process_handler) =
        two_clients_a_coinbase_backed();
    Runtime::new().unwrap().block_on(async {
        sleep(Duration::from_secs(2)).await;
        client_a.do_sync(true).await.unwrap();

        // do_new_address returns a single element json array for some reason
        let ua_of_b = client_b.do_new_address("o").await.unwrap()[0].to_string();
        client_a
            .do_send(vec![(&ua_of_b, 10_000, Some("Orcharding".to_string()))])
            .await
            .unwrap();

        regtest_manager.generate_n_blocks(3).unwrap();
        sleep(Duration::from_secs(2)).await;
        client_b.do_sync(true).await.unwrap();
        client_a.do_sync(true).await.unwrap();

        // We still need to implement sending change to orchard, in librustzcash
        // Even when we do, we'll probably send change to sapling with no
        // preexisting orchard spend authority
        assert_eq!(client_a.do_balance().await["orchard_balance"], 0);
        assert_eq!(client_b.do_balance().await["orchard_balance"], 10_000);

        let ua_of_a = client_a.do_new_address("o").await.unwrap()[0].to_string();
        client_b
            .do_send(vec![(&ua_of_a, 5_000, Some("Sending back".to_string()))])
            .await
            .unwrap();

        regtest_manager.generate_n_blocks(3).unwrap();
        sleep(Duration::from_secs(2)).await;
        client_a.do_sync(true).await.unwrap();
        client_b.do_sync(true).await.unwrap();

        assert_eq!(client_a.do_balance().await["orchard_balance"], 5_000);
        assert_eq!(client_b.do_balance().await["sapling_balance"], 4_000);
        assert_eq!(client_b.do_balance().await["orchard_balance"], 0);

        // Unneeded, but more explicit than having child_process_handler be an
        // unused variable
        drop(child_process_handler);
    });
}
// Proposed Test:
//#[test]
//fn two_zcashds_with_colliding_configs() {
// Expectations:
//   The children are terminated by the test run end.
// Setup:
//   Two different zcashds are configured to launch with 1 config-location
//todo!("let _regtest_manager = setup::basic_funded_zcashd_lwd_zingolib_connected();");
// Execution:
//   Launch the two "bonking" zcashd instances
// Assertions:
//   zcashd A is terminated
//   zcashd B is terminated
//   The incorrectly configured location is still present (and not checked in)
//   The test-or-scenario that caused this situation has failed/panicked.
//}
