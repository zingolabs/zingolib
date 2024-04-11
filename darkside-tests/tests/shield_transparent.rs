use darkside_tests::utils::scenarios::{DarksideScenario, DarksideSender};
use zingolib::{get_base_address, wallet::Pool};

#[tokio::test]
async fn shield_transparent() {
    let mut targ_height = 10; //arbitrary?
    let mut scenario = DarksideScenario::default_faucet_recipient(Pool::Orchard).await;
    scenario.stage_and_apply_blocks(targ_height, 0).await;
    targ_height += 2;
    scenario.get_faucet().do_sync(false).await.unwrap();
    println!(
        "scenario initial
            faucet: {}
            recipient: {}",
        serde_json::to_string_pretty(&scenario.get_faucet().do_balance().await).unwrap(),
        serde_json::to_string_pretty(&scenario.get_lightclient(0).do_balance().await).unwrap(),
    );
    scenario
        .send_transaction(
            DarksideSender::Faucet,
            &get_base_address!(scenario.get_lightclient(0), "transparent"),
            100_000,
        )
        .await;
    scenario.stage_and_apply_blocks(targ_height, 0).await;
    targ_height += 2;
    scenario.get_lightclient(0).do_sync(false).await.unwrap();

    println!(
        "sent to recipient
            faucet: {}
            recipient: {}",
        serde_json::to_string_pretty(&scenario.get_faucet().do_balance().await).unwrap(),
        serde_json::to_string_pretty(&scenario.get_lightclient(0).do_balance().await).unwrap(),
    );

    scenario
        .shield_transaction(DarksideSender::IndexedClient(0), Pool::Transparent)
        .await;
    println!(
        "post-shield recipient
            faucet: {}
            recipient: {}",
        serde_json::to_string_pretty(&scenario.get_faucet().do_balance().await).unwrap(),
        serde_json::to_string_pretty(&scenario.get_lightclient(0).do_balance().await).unwrap(),
    );
    scenario.stage_and_apply_blocks(targ_height, 0).await;
}
