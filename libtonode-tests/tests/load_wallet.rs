#![forbid(unsafe_code)]
mod load_wallet {

    use zcash_primitives::zip339::Mnemonic;
    use zingo_testutils::get_base_address_macro;
    use zingo_testutils::lightclient::from_inputs;
    use zingo_testvectors::seeds::CHIMNEY_BETTER_SEED;
    use zingoconfig::ChainType;
    use zingoconfig::RegtestNetwork;
    use zingolib::lightclient::LightClient;
    use zingolib::wallet::keys::extended_transparent::ExtendedPrivKey;
    use zingolib::wallet::keys::unified::Capability;
    use zingolib::wallet::keys::unified::WalletCapability;
    use zingolib::wallet::LightWallet;

    async fn load_wallet_from_data_and_assert(
        data: &[u8],
        expected_balance: u64,
        num_addresses: usize,
    ) {
        let wallet = LightWallet::unsafe_from_buffer_testnet(data).await;
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
            zingo_testutils::get_wallet_nym("sap_only").unwrap();
        let (_loaded_wallet, _) =
            zingo_testutils::load_wallet(sap_dir, ChainType::Regtest(regtest_network)).await;
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
        let data = include_bytes!("zingo-wallet-v26.dat");

        load_wallet_from_data_and_assert(data, 0, 3).await;
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

        load_wallet_from_data_and_assert(data, 10177826, 1).await;
    }

    #[ignore = "flakey test"]
    #[tokio::test]
    async fn load_wallet_from_v28_dat_file() {
        // We test that the LightWallet can be read from v28 .dat file
        // --seed "chimney better bulb horror rebuild whisper improve intact letter giraffe brave rib appear bulk aim burst snap salt hill sad merge tennis phrase raise"
        // with 3 addresses containing all receivers.
        let data = include_bytes!("zingo-wallet-v28.dat");

        load_wallet_from_data_and_assert(data, 10342837, 3).await;
    }
}
