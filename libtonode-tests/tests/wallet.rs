#![forbid(unsafe_code)]
mod load_wallet {

    use std::fs::File;

    use zcash_client_backend::PoolType;
    use zcash_client_backend::ShieldedProtocol;
    use zingolib::check_client_balances;
    use zingolib::config::RegtestNetwork;
    use zingolib::config::ZingoConfig;
    use zingolib::get_base_address_macro;
    use zingolib::lightclient::send::send_with_proposal::QuickSendError;
    use zingolib::lightclient::LightClient;
    use zingolib::lightclient::PoolBalances;
    use zingolib::testutils::lightclient::from_inputs;
    use zingolib::testutils::paths::get_cargo_manifest_dir;
    use zingolib::testutils::scenarios;
    use zingolib::utils;
    use zingolib::wallet::disk::testing::examples;
    use zingolib::wallet::propose::ProposeSendError::Proposal;

    #[tokio::test]
    async fn load_old_wallet_at_reorged_height() {
        let regtest_network = RegtestNetwork::new(1, 1, 1, 1, 1, 1, 200);
        let (ref regtest_manager, cph, ref faucet) = scenarios::faucet(
            PoolType::Shielded(ShieldedProtocol::Orchard),
            regtest_network,
        )
        .await;
        println!("Shutting down initial zcd/lwd unneeded processes");
        drop(cph);

        let zcd_datadir = &regtest_manager.zcashd_data_dir;
        let zingo_datadir = &regtest_manager.zingo_datadir;
        // This test is the unique consumer of:
        // zingolib/src/testvectors/old_wallet_reorg_test_wallet
        let cached_data_dir = get_cargo_manifest_dir()
            .parent()
            .unwrap()
            .join("zingolib/src/testvectors")
            .join("old_wallet_reorg_test_wallet");
        let zcd_source = cached_data_dir
            .join("zcashd")
            .join(".")
            .to_string_lossy()
            .to_string();
        let zcd_dest = zcd_datadir.to_string_lossy().to_string();
        std::process::Command::new("rm")
            .arg("-r")
            .arg(&zcd_dest)
            .output()
            .expect("directory rm failed");
        std::fs::DirBuilder::new()
            .create(&zcd_dest)
            .expect("Dir recreate failed");
        std::process::Command::new("cp")
            .arg("-r")
            .arg(zcd_source)
            .arg(zcd_dest)
            .output()
            .expect("directory copy failed");
        let zingo_source = cached_data_dir
            .join("zingo-wallet.dat")
            .to_string_lossy()
            .to_string();
        let zingo_dest = zingo_datadir.to_string_lossy().to_string();
        std::process::Command::new("cp")
            .arg("-f")
            .arg(zingo_source)
            .arg(&zingo_dest)
            .output()
            .expect("wallet copy failed");
        let _cph = regtest_manager.launch(false).unwrap();
        println!("loading wallet");

        let wallet = examples::NetworkSeedVersion::Regtest(
            examples::RegtestSeedVersion::HospitalMuseum(examples::HospitalMuseumVersion::V27),
        )
        .load_example_wallet()
        .await;

        // let wallet = zingolib::testutils::load_wallet(
        //     zingo_dest.into(),
        //     ChainType::Regtest(regtest_network),
        // )
        // .await;
        println!("setting uri");
        *wallet
            .transaction_context
            .config
            .lightwalletd_uri
            .write()
            .unwrap() = faucet.get_server_uri();
        println!("creating lightclient");
        let recipient = LightClient::create_from_wallet_async(wallet).await.unwrap();
        println!(
            "pre-sync transactions: {}",
            recipient.do_list_transactions().await.pretty(2)
        );
        let expected_pre_sync_transactions = r#"[
  {
    "outgoing_metadata": [],
    "amount": 100000,
    "memo": "null, null",
    "block_height": 3,
    "pending": false,
    "datetime": 1692212261,
    "position": 0,
    "txid": "7a9d41caca143013ebd2f710e4dad04f0eb9f0ae98b42af0f58f25c61a9d439e",
    "zec_price": null,
    "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8"
  },
  {
    "outgoing_metadata": [],
    "amount": 50000,
    "memo": "null, null",
    "block_height": 8,
    "pending": false,
    "datetime": 1692212266,
    "position": 0,
    "txid": "122f8ab8dc5483e36256a4fbd7ff8d60eb7196670716a6690f9215f1c2a4d841",
    "zec_price": null,
    "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8"
  },
  {
    "outgoing_metadata": [],
    "amount": 30000,
    "memo": "null, null",
    "block_height": 9,
    "pending": false,
    "datetime": 1692212299,
    "position": 0,
    "txid": "0a014017add7dc9eb57ada3e70f905c9dce610ef055e135b03f4907dd5dc99a4",
    "zec_price": null,
    "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8"
  }
]"#;
        assert_eq!(
            expected_pre_sync_transactions,
            recipient.do_list_transactions().await.pretty(2)
        );
        recipient.do_sync(false).await.unwrap();
        let expected_post_sync_transactions = r#"[
  {
    "outgoing_metadata": [],
    "amount": 100000,
    "memo": "null, null",
    "block_height": 3,
    "pending": false,
    "datetime": 1692212261,
    "position": 0,
    "txid": "7a9d41caca143013ebd2f710e4dad04f0eb9f0ae98b42af0f58f25c61a9d439e",
    "zec_price": null,
    "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8"
  },
  {
    "outgoing_metadata": [],
    "amount": 50000,
    "memo": "null, null",
    "block_height": 8,
    "pending": false,
    "datetime": 1692212266,
    "position": 0,
    "txid": "122f8ab8dc5483e36256a4fbd7ff8d60eb7196670716a6690f9215f1c2a4d841",
    "zec_price": null,
    "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8"
  }
]"#;
        assert_eq!(
            expected_post_sync_transactions,
            recipient.do_list_transactions().await.pretty(2)
        );
        let expected_post_sync_balance = PoolBalances {
            sapling_balance: Some(0),
            verified_sapling_balance: Some(0),
            spendable_sapling_balance: Some(0),
            unverified_sapling_balance: Some(0),
            orchard_balance: Some(150000),
            verified_orchard_balance: Some(150000),
            spendable_orchard_balance: Some(150000),
            unverified_orchard_balance: Some(0),
            transparent_balance: Some(0),
        };
        assert_eq!(expected_post_sync_balance, recipient.do_balance().await);
        let missing_output_index = from_inputs::quick_send(
            &recipient,
            vec![(&get_base_address_macro!(faucet, "unified"), 14000, None)],
        )
        .await;
        if let Err(QuickSendError::ProposeSend(Proposal(
                zcash_client_backend::data_api::error::Error::DataSource(zingolib::wallet::tx_map::TxMapTraitError::InputSource(
                    zingolib::wallet::transaction_records_by_id::trait_inputsource::InputSourceError::MissingOutputIndexes(output_error)
                )),
            ))) = missing_output_index {
            let txid1 = utils::conversion::txid_from_hex_encoded_str("122f8ab8dc5483e36256a4fbd7ff8d60eb7196670716a6690f9215f1c2a4d841").unwrap();
            let txid2 = utils::conversion::txid_from_hex_encoded_str("7a9d41caca143013ebd2f710e4dad04f0eb9f0ae98b42af0f58f25c61a9d439e").unwrap();
            let expected_txids = vec![txid1, txid2];
            // in case the txids are in reverse order
            let missing_index_txids: Vec<zcash_primitives::transaction::TxId> = output_error.into_iter().map(|(txid, _)| txid).collect();
            if missing_index_txids != expected_txids {
                let expected_txids = vec![txid2, txid1];
                assert!(missing_index_txids == expected_txids, "{:?}\n\n{:?}", missing_index_txids, expected_txids);
            }
        };
    }

    #[tokio::test]
    async fn pending_notes_are_not_saved() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, faucet, recipient) = scenarios::faucet_recipient(
            PoolType::Shielded(ShieldedProtocol::Sapling),
            regtest_network,
        )
        .await;
        zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();

        check_client_balances!(faucet, o: 0 s: 2_500_000_000u64 t: 0u64);
        let pending_txid = *from_inputs::quick_send(
            &faucet,
            vec![(
                get_base_address_macro!(recipient, "unified").as_str(),
                5_000,
                Some("this note never makes it to the wallet! or chain"),
            )],
        )
        .await
        .unwrap()
        .first();

        assert!(!faucet
            .transaction_summaries()
            .await
            .iter()
            .find(|transaction_summary| transaction_summary.txid() == pending_txid)
            .unwrap()
            .status()
            .is_confirmed());

        assert_eq!(
            faucet.do_list_notes(true).await["unspent_orchard_notes"].len(),
            1
        );
        // Create a new client using the faucet's wallet

        // Create zingo config
        let mut wallet_location = regtest_manager.zingo_datadir;
        wallet_location.pop();
        wallet_location.push("zingo_client_1");
        let zingo_config =
            ZingoConfig::build(zingolib::config::ChainType::Regtest(regtest_network))
                .set_wallet_dir(wallet_location.clone())
                .create();
        wallet_location.push("zingo-wallet.dat");
        let read_buffer = File::open(wallet_location.clone()).unwrap();

        // Create wallet from faucet zingo-wallet.dat
        let faucet_wallet =
            zingolib::wallet::LightWallet::read_internal(read_buffer, &zingo_config)
                .await
                .unwrap();

        // Create client based on config and wallet of faucet
        let faucet_copy = LightClient::create_from_wallet_async(faucet_wallet)
            .await
            .unwrap();
        assert_eq!(
            &faucet_copy.do_seed_phrase().await.unwrap(),
            &faucet.do_seed_phrase().await.unwrap()
        ); // Sanity check identity
        assert_eq!(
            faucet.do_list_notes(true).await["unspent_orchard_notes"].len(),
            1
        );
        assert_eq!(
            faucet_copy.do_list_notes(true).await["unspent_orchard_notes"].len(),
            0
        );
        assert!(!faucet_copy
            .transaction_summaries()
            .await
            .iter()
            .any(|transaction_summary| transaction_summary.txid() == pending_txid));
        let mut faucet_transactions = faucet.do_list_transactions().await;
        faucet_transactions.pop();
        faucet_transactions.pop();
        let mut faucet_copy_transactions = faucet_copy.do_list_transactions().await;
        faucet_copy_transactions.pop();
        assert_eq!(faucet_transactions, faucet_copy_transactions);
    }

    #[tokio::test]
    async fn verify_old_wallet_uses_server_height_in_send() {
        // An earlier version of zingolib used the _wallet's_ 'height' when
        // constructing transactions.  This worked well enough when the
        // client completed sync prior to sending, but when we introduced
        // interrupting send, it made it immediately obvious that this was
        // the wrong height to use!  The correct height is the
        // "mempool height" which is the server_height + 1
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        // Ensure that the client has confirmed spendable funds
        zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 5)
            .await
            .unwrap();

        // Without sync push server forward 2 blocks
        zingolib::testutils::increase_server_height(&regtest_manager, 2).await;
        let client_wallet_height = faucet.do_wallet_last_scanned_height().await;

        // Verify that wallet is still back at 6.
        assert_eq!(client_wallet_height.as_fixed_point_u64(0).unwrap(), 8);

        // Interrupt generating send
        from_inputs::quick_send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                10_000,
                Some("Interrupting sync!!"),
            )],
        )
        .await
        .unwrap();
    }
}
