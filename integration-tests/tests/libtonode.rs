#![forbid(unsafe_code)]

use json::JsonValue;
use orchard::tree::MerkleHashOrchard;
use shardtree::store::memory::MemoryShardStore;
use shardtree::ShardTree;
use std::{fs::File, path::Path, time::Duration};
use zcash_address::unified::Fvk;
use zcash_client_backend::encoding::encode_payment_address;
use zcash_primitives::zip339::Mnemonic;
use zcash_primitives::{
    consensus::{BlockHeight, Parameters},
    transaction::fees::zip317::MINIMUM_FEE,
};
use zingo_testutils::{
    self, build_fvk_client, check_client_balances, check_transaction_equality,
    get_base_address_macro, increase_height_and_wait_for_client, paths::get_cargo_manifest_dir,
    scenarios,
};

use zingo_testvectors::{
    block_rewards,
    seeds::{CHIMNEY_BETTER_SEED, HOSPITAL_MUSEUM_SEED},
    BASE_HEIGHT,
};
use zingoconfig::{ChainType, RegtestNetwork, ZingoConfig, MAX_REORG};
use zingolib::{
    lightclient::{LightClient, PoolBalances},
    utils,
    wallet::{
        data::{COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL},
        keys::{
            extended_transparent::ExtendedPrivKey,
            unified::{Capability, WalletCapability},
        },
        LightWallet,
    },
};

fn extract_value_as_u64(input: &JsonValue) -> u64 {
    let note = &input["value"].as_fixed_point_u64(0).unwrap();
    *note
}
fn check_expected_balance_with_fvks(
    fvks: &Vec<&Fvk>,
    balance: PoolBalances,
    o_expect: u64,
    s_expect: u64,
    t_expect: u64,
) {
    for fvk in fvks {
        match fvk {
            Fvk::Sapling(_) => {
                assert_eq!(balance.sapling_balance.unwrap(), s_expect);
                assert_eq!(balance.verified_sapling_balance.unwrap(), s_expect);
                assert_eq!(balance.unverified_sapling_balance.unwrap(), s_expect);
            }
            Fvk::Orchard(_) => {
                assert_eq!(balance.orchard_balance.unwrap(), o_expect);
                assert_eq!(balance.verified_orchard_balance.unwrap(), o_expect);
                assert_eq!(balance.unverified_orchard_balance.unwrap(), o_expect);
            }
            Fvk::P2pkh(_) => {
                assert_eq!(balance.transparent_balance.unwrap(), t_expect);
            }
            _ => panic!(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_view_capability_bounds(
    balance: &PoolBalances,
    watch_wc: &WalletCapability,
    fvks: &[&Fvk],
    ovk: &Fvk,
    svk: &Fvk,
    tvk: &Fvk,
    sent_o_value: Option<u64>,
    sent_s_value: Option<u64>,
    sent_t_value: Option<u64>,
    notes: &JsonValue,
) {
    //Orchard
    if !fvks.contains(&ovk) {
        assert!(!watch_wc.orchard.can_view());
        assert_eq!(balance.orchard_balance, None);
        assert_eq!(balance.verified_orchard_balance, None);
        assert_eq!(balance.unverified_orchard_balance, None);
        assert_eq!(notes["unspent_orchard_notes"].members().count(), 0);
    } else {
        assert!(watch_wc.orchard.can_view());
        assert_eq!(balance.orchard_balance, sent_o_value);
        assert_eq!(balance.verified_orchard_balance, sent_o_value);
        assert_eq!(balance.unverified_orchard_balance, Some(0));
        // assert 1 Orchard note, or 2 notes if a dummy output is included
        let orchard_notes_count = notes["unspent_orchard_notes"].members().count();
        assert!((1..=2).contains(&orchard_notes_count));
    }
    //Sapling
    if !fvks.contains(&svk) {
        assert!(!watch_wc.sapling.can_view());
        assert_eq!(balance.sapling_balance, None);
        assert_eq!(balance.verified_sapling_balance, None);
        assert_eq!(balance.unverified_sapling_balance, None);
        assert_eq!(notes["unspent_sapling_notes"].members().count(), 0);
    } else {
        assert!(watch_wc.sapling.can_view());
        assert_eq!(balance.sapling_balance, sent_s_value);
        assert_eq!(balance.verified_sapling_balance, sent_s_value);
        assert_eq!(balance.unverified_sapling_balance, Some(0));
        assert_eq!(notes["unspent_sapling_notes"].members().count(), 1);
    }
    if !fvks.contains(&tvk) {
        assert!(!watch_wc.transparent.can_view());
        assert_eq!(balance.transparent_balance, None);
        assert_eq!(notes["utxos"].members().count(), 0);
    } else {
        assert!(watch_wc.transparent.can_view());
        assert_eq!(balance.transparent_balance, sent_t_value);
        assert_eq!(notes["utxos"].members().count(), 1);
    }
}

mod fast {
    use zcash_address::unified::Encoding;
    use zcash_client_backend::{PoolType, ShieldedProtocol};
    use zcash_primitives::transaction::components::amount::NonNegativeAmount;
    use zingo_testutils::lightclient::from_inputs;
    use zingolib::wallet::WalletBase;

    use super::*;
    #[tokio::test]
    async fn utxos_are_not_prematurely_confirmed() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        from_inputs::quick_send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "transparent"),
                100_000,
                None,
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        let preshield_utxos = dbg!(recipient.wallet.get_utxos().await);
        from_inputs::shield(&recipient, &[PoolType::Transparent], None)
            .await
            .unwrap();
        let postshield_utxos = dbg!(recipient.wallet.get_utxos().await);
        assert_eq!(preshield_utxos[0].address, postshield_utxos[0].address);
        assert_eq!(
            preshield_utxos[0].output_index,
            postshield_utxos[0].output_index
        );
        assert_eq!(preshield_utxos[0].value, postshield_utxos[0].value);
        assert_eq!(preshield_utxos[0].script, postshield_utxos[0].script);
        assert!(preshield_utxos[0].pending_spent.is_none());
        assert!(postshield_utxos[0].pending_spent.is_some());
    }
    #[tokio::test]
    async fn send_without_reorg_buffer_blocks_gives_correct_error() {
        let (_regtest_manager, _cph, faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;
        recipient
            .wallet
            .transaction_context
            .config
            .reorg_buffer_offset = 4;
        println!(
            "{}",
            serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
        );
        assert_eq!(
        from_inputs::send(&recipient, vec![(&get_base_address_macro!(faucet, "unified"), 100_000, None)])
            .await
            .unwrap_err(),
        "The reorg buffer offset has been set to 4 but there are only 1 blocks in the wallet. Please sync at least 4 more blocks before trying again"
    );
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
    async fn list_transactions_include_foreign() {
        let wallet_nym = format!(
            "{}/tests/data/wallets/missing_data_test/zingo-wallet.dat",
            get_cargo_manifest_dir().to_string_lossy()
        );
        let wallet_path = Path::new(&wallet_nym);
        let wallet_dir = wallet_path.parent().unwrap();
        let (wallet, config) =
            zingo_testutils::load_wallet(wallet_dir.to_path_buf(), ChainType::Mainnet).await;
        let client = LightClient::create_from_wallet_async(wallet, config)
            .await
            .unwrap();
        let transactions = client.do_list_transactions().await[0].clone();
        //env_logger::init();
        let expected_consumer_ui_note = r#"{
  "amount": 100000,
  "memo": "Enviado desde YWallet, Enviado desde YWallet",
  "block_height": 2060028,
  "pending": false,
  "datetime": 1682127442,
  "position": 0,
  "txid": "d93fbb42a101ac148b4e610eea1fe519c0131b17d49af53f29b5e35a778145cb",
  "zec_price": null,
  "address": "u1n5zgv8c9px4hfmq7cr9f9t0av6q9nj5dwca9w0z9jxegut65gxs2y4qnx7ppng6k2hyt0asyycqrywalzyasxu2302xt4spfqnkh25nevr3h9exc3clh9tfpr5hyhc9dwee50l0cxm7ajun5xs9ycqhlw8rd39jql8z5zlv9hw4q8azcgpv04dez5547geuvyh8pfzezpw52cg2qknm"
}"#;
        assert_eq!(
            expected_consumer_ui_note,
            json::stringify_pretty(transactions, 2)
        );
    }

    #[tokio::test]
    async fn zcashd_sapling_commitment_tree() {
        //  TODO:  Make this test assert something, what is this a test of?
        //  TODO:  Add doc-comment explaining what constraints this test
        //  enforces
        let (regtest_manager, _cph, _faucet) = scenarios::faucet_default().await;
        let trees = regtest_manager
            .get_cli_handle()
            .args(["z_gettreestate", "1"])
            .output()
            .expect("Couldn't get the trees.");
        let trees = json::parse(&String::from_utf8_lossy(&trees.stdout));
        let pretty_trees = json::stringify_pretty(trees.unwrap(), 4);
        println!("{}", pretty_trees);
    }

    #[tokio::test]
    async fn actual_empty_zcashd_sapling_commitment_tree() {
        // Expectations:
        let sprout_commitments_finalroot =
            "59d2cde5e65c1414c32ba54f0fe4bdb3d67618125286e6a191317917c812c6d7";
        let sapling_commitments_finalroot =
            "3e49b5f954aa9d3545bc6c37744661eea48d7c34e3000d82b7f0010c30f4c2fb";
        let orchard_commitments_finalroot =
            "ae2935f1dfd8a24aed7c70df7de3a668eb7a49b1319880dde2bbd9031ae5d82f";
        let finalstates = "000000";
        // Setup
        let (regtest_manager, _cph, _client) = scenarios::unfunded_client_default().await;
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
        //dbg!(std::process::Command::new("grpcurl").args(["-plaintext", "127.0.0.1:9067"]));
    }

    #[tokio::test]
    async fn unspent_notes_are_not_saved() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, faucet, recipient) = scenarios::faucet_recipient(
            PoolType::Shielded(ShieldedProtocol::Sapling),
            regtest_network,
        )
        .await;
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();

        check_client_balances!(faucet, o: 0 s: 2_500_000_000u64 t: 0u64);
        from_inputs::send(
            &faucet,
            vec![(
                get_base_address_macro!(recipient, "unified").as_str(),
                5_000,
                Some("this note never makes it to the wallet! or chain"),
            )],
        )
        .await
        .unwrap();

        assert_eq!(
            faucet.do_list_notes(true).await["unspent_orchard_notes"].len(),
            1
        );
        // Create a new client using the faucet's wallet

        // Create zingo config
        let mut wallet_location = regtest_manager.zingo_datadir;
        wallet_location.pop();
        wallet_location.push("zingo_client_1");
        let zingo_config = ZingoConfig::build(zingoconfig::ChainType::Regtest(regtest_network))
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
        let faucet_copy =
            LightClient::create_from_wallet_async(faucet_wallet, zingo_config.clone())
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
        let mut faucet_transactions = faucet.do_list_transactions().await;
        faucet_transactions.pop();
        faucet_transactions.pop();
        let mut faucet_copy_transactions = faucet_copy.do_list_transactions().await;
        faucet_copy_transactions.pop();
        assert_eq!(faucet_transactions, faucet_copy_transactions);
    }

    #[tokio::test]
    async fn diversified_addresses_receive_funds_in_best_pool() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        for code in ["o", "zo", "z"] {
            recipient.do_new_address(code).await.unwrap();
        }
        let addresses = recipient.do_addresses().await;
        let address_5000_nonememo_tuples = addresses
            .members()
            .map(|ua| (ua["address"].as_str().unwrap(), 5_000, None))
            .collect::<Vec<(&str, u64, Option<&str>)>>();
        from_inputs::send(&faucet, address_5000_nonememo_tuples)
            .await
            .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        let balance_b = recipient.do_balance().await;
        assert_eq!(
            balance_b,
            PoolBalances {
                sapling_balance: Some(5000),
                verified_sapling_balance: Some(5000),
                spendable_sapling_balance: Some(5000),
                unverified_sapling_balance: Some(0),
                orchard_balance: Some(15000),
                verified_orchard_balance: Some(15000),
                spendable_orchard_balance: Some(15000),
                unverified_orchard_balance: Some(0),
                transparent_balance: Some(0)
            }
        );
        // Unneeded, but more explicit than having _cph be an
        // unused variable
    }

    #[tokio::test]
    async fn diversification_deterministic_and_coherent() {
        let (_regtest_manager, _cph, mut client_builder, regtest_network) =
            scenarios::custom_clients_default().await;
        let seed_phrase = zcash_primitives::zip339::Mnemonic::from_entropy([1; 32])
            .unwrap()
            .to_string();
        let recipient1 = client_builder
            .build_client(seed_phrase, 0, false, regtest_network)
            .await;
        let base_transparent_receiver = "tmS9nbexug7uT8x1cMTLP1ABEyKXpMjR5F1";
        assert_eq!(
            &get_base_address_macro!(recipient1, "transparent"),
            &base_transparent_receiver
        );
        let base_sapling_receiver = "\
        zregtestsapling1lhjvuj4s3ghhccnjaefdzuwp3h3mfluz6tm8h0dsq2ym3f77zsv0wrrszpmaqlezm3kt6ajdvlw";
        assert_eq!(
            &get_base_address_macro!(recipient1, "sapling"),
            &base_sapling_receiver
        );
        // Verify that the provided seed generates the expected uregtest1qtqr46..  unified address (UA)
        let base_unified_address = "\
        uregtest1qtqr46fwkhmdn336uuyvvxyrv0l7trgc0z9clpryx6vtladnpyt4wvq99p59f4rcyuvpmmd0hm4k5vv6j8\
        edj6n8ltk45sdkptlk7rtzlm4uup4laq8ka8vtxzqemj3yhk6hqhuypupzryhv66w65lah9ms03xa8nref7gux2zzhj\
        nfanxnnrnwscmz6szv2ghrurhu3jsqdx25y2yh";
        assert_eq!(
            &get_base_address_macro!(recipient1, "unified"),
            &base_unified_address
        );

        //Verify that 1 increment of diversification with a tz receiver set produces uregtest1m8un60u... UA
        let new_address = recipient1.do_new_address("tzo").await.unwrap();
        let ua_index_1 = recipient1.do_addresses().await[1].clone();
        let ua_address_index_1 = ua_index_1["address"].clone().to_string();
        assert_eq!(&new_address[0].to_string(), &ua_address_index_1);
        let sapling_index_1 = ua_index_1["receivers"]["sapling"].clone().to_string();
        let transparent_index_1 = ua_index_1["receivers"]["transparent"].clone().to_string();
        let ua_address_index_1_match = ua_address_index_1
            == "\
            uregtest1yhu9ke9hung002w5vcez7y6fe7sgqe4rnc3l2tqyz3yqctmtays6peukkhj2lx45urq666h4dpduz0\
            rjzlmky7cuayj285d003futaljg355tz94l6xnklk5kgthe2x942s3qkxedypsadla56fjx4e5nca9672jmxekj\
            pp94ahz0ax963r2v9wwxfzadnzt3fgwa8pytdhcy4l6z0h";
        let sapling_index_1_match = sapling_index_1
        == "zregtestsapling14wl6gy5h2tg528znyrqayfh2sekntk3lvmwsw68wjz2g205t62sv5xeyzvfk4hlxdwd9gh4ws9n";
        let transparent_index_1_match =
            transparent_index_1 == "tmQuMoTTjU3GFfTjrhPiBYihbTVfYmPk5Gr";

        //  Show orchard diversification is working (regardless of other diversifiers, both previous and other-pool).
        let new_orchard_only_address = recipient1.do_new_address("o").await.unwrap();
        let ua_address_index_2 = new_orchard_only_address[0].to_string();
        let ua_2_orchard_match = ua_address_index_2 ==  "\
        uregtest1yyw480060mdzvnfpfayfhackhgh0jjsuq5lfjf9u68hulmn9efdalmz583xlq6pt8lmyylky6p2usx57lfv7tqu9j0tqqs8asq25p49n";
        assert!(
            ua_address_index_1_match && sapling_index_1_match && transparent_index_1_match,
            "\n\
            ua_1, match: {} Observed:\n\
            {}\n\n\
            sapling_1, match: {} Observed:\n\
            {}\n\n\
            transparent_1, match: {} Observed:\n\
            {}\n\n\
            ua_address_index_2, match: {} Observed:\n\
            {}\n
        ",
            ua_address_index_1_match,
            ua_address_index_1,
            sapling_index_1_match,
            sapling_index_1,
            transparent_index_1_match,
            transparent_index_1,
            ua_2_orchard_match,
            ua_address_index_2
        );
    }

    #[tokio::test]
    async fn ensure_taddrs_from_old_seeds_work() {
        let (_regtest_manager, _cph, mut client_builder, regtest_network) =
            scenarios::custom_clients_default().await;
        // The first taddr generated on commit 9e71a14eb424631372fd08503b1bd83ea763c7fb
        let transparent_address = "tmFLszfkjgim4zoUMAXpuohnFBAKy99rr2i";

        let client_b = client_builder
            .build_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false, regtest_network)
            .await;

        assert_eq!(
            get_base_address_macro!(client_b, "transparent"),
            transparent_address
        );
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

        let config = zingoconfig::ZingoConfig::build(ChainType::Testnet).create();
        let mid_wallet = LightWallet::read_internal(&data[..], &config)
            .await
            .map_err(|e| format!("Cannot deserialize LightWallet version 28 file: {}", e))
            .unwrap();

        let mid_client = LightClient::create_from_wallet_async(mid_wallet, config.clone())
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

        let client = LightClient::create_from_wallet_async(wallet, config)
            .await
            .unwrap();
        let balance = client.do_balance().await;
        assert_eq!(balance.orchard_balance, Some(10342837));
    }

    #[tokio::test]
    async fn sync_all_epochs_from_sapling() {
        let regtest_network = RegtestNetwork::new(1, 1, 3, 5, 7, 9);
        let (regtest_manager, _cph, lightclient) =
            scenarios::unfunded_client(regtest_network).await;
        increase_height_and_wait_for_client(&regtest_manager, &lightclient, 12)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn mine_to_orchard() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, faucet) = scenarios::faucet(
            PoolType::Shielded(ShieldedProtocol::Orchard),
            regtest_network,
        )
        .await;
        check_client_balances!(faucet, o: 1_875_000_000 s: 0 t: 0);
        increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        check_client_balances!(faucet, o: 2_500_000_000u64 s: 0 t: 0);
    }

    #[tokio::test]
    async fn mine_to_sapling() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, faucet) = scenarios::faucet(
            PoolType::Shielded(ShieldedProtocol::Sapling),
            regtest_network,
        )
        .await;
        check_client_balances!(faucet, o: 0 s: 1_875_000_000 t: 0);
        increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        check_client_balances!(faucet, o: 0 s: 2_500_000_000u64 t: 0);
    }

    #[tokio::test]
    async fn mine_to_transparent() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, faucet, _recipient) =
            scenarios::faucet_recipient(PoolType::Transparent, regtest_network).await;
        check_client_balances!(faucet, o: 0 s: 0 t: 1_875_000_000);
        increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        check_client_balances!(faucet, o: 0 s: 0 t: 2_500_000_000u64);
    }

    // test fails to exit when syncing pre-sapling
    // possible issue with dropping child process handler?
    #[ignore]
    #[tokio::test]
    async fn sync_all_epochs() {
        let regtest_network = RegtestNetwork::new(1, 3, 5, 7, 9, 11);
        let (regtest_manager, _cph, lightclient) =
            scenarios::unfunded_client(regtest_network).await;
        increase_height_and_wait_for_client(&regtest_manager, &lightclient, 12)
            .await
            .unwrap();
    }

    // test fails with error message: "66: tx unpaid action limit exceeded"
    #[ignore]
    #[tokio::test]
    async fn mine_to_transparent_and_shield() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, faucet, _recipient) =
            scenarios::faucet_recipient(PoolType::Transparent, regtest_network).await;
        increase_height_and_wait_for_client(&regtest_manager, &faucet, 100)
            .await
            .unwrap();
        from_inputs::shield(&faucet, &[PoolType::Transparent], None)
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn mine_to_transparent_and_propose_shielding() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, faucet, _recipient) =
            scenarios::faucet_recipient(PoolType::Transparent, regtest_network).await;
        increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        let proposal = faucet.propose_shield().await.unwrap();
        let only_step = proposal.steps().first();

        // Orchard action and dummy, plus 4 transparent inputs
        let expected_fee = 30_000;

        assert_eq!(proposal.steps().len(), 1);
        assert_eq!(only_step.transparent_inputs().len(), 4);
        assert_eq!(
            only_step.balance().fee_required(),
            NonNegativeAmount::const_from_u64(expected_fee)
        );
        // Only one change item. I guess change could be split between pools?
        assert_eq!(only_step.balance().proposed_change().len(), 1);
        assert_eq!(
            only_step
                .balance()
                .proposed_change()
                .first()
                .unwrap()
                .value(),
            NonNegativeAmount::const_from_u64(
                (zingo_testvectors::block_rewards::CANOPY * 4) - expected_fee
            )
        )
    }
}
mod slow {
    use orchard::note_encryption::OrchardDomain;
    use zcash_client_backend::{PoolType, ShieldedProtocol};
    use zcash_primitives::consensus::NetworkConstants;
    use zingo_testutils::lightclient::from_inputs;

    use super::*;

    #[tokio::test]
    async fn zero_value_receipts() {
        let (regtest_manager, _cph, faucet, recipient, _txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        let sent_value = 0;
        let _sent_transaction_id = from_inputs::send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                sent_value,
                None,
            )],
        )
        .await
        .unwrap();

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 5)
            .await
            .unwrap();
        let _sent_transaction_id = from_inputs::send(
            &recipient,
            vec![(&get_base_address_macro!(faucet, "unified"), 1000, None)],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 5)
            .await
            .unwrap();

        println!("{}", recipient.do_list_transactions().await.pretty(4));
        println!(
            "{}",
            serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
        );
        println!(
            "{}",
            JsonValue::from(recipient.list_txsummaries().await).pretty(4)
        );
    }
    #[tokio::test]
    async fn zero_value_change() {
        // 2. Send an incoming transaction to fill the wallet
        let value = 100_000;
        let (regtest_manager, _cph, faucet, recipient, _txid) =
            scenarios::faucet_funded_recipient_default(value).await;

        let sent_value = value - u64::from(MINIMUM_FEE);
        let sent_transaction_id = from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                sent_value,
                None,
            )],
        )
        .await
        .unwrap();

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 5)
            .await
            .unwrap();

        let notes = recipient.do_list_notes(true).await;
        assert_eq!(notes["unspent_sapling_notes"].len(), 0);
        assert_eq!(notes["pending_sapling_notes"].len(), 0);
        assert_eq!(notes["unspent_orchard_notes"].len(), 1);
        assert_eq!(notes["pending_orchard_notes"].len(), 0);
        assert_eq!(notes["utxos"].len(), 0);
        assert_eq!(notes["pending_utxos"].len(), 0);

        assert_eq!(notes["spent_sapling_notes"].len(), 0);
        assert_eq!(notes["spent_orchard_notes"].len(), 1);
        assert_eq!(notes["spent_utxos"].len(), 0);
        // We should still have a change note even of zero value, as we send
        // ourself a wallet-readable memo
        assert_eq!(notes["unspent_orchard_notes"][0]["value"], 0);
        assert_eq!(
            notes["spent_orchard_notes"][0]["spent"],
            sent_transaction_id
        );

        check_client_balances!(recipient, o: 0 s: 0 t: 0);
    }
    #[tokio::test]
    async fn witness_clearing() {
        let (regtest_manager, _cph, faucet, recipient, txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;
        let txid = utils::conversion::txid_from_hex_encoded_str(&txid).unwrap();

        // 3. Send z-to-z transaction to external z address with a memo
        let sent_value = 2000;
        let outgoing_memo = "Outgoing Memo";

        let faucet_ua = get_base_address_macro!(faucet, "unified");

        let _sent_transaction_id = from_inputs::send(
            &recipient,
            vec![(&faucet_ua, sent_value, Some(outgoing_memo))],
        )
        .await
        .unwrap();

        for txid_known in recipient
            .wallet
            .transactions()
            .read()
            .await
            .transaction_records_by_id
            .keys()
        {
            dbg!(txid_known);
        }

        // transaction is not yet mined, so witnesses should still be there
        let position = recipient
            .wallet
            .transactions()
            .read()
            .await
            .transaction_records_by_id
            .get(&txid)
            .unwrap()
            .orchard_notes
            .first()
            .unwrap()
            .witnessed_position
            .unwrap();
        assert!(recipient
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .witness_trees()
            .unwrap()
            .witness_tree_orchard
            .marked_positions()
            .unwrap()
            .contains(&position));

        // 4. Mine the sent transaction
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();

        // transaction is now mined, but witnesses should still be there because not 100 blocks yet (i.e., could get reorged)
        let position = recipient
            .wallet
            .transactions()
            .read()
            .await
            .transaction_records_by_id
            .get(&txid)
            .unwrap()
            .orchard_notes
            .first()
            .unwrap()
            .witnessed_position
            .unwrap();
        assert!(recipient
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .witness_trees()
            .unwrap()
            .witness_tree_orchard
            .marked_positions()
            .unwrap()
            .contains(&position));
        dbg!(
            &recipient
                .wallet
                .transaction_context
                .transaction_metadata_set
                .read()
                .await
                .witness_trees()
                .unwrap()
                .witness_tree_orchard
        );

        // 5. Mine 50 blocks, witness should still be there
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 50)
            .await
            .unwrap();
        let position = recipient
            .wallet
            .transactions()
            .read()
            .await
            .transaction_records_by_id
            .get(&txid)
            .unwrap()
            .orchard_notes
            .first()
            .unwrap()
            .witnessed_position
            .unwrap();
        assert!(recipient
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .witness_trees()
            .unwrap()
            .witness_tree_orchard
            .marked_positions()
            .unwrap()
            .contains(&position));

        // 5. Mine 100 blocks, witness should now disappear
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 50)
            .await
            .unwrap();
        let position = recipient
            .wallet
            .transactions()
            .read()
            .await
            .transaction_records_by_id
            .get(&txid)
            .unwrap()
            .orchard_notes
            .first()
            .unwrap()
            .witnessed_position
            .unwrap();
        //Note: This is a negative assertion. Notice the "!"
        dbg!(
            &recipient
                .wallet
                .transaction_context
                .transaction_metadata_set
                .read()
                .await
                .witness_trees()
                .unwrap()
                .witness_tree_orchard
        );
        assert!(!recipient
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .witness_trees()
            .unwrap()
            .witness_tree_orchard
            .marked_positions()
            .unwrap()
            .contains(&position));
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
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 5)
            .await
            .unwrap();

        // Without sync push server forward 2 blocks
        zingo_testutils::increase_server_height(&regtest_manager, 2).await;
        let client_wallet_height = faucet.do_wallet_last_scanned_height().await;

        // Verify that wallet is still back at 6.
        assert_eq!(client_wallet_height.as_fixed_point_u64(0).unwrap(), 8);

        // Interrupt generating send
        from_inputs::send(
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
    #[tokio::test]
    async fn test_scanning_in_watch_only_mode() {
        // # Scenario:
        // 3. reset wallet
        // 4. for every combination of FVKs
        //     4.1. init a wallet with UFVK
        //     4.2. check that the wallet is empty
        //     4.3. rescan
        //     4.4. check that notes and utxos were detected by the wallet
        //
        // # Current watch-only mode limitations:
        // - wallet will not detect funds on all transparent addresses
        //   see: https://github.com/zingolabs/zingolib/issues/245
        // - wallet will not detect funds on internal addresses
        //   see: https://github.com/zingolabs/zingolib/issues/246

        let (regtest_manager, _cph, mut client_builder, regtest_network) =
            scenarios::custom_clients_default().await;
        let faucet = client_builder.build_faucet(false, regtest_network).await;
        let original_recipient = client_builder
            .build_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false, regtest_network)
            .await;
        let zingo_config = zingoconfig::load_clientconfig(
            client_builder.server_id,
            Some(client_builder.zingo_datadir),
            ChainType::Regtest(regtest_network),
            true,
        )
        .unwrap();

        let (recipient_taddr, recipient_sapling, recipient_unified) = (
            get_base_address_macro!(original_recipient, "transparent"),
            get_base_address_macro!(original_recipient, "sapling"),
            get_base_address_macro!(original_recipient, "unified"),
        );
        let addr_amount_memos = vec![
            (recipient_taddr.as_str(), 1_000u64, None),
            (recipient_sapling.as_str(), 2_000u64, None),
            (recipient_unified.as_str(), 3_000u64, None),
        ];
        // 1. fill wallet with a coinbase transaction by syncing faucet with 1-block increase
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        // 2. send a transaction containing all types of outputs
        from_inputs::send(&faucet, addr_amount_memos).await.unwrap();
        zingo_testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &original_recipient,
            1,
        )
        .await
        .unwrap();
        let original_recipient_balance = original_recipient.do_balance().await;
        let sent_t_value = original_recipient_balance.transparent_balance.unwrap();
        let sent_s_value = original_recipient_balance.sapling_balance.unwrap();
        let sent_o_value = original_recipient_balance.orchard_balance.unwrap();
        assert_eq!(sent_t_value, 1000u64);
        assert_eq!(sent_s_value, 2000u64);
        assert_eq!(sent_o_value, 3000u64);

        // check that do_rescan works
        original_recipient.do_rescan().await.unwrap();
        check_client_balances!(original_recipient, o: sent_o_value s: sent_s_value t: sent_t_value);

        // Extract viewing keys
        let wallet_capability = original_recipient.wallet.wallet_capability().clone();
        let [o_fvk, s_fvk, t_fvk] =
            zingo_testutils::build_fvks_from_wallet_capability(&wallet_capability);
        let fvks_sets = [
            vec![&o_fvk],
            vec![&s_fvk],
            vec![&o_fvk, &s_fvk],
            vec![&o_fvk, &t_fvk],
            vec![&s_fvk, &t_fvk],
            vec![&o_fvk, &s_fvk, &t_fvk],
        ];
        for fvks in fvks_sets.iter() {
            log::info!("testing UFVK containing:");
            log::info!("    orchard fvk: {}", fvks.contains(&&o_fvk));
            log::info!("    sapling fvk: {}", fvks.contains(&&s_fvk));
            log::info!("    transparent fvk: {}", fvks.contains(&&t_fvk));

            let watch_client = build_fvk_client(fvks, &zingo_config).await;
            let watch_wc = watch_client.wallet.wallet_capability();
            // assert empty wallet before rescan
            let balance = watch_client.do_balance().await;
            check_expected_balance_with_fvks(fvks, balance, 0, 0, 0);
            watch_client.do_rescan().await.unwrap();
            let balance = watch_client.do_balance().await;
            let notes = watch_client.do_list_notes(true).await;

            check_view_capability_bounds(
                &balance,
                &watch_wc,
                fvks,
                &o_fvk,
                &s_fvk,
                &t_fvk,
                Some(sent_o_value),
                Some(sent_s_value),
                Some(sent_t_value),
                &notes,
            );

            watch_client.do_rescan().await.unwrap();
            assert_eq!(
                from_inputs::send(
                    &watch_client,
                    vec![(zingo_testvectors::EXT_TADDR, 1000, None)]
                )
                .await,
                Err("Wallet is in watch-only mode and thus it cannot spend.".to_string())
            );
        }
    }
    #[tokio::test]
    async fn t_incoming_t_outgoing_disallowed() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;

        // 2. Get an incoming transaction to a t address
        let taddr = get_base_address_macro!(recipient, "transparent");
        let value = 100_000;

        from_inputs::send(&faucet, vec![(taddr.as_str(), value, None)])
            .await
            .unwrap();

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        recipient.do_sync(true).await.unwrap();

        // 3. Test the list
        let list = recipient.do_list_transactions().await;
        assert_eq!(list[0]["block_height"].as_u64().unwrap(), 4);
        assert_eq!(list[0]["address"], taddr);
        assert_eq!(list[0]["amount"].as_u64().unwrap(), value);

        // 4. We can't spend the funds, as they're transparent. We need to shield first
        let sent_value = 20_000;
        let sent_transaction_error = from_inputs::send(
            &recipient,
            vec![(zingo_testvectors::EXT_TADDR, sent_value, None)],
        )
        .await
        .unwrap_err();
        assert_eq!(sent_transaction_error, "Insufficient verified shielded funds. Have 0 zats, need 30000 zats. NOTE: funds need at least 1 confirmations before they can be spent. Transparent funds must be shielded before they can be spent. If you are trying to spend transparent funds, please use the shield button and try again in a few minutes.");
    }
    #[tokio::test]
    async fn shield_sapling() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;

        let sapling_dust = 100;
        let _sent_transaction_id = from_inputs::send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "sapling"),
                sapling_dust,
                None,
            )],
        )
        .await
        .unwrap();

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        println!(
            "{}",
            serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
        );

        assert_eq!(
            from_inputs::shield(
                &recipient,
                &[PoolType::Shielded(ShieldedProtocol::Sapling)],
                None
            )
            .await,
            Err(
                "Not enough transparent/sapling balance to shield. Have 100 zats, \
        need more than 10000 zats to cover tx fee"
                    .to_string()
            )
        );

        let sapling_enough_for_fee = 10_100;
        faucet.do_sync(false).await.unwrap();
        let _sent_transaction_id = from_inputs::send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "sapling"),
                sapling_enough_for_fee,
                None,
            )],
        )
        .await
        .unwrap();

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        from_inputs::shield(
            &recipient,
            &[
                PoolType::Shielded(ShieldedProtocol::Sapling),
                PoolType::Transparent,
            ],
            None,
        )
        .await
        .unwrap();

        // The exact same thing again, but with pre-existing orchard funds
        // already in the shielding wallet
        faucet.do_sync(false).await.unwrap();
        let _sent_transaction_id = from_inputs::send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "sapling"),
                sapling_enough_for_fee,
                None,
            )],
        )
        .await
        .unwrap();

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        from_inputs::shield(
            &recipient,
            &[
                PoolType::Shielded(ShieldedProtocol::Sapling),
                PoolType::Transparent,
            ],
            None,
        )
        .await
        .unwrap();

        println!(
            "{}",
            serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
        );
    }
    #[tokio::test]
    async fn shield_heartwood_sapling_funds() {
        let regtest_network = RegtestNetwork::new(1, 1, 1, 1, 3, 5);
        let (regtest_manager, _cph, faucet) = scenarios::faucet(
            PoolType::Shielded(ShieldedProtocol::Sapling),
            regtest_network,
        )
        .await;
        increase_height_and_wait_for_client(&regtest_manager, &faucet, 3)
            .await
            .unwrap();
        check_client_balances!(faucet, o: 0 s: 3_500_000_000u64 t: 0);
        from_inputs::shield(
            &faucet,
            &[PoolType::Shielded(ShieldedProtocol::Sapling)],
            None,
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        check_client_balances!(faucet, o: 3_499_990_000u64 s: 625_010_000 t: 0);
    }
    #[tokio::test]
    async fn sends_to_self_handle_balance_properly() {
        let transparent_funding = 100_000;
        let (ref regtest_manager, _cph, faucet, ref recipient) =
            scenarios::faucet_recipient_default().await;
        from_inputs::send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "sapling"),
                transparent_funding,
                None,
            )],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(regtest_manager, recipient, 1)
            .await
            .unwrap();
        from_inputs::shield(
            recipient,
            &[
                PoolType::Shielded(ShieldedProtocol::Sapling),
                PoolType::Transparent,
            ],
            None,
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(regtest_manager, recipient, 1)
            .await
            .unwrap();
        println!(
            "{}",
            serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
        );
        println!("{}", recipient.do_list_transactions().await.pretty(2));
        println!(
            "{}",
            JsonValue::from(
                recipient
                    .list_txsummaries()
                    .await
                    .into_iter()
                    .map(JsonValue::from)
                    .collect::<Vec<_>>()
            )
            .pretty(2)
        );
        recipient.do_rescan().await.unwrap();
        println!(
            "{}",
            serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
        );
        println!("{}", recipient.do_list_transactions().await.pretty(2));
        println!(
            "{}",
            JsonValue::from(
                recipient
                    .list_txsummaries()
                    .await
                    .into_iter()
                    .map(JsonValue::from)
                    .collect::<Vec<_>>()
            )
            .pretty(2)
        );
    }
    #[tokio::test]
    async fn send_to_ua_saves_full_ua_in_wallet() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        //utils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 5).await;
        let recipient_unified_address = get_base_address_macro!(recipient, "unified");
        let sent_value = 50_000;
        from_inputs::send(
            &faucet,
            vec![(recipient_unified_address.as_str(), sent_value, None)],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        let list = faucet.do_list_transactions().await;
        assert!(list.members().any(|transaction| {
            transaction.entries().any(|(key, value)| {
                if key == "outgoing_metadata" {
                    value[0]["address"] == recipient_unified_address
                } else {
                    false
                }
            })
        }));
        faucet.do_rescan().await.unwrap();
        let new_list = faucet.do_list_transactions().await;
        assert!(new_list.members().any(|transaction| {
            transaction.entries().any(|(key, value)| {
                if key == "outgoing_metadata" {
                    value[0]["address"] == recipient_unified_address
                } else {
                    false
                }
            })
        }));
        assert_eq!(
            list,
            new_list,
            "Pre-Rescan: {}\n\n\nPost-Rescan: {}\n\n\n",
            json::stringify_pretty(list.clone(), 4),
            json::stringify_pretty(new_list.clone(), 4)
        );
    }
    #[tokio::test]
    async fn send_to_transparent_and_sapling_maintain_balance() {
        let recipient_initial_funds = 100_000_000;
        let first_send_to_sapling = 20_000;
        let first_send_to_transparent = 20_000;
        let recipient_second_wave = 1_000_000;
        let second_send_to_transparent = 20_000;
        let second_send_to_sapling = 20_000;
        let third_send_to_transparent = 20_000;

        let (ref regtest_manager, _cph, faucet, recipient, _txid) =
            scenarios::faucet_funded_recipient_default(recipient_initial_funds).await;

        let expected_transactions = json::parse(
        r#"
        [
            {
                "block_height": 5,
                "pending": false,
                "datetime": 1694820763,
                "position": 0,
                "txid": "d5eaac5563f8bc1a0406588e05953977ad768d02f1cf8449e9d7d9cc8de3801c",
                "amount": 100000000,
                "zec_price": null,
                "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8",
                "memo": null
            },
            {
                "block_height": 6,
                "pending": false,
                "datetime": 1694825595,
                "txid": "4ee5a583e6462eb4c39f9d8188e855bb1e37d989fcb8b417cff93c27b006e72d",
                "zec_price": null,
                "amount": -30000,
                "outgoing_metadata": [
                    {
                        "address": "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p",
                        "value": 20000,
                        "memo": null
                    }
                ]
            },
            {
                "block_height": 7,
                "pending": true,
                "datetime": 1694825735,
                "txid": "55de92ebf5effc3ed67a289788ede88514a9d2c407af6154b00969325e2fdf00",
                "zec_price": null,
                "amount": -30000,
                "outgoing_metadata": [
                    {
                        "address": "tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd",
                        "value": 20000,
                        "memo": null
                    }
                ]
            }
        ]"#,
    ).unwrap();

        from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                first_send_to_sapling,
                None,
            )],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(regtest_manager, &recipient, 1)
            .await
            .unwrap();
        from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "transparent"),
                first_send_to_transparent,
                None,
            )],
        )
        .await
        .unwrap();

        let expected_funds = recipient_initial_funds
            - first_send_to_sapling
            - first_send_to_transparent
            - (2 * u64::from(MINIMUM_FEE));
        assert_eq!(
            recipient.wallet.maybe_verified_orchard_balance(None).await,
            Some(expected_funds)
        );
        assert_eq!(
            recipient
                .wallet
                .verified_balance::<OrchardDomain>(None)
                .await,
            Some(0)
        );

        let transactions = recipient.do_list_transactions().await;
        assert_eq!(
            transactions.members().len(),
            expected_transactions.members().len()
        );
        for (t1, t2) in transactions.members().zip(expected_transactions.members()) {
            assert!(
                check_transaction_equality(t1, t2),
                "\n\n\nobserved: {}\n\n\nexpected: {}\n\n\n",
                t1.pretty(4),
                t2.pretty(4)
            );
        }

        faucet.do_sync(false).await.unwrap();
        from_inputs::send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                recipient_second_wave,
                Some("Second wave incoming"),
            )],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(regtest_manager, &recipient, 1)
            .await
            .unwrap();
        from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "transparent"),
                second_send_to_transparent,
                None,
            )],
        )
        .await
        .unwrap();
        from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                second_send_to_sapling,
                None,
            )],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(regtest_manager, &recipient, 1)
            .await
            .unwrap();

        from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "transparent"),
                third_send_to_transparent,
                None,
            )],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(regtest_manager, &recipient, 1)
            .await
            .unwrap();

        let second_wave_expected_funds = expected_funds + recipient_second_wave
            - second_send_to_sapling
            - second_send_to_transparent
            - third_send_to_transparent
            - (3 * u64::from(MINIMUM_FEE));
        assert_eq!(
            recipient.wallet.maybe_verified_orchard_balance(None).await,
            Some(second_wave_expected_funds),
        );

        let second_wave_expected_transactions = json::parse(r#"
        [
            {
                "block_height": 5,
                "pending": false,
                "datetime": 1686330002,
                "position": 0,
                "txid": "f040440eade0afc99800fee54753afb71fb09894483f1f1fa7462dedb63e7c02",
                "amount": 100000000,
                "zec_price": null,
                "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8",
                "memo": null
            },
            {
                "block_height": 6,
                "pending": false,
                "datetime": 1686330013,
                "txid": "db532064c89c7d8266e107ffefc614f3c34050af922973199e398fcd18c43ea5",
                "zec_price": null,
                "amount": -30000,
                "outgoing_metadata": [
                    {
                        "address": "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p",
                        "value": 20000,
                        "memo": null
                    }
                ]
            },
            {
                "block_height": 7,
                "pending": false,
                "datetime": 1686330006,
                "txid": "be81f76bf37bb6d5d762c7bb48419f239787023b8344c30ce0771c8ce21e480f",
                "zec_price": null,
                "amount": -30000,
                "outgoing_metadata": [
                    {
                        "address": "tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd",
                        "value": 20000,
                        "memo": null
                    }
                ]
            },
            {
                "block_height": 7,
                "pending": false,
                "datetime": 1686330013,
                "position": 0,
                "txid": "caf9438c9c61923d24a9594651cc694edc660eabb0082122c4588ae381edc3b4",
                "amount": 1000000,
                "zec_price": null,
                "address": "uregtest1wdukkmv5p5n824e8ytnc3m6m77v9vwwl7hcpj0wangf6z23f9x0fnaen625dxgn8cgp67vzw6swuar6uwp3nqywfvvkuqrhdjffxjfg644uthqazrtxhrgwac0a6ujzgwp8y9cwthjeayq8r0q6786yugzzyt9vevxn7peujlw8kp3vf6d8p4fvvpd8qd5p7xt2uagelmtf3vl6w3u8",
                "memo": "Second wave incoming"
            },
            {
                "block_height": 8,
                "pending": false,
                "datetime": 1686330021,
                "txid": "95a41ba1c6e2b7edf63ddde7899567431a6b36b7583ba1e359560041e5f8ce2b",
                "zec_price": null,
                "amount": -30000,
                "outgoing_metadata": [
                    {
                        "address": "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p",
                        "value": 20000,
                        "memo": null
                    }
                ]
            },
            {
                "block_height": 8,
                "pending": false,
                "datetime": 1686330021,
                "txid": "c1004c32395ff45448fb943a7da4cc2819762066eea2628cd0a4aee65106207d",
                "zec_price": null,
                "amount": -30000,
                "outgoing_metadata": [
                    {
                        "address": "tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd",
                        "value": 20000,
                        "memo": null
                    }
                ]
            },
            {
                "block_height": 9,
                "pending": false,
                "datetime": 1686330024,
                "txid": "c5e94f462218634b37a2a3324f89bd288bc55ab877ea516a6203e48c207ba955",
                "zec_price": null,
                "amount": -30000,
                "outgoing_metadata": [
                    {
                        "address": "tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd",
                        "value": 20000,
                        "memo": null
                    }
                ]
            }
        ]"#)
    .unwrap();
        let second_wave_transactions = recipient.do_list_transactions().await;
        assert_eq!(
            second_wave_transactions.len(),
            second_wave_expected_transactions.len()
        );
        for transaction in second_wave_transactions.members() {
            assert!(
                second_wave_expected_transactions
                    .members()
                    .any(|t2| check_transaction_equality(transaction, t2)),
                "fail on: {:#?}",
                transaction
            );
        }
    }
    #[tokio::test]
    async fn send_orchard_back_and_forth() {
        // setup
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        let faucet_to_recipient_amount = 20_000u64;
        let recipient_to_faucet_amount = 5_000u64;
        // check start state
        faucet.do_sync(true).await.unwrap();
        let wallet_height = faucet.do_wallet_last_scanned_height().await;
        assert_eq!(
            wallet_height.as_fixed_point_u64(0).unwrap(),
            BASE_HEIGHT as u64
        );
        let three_blocks_reward = block_rewards::CANOPY
            .checked_mul(BASE_HEIGHT as u64)
            .unwrap();
        check_client_balances!(faucet, o: three_blocks_reward s: 0 t: 0);

        // post transfer to recipient, and verify
        from_inputs::send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                faucet_to_recipient_amount,
                Some("Orcharding"),
            )],
        )
        .await
        .unwrap();
        let orch_change =
            block_rewards::CANOPY - (faucet_to_recipient_amount + u64::from(MINIMUM_FEE));
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        faucet.do_sync(true).await.unwrap();
        let faucet_orch = three_blocks_reward + orch_change + u64::from(MINIMUM_FEE);

        println!(
            "{}",
            JsonValue::from(faucet.list_txsummaries().await).pretty(4)
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&faucet.do_balance().await).unwrap()
        );

        check_client_balances!(faucet, o: faucet_orch s: 0 t: 0);
        check_client_balances!(recipient, o: faucet_to_recipient_amount s: 0 t: 0);

        // post half back to faucet, and verify
        from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                recipient_to_faucet_amount,
                Some("Sending back"),
            )],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        recipient.do_sync(true).await.unwrap();

        let faucet_final_orch = faucet_orch
            + recipient_to_faucet_amount
            + block_rewards::CANOPY
            + u64::from(MINIMUM_FEE);
        let recipient_final_orch =
            faucet_to_recipient_amount - (u64::from(MINIMUM_FEE) + recipient_to_faucet_amount);
        check_client_balances!(
            faucet,
            o: faucet_final_orch s: 0 t: 0
        );
        check_client_balances!(recipient, o: recipient_final_orch s: 0 t: 0);
    }
    #[tokio::test]
    async fn send_mined_sapling_to_orchard() {
        // This test shows a confirmation changing the state of balance by
        // debiting unverified_orchard_balance and crediting verified_orchard_balance.  The debit amount is
        // consistent with all the notes in the relevant block changing state.
        // NOTE that the balance doesn't give insight into the distribution across notes.
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, faucet) = scenarios::faucet(
            PoolType::Shielded(ShieldedProtocol::Sapling),
            regtest_network,
        )
        .await;
        let amount_to_send = 5_000;
        from_inputs::send(
            &faucet,
            vec![(
                get_base_address_macro!(faucet, "unified").as_str(),
                amount_to_send,
                Some("Scenario test: engage!"),
            )],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        let balance = faucet.do_balance().await;
        // We send change to orchard now, so we should have the full value of the note
        // we spent, minus the transaction fee
        assert_eq!(balance.unverified_orchard_balance, Some(0));
        assert_eq!(
            balance.verified_orchard_balance.unwrap(),
            625_000_000 - u64::from(MINIMUM_FEE)
        );
    }
    #[tokio::test]
    async fn send_heartwood_sapling_funds() {
        let regtest_network = RegtestNetwork::new(1, 1, 1, 1, 3, 5);
        let (regtest_manager, _cph, faucet, recipient) = scenarios::faucet_recipient(
            PoolType::Shielded(ShieldedProtocol::Sapling),
            regtest_network,
        )
        .await;
        increase_height_and_wait_for_client(&regtest_manager, &faucet, 3)
            .await
            .unwrap();
        check_client_balances!(faucet, o: 0 s: 3_500_000_000u64 t: 0);
        from_inputs::send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                3_499_990_000u64,
                None,
            )],
        )
        .await
        .unwrap();
        check_client_balances!(faucet, o: 0 s: 0 t: 0);
        increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        check_client_balances!(recipient, o: 3_499_990_000u64 s: 0 t: 0);
    }
    #[tokio::test]
    async fn send_funds_to_all_pools() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (
            _regtest_manager,
            _cph,
            _faucet,
            recipient,
            _orchard_txid,
            _sapling_txid,
            _transparent_txid,
        ) = scenarios::faucet_funded_recipient(
            Some(100_000),
            Some(100_000),
            Some(100_000),
            PoolType::Shielded(ShieldedProtocol::Orchard),
            regtest_network,
        )
        .await;
        check_client_balances!(recipient, o: 100_000 s: 100_000 t: 100_000);
    }
    #[tokio::test]
    async fn self_send_to_t_displays_as_one_transaction() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        let recipient_unified_address = get_base_address_macro!(recipient, "unified");
        let sent_value = 50_000;
        from_inputs::send(
            &faucet,
            vec![(recipient_unified_address.as_str(), sent_value, None)],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        let recipient_taddr = get_base_address_macro!(recipient, "transparent");
        let recipient_zaddr = get_base_address_macro!(recipient, "sapling");
        let sent_to_taddr_value = 5_000;
        let sent_to_zaddr_value = 11_000;
        let sent_to_self_orchard_value = 1_000;
        from_inputs::send(
            &recipient,
            vec![(recipient_taddr.as_str(), sent_to_taddr_value, None)],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        from_inputs::send(
            &recipient,
            vec![
                (recipient_taddr.as_str(), sent_to_taddr_value, None),
                (recipient_zaddr.as_str(), sent_to_zaddr_value, Some("foo")),
                (
                    recipient_unified_address.as_str(),
                    sent_to_self_orchard_value,
                    Some("bar"),
                ),
            ],
        )
        .await
        .unwrap();
        faucet.do_sync(false).await.unwrap();
        from_inputs::send(
            &faucet,
            vec![
                (recipient_taddr.as_str(), sent_to_taddr_value, None),
                (recipient_zaddr.as_str(), sent_to_zaddr_value, Some("foo2")),
                (
                    recipient_unified_address.as_str(),
                    sent_to_self_orchard_value,
                    Some("bar2"),
                ),
            ],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        println!(
            "{}",
            json::stringify_pretty(recipient.do_list_transactions().await, 4)
        );
        let transactions = recipient.do_list_transactions().await;
        let mut txids = transactions
            .members()
            .map(|transaction| transaction["txid"].as_str());
        assert!(itertools::Itertools::all_unique(&mut txids));
    }
    #[tokio::test]
    async fn sapling_to_sapling_scan_together() {
        // Create an incoming transaction, and then send that transaction, and scan everything together, to make sure it works.
        // (For this test, the Sapling Domain is assumed in all cases.)
        // Sender Setup:
        // 1. create a spend key: SpendK_S
        // 2. derive a Shielded Payment Address from SpendK_S: SPA_KS
        // 3. construct a Block Reward Transaction where SPA_KS receives a block reward: BRT
        // 4. publish BRT
        // 5. optionally mine a block including BRT <-- There are two separate tests to run
        // 6. optionally mine sufficient subsequent blocks to "validate" BRT
        // Recipient Setup:
        // 1. create a spend key: "SpendK_R"
        // 2. from SpendK_R derive a Shielded Payment Address: SPA_R
        // Test Procedure:
        // 1. construct a transaction "spending" from a SpendK_S output to SPA_R
        // 2. publish the transaction to the mempool
        // 3. mine a block
        // Constraints:
        // 1. SpendK_S controls start - spend funds
        // 2. SpendK_R controls 0 + spend funds
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;

        // Give the faucet a block reward
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        let value = 100_000;

        // Send some sapling value to the recipient
        let txid = zingo_testutils::send_value_between_clients_and_sync(
            &regtest_manager,
            &faucet,
            &recipient,
            value,
            "sapling",
        )
        .await
        .unwrap();

        let spent_value = 250;

        // Construct transaction to wallet-external recipient-address.
        let exit_zaddr = get_base_address_macro!(faucet, "sapling");
        let spent_txid = from_inputs::send(&recipient, vec![(&exit_zaddr, spent_value, None)])
            .await
            .unwrap();

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        // 5. Check the transaction list to make sure we got all transactions
        let list = recipient.do_list_transactions().await;

        assert_eq!(list[0]["block_height"].as_u64().unwrap(), 5);
        assert_eq!(list[0]["txid"], txid.to_string());
        assert_eq!(list[0]["amount"].as_i64().unwrap(), (value as i64));

        assert_eq!(list[1]["block_height"].as_u64().unwrap(), 6);
        assert_eq!(list[1]["txid"], spent_txid.to_string());
        assert_eq!(
            list[1]["amount"].as_i64().unwrap(),
            -((spent_value + u64::from(MINIMUM_FEE)) as i64)
        );
        assert_eq!(list[1]["outgoing_metadata"][0]["address"], exit_zaddr);
        assert_eq!(
            list[1]["outgoing_metadata"][0]["value"].as_u64().unwrap(),
            spent_value
        );
    }
    #[tokio::test]
    async fn sapling_incoming_sapling_outgoing() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        let value = 100_000;

        // 2. Send an incoming transaction to fill the wallet
        let faucet_funding_txid = from_inputs::send(
            &faucet,
            vec![(&get_base_address_macro!(recipient, "sapling"), value, None)],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();

        assert_eq!(recipient.wallet.last_synced_height().await, 4);

        // 3. Check the balance is correct, and we received the incoming transaction from ?outside?
        let b = recipient.do_balance().await;
        let addresses = recipient.do_addresses().await;
        assert_eq!(b.sapling_balance.unwrap(), value);
        assert_eq!(b.unverified_sapling_balance.unwrap(), 0);
        assert_eq!(b.spendable_sapling_balance.unwrap(), value);
        assert_eq!(
            addresses[0]["receivers"]["sapling"],
            encode_payment_address(
                recipient.config().chain.hrp_sapling_payment_address(),
                recipient.wallet.wallet_capability().addresses()[0]
                    .sapling()
                    .unwrap()
            ),
        );

        let list = recipient.do_list_transactions().await;
        if let JsonValue::Array(list) = list {
            assert_eq!(list.len(), 1);
            let faucet_sent_transaction = list[0].clone();

            assert_eq!(
                faucet_sent_transaction["txid"],
                faucet_funding_txid.to_string()
            );
            assert_eq!(faucet_sent_transaction["amount"].as_u64().unwrap(), value);
            assert_eq!(
                faucet_sent_transaction["address"],
                recipient.wallet.wallet_capability().addresses()[0]
                    .encode(&recipient.config().chain)
            );
            assert_eq!(faucet_sent_transaction["block_height"].as_u64().unwrap(), 4);
        } else {
            panic!("Expecting an array");
        }

        // 4. Send z-to-z transaction to external z address with a memo
        let sent_value = 2000;
        let outgoing_memo = "Outgoing Memo";

        let sent_transaction_id = from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                sent_value,
                Some(outgoing_memo),
            )],
        )
        .await
        .unwrap();

        // 5. Check the pending transaction is present
        // 5.1 Check notes

        let notes = recipient.do_list_notes(true).await;
        // Has a new (pending) unspent note (the change)
        assert_eq!(notes["unspent_orchard_notes"].len(), 1);
        assert_eq!(
            notes["unspent_orchard_notes"][0]["created_in_txid"],
            sent_transaction_id
        );
        assert!(notes["unspent_orchard_notes"][0]["pending"]
            .as_bool()
            .unwrap());

        assert_eq!(notes["spent_sapling_notes"].len(), 0);
        assert_eq!(notes["pending_sapling_notes"].len(), 1);
        assert_eq!(
            notes["pending_sapling_notes"][0]["created_in_txid"],
            faucet_funding_txid.to_string()
        );
        assert_eq!(
            notes["pending_sapling_notes"][0]["pending_spent"],
            sent_transaction_id
        );
        assert!(notes["pending_sapling_notes"][0]["spent"].is_null());
        assert!(notes["pending_sapling_notes"][0]["spent_at_height"].is_null());

        // Check transaction list
        let list = recipient.do_list_transactions().await;

        assert_eq!(list.len(), 2);
        let send_transaction = list
            .members()
            .find(|transaction| transaction["txid"] == sent_transaction_id)
            .unwrap();

        assert_eq!(send_transaction["txid"], sent_transaction_id);
        assert_eq!(
            send_transaction["amount"].as_i64().unwrap(),
            -(sent_value as i64 + u64::from(MINIMUM_FEE) as i64)
        );
        assert!(send_transaction["pending"].as_bool().unwrap());
        assert_eq!(send_transaction["block_height"].as_u64().unwrap(), 5);

        assert_eq!(
            send_transaction["outgoing_metadata"][0]["address"],
            get_base_address_macro!(faucet, "sapling")
        );
        assert_eq!(
            send_transaction["outgoing_metadata"][0]["memo"],
            outgoing_memo
        );
        assert_eq!(
            send_transaction["outgoing_metadata"][0]["value"]
                .as_u64()
                .unwrap(),
            sent_value
        );

        // 6. Mine the sent transaction
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();

        assert!(!send_transaction.contains("pending"));
        assert_eq!(send_transaction["block_height"].as_u64().unwrap(), 5);

        // 7. Check the notes to see that we have one spent sapling note and one unspent orchard note (change)
        // Which is immediately spendable.
        let notes = recipient.do_list_notes(true).await;
        println!("{}", json::stringify_pretty(notes.clone(), 4));
        assert_eq!(notes["unspent_orchard_notes"].len(), 1);
        assert_eq!(
            notes["unspent_orchard_notes"][0]["created_in_block"]
                .as_u64()
                .unwrap(),
            5
        );
        assert_eq!(
            notes["unspent_orchard_notes"][0]["created_in_txid"],
            sent_transaction_id
        );
        assert_eq!(
            notes["unspent_orchard_notes"][0]["value"].as_u64().unwrap(),
            value - sent_value - u64::from(MINIMUM_FEE)
        );
        assert!(notes["unspent_orchard_notes"][0]["is_change"]
            .as_bool()
            .unwrap());
        assert!(notes["unspent_orchard_notes"][0]["spendable"]
            .as_bool()
            .unwrap()); // Spendable

        assert_eq!(notes["spent_sapling_notes"].len(), 1);
        assert_eq!(
            notes["spent_sapling_notes"][0]["created_in_block"]
                .as_u64()
                .unwrap(),
            4
        );
        assert_eq!(
            notes["spent_sapling_notes"][0]["value"].as_u64().unwrap(),
            value
        );
        assert!(!notes["spent_sapling_notes"][0]["is_change"]
            .as_bool()
            .unwrap());
        assert!(!notes["spent_sapling_notes"][0]["spendable"]
            .as_bool()
            .unwrap()); // Already spent
        assert_eq!(
            notes["spent_sapling_notes"][0]["spent"],
            sent_transaction_id
        );
        assert_eq!(
            notes["spent_sapling_notes"][0]["spent_at_height"]
                .as_u64()
                .unwrap(),
            5
        );
    }
    #[tokio::test]
    async fn sapling_dust_fee_collection() {
        let (regtest_manager, __cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        let recipient_sapling = get_base_address_macro!(recipient, "sapling");
        let recipient_unified = get_base_address_macro!(recipient, "unified");
        check_client_balances!(recipient, o: 0 s: 0 t: 0);
        let fee = u64::from(MINIMUM_FEE);
        let for_orchard = dbg!(fee * 10);
        let for_sapling = dbg!(fee / 10);
        from_inputs::send(
            &faucet,
            vec![
                (&recipient_unified, for_orchard, Some("Plenty for orchard.")),
                (&recipient_sapling, for_sapling, Some("Dust for sapling.")),
            ],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        check_client_balances!(recipient, o: for_orchard s: for_sapling t: 0 );

        from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                fee * 5,
                Some("Five times fee."),
            )],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        let remaining_orchard = for_orchard - (6 * fee);
        check_client_balances!(recipient, o: remaining_orchard s: for_sapling t: 0);
    }
    #[tokio::test]
    async fn sandblast_filter_preserves_trees() {
        let (ref regtest_manager, _cph, ref faucet, ref recipient, _txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;
        recipient
            .wallet
            .wallet_options
            .write()
            .await
            .transaction_size_filter = Some(10);
        recipient.do_sync(false).await.unwrap();
        dbg!(
            recipient
                .wallet
                .wallet_options
                .read()
                .await
                .transaction_size_filter
        );

        println!("creating vec");
        from_inputs::send(
            faucet,
            vec![(&get_base_address_macro!(faucet, "unified"), 10, None); 15],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(regtest_manager, recipient, 10)
            .await
            .unwrap();
        from_inputs::send(
            recipient,
            vec![(&get_base_address_macro!(faucet, "unified"), 10, None)],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(regtest_manager, recipient, 10)
            .await
            .unwrap();
        faucet.do_sync(false).await.unwrap();
        assert_eq!(
            faucet
                .wallet
                .transaction_context
                .transaction_metadata_set
                .read()
                .await
                .witness_trees()
                .unwrap()
                .witness_tree_orchard
                .max_leaf_position(0),
            recipient
                .wallet
                .transaction_context
                .transaction_metadata_set
                .read()
                .await
                .witness_trees()
                .unwrap()
                .witness_tree_orchard
                .max_leaf_position(0)
        );
    }
    #[tokio::test]
    async fn rescan_still_have_outgoing_metadata_with_sends_to_self() {
        let (regtest_manager, _cph, faucet) = scenarios::faucet_default().await;
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        let sapling_addr = get_base_address_macro!(faucet, "sapling");
        for memo in [None, Some("foo")] {
            from_inputs::send(
                &faucet,
                vec![(
                    sapling_addr.as_str(),
                    {
                        let balance = faucet.do_balance().await;
                        balance.spendable_sapling_balance.unwrap()
                            + balance.spendable_orchard_balance.unwrap()
                    } - u64::from(MINIMUM_FEE),
                    memo,
                )],
            )
            .await
            .unwrap();
            zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
                .await
                .unwrap();
        }
        let transactions = faucet.do_list_transactions().await;
        let notes = faucet.do_list_notes(true).await;
        faucet.do_rescan().await.unwrap();
        let post_rescan_transactions = faucet.do_list_transactions().await;
        let post_rescan_notes = faucet.do_list_notes(true).await;
        assert_eq!(
            transactions,
            post_rescan_transactions,
            "Pre-Rescan: {}\n\n\nPost-Rescan: {}",
            json::stringify_pretty(transactions.clone(), 4),
            json::stringify_pretty(post_rescan_transactions.clone(), 4)
        );

        // Notes are not in deterministic order after rescan. Instead, iterate over all
        // the notes and check that they exist post-rescan
        for (field_name, field) in notes.entries() {
            for note in field.members() {
                assert!(post_rescan_notes[field_name]
                    .members()
                    .any(|post_rescan_note| post_rescan_note == note));
            }
            assert_eq!(field.len(), post_rescan_notes[field_name].len());
        }
    }
    #[tokio::test]
    async fn rescan_still_have_outgoing_metadata() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        from_inputs::send(
            &faucet,
            vec![(
                get_base_address_macro!(recipient, "sapling").as_str(),
                1_000,
                Some("foo"),
            )],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        let transactions = faucet.do_list_transactions().await;
        faucet.do_rescan().await.unwrap();
        let post_rescan_transactions = faucet.do_list_transactions().await;
        assert_eq!(transactions, post_rescan_transactions);
    }
    #[tokio::test]
    async fn note_selection_order() {
        // In order to fund a transaction multiple notes may be selected and consumed.
        // To minimize note selection operations notes are consumed from largest to smallest.
        // In addition to testing the order in which notes are selected this test:
        //   * sends to a sapling address
        //   * sends back to the original sender's UA
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 5)
            .await
            .unwrap();

        let client_2_saplingaddress = get_base_address_macro!(recipient, "sapling");
        // Send three transfers in increasing 1000 zat increments
        // These are sent from the coinbase funded client which will
        // subsequently receive funding via it's orchard-packed UA.
        let memos = ["1", "2", "3"];
        from_inputs::send(
            &faucet,
            (1..=3)
                .map(|n| {
                    (
                        client_2_saplingaddress.as_str(),
                        n * 10000,
                        Some(memos[(n - 1) as usize]),
                    )
                })
                .collect(),
        )
        .await
        .unwrap();

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 5)
            .await
            .unwrap();
        // We know that the largest single note that 2 received from 1 was 3000, for 2 to send
        // 3000 back to 1 it will have to collect funds from two notes to pay the full 3000
        // plus the transaction fee.
        from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                30000,
                Some("Sending back, should have 2 inputs"),
            )],
        )
        .await
        .unwrap();
        let client_2_notes = recipient.do_list_notes(false).await;
        // The 3000 zat note to cover the value, plus another for the tx-fee.
        let first_value = client_2_notes["pending_sapling_notes"][0]["value"]
            .as_fixed_point_u64(0)
            .unwrap();
        let second_value = client_2_notes["pending_sapling_notes"][1]["value"]
            .as_fixed_point_u64(0)
            .unwrap();
        assert!(
            first_value == 30000u64 && second_value == 20000u64
                || first_value == 20000u64 && second_value == 30000u64
        );
        //);
        // Because the above tx fee won't consume a full note, change will be sent back to 2.
        // This implies that client_2 will have a total of 2 unspent notes:
        //  * one (sapling) from client_1 sent above (and never used) + 1 (orchard) as change to itself
        assert_eq!(client_2_notes["unspent_sapling_notes"].len(), 1);
        assert_eq!(client_2_notes["unspent_orchard_notes"].len(), 1);
        let change_note = client_2_notes["unspent_orchard_notes"]
            .members()
            .filter(|note| note["is_change"].as_bool().unwrap())
            .collect::<Vec<_>>()[0];
        // Because 2000 is the size of the second largest note.
        assert_eq!(change_note["value"], 20000 - u64::from(MINIMUM_FEE));
        let non_change_note_values = client_2_notes["unspent_sapling_notes"]
            .members()
            .filter(|note| !note["is_change"].as_bool().unwrap())
            .map(extract_value_as_u64)
            .collect::<Vec<_>>();
        // client_2 got a total of 3000+2000+1000
        // It sent 3000 to the client_1, and also
        // paid the default transaction fee.
        // In non change notes it has 1000.
        // There is an outstanding 2000 that is marked as change.
        // After sync the unspent_sapling_notes should go to 3000.
        assert_eq!(non_change_note_values.iter().sum::<u64>(), 10000u64);

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 5)
            .await
            .unwrap();
        let client_2_post_transaction_notes = recipient.do_list_notes(false).await;
        assert_eq!(
            client_2_post_transaction_notes["pending_sapling_notes"].len(),
            0
        );
        assert_eq!(
            client_2_post_transaction_notes["unspent_sapling_notes"].len(),
            1
        );
        assert_eq!(
            client_2_post_transaction_notes["unspent_orchard_notes"].len(),
            1
        );
        assert_eq!(
            client_2_post_transaction_notes["unspent_sapling_notes"]
                .members()
                .chain(client_2_post_transaction_notes["unspent_orchard_notes"].members())
                .map(extract_value_as_u64)
                .sum::<u64>(),
            20000u64 // 10000 received and unused + (20000 - 10000 txfee)
        );

        // More explicit than ignoring the unused variable, we only care about this in order to drop it
    }
    #[tokio::test]
    async fn multiple_outgoing_metadatas_work_right_on_restore() {
        let inital_value = 100_000;
        let (ref regtest_manager, _cph, faucet, ref recipient, _txid) =
            scenarios::faucet_funded_recipient_default(inital_value).await;
        from_inputs::send(
            recipient,
            vec![(&get_base_address_macro!(faucet, "unified"), 10_000, None); 2],
        )
        .await
        .unwrap();
        zingo_testutils::increase_height_and_wait_for_client(regtest_manager, recipient, 1)
            .await
            .unwrap();
        let pre_rescan_transactions = recipient.do_list_transactions().await;
        let pre_rescan_summaries = recipient.list_txsummaries().await;
        recipient.do_rescan().await.unwrap();
        let post_rescan_transactions = recipient.do_list_transactions().await;
        let post_rescan_summaries = recipient.list_txsummaries().await;
        assert_eq!(pre_rescan_transactions, post_rescan_transactions);
        assert_eq!(pre_rescan_summaries, post_rescan_summaries);
        let mut outgoing_metadata = pre_rescan_transactions
            .members()
            .find_map(|tx| tx.entries().find(|(key, _val)| key == &"outgoing_metadata"))
            .unwrap()
            .1
            .members();
        // The two outgoing spends were identical. They should be represented as such
        assert_eq!(outgoing_metadata.next(), outgoing_metadata.next());
    }
    #[tokio::test]
    async fn mempool_clearing_and_full_batch_syncs_correct_trees() {
        async fn do_maybe_recent_txid(lc: &LightClient) -> JsonValue {
            json::object! {
                "last_txid" => lc.wallet.transactions().read().await.get_some_txid_from_highest_wallet_block().map(|t| t.to_string())
            }
        }
        let value = 100_000;
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, faucet, recipient, orig_transaction_id, _, _) =
            scenarios::faucet_funded_recipient(
                Some(value),
                None,
                None,
                PoolType::Shielded(ShieldedProtocol::Sapling),
                regtest_network,
            )
            .await;
        let orig_transaction_id = orig_transaction_id.unwrap();
        assert_eq!(
            do_maybe_recent_txid(&recipient).await["last_txid"],
            orig_transaction_id
        );
        // Put some transactions unrelated to the recipient (faucet->faucet) on-chain, to get some clutter
        for _ in 0..5 {
            zingo_testutils::send_value_between_clients_and_sync(
                &regtest_manager,
                &faucet,
                &faucet,
                5_000,
                "unified",
            )
            .await
            .unwrap();
        }

        let sent_to_self = 10;
        // Send recipient->recipient, to make tree equality check at the end simpler
        zingo_testutils::send_value_between_clients_and_sync(
            &regtest_manager,
            &recipient,
            &recipient,
            sent_to_self,
            "unified",
        )
        .await
        .unwrap();

        // 3a. stash zcashd state
        log::debug!(
            "old zcashd chain info {}",
            std::str::from_utf8(
                &regtest_manager
                    .get_cli_handle()
                    .arg("getblockchaininfo")
                    .output()
                    .unwrap()
                    .stdout
            )
            .unwrap()
        );

        // Turn zcashd off and on again, to write down the blocks
        drop(_cph); // turn off zcashd and lightwalletd
        let _cph = regtest_manager.launch(false).unwrap();
        log::debug!(
            "new zcashd chain info {}",
            std::str::from_utf8(
                &regtest_manager
                    .get_cli_handle()
                    .arg("getblockchaininfo")
                    .output()
                    .unwrap()
                    .stdout
            )
            .unwrap()
        );

        let zcd_datadir = &regtest_manager.zcashd_data_dir;
        let zcashd_parent = Path::new(zcd_datadir).parent().unwrap();
        let original_zcashd_directory = zcashd_parent.join("original_zcashd");

        log::debug!(
            "The original zcashd directory is at: {}",
            &original_zcashd_directory.to_string_lossy().to_string()
        );

        let source = &zcd_datadir.to_string_lossy().to_string();
        let dest = &original_zcashd_directory.to_string_lossy().to_string();
        std::process::Command::new("cp")
            .arg("-rf")
            .arg(source)
            .arg(dest)
            .output()
            .expect("directory copy failed");

        // 3. Send z-to-z transaction to external z address with a memo
        let sent_value = 2000;
        let outgoing_memo = "Outgoing Memo";

        let sent_transaction_id = from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                sent_value,
                Some(outgoing_memo),
            )],
        )
        .await
        .unwrap();

        // Sync recipient
        recipient.do_sync(false).await.unwrap();
        dbg!(
            &recipient
                .wallet
                .transaction_context
                .transaction_metadata_set
                .read()
                .await
                .witness_trees()
                .unwrap()
                .witness_tree_orchard
        );

        // 4b write down state before clearing the mempool
        let notes_before = recipient.do_list_notes(true).await;
        let transactions_before = recipient.do_list_transactions().await;
        println!("Transactions before {}", transactions_before.pretty(2));

        // Sync recipient again. We assert this should be a no-op, as we just synced
        recipient.do_sync(false).await.unwrap();
        let post_sync_notes_before = recipient.do_list_notes(true).await;
        let post_sync_transactions_before = recipient.do_list_transactions().await;
        println!(
            "Transactions before, post_sync {}",
            post_sync_transactions_before.pretty(2)
        );
        assert_eq!(post_sync_notes_before, notes_before);
        assert_eq!(post_sync_transactions_before, transactions_before);

        drop(_cph); // Turn off zcashd and lightwalletd

        // 5. check that the sent transaction is correctly marked in the client
        let transactions = recipient.do_list_transactions().await;
        let mempool_only_tx = transactions
            .members()
            .find(|tx| tx["txid"] == sent_transaction_id)
            .unwrap()
            .clone();
        log::debug!("the transactions are: {}", &mempool_only_tx);
        assert_eq!(
            mempool_only_tx["outgoing_metadata"][0]["memo"],
            "Outgoing Memo"
        );
        assert_eq!(mempool_only_tx["txid"], sent_transaction_id);

        // 6. note that the client correctly considers the note pending
        assert_eq!(mempool_only_tx["pending"], true);

        std::process::Command::new("rm")
            .arg("-rf")
            .arg(source)
            .output()
            .expect("recursive rm failed");
        std::process::Command::new("cp")
            .arg("--recursive")
            .arg("--remove-destination")
            .arg(dest)
            .arg(source)
            .output()
            .expect("directory copy failed");
        assert_eq!(
            source,
            &regtest_manager
                .zcashd_data_dir
                .to_string_lossy()
                .to_string()
        );
        let _cph = regtest_manager.launch(false).unwrap();
        let notes_after = recipient.do_list_notes(true).await;
        let transactions_after = recipient.do_list_transactions().await;

        assert_eq!(notes_before.pretty(2), notes_after.pretty(2));
        assert_eq!(transactions_before.pretty(2), transactions_after.pretty(2));

        // 6. Mine 10 blocks, the pending transaction should still be there.
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 10)
            .await
            .unwrap();
        assert_eq!(recipient.wallet.last_synced_height().await, 21);

        let notes = recipient.do_list_notes(true).await;

        let transactions = recipient.do_list_transactions().await;

        // There are 2 unspent notes, the pending transaction, and the final receipt
        println!("{}", json::stringify_pretty(notes.clone(), 4));
        println!("{}", json::stringify_pretty(transactions.clone(), 4));
        // Two unspent notes: one change, pending, one from faucet, confirmed
        assert_eq!(notes["unspent_orchard_notes"].len(), 2);
        assert_eq!(notes["unspent_sapling_notes"].len(), 0);
        let note = notes["unspent_orchard_notes"][1].clone();
        assert_eq!(note["created_in_txid"], sent_transaction_id);
        assert_eq!(
            note["value"].as_u64().unwrap(),
            value - sent_value - (2 * u64::from(MINIMUM_FEE)) - sent_to_self
        );
        assert!(note["pending"].as_bool().unwrap());
        assert_eq!(transactions.len(), 3);

        // 7. Mine 100 blocks, so the mempool expires
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 100)
            .await
            .unwrap();
        assert_eq!(recipient.wallet.last_synced_height().await, 121);

        let notes = recipient.do_list_notes(true).await;
        let transactions = recipient.do_list_transactions().await;

        // There are now three notes, the original (confirmed and spent) note, the send to self note, and its change.
        assert_eq!(notes["unspent_orchard_notes"].len(), 2);
        assert_eq!(
            notes["spent_orchard_notes"][0]["created_in_txid"],
            orig_transaction_id
        );
        assert!(!notes["unspent_orchard_notes"][0]["pending"]
            .as_bool()
            .unwrap());
        assert_eq!(notes["pending_orchard_notes"].len(), 0);
        assert_eq!(transactions.len(), 2);
        let read_lock = recipient
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await;
        let wallet_trees = read_lock.witness_trees().unwrap();
        let last_leaf = wallet_trees
            .witness_tree_orchard
            .max_leaf_position(0)
            .unwrap();
        let server_trees = zingolib::grpc_connector::get_trees(
            recipient.get_server_uri(),
            recipient.wallet.last_synced_height().await,
        )
        .await
        .unwrap();
        let server_orchard_front = zcash_primitives::merkle_tree::read_commitment_tree::<
            MerkleHashOrchard,
            &[u8],
            { zingolib::wallet::data::COMMITMENT_TREE_LEVELS },
        >(&hex::decode(server_trees.orchard_tree).unwrap()[..])
        .unwrap()
        .to_frontier()
        .take();
        let mut server_orchard_shardtree: ShardTree<_, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL> =
            ShardTree::new(
                MemoryShardStore::<MerkleHashOrchard, BlockHeight>::empty(),
                MAX_REORG,
            );
        server_orchard_shardtree
            .insert_frontier_nodes(
                server_orchard_front.unwrap(),
                zingo_testutils::incrementalmerkletree::Retention::Marked,
            )
            .unwrap();
        assert_eq!(
            wallet_trees
                .witness_tree_orchard
                .witness_at_checkpoint_depth(last_leaf.unwrap(), 0)
                .unwrap_or_else(|_| panic!("{:#?}", wallet_trees.witness_tree_orchard)),
            server_orchard_shardtree
                .witness_at_checkpoint_depth(last_leaf.unwrap(), 0)
                .unwrap()
        )
    }
    #[tokio::test]
    async fn mempool_and_balance() {
        let value = 100_000;
        let (regtest_manager, _cph, faucet, recipient, _txid) =
            scenarios::faucet_funded_recipient_default(value).await;

        let bal = recipient.do_balance().await;
        println!("{}", serde_json::to_string_pretty(&bal).unwrap());
        assert_eq!(bal.orchard_balance.unwrap(), value);
        assert_eq!(bal.unverified_orchard_balance.unwrap(), 0);
        assert_eq!(bal.verified_orchard_balance.unwrap(), value);

        // 3. Mine 10 blocks
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 10)
            .await
            .unwrap();
        let bal = recipient.do_balance().await;
        assert_eq!(bal.orchard_balance.unwrap(), value);
        assert_eq!(bal.verified_orchard_balance.unwrap(), value);
        assert_eq!(bal.unverified_orchard_balance.unwrap(), 0);

        // 4. Spend the funds
        let sent_value = 2000;
        let outgoing_memo = "Outgoing Memo";

        let _sent_transaction_id = from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                sent_value,
                Some(outgoing_memo),
            )],
        )
        .await
        .unwrap();

        let bal = recipient.do_balance().await;

        // Even though the transaction is not mined (in the mempool) the balances should be updated to reflect the spent funds
        let new_bal = value - (sent_value + u64::from(MINIMUM_FEE));
        assert_eq!(bal.orchard_balance.unwrap(), new_bal);
        assert_eq!(bal.verified_orchard_balance.unwrap(), 0);
        assert_eq!(bal.unverified_orchard_balance.unwrap(), new_bal);

        // 5. Mine the pending block, making the funds verified and spendable.
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 10)
            .await
            .unwrap();

        let bal = recipient.do_balance().await;

        assert_eq!(bal.orchard_balance.unwrap(), new_bal);
        assert_eq!(bal.verified_orchard_balance.unwrap(), new_bal);
        assert_eq!(bal.unverified_orchard_balance.unwrap(), 0);
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
        // zingo-testutils/old_wallet_reorg_test_wallet
        let cached_data_dir = get_cargo_manifest_dir()
            .parent()
            .unwrap()
            .join("zingo-testvectors")
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
        let (wallet, conf) =
            zingo_testutils::load_wallet(zingo_dest.into(), ChainType::Regtest(regtest_network))
                .await;
        println!("setting uri");
        *conf.lightwalletd_uri.write().unwrap() = faucet.get_server_uri();
        println!("creating lightclient");
        let recipient = LightClient::create_from_wallet_async(wallet, conf)
            .await
            .unwrap();
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
        from_inputs::send(
            &recipient,
            vec![(&get_base_address_macro!(faucet, "unified"), 14000, None)],
        )
        .await
        .unwrap();
    }
    /// An arbitrary number of diversified addresses may be generated
    /// from a seed.  If the wallet is subsequently lost-or-destroyed
    /// wallet-regeneration-from-seed (sprouting) doesn't regenerate
    /// the previous diversifier list. <-- But the spend capability
    /// is capable of recovering the diversified _receiver_.
    #[tokio::test]
    async fn handling_of_nonregenerated_diversified_addresses_after_seed_restore() {
        let (regtest_manager, _cph, mut client_builder, regtest_network) =
            scenarios::custom_clients_default().await;
        let faucet = client_builder.build_faucet(false, regtest_network).await;
        faucet.do_sync(false).await.unwrap();
        let seed_phrase_of_recipient1 = zcash_primitives::zip339::Mnemonic::from_entropy([1; 32])
            .unwrap()
            .to_string();
        let recipient1 = client_builder
            .build_client(seed_phrase_of_recipient1, 0, false, regtest_network)
            .await;
        let mut expected_unspent_sapling_notes = json::object! {
                "created_in_block" =>  4,
                "datetime" =>  0,
                "created_in_txid" => "",
                "value" =>  14_000,
                "pending" =>  false,
                "is_change" =>  false,
                "address" =>  "uregtest1m8un60udl5ac0928aghy4jx6wp59ty7ct4t8ks9udwn8y6fkdmhe6pq0x5huv8v0pprdlq07tclqgl5fzfvvzjf4fatk8cpyktaudmhvjcqufdsfmktgawvne3ksrhs97pf0u8s8f8h",
                "spendable" =>  true,
                "spent" =>  JsonValue::Null,
                "spent_at_height" =>  JsonValue::Null,
                "pending_spent" =>  JsonValue::Null,
        };
        let original_recipient_address = "\
        uregtest1qtqr46fwkhmdn336uuyvvxyrv0l7trgc0z9clpryx6vtladnpyt4wvq99p59f4rcyuvpmmd0hm4k5vv6j\
        8edj6n8ltk45sdkptlk7rtzlm4uup4laq8ka8vtxzqemj3yhk6hqhuypupzryhv66w65lah9ms03xa8nref7gux2zz\
        hjnfanxnnrnwscmz6szv2ghrurhu3jsqdx25y2yh";
        let seed_of_recipient = {
            assert_eq!(
                &get_base_address_macro!(recipient1, "unified"),
                &original_recipient_address
            );
            let recipient1_diversified_addr = recipient1.do_new_address("tz").await.unwrap();
            from_inputs::send(
                &faucet,
                vec![(
                    recipient1_diversified_addr[0].as_str().unwrap(),
                    14_000,
                    Some("foo"),
                )],
            )
            .await
            .unwrap();
            zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
                .await
                .unwrap();
            recipient1.do_sync(true).await.unwrap();
            let notes = recipient1.do_list_notes(true).await;
            assert_eq!(notes["unspent_sapling_notes"].members().len(), 1);
            let note = notes["unspent_sapling_notes"].members().next().unwrap();
            //The following fields aren't known until runtime, and should be cryptographically nondeterministic
            //Testing that they're generated correctly is beyond the scope if this test
            expected_unspent_sapling_notes["datetime"] = note["datetime"].clone();
            expected_unspent_sapling_notes["created_in_txid"] = note["created_in_txid"].clone();

            assert_eq!(
                note,
                &expected_unspent_sapling_notes,
                "\nExpected:\n{}\n===\nActual:\n{}\n",
                json::stringify_pretty(expected_unspent_sapling_notes.clone(), 4),
                json::stringify_pretty(note.clone(), 4)
            );
            recipient1.do_seed_phrase().await.unwrap()
        };
        drop(recipient1); // Discard original to ensure subsequent data is fresh.
        let mut expected_unspent_sapling_notes_after_restore_from_seed =
            expected_unspent_sapling_notes.clone();
        expected_unspent_sapling_notes_after_restore_from_seed["address"] = JsonValue::String(
        "Diversifier not in wallet. Perhaps you restored from seed and didn't restore addresses"
            .to_string(),
    );
        let recipient_restored = client_builder
            .build_client(
                seed_of_recipient.seed_phrase.clone(),
                0,
                true,
                regtest_network,
            )
            .await;
        let seed_of_recipient_restored = {
            recipient_restored.do_sync(true).await.unwrap();
            let restored_addresses = recipient_restored.do_addresses().await;
            assert_eq!(
                &restored_addresses[0]["address"],
                &original_recipient_address
            );
            let notes = recipient_restored.do_list_notes(true).await;
            assert_eq!(notes["unspent_sapling_notes"].members().len(), 1);
            let note = notes["unspent_sapling_notes"].members().next().unwrap();
            assert_eq!(
                note,
                &expected_unspent_sapling_notes_after_restore_from_seed,
                "\nExpected:\n{}\n===\nActual:\n{}\n",
                json::stringify_pretty(
                    expected_unspent_sapling_notes_after_restore_from_seed.clone(),
                    4
                ),
                json::stringify_pretty(note.clone(), 4)
            );

            //The first address in a wallet should always contain all three currently extant
            //receiver types.
            from_inputs::send(
                &recipient_restored,
                vec![(&get_base_address_macro!(faucet, "sapling"), 4_000, None)],
            )
            .await
            .unwrap();
            let sender_balance = faucet.do_balance().await;
            zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
                .await
                .unwrap();

            //Ensure that recipient_restored was still able to spend the note, despite not having the
            //diversified address associated with it
            assert_eq!(
                faucet.do_balance().await.spendable_sapling_balance.unwrap(),
                sender_balance.spendable_sapling_balance.unwrap() + 4_000
            );
            recipient_restored.do_seed_phrase().await.unwrap()
        };
        assert_eq!(seed_of_recipient, seed_of_recipient_restored);
    }
    #[tokio::test]
    async fn list_txsummaries_check_fees() {
        // Check that list_txsummaries behaves correctly given different fee scenarios
        let (regtest_manager, _cph, mut client_builder, regtest_network) =
            scenarios::custom_clients_default().await;
        let sapling_faucet = client_builder.build_faucet(false, regtest_network).await;
        let pool_migration_client = client_builder
            .build_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false, regtest_network)
            .await;
        let pmc_taddr = get_base_address_macro!(pool_migration_client, "transparent");
        let pmc_sapling = get_base_address_macro!(pool_migration_client, "sapling");
        let pmc_unified = get_base_address_macro!(pool_migration_client, "unified");
        // Ensure that the client has confirmed spendable funds
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &sapling_faucet, 3)
            .await
            .unwrap();
        macro_rules! bump_and_check_pmc {
            (o: $o:tt s: $s:tt t: $t:tt) => {
                zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &pool_migration_client, 1).await.unwrap();
                check_client_balances!(pool_migration_client, o:$o s:$s t:$t);
            };
        }

        // 1 pmc receives 100_000 orchard
        //  # Expected Fees:
        //    - legacy: 0
        //    - 317:    0
        from_inputs::send(&sapling_faucet, vec![(&pmc_unified, 100_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 100_000 s: 0 t: 0);

        // 2 to transparent and sapling from orchard
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    5_000 for transparent + 10_000 for orchard + 10_000 for sapling == 25_000
        from_inputs::send(
            &pool_migration_client,
            vec![(&pmc_taddr, 30_000, None), (&pmc_sapling, 30_000, None)],
        )
        .await
        .unwrap();
        bump_and_check_pmc!(o: 30_000 s: 30_000 t: 30_000);
    }
    #[tokio::test]
    async fn from_t_z_o_tz_to_zo_tzo_to_orchard() {
        // Test all possible promoting note source combinations
        // This test includes combinations that are disallowed in the mobile
        // app and are not recommended in production.
        // An example is a transaction that "shields" both transparent and
        // sapling value into the orchard value pool.
        let (regtest_manager, _cph, mut client_builder, regtest_network) =
            scenarios::custom_clients_default().await;
        let sapling_faucet = client_builder.build_faucet(false, regtest_network).await;
        let pool_migration_client = client_builder
            .build_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false, regtest_network)
            .await;
        let pmc_taddr = get_base_address_macro!(pool_migration_client, "transparent");
        let pmc_sapling = get_base_address_macro!(pool_migration_client, "sapling");
        let pmc_unified = get_base_address_macro!(pool_migration_client, "unified");
        // Ensure that the client has confirmed spendable funds
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &sapling_faucet, 3)
            .await
            .unwrap();
        macro_rules! bump_and_check_pmc {
        (o: $o:tt s: $s:tt t: $t:tt) => {
            zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &pool_migration_client, 1).await.unwrap();
            check_client_balances!(pool_migration_client, o:$o s:$s t:$t);
        };
    }

        // 1 pmc receives 50_000 transparent
        //  # Expected Fees:
        //    - legacy: 0
        //    - 317:    0
        from_inputs::send(&sapling_faucet, vec![(&pmc_taddr, 50_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 0 s: 0 t: 50_000);

        // 2 pmc shields 50_000 transparent, to orchard paying 10_000 fee
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000
        from_inputs::shield(&pool_migration_client, &[PoolType::Transparent], None)
            .await
            .unwrap();
        bump_and_check_pmc!(o: 40_000 s: 0 t: 0);

        // 3 pmc receives 50_000 sapling
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000
        from_inputs::send(&sapling_faucet, vec![(&pmc_sapling, 50_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 40_000 s: 50_000 t: 0);

        // 4 pmc shields 40_000 from sapling to orchard and pays 10_000 fee (should be 20_000 post zip317)
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000
        from_inputs::shield(
            &pool_migration_client,
            &[PoolType::Shielded(ShieldedProtocol::Sapling)],
            None,
        )
        .await
        .unwrap();
        bump_and_check_pmc!(o: 80_000 s: 0 t: 0);

        // 5 Self send of 70_000 paying 10_000 fee
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000
        from_inputs::send(&pool_migration_client, vec![(&pmc_unified, 70_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 70_000 s: 0 t: 0);

        // 4 to transparent and sapling from orchard
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    5_000 for transparent + 10_000 for orchard + 10_000 for sapling == 25_000
        from_inputs::send(
            &pool_migration_client,
            vec![(&pmc_taddr, 30_000, None), (&pmc_sapling, 30_000, None)],
        )
        .await
        .unwrap();
        bump_and_check_pmc!(o: 0 s: 30_000 t: 30_000);

        // 5 Shield transparent and sapling to orchard
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    5_000 for transparent + 10_000 for orchard + 10_000 for sapling == 25_000
        from_inputs::shield(&pool_migration_client, &[PoolType::Transparent], None)
            .await
            .unwrap();
        bump_and_check_pmc!(o: 50_000 s: 0 t: 0);

        // 6 self send orchard to orchard
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000
        from_inputs::send(&pool_migration_client, vec![(&pmc_unified, 20_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 40_000 s: 0 t: 0);

        // 7 Orchard to transparent self-send
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    (orchard = 10_000 + 5_000) 15_000
        from_inputs::send(&pool_migration_client, vec![(&pmc_taddr, 20_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 10_000 s: 0 t: 20_000);

        // 8 Shield transparent to orchard
        // 7 Orchard to transparent self-send
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    disallowed
        from_inputs::shield(&pool_migration_client, &[PoolType::Transparent], None)
            .await
            .unwrap();
        bump_and_check_pmc!(o: 20_000 s: 0 t: 0);

        // 6 sapling and orchard to orchard
        from_inputs::send(&sapling_faucet, vec![(&pmc_sapling, 20_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 20_000 s: 20_000 t: 0);

        from_inputs::send(&pool_migration_client, vec![(&pmc_unified, 30_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 30_000 s: 0 t: 0);

        // 7 tzo --> o
        from_inputs::send(
            &sapling_faucet,
            vec![(&pmc_taddr, 20_000, None), (&pmc_sapling, 20_000, None)],
        )
        .await
        .unwrap();
        bump_and_check_pmc!(o: 30_000 s: 20_000 t: 20_000);

        from_inputs::shield(&pool_migration_client, &[PoolType::Transparent], None)
            .await
            .unwrap();
        from_inputs::send(&pool_migration_client, vec![(&pmc_unified, 40_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 50_000 s: 0 t: 0);

        // Send from Sapling into empty Orchard pool
        from_inputs::send(&pool_migration_client, vec![(&pmc_sapling, 40_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 0 s: 40_000 t: 0);

        from_inputs::send(&pool_migration_client, vec![(&pmc_unified, 30_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 30_000 s: 0 t: 0);
        let mut total_value_to_addrs_iter = pool_migration_client
            .do_total_value_to_address()
            .await
            .0
            .into_iter();
        assert_eq!(
            total_value_to_addrs_iter.next(),
            Some((String::from("fee"), u64::from((MINIMUM_FEE * 13).unwrap())))
        );
        assert!(total_value_to_addrs_iter.next().is_none());
    }
    #[tokio::test]
    async fn factor_do_shield_to_call_do_send() {
        let (regtest_manager, __cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 2)
            .await
            .unwrap();
        from_inputs::send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "transparent"),
                1_000u64,
                None,
            )],
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn dust_sends_change_correctly() {
        let (regtest_manager, _cph, faucet, recipient, _txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        // Send of less that transaction fee
        let sent_value = 1000;
        let _sent_transaction_id = from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                sent_value,
                None,
            )],
        )
        .await
        .unwrap();

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 5)
            .await
            .unwrap();

        println!("{}", recipient.do_list_transactions().await.pretty(4));
        println!(
            "{}",
            serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
        );
    }
    #[tokio::test]
    async fn dont_write_pending() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, faucet, recipient) = scenarios::faucet_recipient(
            PoolType::Shielded(ShieldedProtocol::Orchard),
            regtest_network,
        )
        .await;
        from_inputs::send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                100_000,
                Some("funding to be received by the recipient"),
            )],
        )
        .await
        .unwrap();

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 2)
            .await
            .unwrap();
        let recipient_balance = recipient.do_balance().await;
        assert_eq!(
            recipient_balance,
            PoolBalances {
                sapling_balance: Some(0),
                verified_sapling_balance: Some(0),
                spendable_sapling_balance: Some(0),
                unverified_sapling_balance: Some(0),
                orchard_balance: Some(100000),
                verified_orchard_balance: Some(100000),
                spendable_orchard_balance: Some(100000),
                unverified_orchard_balance: Some(0),
                transparent_balance: Some(0)
            }
        );
        from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                25_000,
                Some("an pending transaction, that shall not be synced"),
            )],
        )
        .await
        .unwrap();
        let recipient_balance = recipient.do_balance().await;

        dbg!(&recipient_balance.unverified_orchard_balance);
        assert_eq!(
            recipient_balance.unverified_orchard_balance.unwrap(),
            65_000
        );

        let loaded_client = zingo_testutils::lightclient::new_client_from_save_buffer(&recipient)
            .await
            .unwrap();
        let loaded_balance = loaded_client.do_balance().await;
        assert_eq!(loaded_balance.unverified_orchard_balance, Some(0),);
        check_client_balances!(loaded_client, o: 100_000 s: 0 t: 0 );
    }
    #[tokio::test]
    async fn by_address_finsight() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        let base_uaddress = get_base_address_macro!(recipient, "unified");
        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 2)
            .await
            .unwrap();
        println!(
            "faucet notes: {}",
            faucet.do_list_notes(true).await.pretty(4)
        );
        from_inputs::send(&faucet, vec![(&base_uaddress, 1_000u64, Some("1"))])
            .await
            .unwrap();
        from_inputs::send(&faucet, vec![(&base_uaddress, 1_000u64, Some("1"))])
            .await
            .expect(
                "We only have sapling notes, plus a pending orchard note from the \
            previous send. If we're allowed to select pending notes, we'll attempt \
            to select that one, and this will fail",
            );
        assert_eq!(
            JsonValue::from(faucet.do_total_memobytes_to_address().await)[&base_uaddress].pretty(4),
            "2".to_string()
        );
        from_inputs::send(&faucet, vec![(&base_uaddress, 1_000u64, Some("aaaa"))])
            .await
            .unwrap();
        assert_eq!(
            JsonValue::from(faucet.do_total_memobytes_to_address().await)[&base_uaddress].pretty(4),
            "6".to_string()
        );
    }
    #[tokio::test]
    async fn aborted_resync() {
        let (regtest_manager, _cph, faucet, recipient, _txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 15)
            .await
            .unwrap();

        // 4. Send a transaction to both external t-addr and external z addr and mine it
        let sent_zvalue = 80_000;
        let sent_zmemo = "Ext z";
        let sent_transaction_id = from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                sent_zvalue,
                Some(sent_zmemo),
            )],
        )
        .await
        .unwrap();

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 5)
            .await
            .unwrap();

        let notes_before = recipient.do_list_notes(true).await;
        let list_before = recipient.do_list_transactions().await;
        let requested_txid = &zingolib::wallet::utils::txid_from_slice(
            hex::decode(sent_transaction_id.clone())
                .unwrap()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .as_slice(),
        );
        let witness_before = recipient
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .witness_trees()
            .unwrap()
            .witness_tree_orchard
            .witness_at_checkpoint_depth(
                recipient
                    .wallet
                    .transaction_context
                    .transaction_metadata_set
                    .read()
                    .await
                    .transaction_records_by_id
                    .get(requested_txid)
                    .unwrap()
                    .orchard_notes
                    .first()
                    .unwrap()
                    .witnessed_position
                    .unwrap(),
                0,
            );

        // 5. Now, we'll manually remove some of the blocks in the wallet, pretending that the sync was aborted in the middle.
        // We'll remove the top 20 blocks, so now the wallet only has the first 3 blocks
        recipient.wallet.blocks.write().await.drain(0..20);
        assert_eq!(recipient.wallet.last_synced_height().await, 5);

        // 6. Do a sync again
        recipient.do_sync(true).await.unwrap();
        assert_eq!(recipient.wallet.last_synced_height().await, 25);

        // 7. Should be exactly the same
        let notes_after = recipient.do_list_notes(true).await;
        let list_after = recipient.do_list_transactions().await;
        let witness_after = recipient
            .wallet
            .transaction_context
            .transaction_metadata_set
            .read()
            .await
            .witness_trees()
            .unwrap()
            .witness_tree_orchard
            .witness_at_checkpoint_depth(
                recipient
                    .wallet
                    .transaction_context
                    .transaction_metadata_set
                    .read()
                    .await
                    .transaction_records_by_id
                    .get(requested_txid)
                    .unwrap()
                    .orchard_notes
                    .first()
                    .unwrap()
                    .witnessed_position
                    .unwrap(),
                0,
            );

        assert_eq!(notes_before, notes_after);
        assert_eq!(list_before, list_after);
        assert_eq!(witness_before.unwrap(), witness_after.unwrap());
    }
    #[tokio::test]
    async fn mempool_spends_correctly_marked_pending_spent() {
        let (_regtest_manager, _cph, _faucet, recipient, _txid) =
            scenarios::faucet_funded_recipient_default(1_000_000).await;
        from_inputs::send(
            &recipient,
            vec![(
                &get_base_address_macro!(recipient, "sapling"),
                100_000,
                None,
            )],
        )
        .await
        .unwrap();
        let recipient_saved = recipient.export_save_buffer_async().await.unwrap();
        let recipient_loaded = std::sync::Arc::new(
            LightClient::read_wallet_from_buffer_async(recipient.config(), &recipient_saved[..])
                .await
                .unwrap(),
        );
        LightClient::start_mempool_monitor(recipient_loaded.clone());
        // This seems to be long enough for the mempool monitor to kick in.
        // One second is insufficient. Even if this fails, this can only ever be
        // a false negative, giving us a balance of 100_000. Still, could be improved.
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(
            recipient_loaded.do_balance().await.orchard_balance,
            Some(890_000)
        );
    }
    #[ignore]
    #[tokio::test]
    async fn timed_sync_interrupt() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        for i in 1..4 {
            let _ = faucet.do_sync(false).await;
            from_inputs::send(
                &faucet,
                vec![(&get_base_address_macro!(recipient, "sapling"), 10_100, None)],
            )
            .await
            .unwrap();
            let chainwait: u32 = 6;
            let amount: u64 = u64::from(chainwait * i);
            zingo_testutils::increase_server_height(&regtest_manager, chainwait).await;
            let _ = recipient.do_sync(false).await;
            from_inputs::send(
                &recipient,
                vec![(&get_base_address_macro!(recipient, "unified"), amount, None)],
            )
            .await
            .unwrap();
        }
        zingo_testutils::increase_server_height(&regtest_manager, 1).await;

        let _synciiyur = recipient.do_sync(false).await;
        // let summ_sim = recipient.do_list_txsummaries().await;
        let bala_sim = recipient.do_balance().await;

        recipient.clear_state().await;
        dbg!("finished basic sync. restarting for interrupted data");
        let timeout = 28;
        let race_condition =
            zingo_testutils::interrupts::sync_with_timeout_millis(&recipient, timeout).await;
        match race_condition {
            Ok(_) => {
                println!("synced in less than {} millis ", timeout);
                dbg!("syncedd");
            }
            Err(_) => {
                println!("interrupted after {} millis ", timeout);
                dbg!("interruptedidd!");
            }
        }

        // let summ_int = recipient.do_list_txsummaries().await;
        // let bala_int = recipient.do_balance().await;
        let _synciiyur = recipient.do_sync(false).await;
        // let summ_syn = recipient.do_list_txsummaries().await;
        let bala_syn = recipient.do_balance().await;

        dbg!(
            &recipient
                .wallet
                .transaction_context
                .transaction_metadata_set
                .read()
                .await
                .transaction_records_by_id
        );

        assert_eq!(bala_sim, bala_syn);
    }
    async fn load_wallet_from_data_and_assert(
        data: &[u8],
        expected_balance: u64,
        num_addresses: usize,
    ) {
        let config = zingoconfig::ZingoConfig::build(ChainType::Testnet)
            .set_lightwalletd_uri(
                ("https://zcash.mysideoftheweb.com:19067")
                    .parse::<http::Uri>()
                    .unwrap(),
            )
            .create();
        let wallet = LightWallet::read_internal(data, &config)
            .await
            .map_err(|e| format!("Cannot deserialize LightWallet file!: {}", e))
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

        let client = LightClient::create_from_wallet_async(wallet, config)
            .await
            .unwrap();
        let balance = client.do_balance().await;
        assert_eq!(balance.orchard_balance, Some(expected_balance));
        if expected_balance > 0 {
            let _ = from_inputs::send(
                &client,
                vec![(&get_base_address_macro!(client, "sapling"), 11011, None)],
            )
            .await
            .unwrap();
            let _ = client.do_sync(true).await.unwrap();
            let _ = from_inputs::send(
                &client,
                vec![(&get_base_address_macro!(client, "transparent"), 28000, None)],
            )
            .await
            .unwrap();
        }
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

mod basic_transactions {
    use zcash_client_backend::PoolType;
    use zingo_testutils::{get_base_address_macro, lightclient::from_inputs, scenarios};

    #[tokio::test]
    async fn send_and_sync_with_multiple_notes_no_panic() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;

        let recipient_addr_ua = get_base_address_macro!(recipient, "unified");
        let faucet_addr_ua = get_base_address_macro!(faucet, "unified");

        zingo_testutils::generate_n_blocks_return_new_height(&regtest_manager, 2)
            .await
            .unwrap();

        recipient.do_sync(true).await.unwrap();
        faucet.do_sync(true).await.unwrap();

        for _ in 0..2 {
            from_inputs::send(&faucet, vec![(recipient_addr_ua.as_str(), 40_000, None)])
                .await
                .unwrap();
        }

        zingo_testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
            .await
            .unwrap();

        recipient.do_sync(true).await.unwrap();
        faucet.do_sync(true).await.unwrap();

        from_inputs::send(&recipient, vec![(faucet_addr_ua.as_str(), 50_000, None)])
            .await
            .unwrap();

        zingo_testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
            .await
            .unwrap();

        recipient.do_sync(true).await.unwrap();
        faucet.do_sync(true).await.unwrap();
    }

    #[tokio::test]
    async fn standard_send_fees() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;

        let txid1 = from_inputs::send(
            &faucet,
            vec![(
                get_base_address_macro!(recipient, "unified").as_str(),
                40_000,
                None,
            )],
        )
        .await
        .unwrap();

        let txid2 = from_inputs::send(
            &faucet,
            vec![(
                get_base_address_macro!(recipient, "sapling").as_str(),
                40_000,
                None,
            )],
        )
        .await
        .unwrap();

        let txid3 = from_inputs::send(
            &faucet,
            vec![(
                get_base_address_macro!(recipient, "transparent").as_str(),
                40_000,
                None,
            )],
        )
        .await
        .unwrap();

        zingo_testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
            .await
            .unwrap();

        faucet.do_sync(true).await.unwrap();
        recipient.do_sync(true).await.unwrap();

        println!(
            "Transaction Inputs:\n{:?}",
            zingo_testutils::tx_inputs(&faucet, txid1.as_str()).await
        );
        println!(
            "Transaction Outputs:\n{:?}",
            zingo_testutils::tx_outputs(&recipient, txid1.as_str()).await
        );
        println!(
            "Transaction Change:\n{:?}",
            zingo_testutils::tx_outputs(&faucet, txid1.as_str()).await
        );

        let tx_actions_txid1 =
            zingo_testutils::tx_actions(&faucet, Some(&recipient), txid1.as_str()).await;
        println!("Transaction Actions:\n{:?}", tx_actions_txid1);

        let calculated_fee_txid1 =
            zingo_testutils::total_tx_value(&faucet, txid1.as_str()).await - 40_000;
        println!("Fee Paid: {}", calculated_fee_txid1);

        let expected_fee_txid1 = 10000;
        // currently expected fee is always 10000 but will change to the following in zip317
        // let expected_fee_txid1 = 5000
        //     * (cmp::max(
        //         2,
        //         tx_actions_txid1.transparent_tx_actions
        //             + tx_actions_txid1.sapling_tx_actions
        //             + tx_actions_txid1.orchard_tx_actions,
        //     ));
        println!("Expected Fee: {}", expected_fee_txid1);

        assert_eq!(calculated_fee_txid1, expected_fee_txid1 as u64);

        println!(
            "Transaction Inputs:\n{:?}",
            zingo_testutils::tx_inputs(&faucet, txid2.as_str()).await
        );
        println!(
            "Transaction Outputs:\n{:?}",
            zingo_testutils::tx_outputs(&recipient, txid2.as_str()).await
        );
        println!(
            "Transaction Change:\n{:?}",
            zingo_testutils::tx_outputs(&faucet, txid2.as_str()).await
        );

        let tx_actions_txid2 =
            zingo_testutils::tx_actions(&faucet, Some(&recipient), txid2.as_str()).await;
        println!("Transaction Actions:\n{:?}", tx_actions_txid2);

        let calculated_fee_txid2 =
            zingo_testutils::total_tx_value(&faucet, txid2.as_str()).await - 40_000;
        println!("Fee Paid: {}", calculated_fee_txid2);

        let expected_fee_txid2 = 10000;
        // currently expected fee is always 10000 but will change to the following in zip317
        // let expected_fee_txid2 = 5000
        //     * (cmp::max(
        //         2,
        //         tx_actions_txid2.transparent_tx_actions
        //             + tx_actions_txid2.sapling_tx_actions
        //             + tx_actions_txid2.orchard_tx_actions,
        //     ));
        println!("Expected Fee: {}", expected_fee_txid2);

        assert_eq!(calculated_fee_txid2, expected_fee_txid2 as u64);

        println!(
            "Transaction Inputs:\n{:?}",
            zingo_testutils::tx_inputs(&faucet, txid3.as_str()).await
        );
        println!(
            "Transaction Outputs:\n{:?}",
            zingo_testutils::tx_outputs(&recipient, txid3.as_str()).await
        );
        println!(
            "Transaction Change:\n{:?}",
            zingo_testutils::tx_outputs(&faucet, txid3.as_str()).await
        );

        let tx_actions_txid3 =
            zingo_testutils::tx_actions(&faucet, Some(&recipient), txid3.as_str()).await;
        println!("Transaction Actions:\n{:?}", tx_actions_txid3);

        let calculated_fee_txid3 =
            zingo_testutils::total_tx_value(&faucet, txid3.as_str()).await - 40_000;
        println!("Fee Paid: {}", calculated_fee_txid3);

        let expected_fee_txid3 = 10000;
        // currently expected fee is always 10000 but will change to the following in zip317
        // let expected_fee_txid3 = 5000
        //     * (cmp::max(
        //         2,
        //         tx_actions_txid3.transparent_tx_actions
        //             + tx_actions_txid3.sapling_tx_actions
        //             + tx_actions_txid3.orchard_tx_actions,
        //     ));
        println!("Expected Fee: {}", expected_fee_txid3);

        assert_eq!(calculated_fee_txid3, expected_fee_txid3 as u64);

        let txid4 = zingo_testutils::lightclient::from_inputs::send(
            &recipient,
            vec![(
                get_base_address_macro!(faucet, "transparent").as_str(),
                60_000,
                None,
            )],
        )
        .await
        .unwrap();

        zingo_testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
            .await
            .unwrap();

        faucet.do_sync(true).await.unwrap();
        recipient.do_sync(true).await.unwrap();

        println!(
            "Transaction Inputs:\n{:?}",
            zingo_testutils::tx_inputs(&recipient, txid4.as_str()).await
        );
        println!(
            "Transaction Outputs:\n{:?}",
            zingo_testutils::tx_outputs(&faucet, txid4.as_str()).await
        );
        println!(
            "Transaction Change:\n{:?}",
            zingo_testutils::tx_outputs(&recipient, txid4.as_str()).await
        );

        let tx_actions_txid4 =
            zingo_testutils::tx_actions(&recipient, Some(&faucet), txid4.as_str()).await;
        println!("Transaction Actions:\n{:?}", tx_actions_txid4);

        let calculated_fee_txid4 =
            zingo_testutils::total_tx_value(&recipient, txid4.as_str()).await - 60_000;
        println!("Fee Paid: {}", calculated_fee_txid4);

        let expected_fee_txid4 = 10000;
        // currently expected fee is always 10000 but will change to the following in zip317
        // let expected_fee_txid4 = 5000
        //     * (cmp::max(
        //         2,
        //         tx_actions_txid4.transparent_tx_actions
        //             + tx_actions_txid4.sapling_tx_actions
        //             + tx_actions_txid4.orchard_tx_actions,
        //     ));
        println!("Expected Fee: {}", expected_fee_txid4);

        assert_eq!(calculated_fee_txid4, expected_fee_txid4 as u64);
    }

    #[tokio::test]
    async fn dust_send_fees() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;

        let txid1 = zingo_testutils::lightclient::from_inputs::send(
            &faucet,
            vec![(
                get_base_address_macro!(recipient, "unified").as_str(),
                0,
                None,
            )],
        )
        .await
        .unwrap();

        zingo_testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
            .await
            .unwrap();

        faucet.do_sync(true).await.unwrap();
        recipient.do_sync(true).await.unwrap();

        println!(
            "Transaction Inputs:\n{:?}",
            zingo_testutils::tx_inputs(&faucet, txid1.as_str()).await
        );
        println!(
            "Transaction Outputs:\n{:?}",
            zingo_testutils::tx_outputs(&recipient, txid1.as_str()).await
        );
        println!(
            "Transaction Change:\n{:?}",
            zingo_testutils::tx_outputs(&faucet, txid1.as_str()).await
        );

        let tx_actions_txid1 =
            zingo_testutils::tx_actions(&faucet, Some(&recipient), txid1.as_str()).await;
        println!("Transaction Actions:\n{:?}", tx_actions_txid1);

        let calculated_fee_txid1 = zingo_testutils::total_tx_value(&faucet, txid1.as_str()).await;
        println!("Fee Paid: {}", calculated_fee_txid1);

        let expected_fee_txid1 = 10000;
        // currently expected fee is always 10000 but will change to the following in zip317
        // let expected_fee_txid1 = 5000
        //     * (cmp::max(
        //         2,
        //         tx_actions_txid1.transparent_tx_actions
        //             + tx_actions_txid1.sapling_tx_actions
        //             + tx_actions_txid1.orchard_tx_actions,
        //     ));
        println!("Expected Fee: {}", expected_fee_txid1);

        assert_eq!(calculated_fee_txid1, expected_fee_txid1 as u64);
    }

    #[tokio::test]
    async fn shield_send_fees() {
        let (regtest_manager, _cph, faucet, recipient) =
            scenarios::faucet_recipient_default().await;

        zingo_testutils::lightclient::from_inputs::send(
            &faucet,
            vec![(
                get_base_address_macro!(recipient, "transparent").as_str(),
                40_000,
                None,
            )],
        )
        .await
        .unwrap();

        zingo_testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
            .await
            .unwrap();

        faucet.do_sync(true).await.unwrap();
        recipient.do_sync(true).await.unwrap();

        let txid1 = from_inputs::shield(
            &recipient,
            &[PoolType::Transparent],
            Some(&get_base_address_macro!(recipient, "unified")),
        )
        .await
        .unwrap();

        zingo_testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
            .await
            .unwrap();

        faucet.do_sync(true).await.unwrap();
        recipient.do_sync(true).await.unwrap();

        println!(
            "Transaction Inputs:\n{:?}",
            zingo_testutils::tx_inputs(&recipient, txid1.as_str()).await
        );
        println!(
            "Transaction Outputs:\n{:?}",
            zingo_testutils::tx_outputs(&recipient, txid1.as_str()).await
        );

        let tx_actions_txid1 = zingo_testutils::tx_actions(&recipient, None, txid1.as_str()).await;
        println!("Transaction Actions:\n{:?}", tx_actions_txid1);

        let calculated_fee_txid1 =
            zingo_testutils::total_tx_value(&recipient, txid1.as_str()).await;
        println!("Fee Paid: {}", calculated_fee_txid1);

        let expected_fee_txid1 = 10000;
        // currently expected fee is always 10000 but will change to the following in zip317
        // let expected_fee_txid1 = 5000
        //     * (cmp::max(
        //         2,
        //         tx_actions_txid1.transparent_tx_actions
        //             + tx_actions_txid1.sapling_tx_actions
        //             + tx_actions_txid1.orchard_tx_actions,
        //     ));
        println!("Expected Fee: {}", expected_fee_txid1);

        assert_eq!(calculated_fee_txid1, expected_fee_txid1 as u64);

        zingo_testutils::lightclient::from_inputs::send(
            &faucet,
            vec![(
                get_base_address_macro!(recipient, "transparent").as_str(),
                40_000,
                None,
            )],
        )
        .await
        .unwrap();

        zingo_testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
            .await
            .unwrap();

        faucet.do_sync(true).await.unwrap();
        recipient.do_sync(true).await.unwrap();

        let txid2 = from_inputs::shield(
            &recipient,
            &[PoolType::Transparent],
            Some(&get_base_address_macro!(recipient, "sapling")),
        )
        .await
        .unwrap();

        zingo_testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
            .await
            .unwrap();

        faucet.do_sync(true).await.unwrap();
        recipient.do_sync(true).await.unwrap();

        println!(
            "Transaction Inputs:\n{:?}",
            zingo_testutils::tx_inputs(&recipient, txid2.as_str()).await
        );
        println!(
            "Transaction Outputs:\n{:?}",
            zingo_testutils::tx_outputs(&recipient, txid2.as_str()).await
        );

        let tx_actions_txid2 = zingo_testutils::tx_actions(&recipient, None, txid2.as_str()).await;
        println!("Transaction Actions:\n{:?}", tx_actions_txid2);

        let calculated_fee_txid2 =
            zingo_testutils::total_tx_value(&recipient, txid2.as_str()).await;
        println!("Fee Paid: {}", calculated_fee_txid2);

        let expected_fee_txid2 = 10000;
        // currently expected fee is always 10000 but will change to the following in zip317
        // let expected_fee_txid2 = 5000
        //     * (cmp::max(
        //         2,
        //         tx_actions_txid2.transparent_tx_actions
        //             + tx_actions_txid2.sapling_tx_actions
        //             + tx_actions_txid2.orchard_tx_actions,
        //     ));
        println!("Expected Fee: {}", expected_fee_txid2);

        assert_eq!(calculated_fee_txid2, expected_fee_txid2 as u64);
    }
}

#[tokio::test]
async fn proxy_server_worky() {
    zingo_testutils::check_proxy_server_works().await
}
