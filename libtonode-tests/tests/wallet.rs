#![forbid(unsafe_code)]
mod load_wallet {

    use std::fs::File;

    use zcash_address::unified::Encoding as _;
    use zcash_client_backend::PoolType;
    use zcash_client_backend::ShieldedProtocol;
    use zcash_primitives::consensus::Parameters as _;
    use zcash_primitives::zip339::Mnemonic;
    use zingolib::check_client_balances;
    use zingolib::config::ChainType;
    use zingolib::config::RegtestNetwork;
    use zingolib::config::ZingoConfig;
    use zingolib::get_base_address_macro;
    use zingolib::lightclient::propose::ProposeSendError::Proposal;
    use zingolib::lightclient::send::send_with_proposal::QuickSendError;
    use zingolib::lightclient::LightClient;
    use zingolib::lightclient::PoolBalances;
    use zingolib::testutils::lightclient::from_inputs;
    use zingolib::testutils::paths::get_cargo_manifest_dir;
    use zingolib::testutils::scenarios;
    use zingolib::testvectors::seeds::CHIMNEY_BETTER_SEED;
    use zingolib::utils;
    use zingolib::wallet::keys::extended_transparent::ExtendedPrivKey;
    use zingolib::wallet::keys::unified::Capability;
    use zingolib::wallet::keys::unified::WalletCapability;
    use zingolib::wallet::LightWallet;
    use zingolib::wallet::WalletBase;

    async fn loaded_wallet_assert(
        wallet: LightWallet,
        expected_balance: u64,
        num_addresses: usize,
    ) {
        let expected_mnemonic = (
            Mnemonic::from_phrase(CHIMNEY_BETTER_SEED.to_string()).unwrap(),
            0,
        );
        assert_eq!(wallet.mnemonic(), Some(&expected_mnemonic));

        let expected_wc = WalletCapability::new_from_phrase(
            &wallet.transaction_context.config,
            &expected_mnemonic.0,
            expected_mnemonic.1,
        )
        .unwrap();
        let wc = wallet.wallet_capability();

        // We don't want the WalletCapability to impl. `Eq` (because it stores secret keys)
        // so we have to compare each component instead

        // Compare Orchard
        let Capability::Spend(orchard_sk) = &wc.orchard else {
            panic!("Expected Orchard Spending Key");
        };
        assert_eq!(
            orchard_sk.to_bytes(),
            orchard::keys::SpendingKey::try_from(&expected_wc)
                .unwrap()
                .to_bytes()
        );

        // Compare Sapling
        let Capability::Spend(sapling_sk) = &wc.sapling else {
            panic!("Expected Sapling Spending Key");
        };
        assert_eq!(
            sapling_sk,
            &zcash_client_backend::keys::sapling::ExtendedSpendingKey::try_from(&expected_wc)
                .unwrap()
        );

        // Compare transparent
        let Capability::Spend(transparent_sk) = &wc.transparent else {
            panic!("Expected transparent extended private key");
        };
        assert_eq!(
            transparent_sk,
            &ExtendedPrivKey::try_from(&expected_wc).unwrap()
        );

        assert_eq!(wc.addresses().len(), num_addresses);
        for addr in wc.addresses().iter() {
            assert!(addr.orchard().is_some());
            assert!(addr.sapling().is_some());
            assert!(addr.transparent().is_some());
        }

        let client = LightClient::create_from_wallet_async(wallet).await.unwrap();
        let balance = client.do_balance().await;
        assert_eq!(balance.orchard_balance, Some(expected_balance));
        if expected_balance > 0 {
            let _ = from_inputs::quick_send(
                &client,
                vec![(&get_base_address_macro!(client, "sapling"), 11011, None)],
            )
            .await
            .unwrap();
            let _ = client.do_sync(true).await.unwrap();
            let _ = from_inputs::quick_send(
                &client,
                vec![(&get_base_address_macro!(client, "transparent"), 28000, None)],
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn load_and_parse_different_wallet_versions() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (_sap_wallet, _sap_path, sap_dir) =
            zingolib::testutils::get_wallet_nym("sap_only").unwrap();
        let _loaded_wallet =
            zingolib::testutils::load_wallet(sap_dir, ChainType::Regtest(regtest_network)).await;
    }
    #[tokio::test]
    async fn load_wallet_from_v26_dat_file() {
        // We test that the LightWallet can be read from v26 .dat file
        // Changes in version 27:
        //   - The wallet does not have to have a mnemonic.
        //     Absence of mnemonic is represented by an empty byte vector in v27.
        //     v26 serialized wallet is always loaded with `Some(mnemonic)`.
        //   - The wallet capabilities can be restricted from spending to view-only or none.
        //     We introduce `Capability` type represent different capability types in v27.
        //     v26 serialized wallet is always loaded with `Capability::Spend(sk)`.

        // A testnet wallet initiated with
        // --seed "chimney better bulb horror rebuild whisper improve intact letter giraffe brave rib appear bulk aim burst snap salt hill sad merge tennis phrase raise"
        // with 3 addresses containing all receivers.
        // including orchard and sapling transactions
        let wallet = zingolib::testutils::legacy_loads::load_legacy_wallet(
            zingolib::testutils::legacy_loads::LegacyWalletCase::ZingoV26,
        )
        .await;

        loaded_wallet_assert(wallet, 0, 3).await;
    }

    #[ignore = "flakey test"]
    #[tokio::test]
    async fn load_wallet_from_v26_2_dat_file() {
        // We test that the LightWallet can be read from v26 .dat file
        // Changes in version 27:
        //   - The wallet does not have to have a mnemonic.
        //     Absence of mnemonic is represented by an empty byte vector in v27.
        //     v26 serialized wallet is always loaded with `Some(mnemonic)`.
        //   - The wallet capabilities can be restricted from spending to view-only or none.
        //     We introduce `Capability` type represent different capability types in v27.
        //     v26 serialized wallet is always loaded with `Capability::Spend(sk)`.

        // A testnet wallet initiated with
        // --seed "chimney better bulb horror rebuild whisper improve intact letter giraffe brave rib appear bulk aim burst snap salt hill sad merge tennis phrase raise"
        // with 3 addresses containing all receivers.
        // including orchard and sapling transactions
        let data = include_bytes!("zingo-wallet-v26-2.dat");
        let wallet = LightWallet::unsafe_from_buffer_testnet(data).await;

        loaded_wallet_assert(wallet, 10177826, 1).await;
    }

    #[ignore = "flakey test"]
    #[tokio::test]
    async fn load_wallet_from_v28_dat_file() {
        // We test that the LightWallet can be read from v28 .dat file
        // --seed "chimney better bulb horror rebuild whisper improve intact letter giraffe brave rib appear bulk aim burst snap salt hill sad merge tennis phrase raise"
        // with 3 addresses containing all receivers.
        let data = include_bytes!("zingo-wallet-v28.dat");
        let wallet = LightWallet::unsafe_from_buffer_testnet(data).await;

        loaded_wallet_assert(wallet, 10342837, 3).await;
    }

    #[tokio::test]
    async fn reload_wallet_from_buffer() {
        // We test that the LightWallet can be read from v28 .dat file
        // A testnet wallet initiated with
        // --seed "chimney better bulb horror rebuild whisper improve intact letter giraffe brave rib appear bulk aim burst snap salt hill sad merge tennis phrase raise"
        // --birthday 0
        // --nosync
        // with 3 addresses containing all receivers.
        let data = include_bytes!("zingo-wallet-v28.dat");

        let config = zingolib::config::ZingoConfig::build(ChainType::Testnet).create();
        let mid_wallet = LightWallet::read_internal(&data[..], &config)
            .await
            .map_err(|e| format!("Cannot deserialize LightWallet version 28 file: {}", e))
            .unwrap();

        let mid_client = LightClient::create_from_wallet_async(mid_wallet)
            .await
            .unwrap();
        let mid_buffer = mid_client.export_save_buffer_async().await.unwrap();
        let wallet = LightWallet::read_internal(&mid_buffer[..], &config)
            .await
            .map_err(|e| format!("Cannot deserialize rebuffered LightWallet: {}", e))
            .unwrap();
        let expected_mnemonic = (
            Mnemonic::from_phrase(CHIMNEY_BETTER_SEED.to_string()).unwrap(),
            0,
        );
        assert_eq!(wallet.mnemonic(), Some(&expected_mnemonic));

        let expected_wc =
            WalletCapability::new_from_phrase(&config, &expected_mnemonic.0, expected_mnemonic.1)
                .unwrap();
        let wc = wallet.wallet_capability();

        let Capability::Spend(orchard_sk) = &wc.orchard else {
            panic!("Expected Orchard Spending Key");
        };
        assert_eq!(
            orchard_sk.to_bytes(),
            orchard::keys::SpendingKey::try_from(&expected_wc)
                .unwrap()
                .to_bytes()
        );

        let Capability::Spend(sapling_sk) = &wc.sapling else {
            panic!("Expected Sapling Spending Key");
        };
        assert_eq!(
            sapling_sk,
            &zcash_client_backend::keys::sapling::ExtendedSpendingKey::try_from(&expected_wc)
                .unwrap()
        );

        let Capability::Spend(transparent_sk) = &wc.transparent else {
            panic!("Expected transparent extended private key");
        };
        assert_eq!(
            transparent_sk,
            &ExtendedPrivKey::try_from(&expected_wc).unwrap()
        );

        assert_eq!(wc.addresses().len(), 3);
        for addr in wc.addresses().iter() {
            assert!(addr.orchard().is_some());
            assert!(addr.sapling().is_some());
            assert!(addr.transparent().is_some());
        }

        let ufvk = wc.ufvk().unwrap();
        let ufvk_string = ufvk.encode(&config.chain.network_type());
        let ufvk_base = WalletBase::Ufvk(ufvk_string.clone());
        let view_wallet =
            LightWallet::new(config.clone(), ufvk_base, wallet.get_birthday().await).unwrap();
        let v_wc = view_wallet.wallet_capability();
        let vv = v_wc.ufvk().unwrap();
        let vv_string = vv.encode(&config.chain.network_type());
        assert_eq!(ufvk_string, vv_string);

        let client = LightClient::create_from_wallet_async(wallet).await.unwrap();
        let balance = client.do_balance().await;
        assert_eq!(balance.orchard_balance, Some(10342837));
    }

    #[tokio::test]
    async fn load_old_wallet_at_reorged_height() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
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
        let wallet = zingolib::testutils::load_wallet(
            zingo_dest.into(),
            ChainType::Regtest(regtest_network),
        )
        .await;
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
    "block_height": 3,
    "pending": false,
    "datetime": 1692212261,
    "position": 0,
    "txid": "7a9d41caca143013ebd2f710e4dad04f0eb9f0ae98b42af0f58f25c61a9d439e",
    "amount": 100000,
    "zec_price": null,
    "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8",
    "memo": null
  },
  {
    "block_height": 8,
    "pending": false,
    "datetime": 1692212266,
    "position": 0,
    "txid": "122f8ab8dc5483e36256a4fbd7ff8d60eb7196670716a6690f9215f1c2a4d841",
    "amount": 50000,
    "zec_price": null,
    "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8",
    "memo": null
  },
  {
    "block_height": 9,
    "pending": false,
    "datetime": 1692212299,
    "position": 0,
    "txid": "0a014017add7dc9eb57ada3e70f905c9dce610ef055e135b03f4907dd5dc99a4",
    "amount": 30000,
    "zec_price": null,
    "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8",
    "memo": null
  }
]"#;
        assert_eq!(
            expected_pre_sync_transactions,
            recipient.do_list_transactions().await.pretty(2)
        );
        recipient.do_sync(false).await.unwrap();
        let expected_post_sync_transactions = r#"[
  {
    "block_height": 3,
    "pending": false,
    "datetime": 1692212261,
    "position": 0,
    "txid": "7a9d41caca143013ebd2f710e4dad04f0eb9f0ae98b42af0f58f25c61a9d439e",
    "amount": 100000,
    "zec_price": null,
    "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8",
    "memo": null
  },
  {
    "block_height": 8,
    "pending": false,
    "datetime": 1692212266,
    "position": 0,
    "txid": "122f8ab8dc5483e36256a4fbd7ff8d60eb7196670716a6690f9215f1c2a4d841",
    "amount": 50000,
    "zec_price": null,
    "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8",
    "memo": null
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
                zcash_client_backend::data_api::error::Error::DataSource(zingolib::wallet::tx_map_and_maybe_trees::TxMapAndMaybeTreesTraitError::InputSource(
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

        assert!(faucet
            .transaction_summaries()
            .await
            .iter()
            .find(|transaction_summary| transaction_summary.txid() == pending_txid)
            .unwrap()
            .status()
            .is_pending());

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
