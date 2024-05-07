use zingo_testutils::scenarios::faucet_recipient_default;
use zingolib::get_base_address;

#[tokio::test]
async fn shield_transparent() {
    let (regtest_manager, _cph, faucet, recipient) = faucet_recipient_default().await;
    let transparent_funds = 100_000;

    println!(
        "scenario initial
            faucet: {}
            recipient: {}",
        serde_json::to_string_pretty(&faucet.do_balance().await).unwrap(),
        serde_json::to_string_pretty(&recipient.do_balance().await).unwrap(),
    );
    let proposal = faucet
        .do_send_test_only(vec![(
            &get_base_address!(recipient, "transparent"),
            transparent_funds,
            None,
        )])
        .await
        .unwrap();

    println!(
        "sent to recipient
            faucet: {}
            recipient: {}",
        serde_json::to_string_pretty(&faucet.do_balance().await).unwrap(),
        serde_json::to_string_pretty(&recipient.do_balance().await).unwrap(),
    );
    zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
        .await
        .unwrap();

    println!(
        "synced recipient
            faucet: {}
            recipient: {}",
        serde_json::to_string_pretty(&faucet.do_balance().await).unwrap(),
        serde_json::to_string_pretty(&recipient.do_balance().await).unwrap(),
    );

    let shielding_proposal = recipient.do_shield_test_only().await.unwrap();

    println!("Initial proposal {:?}", proposal);
    println!("Shielding proposal {:?}", shielding_proposal);

    println!(
        "post-shield recipient
            faucet: {}
            recipient: {}",
        serde_json::to_string_pretty(&faucet.do_balance().await).unwrap(),
        serde_json::to_string_pretty(&recipient.do_balance().await).unwrap(),
    );
}
