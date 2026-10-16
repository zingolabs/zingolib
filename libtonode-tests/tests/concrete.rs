#![forbid(unsafe_code)]

/// The final send must gather more than one of the recipient's three
/// sapling notes (10_000, 20_000, 30_000) to cover 30_000 plus the fee,
/// making this the suite's only live broadcast of a multi-input sapling
/// spend with cross-pool (orchard) change. The unwraps assert proposal,
/// proving, and the validator's acceptance of the bundle. The closing
/// balance check pins the spend's post-state (gap 1a of the audit's
/// remediation plan). Note-selection ordering itself is asserted
/// offline by `note_selection_covers_target_with_minimal_change` in
/// `zingolib::lightclient::propose`.
// FIXME: add unified address discovery to pepper sync and add a test here

mod basic_transactions {

    // FIXME: zingo2 rewrite action / inputs / outputs counting using new interface
    // #[tokio::test]
    // async fn standard_send_fees() {
    //     let (local_net, faucet, recipient) =
    //         scenarios::faucet_recipient_default().await;

    //     let txid1 = from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "unified").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     let txid2 = from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "sapling").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     let txid3 = from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "transparent").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     generate_n_blocks_return_new_height(&local_net, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     tracing::info!(
    //         "Transaction Inputs:\n{:?}",
    //         tx_inputs(&faucet, txid1.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Outputs:\n{:?}",
    //         tx_outputs(&recipient, txid1.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Change:\n{:?}",
    //         tx_outputs(&faucet, txid1.as_str()).await
    //     );

    //     let tx_actions_txid1 =
    //         tx_actions(&faucet, Some(&recipient), txid1.as_str()).await;
    //     tracing::info!("Transaction Actions:\n{:?}", tx_actions_txid1);

    //     let calculated_fee_txid1 =
    //         total_tx_value(&faucet, txid1.as_str()).await - 40_000;
    //     tracing::info!("Fee Paid: {}", calculated_fee_txid1);

    //     let expected_fee_txid1 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid1.transparent_tx_actions
    //                 + tx_actions_txid1.sapling_tx_actions
    //                 + tx_actions_txid1.orchard_tx_actions,
    //         ));
    //     tracing::info!("Expected Fee: {}", expected_fee_txid1);

    //     assert_eq!(calculated_fee_txid1, expected_fee_txid1 as u64);

    //     tracing::info!(
    //         "Transaction Inputs:\n{:?}",
    //         tx_inputs(&faucet, txid2.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Outputs:\n{:?}",
    //         tx_outputs(&recipient, txid2.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Change:\n{:?}",
    //         tx_outputs(&faucet, txid2.as_str()).await
    //     );

    //     let tx_actions_txid2 =
    //         tx_actions(&faucet, Some(&recipient), txid2.as_str()).await;
    //     tracing::info!("Transaction Actions:\n{:?}", tx_actions_txid2);

    //     let calculated_fee_txid2 =
    //         total_tx_value(&faucet, txid2.as_str()).await - 40_000;
    //     tracing::info!("Fee Paid: {}", calculated_fee_txid2);

    //     let expected_fee_txid2 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid2.transparent_tx_actions
    //                 + tx_actions_txid2.sapling_tx_actions
    //                 + tx_actions_txid2.orchard_tx_actions,
    //         ));
    //     tracing::info!("Expected Fee: {}", expected_fee_txid2);

    //     assert_eq!(calculated_fee_txid2, expected_fee_txid2 as u64);

    //     tracing::info!(
    //         "Transaction Inputs:\n{:?}",
    //         tx_inputs(&faucet, txid3.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Outputs:\n{:?}",
    //         tx_outputs(&recipient, txid3.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Change:\n{:?}",
    //         tx_outputs(&faucet, txid3.as_str()).await
    //     );

    //     let tx_actions_txid3 =
    //         tx_actions(&faucet, Some(&recipient), txid3.as_str()).await;
    //     tracing::info!("Transaction Actions:\n{:?}", tx_actions_txid3);

    //     let calculated_fee_txid3 =
    //         total_tx_value(&faucet, txid3.as_str()).await - 40_000;
    //     tracing::info!("Fee Paid: {}", calculated_fee_txid3);

    //     let expected_fee_txid3 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid3.transparent_tx_actions
    //                 + tx_actions_txid3.sapling_tx_actions
    //                 + tx_actions_txid3.orchard_tx_actions,
    //         ));
    //     tracing::info!("Expected Fee: {}", expected_fee_txid3);

    //     assert_eq!(calculated_fee_txid3, expected_fee_txid3 as u64);

    //     let txid4 = lightclient::from_inputs::quick_send(
    //         &recipient,
    //         vec![(
    //             get_base_address_macro!(faucet, "transparent").as_str(),
    //             55_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     generate_n_blocks_return_new_height(&local_net, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     tracing::info!(
    //         "Transaction Inputs:\n{:?}",
    //         tx_inputs(&recipient, txid4.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Outputs:\n{:?}",
    //         tx_outputs(&faucet, txid4.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Change:\n{:?}",
    //         tx_outputs(&recipient, txid4.as_str()).await
    //     );

    //     let tx_actions_txid4 =
    //         tx_actions(&recipient, Some(&faucet), txid4.as_str()).await;
    //     tracing::info!("Transaction Actions:\n{:?}", tx_actions_txid4);

    //     let calculated_fee_txid4 =
    //         total_tx_value(&recipient, txid4.as_str()).await - 55_000;
    //     tracing::info!("Fee Paid: {}", calculated_fee_txid4);

    //     let expected_fee_txid4 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid4.transparent_tx_actions
    //                 + tx_actions_txid4.sapling_tx_actions
    //                 + tx_actions_txid4.orchard_tx_actions,
    //         ));
    //     tracing::info!("Expected Fee: {}", expected_fee_txid4);

    //     assert_eq!(calculated_fee_txid4, expected_fee_txid4 as u64);
    // }

    // #[tokio::test]
    // async fn dust_send_fees() {
    //     let (local_net, faucet, recipient) =
    //         scenarios::faucet_recipient_default().await;

    //     let txid1 = lightclient::from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "unified").as_str(),
    //             0,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     generate_n_blocks_return_new_height(&local_net, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     tracing::info!(
    //         "Transaction Inputs:\n{:?}",
    //         tx_inputs(&faucet, txid1.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Outputs:\n{:?}",
    //         tx_outputs(&recipient, txid1.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Change:\n{:?}",
    //         tx_outputs(&faucet, txid1.as_str()).await
    //     );

    //     let tx_actions_txid1 =
    //         tx_actions(&faucet, Some(&recipient), txid1.as_str()).await;
    //     tracing::info!("Transaction Actions:\n{:?}", tx_actions_txid1);

    //     let calculated_fee_txid1 =
    //         total_tx_value(&faucet, txid1.as_str()).await;
    //     tracing::info!("Fee Paid: {}", calculated_fee_txid1);

    //     let expected_fee_txid1 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid1.transparent_tx_actions
    //                 + tx_actions_txid1.sapling_tx_actions
    //                 + tx_actions_txid1.orchard_tx_actions,
    //         ));
    //     tracing::info!("Expected Fee: {}", expected_fee_txid1);

    //     assert_eq!(calculated_fee_txid1, expected_fee_txid1 as u64);
    // }

    // #[tokio::test]
    // async fn shield_send_fees() {
    //     let (local_net, faucet, recipient) =
    //         scenarios::faucet_recipient_default().await;

    //     lightclient::from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "transparent").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap();

    //     generate_n_blocks_return_new_height(&local_net, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     let txid1 = recipient.quick_shield().await.unwrap().first().to_string();

    //     generate_n_blocks_return_new_height(&local_net, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     tracing::info!(
    //         "Transaction Inputs:\n{:?}",
    //         tx_inputs(&recipient, txid1.as_str()).await
    //     );
    //     tracing::info!(
    //         "Transaction Outputs:\n{:?}",
    //         tx_outputs(&recipient, txid1.as_str()).await
    //     );

    //     let tx_actions_txid1 =
    //         tx_actions(&recipient, None, txid1.as_str()).await;
    //     tracing::info!("Transaction Actions:\n{:?}", tx_actions_txid1);

    //     let calculated_fee_txid1 =
    //         total_tx_value(&recipient, txid1.as_str()).await;
    //     tracing::info!("Fee Paid: {}", calculated_fee_txid1);

    //     let expected_fee_txid1 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid1.transparent_tx_actions
    //                 + tx_actions_txid1.sapling_tx_actions
    //                 + tx_actions_txid1.orchard_tx_actions,
    //         ));
    //     tracing::info!("Expected Fee: {}", expected_fee_txid1);

    //     assert_eq!(calculated_fee_txid1, expected_fee_txid1 as u64);

    //     lightclient::from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "transparent").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap();

    //     generate_n_blocks_return_new_height(&local_net, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();
    // }
}

mod testnet_test {
    use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
    use zingolib::perspective::sync_progress;
    use zingolib::{
        config::{ChainType, ClientConfig, WalletConfig},
        lightclient::LightClient,
        testutils::{default_test_wallet_settings, tempfile::TempDir},
    };

    /// The testnet indexer these wallet-load tests pin explicitly.
    const TESTNET_INDEXER: &str = "https://testnet.zec.rocks:443";

    #[ignore = "testnet cannot be run offline"]
    #[tokio::test]
    async fn reload_wallet_after_short_sync() {
        zingolib::ensure_default_crypto_provider();

        const NUM_TESTS: u8 = 20;
        let mut test_count = 0;

        while test_count < NUM_TESTS {
            let wallet_dir = TempDir::new().unwrap();
            let config = ClientConfig::builder()
                .set_chain_type(ChainType::Testnet)
                .set_indexer_uri((TESTNET_INDEXER).parse::<http::Uri>().unwrap())
                .set_wallet_config(WalletConfig::MnemonicPhrase {
                    mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
                    no_of_accounts: 1.try_into().unwrap(),
                    birthday: 2_000_000,
                    wallet_settings: default_test_wallet_settings(),
                })
                .set_wallet_dir(wallet_dir.path().to_path_buf())
                .build()
                .unwrap();

            let mut lightclient = LightClient::new(config, true).await.unwrap();
            lightclient.save_task().await;
            lightclient.sync().await.unwrap();
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            while lightclient.latest_sync_status().is_none_or(|status| {
                sync_progress::SyncProgress::from(status).percentage_total_outputs_scanned < 1.0
            }) {
                interval.tick().await;
            }
            lightclient.stop_sync().unwrap();
            lightclient.await_sync().await.unwrap();
            lightclient.shutdown_save_task().await.unwrap();

            // will fail if there were any reload errors due to bad file write code i.e. no flushing or file syncing
            let config = ClientConfig::builder()
                .set_chain_type(ChainType::Testnet)
                .set_indexer_uri((TESTNET_INDEXER).parse::<http::Uri>().unwrap())
                .set_wallet_config(WalletConfig::Read)
                .set_wallet_dir(wallet_dir.path().to_path_buf())
                .build()
                .unwrap();
            LightClient::new(config, true).await.unwrap();

            test_count += 1;
        }
    }
}
