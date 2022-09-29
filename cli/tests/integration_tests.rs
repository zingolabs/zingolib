#![forbid(unsafe_code)]
use std::time::Duration;

mod data;
mod setup;
use setup::two_clients_a_coinbase_backed;
use tokio::time::sleep;

#[test]
fn basic_connectivity_scenario_a() {
    let _ = setup::coinbasebacked_spendcapable();
}
#[test]
fn basic_connectivity_scenario_b() {
    let _ = setup::coinbasebacked_spendcapable();
}
#[test]
fn zcashd_sapling_commitment_tree() {
    let (regtest_manager, _child_process_handler, _client, _runtime) =
        setup::coinbasebacked_spendcapable();
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
    let (_regtest_manager, _child_process_handler, client, runtime) =
        setup::coinbasebacked_spendcapable();

    runtime.block_on(async {
        sleep(Duration::from_secs(2)).await;
        client.do_sync(true).await.unwrap();

        let balance = client.do_balance().await;
        assert_eq!(balance["sapling_balance"], 3_750_000_000u64);
    });
}

#[test]
fn send_mined_sapling_to_orchard() {
    let (regtest_manager, _child_process_handler, client, runtime) =
        setup::coinbasebacked_spendcapable();
    runtime.block_on(async {
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

        regtest_manager.generate_n_blocks(2).unwrap();
        sleep(Duration::from_secs(2)).await;

        client.do_sync(true).await.unwrap();
        let balance = client.do_balance().await;
        assert_eq!(balance["verified_orchard_balance"], 5000);
    });
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
}

/// This uses a manual outdated version of two_clients_a_spendcapable, but with the
/// advantage of starting client_b on a different server, thus testing the ability
/// to change servers after boot
#[test]
fn note_selection_order() {
    let (regtest_manager_1, _child_process_handler_1, client_1, runtime) =
        setup::coinbasebacked_spendcapable();
    let (_regtest_manager_2, child_process_handler_2, client_2, _) =
        setup::coinbasebacked_spendcapable();
    // We just want the second client, we don't want the zcashd or lightwalletd
    drop(child_process_handler_2);

    runtime.block_on(async {
        sleep(Duration::from_secs(1)).await;
        regtest_manager_1.generate_n_blocks(5).unwrap();
        sleep(Duration::from_secs(1)).await;
        client_1.do_sync(true).await.unwrap();

        client_2.set_server(client_1.get_server().clone());
        client_2.do_rescan().await.unwrap();
        let address_of_2 = client_2.do_address().await["sapling_addresses"][0].clone();
        for n in 1..=5 {
            client_1
                .do_send(vec![(
                    dbg!(&address_of_2.to_string()),
                    n * 1000,
                    Some(n.to_string()),
                )])
                .await
                .unwrap();
        }
        regtest_manager_1.generate_n_blocks(5).unwrap();
        sleep(Duration::from_secs(2)).await;
        client_2.do_sync(true).await.unwrap();
        let address_of_1 = client_1.do_address().await["sapling_addresses"][0].clone();
        client_2
            .do_send(vec![(
                dbg!(&address_of_1.to_string()),
                5000,
                Some("Sending back, should have 2 inputs".to_string()),
            )])
            .await
            .unwrap();
        let notes = client_2.do_list_notes(false).await;
        assert_eq!(notes["pending_notes"].len(), 2);
        assert_eq!(notes["unspent_notes"].len(), 4);
        assert_eq!(
            notes["unspent_notes"]
                .members()
                .filter(|note| note["is_change"].as_bool().unwrap())
                .collect::<Vec<_>>()
                .len(),
            1
        );
    });
}

#[test]
fn send_orchard_back_and_forth() {
    let (regtest_manager, client_a, client_b, child_process_handler, runtime) =
        two_clients_a_coinbase_backed();
    runtime.block_on(async {
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
