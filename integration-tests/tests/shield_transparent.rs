use zingo_testutils::{
    get_base_address_macro, lightclient::from_inputs, scenarios::faucet_recipient_default,
};

#[tokio::test]
#[ignore]
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
    let proposal = from_inputs::send(
        &faucet,
        vec![(
            &get_base_address_macro!(recipient, "transparent"),
            transparent_funds,
            None,
        )],
    )
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

    let shielding_proposal = recipient.propose_shield().await.unwrap();

    println!("Initial proposal {:?}", proposal);
    println!("Shielding proposal {:?}", shielding_proposal);

    recipient
        .complete_and_broadcast_stored_proposal()
        .await
        .unwrap();
    zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
        .await
        .unwrap();

    println!(
        "post-shield recipient
            faucet: {}
            recipient: {}",
        serde_json::to_string_pretty(&faucet.do_balance().await).unwrap(),
        serde_json::to_string_pretty(&recipient.do_balance().await).unwrap(),
    );
}
