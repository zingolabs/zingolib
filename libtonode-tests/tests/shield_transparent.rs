use zingolib::get_base_address_macro;
use zingolib::testutils::lightclient::from_inputs;
use zingolib_testutils::scenarios::{
    faucet_recipient_default, increase_height_and_wait_for_client,
};

#[tokio::test]
#[ignore]
async fn shield_transparent() {
    let (local_net, mut faucet, mut recipient) = faucet_recipient_default().await;
    let transparent_funds = 100_000;

    println!(
        "scenario initial
            faucet: {}
            recipient: {}",
        &faucet
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap(),
        &recipient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap(),
    );
    let proposal = from_inputs::quick_send(
        &mut faucet,
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
        &faucet
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap(),
        &recipient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap(),
    );
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    println!(
        "synced recipient
            faucet: {}
            recipient: {}",
        &faucet
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap(),
        &recipient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap(),
    );

    let shielding_proposal = recipient
        .propose_shield(zip32::AccountId::ZERO)
        .await
        .unwrap();

    println!("Initial proposal {proposal:?}");
    println!("Shielding proposal {shielding_proposal:?}");

    recipient.send_stored_proposal().await.unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    println!(
        "post-shield recipient
            faucet: {}
            recipient: {}",
        &faucet
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap(),
        &recipient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap(),
    );
}
