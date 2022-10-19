#![forbid(unsafe_code)]

mod data;
mod utils;
use tokio::runtime::Runtime;
use utils::setup::{
    basic_no_spendable, saplingcoinbasebacked_spendcapable, two_clients_a_saplingcoinbase_backed,
};
#[test]
fn create_network_disconnected_client() {
    let (_regtest_manager_1, _child_process_handler_1, _client_1) =
        saplingcoinbasebacked_spendcapable();
}

#[test]
fn zcashd_sapling_commitment_tree() {
    let (regtest_manager, _child_process_handler, _client) = saplingcoinbasebacked_spendcapable();
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
    let (regtest_manager, _child_process_handler, _client) = basic_no_spendable();
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
    let (regtest_manager, _child_process_handler, client) = saplingcoinbasebacked_spendcapable();

    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client, 5).await;

        let balance = client.do_balance().await;
        assert_eq!(balance["sapling_balance"], 3_750_000_000u64);
    });
}

#[test]
fn send_mined_sapling_to_orchard() {
    let (regtest_manager, _child_process_handler, client) = saplingcoinbasebacked_spendcapable();
    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client, 5).await;

        let o_addr = client.do_new_address("o").await.unwrap()[0].take();
        client
            .do_send(vec![(
                o_addr.to_string().as_str(),
                5000,
                Some("Scenario test: engage!".to_string()),
            )])
            .await
            .unwrap();

        utils::increase_height_and_sync_client(&regtest_manager, &client, 2).await;
        let balance = client.do_balance().await;
        assert_eq!(balance["verified_orchard_balance"], 5000);
    });
}
use zcash_primitives::transaction::components::amount::DEFAULT_FEE;
#[test]
fn note_selection_order() {
    //! In order to fund a transaction multiple notes may be selected and consumed.
    //! To minimize note selection operations notes are consumed from largest to smallest.
    //! In addition to testing the order in which notes are selected this test:
    //!   * sends to a sapling address
    //!   * sends back to the original sender's UA
    let (
        regtest_manager,
        sapling_sender_orchard_receiver,
        sapling_receiver_and_sender,
        child_process_handler,
    ) = two_clients_a_saplingcoinbase_backed();

    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(
            &regtest_manager,
            &sapling_sender_orchard_receiver,
            5,
        )
        .await;

        // Note that do_addresses returns an array, each element is a JSON representation
        // of a UA.  Legacy addresses can be extracted from the receivers, per:
        // <https://zips.z.cash/zip-0316>
        let receiver_sender_sap_addr =
            sapling_receiver_and_sender.do_addresses().await[0]["receivers"]["sapling"].clone();

        // Send five transfers in increasing 1000 zat increments
        // These are sent from the coinbase funded client which will
        // subequently receive funding via it's orchard-packed UA.
        for n in 1..=5 {
            sapling_sender_orchard_receiver
                .do_send(vec![(
                    &receiver_sender_sap_addr.to_string(),
                    n * 1000,
                    Some(n.to_string()),
                )])
                .await
                .unwrap();
        }
        utils::increase_height_and_sync_client(&regtest_manager, &sapling_receiver_and_sender, 5)
            .await;
        let orch_receiver_ua_addr =
            sapling_sender_orchard_receiver.do_addresses().await[0]["address"].clone();
        // We know that the largest single note that 2 received from 1 was 5000, for 2 to send
        // 5000 back to 1 it will have to collect funds from two notes to pay the full 5000
        // plus the transaction fee.
        sapling_receiver_and_sender
            .do_send(vec![(
                &orch_receiver_ua_addr.to_string(),
                5000,
                Some("Sending back, should have 2 inputs".to_string()),
            )])
            .await
            .unwrap();
        let receiver_sender_notes = sapling_receiver_and_sender.do_list_notes(false).await;
        // The 5000 zat note to cover the value, plus another for the tx-fee.
        //assert_eq!(notes["pending_sapling_notes"].len(), 2);
        let first_value = receiver_sender_notes["pending_sapling_notes"][0]["value"]
            .as_fixed_point_u64(0)
            .unwrap();
        let second_value = receiver_sender_notes["pending_sapling_notes"][1]["value"]
            .as_fixed_point_u64(0)
            .unwrap();
        assert!(
            first_value == 5000u64 && second_value == 4000u64
                || first_value == 4000u64 && second_value == 5000u64
        );
        //);
        //assert_eq!(note_providing_change["value"], 4000);
        // Because the above tx fee won't consume a full note, change will be sent back to 2.
        // This implies that client_2 will have a total of 4 unspent notes:
        //  * three from client_1 sent above + 1 as change to itself
        assert_eq!(receiver_sender_notes["unspent_sapling_notes"].len(), 4);
        let change_note = receiver_sender_notes["unspent_sapling_notes"]
            .members()
            .filter(|note| note["is_change"].as_bool().unwrap())
            .collect::<Vec<_>>()[0];
        // Because 4000 is the size of the second largest note.
        assert_eq!(change_note["value"], 4000 - u64::from(DEFAULT_FEE));
        let non_change_note_values = receiver_sender_notes["unspent_sapling_notes"]
            .members()
            .filter(|note| !note["is_change"].as_bool().unwrap())
            .map(|x| {
                let v = &x["value"].as_fixed_point_u64(0).unwrap();
                v.clone()
            })
            .collect::<Vec<_>>();
        assert_eq!(non_change_note_values.iter().fold(0, |x, y| x + y), 6000u64);
        //let balance_1 = sapling_sender_orchard_receiver.do_balance().await;
        //let _balance_2 = sapling_receiver_and_sender.do_balance().await;
        //dbg!(&balance_1["sapling_balance"]);
        //dbg!(&balance_2["sapling_balance"]);
        //dbg!(&change_note["created_in_block"]);
        //dbg!(&change_note["value"]);
        //dbg!(DEFAULT_FEE);
    });

    // More explicit than ignoring the unused variable, we only care about this in order to drop it
    drop(child_process_handler);
}

#[test]
fn send_orchard_back_and_forth() {
    let (regtest_manager, client_a, client_b, child_process_handler) =
        two_clients_a_saplingcoinbase_backed();
    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client_a, 5).await;

        // do_new_address returns a single element json array for some reason
        let ua_of_b = client_b.do_new_address("o").await.unwrap()[0].to_string();
        client_a
            .do_send(vec![(&ua_of_b, 10_000, Some("Orcharding".to_string()))])
            .await
            .unwrap();

        utils::increase_height_and_sync_client(&regtest_manager, &client_b, 3).await;
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

        utils::increase_height_and_sync_client(&regtest_manager, &client_a, 3).await;
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
