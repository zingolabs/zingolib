#![forbid(unsafe_code)]
use json::JsonValue;

use zcash_address::unified::Fvk;
use zcash_primitives::transaction::fees::zip317::MINIMUM_FEE;

use pepper_sync::wallet::{IronwoodNote, TransparentCoin};
use zcash_protocol::PoolType;
use zcash_protocol::consensus::COINBASE_MATURITY_BLOCKS;
use zcash_protocol::value::Zatoshis;
use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingolib::wallet::balance::AccountBalance;
use zingolib::wallet::keys::unified::UnifiedKeyStore;
use zingolib::wallet::summary::data::{CoinSummary, NoteSummary};
use zingolib::{check_client_balances, get_base_address_macro};
use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};

fn check_expected_balance_with_fvks(
    fvks: &Vec<&Fvk>,
    balance: AccountBalance,
    o_expect: u64,
    s_expect: u64,
    t_expect: u64,
) {
    for fvk in fvks {
        match fvk {
            Fvk::Sapling(_) => {
                assert_eq!(balance.total_sapling_balance.unwrap().into_u64(), s_expect);
                assert_eq!(
                    balance.confirmed_sapling_balance.unwrap().into_u64(),
                    s_expect
                );
                assert_eq!(
                    balance.unconfirmed_sapling_balance.unwrap().into_u64(),
                    s_expect
                );
            }
            Fvk::Orchard(_) => {
                assert_eq!(balance.total_orchard_balance.unwrap().into_u64(), o_expect);
                assert_eq!(
                    balance.confirmed_orchard_balance.unwrap().into_u64(),
                    o_expect
                );
                assert_eq!(
                    balance.unconfirmed_orchard_balance.unwrap().into_u64(),
                    o_expect
                );
            }
            Fvk::P2pkh(_) => {
                assert_eq!(
                    balance.confirmed_transparent_balance.unwrap().into_u64(),
                    t_expect
                );
            }
            _ => panic!(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_view_capability_bounds(
    balance: &AccountBalance,
    unified_key_store: &UnifiedKeyStore,
    fvks: &[&Fvk],
    orchard_fvk: &Fvk,
    sapling_fvk: &Fvk,
    transparent_fvk: &Fvk,
    sent_i_value: Option<Zatoshis>,
    sent_o_value: Option<Zatoshis>,
    sent_s_value: Option<Zatoshis>,
    sent_t_value: Option<Zatoshis>,
    ironwood_notes: &[NoteSummary],
    orchard_notes: &[NoteSummary],
    sapling_notes: &[NoteSummary],
    transparent_coins: &[CoinSummary],
) {
    let UnifiedKeyStore::View(ufvk) = unified_key_store else {
        panic!("should be viewing key!")
    };
    //Orchard
    if fvks.contains(&orchard_fvk) {
        assert!(ufvk.orchard().is_some());
        assert_eq!(balance.total_orchard_balance, sent_o_value);
        assert_eq!(balance.confirmed_orchard_balance, sent_o_value);
        assert_eq!(balance.total_ironwood_balance, sent_i_value);
        assert_eq!(balance.confirmed_ironwood_balance, sent_i_value);
        assert_eq!(balance.unconfirmed_ironwood_balance, Some(Zatoshis::ZERO));
        // assert 1 Orchard note, or 2 notes if a dummy output is included
        let ironwood_notes_count = ironwood_notes
            .iter()
            .filter(|note| note.spend_status.is_unspent())
            .count();
        assert!((1..=2).contains(&ironwood_notes_count));
    } else {
        assert!(ufvk.orchard().is_none());
        assert_eq!(balance.total_orchard_balance, None);
        assert_eq!(balance.confirmed_orchard_balance, None);
        assert_eq!(balance.unconfirmed_orchard_balance, None);
        assert_eq!(orchard_notes.len(), 0);
        assert_eq!(balance.total_ironwood_balance, None);
        assert_eq!(balance.confirmed_ironwood_balance, None);
        assert_eq!(balance.unconfirmed_ironwood_balance, None);
        assert_eq!(ironwood_notes.len(), 0);
    }
    //Sapling
    if fvks.contains(&sapling_fvk) {
        assert!(ufvk.sapling().is_some());
        assert_eq!(balance.total_sapling_balance, sent_s_value);
        assert_eq!(balance.confirmed_sapling_balance, sent_s_value);
        assert_eq!(balance.unconfirmed_sapling_balance, Some(Zatoshis::ZERO));
        assert_eq!(
            sapling_notes
                .iter()
                .filter(|note| note.spend_status.is_unspent())
                .count(),
            1
        );
    } else {
        assert!(ufvk.sapling().is_none());
        assert_eq!(balance.total_sapling_balance, None);
        assert_eq!(balance.confirmed_sapling_balance, None);
        assert_eq!(balance.unconfirmed_sapling_balance, None);
        assert_eq!(sapling_notes.len(), 0);
    }
    if fvks.contains(&transparent_fvk) {
        assert!(ufvk.transparent().is_some());
        assert_eq!(balance.confirmed_transparent_balance, sent_t_value);
        assert_eq!(transparent_coins.len(), 1);
    } else {
        assert!(ufvk.transparent().is_none());
        assert_eq!(balance.confirmed_transparent_balance, None);
        assert_eq!(transparent_coins.len(), 0);
    }
}

use libtonode_tests::chain_generics::LibtonodeEnvironment;
use pepper_sync::wallet::OutputInterface;
use zcash_client_backend::encoding::encode_payment_address_p;
use zcash_protocol::consensus::BlockHeight;
use zingo_status::confirmation_status::ConfirmationStatus;
use zingolib::config::WalletConfig;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::default_test_wallet_settings;
use zingolib::testutils::lightclient::from_inputs;
use zingolib::wallet::keys::unified::{ReceiverSelection, UnifiedAddressId};
use zip32::AccountId;

#[tokio::test]
async fn unified_address_discovery() {
    let (local_net, mut client_builder) = scenarios::custom_clients_default().await;
    let mut faucet = client_builder.build_faucet(true).await;
    let mut recipient = client_builder
        .build_client(
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
            true,
        )
        .await;
    let network = recipient.chain_type();

    // create a range of UAs to be discovered when recipient is reset
    let orchard_only_addr = recipient
        .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
        .await
        .map(|(_, ua)| ua.encode(&network))
        .unwrap();
    let sapling_only_addr = recipient
        .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
        .await
        .map(|(_, ua)| ua.encode(&network))
        .unwrap();
    let (_, all_shielded_addr) = recipient
        .generate_unified_address(ReceiverSelection::all_shielded(), zip32::AccountId::ZERO)
        .await
        .unwrap();
    let all_shielded_encoded = all_shielded_addr.encode(&network);
    let all_shielded_sapling_addr = all_shielded_addr
        .sapling()
        .map(|addr| encode_payment_address_p(&network, addr))
        .unwrap();

    // send to the UAs so they are recorded on chain
    increase_height_and_wait_for_client(&local_net, &mut faucet, 3)
        .await
        .unwrap();
    scenarios::send_and_bump(
        &local_net,
        &mut faucet,
        vec![
            (&orchard_only_addr, 100_000, Some("orchard_only")),
            (&sapling_only_addr, 200_000, Some("sapling_only")),
            (&all_shielded_encoded, 300_000, Some("all_shielded")),
            (
                &all_shielded_sapling_addr,
                400_000,
                Some("all_shielded_sapling"),
            ),
        ],
    )
    .await;

    // rebuild recipient and check the UAs don't exist in the wallet
    let mut recipient = client_builder
        .build_client(
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
            true,
        )
        .await;
    if let Some(_ua) = recipient
        .wallet()
        .read()
        .await
        .unified_addresses()
        .get(&UnifiedAddressId {
            account_id: zip32::AccountId::ZERO,
            address_index: 2,
        })
    {
        panic!("ua should not be in fresh wallet yet!");
    }
    if let Some(_ua) = recipient
        .wallet()
        .read()
        .await
        .unified_addresses()
        .get(&UnifiedAddressId {
            account_id: zip32::AccountId::ZERO,
            address_index: 3,
        })
    {
        panic!("ua should not be in fresh wallet yet!");
    }
    if let Some(_ua) = recipient
        .wallet()
        .read()
        .await
        .unified_addresses()
        .get(&UnifiedAddressId {
            account_id: zip32::AccountId::ZERO,
            address_index: 4,
        })
    {
        panic!("ua should not be in fresh wallet yet!");
    }

    // sync recipient (to the Validator's tip, since a bare sync races the
    // Indexer's ingestion of the block confirming the sends) and check
    // the UAs have been discovered
    scenarios::sync_client_to_validator_tip(&local_net, &mut recipient).await;
    assert_eq!(
        recipient
            .wallet()
            .read()
            .await
            .unified_addresses()
            .get(&UnifiedAddressId {
                account_id: zip32::AccountId::ZERO,
                address_index: 2,
            })
            .unwrap()
            .encode(&network),
        orchard_only_addr
    );
    assert_eq!(
        recipient
            .wallet()
            .read()
            .await
            .unified_addresses()
            .get(&UnifiedAddressId {
                account_id: zip32::AccountId::ZERO,
                address_index: 3,
            })
            .unwrap()
            .encode(&network),
        sapling_only_addr
    );
    assert_eq!(
        recipient
            .wallet()
            .read()
            .await
            .unified_addresses()
            .get(&UnifiedAddressId {
                account_id: zip32::AccountId::ZERO,
                address_index: 4,
            })
            .unwrap()
            .encode(&network),
        all_shielded_encoded
    );
}

/// Diagnostic probe for the Core-stack coinbase model. Each assert tests
/// one hypothesis, and each failure mode has a distinct quantized delta:
/// - orchard off by one POST_STREAM_BLOCK_REWARD (618_750_000):
///   ORCHARD_COINBASE_START_HEIGHT is wrong (flip 2 <-> 3)
/// - sapling delta of BLOCK_ONE_SAPLING_COINBASE (625_000_000): the
///   block-1-pays-the-sapling-receiver rule is wrong
/// - transparent nonzero: pre-NU5 or activation-block coinbase pays a
///   transparent output instead
/// - balances short by whole blocks: the deterministic
///   sync_client_to_validator_tip is not actually deterministic.
#[tokio::test]
async fn ironwood_miner_coinbase_distribution() {
    let mut environment = LibtonodeEnvironment::setup().await;
    let mut faucet = environment.create_faucet().await;
    environment.increase_chain_height().await;
    scenarios::sync_client_to_validator_tip(&environment.local_net, &mut faucet).await;

    // Tip is height 4: launch block + 2 setup blocks + 1 above. Every
    // coinbase block (2..=4) predates the fixture's NU6.3 activation at 5,
    // so the orchard-receiver rewards are legacy Orchard notes and the
    // Ironwood pool is empty.
    check_client_balances!(
        faucet,
        i: 0 o: (scenarios::orchard_coinbase_total(4)) s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0u64
    );
}

#[tokio::test]
async fn received_tx_status_pending_to_confirmed_with_mempool_monitor() {
    tracing_subscriber::fmt().init();

    let (local_net, mut faucet, mut recipient, _txid) =
        scenarios::faucet_funded_recipient_default(100_000).await;

    from_inputs::quick_send(
        &mut faucet,
        vec![(
            &get_base_address_macro!(&recipient, "unified"),
            // &get_base_address_macro!(&recipient, "sapling"),
            20_000,
            None,
        )],
    )
    .await
    .unwrap();

    recipient.sync_and_await().await.unwrap();

    let transactions = &recipient.transaction_summaries(false).await.unwrap().0;
    for tx in transactions {
        dbg!(tx);
    }
    // Setup ends at FUNDED_FAUCET_SETUP_HEIGHT; faucet_funded_recipient
    // mines one pre-send block and one funding-confirm block, so the
    // 20_000 send targets the block after that.
    let mempool_target_height = BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 3);
    assert_eq!(
        transactions
            .iter()
            .find(|tx| tx.value == 20_000)
            .unwrap()
            .status,
        ConfirmationStatus::Mempool(mempool_target_height)
    );

    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    let transactions = &recipient.transaction_summaries(false).await.unwrap().0;
    assert_eq!(
        transactions
            .iter()
            .find(|tx| tx.value == 20_000)
            .unwrap()
            .status,
        ConfirmationStatus::Confirmed(mempool_target_height)
    );
}

#[tokio::test]
async fn utxos_are_not_prematurely_confirmed() {
    let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
    from_inputs::quick_send(
        &mut faucet,
        vec![(
            &get_base_address_macro!(recipient, "transparent"),
            100_000,
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    let wallet = recipient.wallet().read().await;
    let preshield_utxos = wallet
        .wallet_outputs::<TransparentCoin>()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(preshield_utxos.len(), 1);
    assert!(
        wallet
            .output_spend_status(preshield_utxos.first().unwrap())
            .is_unspent()
    );
    drop(wallet);

    recipient
        .quick_shield(zip32::AccountId::ZERO)
        .await
        .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();

    let wallet = recipient.wallet().read().await;
    let postshield_utxos = wallet.wallet_outputs::<TransparentCoin>();
    assert_eq!(postshield_utxos.len(), 1);
    assert!(
        wallet
            .output_spend_status(*postshield_utxos.first().unwrap())
            .is_confirmed_spent()
    );
    assert_eq!(
        preshield_utxos.first().unwrap().output_id(),
        postshield_utxos.first().unwrap().output_id(),
    );
}

#[tokio::test]
async fn mine_to_ironwood() {
    let (local_net, mut faucet) = scenarios::faucet(
        PoolType::IRONWOOD,
        scenarios::default_test_activation_heights(),
        scenarios::ChainCachePolicy::PerTest,
    )
    .await;
    check_client_balances!(
        faucet,
        i: (scenarios::funded_faucet_ironwood_balance()) o: 0 s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0
    );
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();
    check_client_balances!(
        faucet,
        i: (scenarios::funded_faucet_ironwood_balance() + scenarios::POST_STREAM_BLOCK_REWARD) o: 0 s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0
    );
}

#[tokio::test]
async fn mine_to_orchard() {
    let fixture = scenarios::wallet_activation_heights(
        &zcash_local_net::validator::regtest_test_activation_heights(),
    );
    let activation_heights = zingolib::ActivationHeights::builder()
        .set_overwinter(fixture.overwinter())
        .set_sapling(fixture.sapling())
        .set_blossom(fixture.blossom())
        .set_heartwood(fixture.heartwood())
        .set_canopy(fixture.canopy())
        .set_nu5(fixture.nu5())
        .set_nu6(fixture.nu6())
        .set_nu6_1(fixture.nu6_1())
        .set_nu6_2(fixture.nu6_2())
        .set_nu6_3(None)
        .set_nu7(None)
        .build();
    let (local_net, mut faucet) = scenarios::faucet(
        PoolType::ORCHARD,
        activation_heights,
        scenarios::ChainCachePolicy::PerTest,
    )
    .await;
    check_client_balances!(
        faucet,
        i: 0 o: 1_237_500_000 s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0
    );
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();
    check_client_balances!(
        faucet,
        i: 0 o: (1_237_500_000 + scenarios::POST_STREAM_BLOCK_REWARD) s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0
    );
}

/// Tests that the miner's address receives (immature) rewards from mining to the transparent pool.
#[tokio::test]
async fn mine_to_transparent() {
    let (local_net, mut faucet, _recipient) = scenarios::faucet_recipient(
        PoolType::Transparent,
        scenarios::default_test_activation_heights(),
        scenarios::ChainCachePolicy::PerTest,
    )
    .await;

    let unconfirmed_balance = faucet
        .wallet()
        .read()
        .await
        .get_filtered_balance_mut::<TransparentCoin, _>(|_, _| true, AccountId::ZERO)
        .unwrap();

    assert_eq!(
        unconfirmed_balance,
        Zatoshis::const_from_u64(scenarios::mined_block_rewards_total(3))
    );

    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();

    assert_eq!(
        faucet
            .wallet()
            .read()
            .await
            .get_filtered_balance_mut::<TransparentCoin, _>(|_, _| true, AccountId::ZERO)
            .unwrap(),
        Zatoshis::const_from_u64(scenarios::mined_block_rewards_total(4))
    );
}

#[tokio::test]
async fn sync_all_expressible_epochs() {
    // The zebrad config writer requires every upgrade through Canopy
    // active at height 1, and the harness subsidy fixtures pair only
    // with the fixture shape, so the expressible epoch boundaries are
    // NU5/NU6 at height 2 and NU6.1/NU6.2 at height 5. Sync across all
    // of them with room to spare.
    let (local_net, mut lightclient) = scenarios::unfunded_client(
        scenarios::default_test_activation_heights(),
        scenarios::ChainCachePolicy::PerTest,
    )
    .await;
    increase_height_and_wait_for_client(&local_net, &mut lightclient, 12)
        .await
        .unwrap();
}
use pepper_sync::wallet::{OrchardNote, SaplingNote};
use zcash_local_net::validator::Validator;
use zingolib::config::{ChainType, ClientConfig};
use zingolib::lightclient::LightClient;
use zingolib::lightclient::error::{LightClientError, SendError};
use zingolib::perspective::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransferKind,
};
use zingolib::testutils::build_fvks_from_unified_keystore;
use zingolib::wallet::error::CalculateTransactionError;
use zingolib::wallet::output::SpendStatus;

#[tokio::test]
async fn test_scanning_in_watch_only_mode() {
    // # Scenario:
    // 3. reset wallet
    // 4. for every combination of FVKs
    //     4.1. init a wallet with UFVK
    //     4.2. check that the wallet is empty
    //     4.3. rescan
    //     4.4. check that notes and utxos were detected by the wallet

    let (local_net, mut client_builder) = scenarios::custom_clients_default().await;
    let mut faucet = client_builder.build_faucet(false).await;
    let mut original_recipient = client_builder
        .build_client(
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
            false,
        )
        .await;

    let (recipient_taddr, recipient_sapling, recipient_unified) = (
        get_base_address_macro!(original_recipient, "transparent"),
        get_base_address_macro!(original_recipient, "sapling"),
        get_base_address_macro!(original_recipient, "unified"),
    );
    let addr_amount_memos = vec![
        (recipient_taddr.as_str(), 10_000u64, None),
        (recipient_sapling.as_str(), 20_000u64, None),
        (recipient_unified.as_str(), 30_000u64, None),
    ];
    // 1. fill wallet with a coinbase transaction by syncing faucet with 1-block increase
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();
    // 2. send a transaction containing all types of outputs
    from_inputs::quick_send(&mut faucet, addr_amount_memos)
        .await
        .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut original_recipient, 1)
        .await
        .unwrap();
    let original_recipient_balance = original_recipient
        .account_balance(zip32::AccountId::ZERO)
        .await
        .unwrap();
    let sent_t_value = original_recipient_balance
        .confirmed_transparent_balance
        .unwrap()
        .into_u64();
    let sent_s_value = original_recipient_balance
        .total_sapling_balance
        .unwrap()
        .into_u64();
    let sent_o_value = original_recipient_balance
        .total_orchard_balance
        .unwrap()
        .into_u64();
    let sent_i_value = original_recipient_balance
        .total_ironwood_balance
        .unwrap()
        .into_u64();
    assert_eq!(sent_t_value, 10_000u64);
    assert_eq!(sent_s_value, 20_000u64);
    // The unified-address payment lands in the Ironwood pool (ADR 0009).
    assert_eq!(sent_o_value, 0u64);
    assert_eq!(sent_i_value, 30_000u64);

    // check that do_rescan works
    original_recipient.rescan_and_await().await.unwrap();
    check_client_balances!(original_recipient, i: sent_i_value o: sent_o_value s: sent_s_value t: sent_t_value);

    // Extract viewing keys
    let original_wallet = original_recipient.wallet().read().await;
    let [o_fvk, s_fvk, t_fvk] = build_fvks_from_unified_keystore(
        original_wallet
            .unified_key_store
            .get(&zip32::AccountId::ZERO)
            .unwrap(),
    );
    let fvks_sets = [
        vec![&o_fvk],
        vec![&s_fvk],
        vec![&o_fvk, &s_fvk],
        vec![&o_fvk, &t_fvk],
        vec![&s_fvk, &t_fvk],
        vec![&o_fvk, &s_fvk, &t_fvk],
    ];
    for fvks in &fvks_sets {
        tracing::info!("testing UFVK containing:");
        tracing::info!("    orchard fvk: {}", fvks.contains(&&o_fvk));
        tracing::info!("    sapling fvk: {}", fvks.contains(&&s_fvk));
        tracing::info!("    transparent fvk: {}", fvks.contains(&&t_fvk));

        let ufvk = zcash_address::unified::Encoding::encode(
            &<zcash_address::unified::Ufvk as zcash_address::unified::Encoding>::try_from_items(
                fvks.iter().copied().cloned().collect(),
            )
            .unwrap(),
            &zcash_protocol::consensus::NetworkType::Regtest,
        );
        let zingo_config = ClientConfig::builder()
            .set_indexer_uri(client_builder.server_id.clone())
            .set_chain_type(ChainType::Regtest(
                zingolib_testutils::scenarios::wallet_activation_heights(
                    &local_net.validator().get_activation_heights().await,
                ),
            ))
            .set_wallet_dir(client_builder.zingo_datadir.path().to_path_buf())
            .set_wallet_config(WalletConfig::Ufvk {
                ufvk,
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            })
            .build()
            .unwrap();
        let mut watch_client = LightClient::new_clearnet_consented(zingo_config, false)
            .await
            .unwrap();
        // assert empty wallet before rescan
        let balance = watch_client
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();
        check_expected_balance_with_fvks(fvks, balance, 0, 0, 0);
        watch_client.rescan_and_await().await.unwrap();
        let balance = watch_client
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();
        {
            let watch_wallet = watch_client.wallet().read().await;
            let ironwood_notes = watch_wallet.note_summaries::<IronwoodNote>(true);
            let orchard_notes = watch_wallet.note_summaries::<OrchardNote>(true);
            let sapling_notes = watch_wallet.note_summaries::<SaplingNote>(true);
            let transparent_coin = watch_wallet.coin_summaries(true);

            check_view_capability_bounds(
                &balance,
                watch_wallet
                    .unified_key_store
                    .get(&zip32::AccountId::ZERO)
                    .unwrap(),
                fvks,
                &o_fvk,
                &s_fvk,
                &t_fvk,
                Some(sent_i_value.try_into().unwrap()),
                Some(sent_o_value.try_into().unwrap()),
                Some(sent_s_value.try_into().unwrap()),
                Some(sent_t_value.try_into().unwrap()),
                &ironwood_notes,
                &orchard_notes,
                &sapling_notes,
                &transparent_coin,
            );
        }

        watch_client.rescan_and_await().await.unwrap();
        assert!(matches!(
            from_inputs::quick_send(
                &mut watch_client,
                vec![(zingo_test_vectors::EXT_TADDR, 1000, None)]
            )
            .await,
            Err(LightClientError::SendError(SendError::CalculateSendError(
                CalculateTransactionError::NoSpendingKey(_)
            )))
        ));
    }
}
#[tokio::test]
async fn sends_to_self_handle_balance_properly() {
    let transparent_funding = 100_000;
    let (ref local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
    from_inputs::quick_send(
        &mut faucet,
        vec![(
            &get_base_address_macro!(recipient, "transparent"),
            transparent_funding,
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(local_net, &mut recipient, 1)
        .await
        .unwrap();
    recipient
        .quick_shield(zip32::AccountId::ZERO)
        .await
        .unwrap();
    increase_height_and_wait_for_client(local_net, &mut recipient, 1)
        .await
        .unwrap();

    // The shield sweeps the whole transparent funding into the Ironwood
    // pool (a V6 shield's change lands in the ironwood bundle, ADR 0009),
    // less the 15_000 ZIP-317 fee (one transparent input plus the padded
    // two-action ironwood pair).
    let shielded_value = transparent_funding - 15_000;
    check_client_balances!(recipient, i: shielded_value o: 0 s: 0 t: 0);

    let pre_rescan_summaries = recipient.transaction_summaries(false).await.unwrap();
    let pre_rescan_value_transfers = recipient.value_transfers(true).await.unwrap();
    // Exactly two transfers: the transparent funding receipt and the
    // shield, the latter represented as a self-send of the Shield
    // kind carrying the swept value into the orchard pool.
    assert_eq!(pre_rescan_value_transfers.iter().count(), 2);
    assert!(pre_rescan_value_transfers.iter().any(|vt| {
        vt.kind == ValueTransferKind::Received
            && vt.value == transparent_funding
            && vt.pools_received == [PoolType::TRANSPARENT]
            && vt.pools_sent_from.is_empty()
    }));
    assert_eq!(
        pre_rescan_value_transfers
            .iter()
            .filter(|vt| {
                vt.kind
                    == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                        SelfSendValueTransfer::Shield,
                    ))
                    && vt.value == shielded_value
                    && vt.transaction_fee == Some(15_000)
                    && vt.pools_sent_from == [PoolType::TRANSPARENT]
                    && vt.pools_received == [PoolType::IRONWOOD]
            })
            .count(),
        1
    );

    // Rescanning from scratch must reproduce the same balance and
    // identical records.
    recipient.rescan_and_await().await.unwrap();
    check_client_balances!(recipient, i: shielded_value o: 0 s: 0 t: 0);
    assert_eq!(
        pre_rescan_summaries,
        recipient.transaction_summaries(false).await.unwrap()
    );
    assert_eq!(
        pre_rescan_value_transfers,
        recipient.value_transfers(true).await.unwrap()
    );
}
#[tokio::test]
async fn send_to_ua_saves_full_ua_in_wallet() {
    let (local_net, mut faucet, recipient) = scenarios::faucet_recipient_default().await;
    //utils::increase_height_and_wait_for_client(&local_net, &faucet, 5).await;
    let recipient_unified_address = get_base_address_macro!(recipient, "unified");
    let sent_value = 50_000;
    from_inputs::quick_send(
        &mut faucet,
        vec![(recipient_unified_address.as_str(), sent_value, None)],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();
    let transactions = faucet.transaction_summaries(false).await.unwrap().0;
    assert!(transactions.iter().any(|transaction| {
        transaction
            .outgoing_ironwood_notes
            .iter()
            .chain(transaction.outgoing_sapling_notes.iter())
            .any(|note| note.recipient_unified_address == Some(recipient_unified_address.clone()))
    }));
    faucet.rescan_and_await().await.unwrap();
    let rescanned_transactions = faucet.transaction_summaries(false).await.unwrap().0;
    assert!(rescanned_transactions.iter().any(|transaction| {
        transaction
            .outgoing_ironwood_notes
            .iter()
            .chain(transaction.outgoing_sapling_notes.iter())
            .any(|note| note.recipient_unified_address == Some(recipient_unified_address.clone()))
    }));
    assert_eq!(
        transactions,
        rescanned_transactions,
        "Pre-Rescan: {}\n\n\nPost-Rescan: {}\n\n\n",
        json::stringify_pretty(transactions.clone(), 4),
        json::stringify_pretty(rescanned_transactions.clone(), 4)
    );
}

#[tokio::test]
async fn send_orchard_back_and_forth() {
    // setup
    let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
    let faucet_to_recipient_amount = 20_000u64;
    let recipient_to_faucet_amount = 10_000u64;
    // check start state
    faucet.sync_and_await().await.unwrap();
    let wallet_fully_scanned_height = faucet
        .wallet()
        .read()
        .await
        .sync_state
        .fully_scanned_height()
        .unwrap();
    assert_eq!(
        wallet_fully_scanned_height,
        scenarios::FUNDED_FAUCET_SETUP_HEIGHT.into()
    );
    let setup_reward = scenarios::funded_faucet_ironwood_balance();
    check_client_balances!(faucet, i:setup_reward o: 0 s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0);

    // post transfer to recipient, and verify
    from_inputs::quick_send(
        &mut faucet,
        vec![(
            &get_base_address_macro!(recipient, "unified"),
            faucet_to_recipient_amount,
            Some("Orcharding"),
        )],
    )
    .await
    .unwrap();
    // The mined block credits the faucet (the miner) with a fresh
    // post-stream coinbase reward plus the send's fee; the send itself
    // debits the amount and fee. Aggregate balance is deterministic even
    // though note selection is not.
    let ironwood_change =
        scenarios::POST_STREAM_BLOCK_REWARD - (faucet_to_recipient_amount + u64::from(MINIMUM_FEE));
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    faucet.sync_and_await().await.unwrap();
    let faucet_ironwood = setup_reward + ironwood_change + u64::from(MINIMUM_FEE);

    tracing::info!(
        "{}",
        JsonValue::from(faucet.value_transfers(true).await.unwrap()).pretty(4)
    );
    tracing::info!(
        "{}",
        &faucet
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap()
    );

    check_client_balances!(faucet, i:faucet_ironwood o: 0 s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0);
    check_client_balances!(recipient, i: faucet_to_recipient_amount o: 0 s: 0 t: 0);

    // post half back to faucet, and verify
    from_inputs::quick_send(
        &mut recipient,
        vec![(
            &get_base_address_macro!(faucet, "unified"),
            recipient_to_faucet_amount,
            Some("Sending back"),
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();
    recipient.sync_and_await().await.unwrap();

    let faucet_final_ironwood = faucet_ironwood
        + recipient_to_faucet_amount
        + scenarios::POST_STREAM_BLOCK_REWARD
        + u64::from(MINIMUM_FEE);
    let recipient_final_ironwood =
        faucet_to_recipient_amount - (u64::from(MINIMUM_FEE) + recipient_to_faucet_amount);
    check_client_balances!(
        faucet,
        i: faucet_final_ironwood o: 0 s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0
    );
    check_client_balances!(recipient, i: recipient_final_ironwood o: 0 s: 0 t: 0);
}

#[tokio::test]
async fn send_mined_ironwood_to_ironwood() {
    // This test shows a confirmation changing the state of balance by
    // debiting unverified_orchard_balance and crediting verified_orchard_balance.  The debit amount is
    // consistent with all the notes in the relevant block changing state.
    // NOTE that the balance doesn't give insight into the distribution across notes.
    let (local_net, mut faucet) = scenarios::faucet(
        PoolType::IRONWOOD,
        scenarios::default_test_activation_heights(),
        scenarios::ChainCachePolicy::PerTest,
    )
    .await;

    let amount_to_send = 10_000;
    let faucet_ua = get_base_address_macro!(faucet, "unified");
    from_inputs::quick_send(
        &mut faucet,
        vec![(&faucet_ua, amount_to_send, Some("Scenario test: engage!"))],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();
    let balance = faucet
        .account_balance(zip32::AccountId::ZERO)
        .await
        .unwrap();
    assert_eq!(
        balance.unconfirmed_ironwood_balance,
        Some(0.try_into().unwrap())
    );
    // The send is to self, so only the fee leaves the wallet, and the
    // faucet mines the confirming block, collecting a fresh coinbase
    // reward plus that same fee back.
    assert_eq!(
        balance.confirmed_ironwood_balance.unwrap().into_u64(),
        scenarios::funded_faucet_ironwood_balance() + scenarios::POST_STREAM_BLOCK_REWARD
    );
}

/// This mod collects tests of `outgoing_metadata` (a `TransactionRecordField`) across rescans
mod rescan_still_have_outgoing_notes {
    use super::*;

    #[tokio::test]
    async fn self_send() {
        let (local_net, mut faucet) = scenarios::faucet_default().await;
        let faucet_sapling_addr = get_base_address_macro!(faucet, "sapling");
        let mut txids = vec![];
        for memo in [None, Some("Second Transaction")] {
            txids.push(
                *from_inputs::quick_send(
                    &mut faucet,
                    vec![(faucet_sapling_addr.as_str(), 100_000, memo)],
                )
                .await
                .unwrap()
                .first(),
            );
            increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
                .await
                .unwrap();
        }

        let pre_rescan_summaries = faucet.transaction_summaries(false).await.unwrap();
        faucet.rescan_and_await().await.unwrap();
        let post_rescan_summaries = faucet.transaction_summaries(false).await.unwrap();
        assert_eq!(pre_rescan_summaries, post_rescan_summaries);
    }
    /// Rescan survival of the spender's records, asserted on BOTH
    /// `value_transfers` and `transaction_summaries`, across the
    /// full {sapling, orchard} × {memo, memoless} outgoing-output
    /// matrix in one transaction. Absorbs the former
    /// `external_send` (sapling memo/memoless outgoing metadata
    /// across rescan) per the protection-dominance analysis, and
    /// adds the previously untested orchard-with-memo cell.
    #[tokio::test]
    async fn check_list_value_transfers_across_rescan() {
        let inital_value = 100_000;
        let (ref local_net, faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(inital_value).await;
        let faucet_sapling = get_base_address_macro!(faucet, "sapling");
        let faucet_unified = get_base_address_macro!(faucet, "unified");
        from_inputs::quick_send(
            &mut recipient,
            vec![
                (faucet_sapling.as_str(), 10_000, Some("sapling with memo")),
                (faucet_sapling.as_str(), 10_000, None),
                (faucet_unified.as_str(), 10_000, Some("orchard with memo")),
                (faucet_unified.as_str(), 10_000, None),
            ],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(local_net, &mut recipient, 1)
            .await
            .unwrap();
        let pre_rescan_transactions = recipient.transaction_summaries(false).await.unwrap();
        let pre_rescan_summaries = recipient.value_transfers(true).await.unwrap();
        recipient.rescan_and_await().await.unwrap();
        let post_rescan_transactions = recipient.transaction_summaries(false).await.unwrap();
        let post_rescan_summaries = recipient.value_transfers(true).await.unwrap();
        assert_eq!(pre_rescan_transactions, post_rescan_transactions);
        assert_eq!(pre_rescan_summaries, post_rescan_summaries);
    }
}
/// The final send must gather more than one of the recipient's three
/// sapling notes (10_000, 20_000, 30_000) to cover 30_000 plus the fee,
/// making this the suite's only live broadcast of a multi-input sapling
/// spend with cross-pool (orchard) change. The unwraps assert proposal,
/// proving, and the validator's acceptance of the bundle. The closing
/// balance check pins the spend's post-state (gap 1a of the audit's
/// remediation plan). Note-selection ordering itself is asserted
/// offline by `note_selection_covers_target_with_minimal_change` in
/// `zingolib::lightclient::propose`.
#[tokio::test]
async fn multi_input_sapling_send_with_orchard_change_no_panic() {
    let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
    increase_height_and_wait_for_client(&local_net, &mut faucet, 5)
        .await
        .unwrap();

    let client_2_saplingaddress = get_base_address_macro!(recipient, "sapling");
    // Send three transfers in increasing 10_000 zat increments
    // These are sent from the coinbase funded client which will
    // subsequently receive funding via it's orchard-packed UA.
    let memos = ["1", "2", "3"];
    from_inputs::quick_send(
        &mut faucet,
        (1..=3)
            .map(|n| {
                (
                    client_2_saplingaddress.as_str(),
                    n * 10_000,
                    Some(memos[(n - 1) as usize]),
                )
            })
            .collect(),
    )
    .await
    .unwrap();

    increase_height_and_wait_for_client(&local_net, &mut recipient, 5)
        .await
        .unwrap();
    // We know that the largest single note that 2 received from 1 was 30_000, for 2 to send
    // 30_000 back to 1 it will have to collect funds from two notes to pay the full 30_000
    // plus the transaction fee.
    from_inputs::quick_send(
        &mut recipient,
        vec![(
            &get_base_address_macro!(faucet, "unified"),
            30_000,
            Some("Sending back, should have 2 inputs"),
        )],
    )
    .await
    .unwrap();

    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    // Post-state, observed and pinned: the 30_000 payment plus its
    // 20_000 ZIP-317 fee (two sapling spends, two orchard actions) is
    // covered exactly by the 20_000 and 30_000 notes, so the untouched
    // 10_000 sapling note is the whole remaining balance and the
    // orchard change is zero-value.
    check_client_balances!(recipient, i: 0 o: 0 s: 10_000 t: 0);
}

/// Assert the recipient's three-way orchard balance split.
async fn assert_ironwood_split(
    client: &zingolib::lightclient::LightClient,
    total: u64,
    confirmed: u64,
    unconfirmed: u64,
) {
    let balance = client
        .account_balance(zip32::AccountId::ZERO)
        .await
        .unwrap();
    assert_eq!(balance.total_ironwood_balance.unwrap().into_u64(), total);
    assert_eq!(
        balance.confirmed_ironwood_balance.unwrap().into_u64(),
        confirmed
    );
    assert_eq!(
        balance.unconfirmed_ironwood_balance.unwrap().into_u64(),
        unconfirmed
    );
}

/// Assert the recipient holds no sapling notes and exactly the
/// expected ironwood notes, each with its expected `SpendStatus` and
/// (where given) its transaction's confirmation state.
async fn assert_ironwood_note_statuses(
    client: &zingolib::lightclient::LightClient,
    expected: &[(u64, SpendStatus, Option<bool>)],
) {
    let wallet = client.wallet().read().await;
    assert_eq!(wallet.wallet_outputs::<SaplingNote>().len(), 0);
    let ironwood_notes = wallet.wallet_outputs::<IronwoodNote>();
    assert_eq!(ironwood_notes.len(), expected.len());
    for (value, status, confirmed) in expected {
        let note = (*ironwood_notes
            .iter()
            .find(|&&note| note.value() == *value)
            .unwrap_or_else(|| panic!("no ironwood note of value {value}")))
        .clone();
        assert_eq!(wallet.output_spend_status(&note), *status, "note {value}");
        if let Some(want_confirmed) = confirmed {
            assert_eq!(
                wallet.output_transaction(&note).status().is_confirmed(),
                *want_confirmed,
                "note {value} transaction confirmation"
            );
        }
    }
}

/// Coalesced from the former `mempool_and_balance` and
/// `mempool_spends_correctly_marked_pending_spent`
/// (protection-dominance analysis): one funded recipient walks two
/// complete mempool-then-confirmed spend cycles, asserted at BOTH
/// granularities (the three-way orchard balance split and per-note
/// `SpendStatus`) at every phase, including the pre-spend steady
/// state and a post-confirmation stability window neither original
/// covered at note level. Both former send amounts survive the
/// merge deliberately: the 2_000-zat cycle preserves near-dust
/// change arithmetic. The 100_000 cycle preserves the note-status
/// shape of the original per-note test.
#[tokio::test]
async fn mempool_spend_balance_and_note_status_accounting() {
    let funded = 1_000_000;
    let (local_net, faucet, mut recipient, _txid) =
        scenarios::faucet_funded_recipient_default(funded).await;

    // Steady state, and its stability across an empty ten-block mine.
    assert_ironwood_split(&recipient, funded, funded, 0).await;
    increase_height_and_wait_for_client(&local_net, &mut recipient, 10)
        .await
        .unwrap();
    assert_ironwood_split(&recipient, funded, funded, 0).await;

    // Cycle one: a small send whose change sits near the dust line.
    let small = 2_000;
    let small_txids = from_inputs::quick_send(
        &mut recipient,
        vec![(
            &get_base_address_macro!(faucet, "unified"),
            small,
            Some("Outgoing Memo"),
        )],
    )
    .await
    .unwrap();
    recipient.sync_and_await().await.unwrap();
    let after_small = funded - (small + u64::from(MINIMUM_FEE));
    assert_ironwood_split(&recipient, after_small, 0, after_small).await;
    assert_ironwood_note_statuses(
        &recipient,
        &[
            (
                funded,
                SpendStatus::MempoolSpent(*small_txids.first()),
                None,
            ),
            (after_small, SpendStatus::Unspent, Some(false)),
        ],
    )
    .await;

    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    assert_ironwood_split(&recipient, after_small, after_small, 0).await;
    assert_ironwood_note_statuses(
        &recipient,
        &[
            (funded, SpendStatus::Spent(*small_txids.first()), None),
            (after_small, SpendStatus::Unspent, Some(true)),
        ],
    )
    .await;

    // Cycle two: a larger cross-pool send, spending the change note.
    let big = 100_000;
    let big_txids = from_inputs::quick_send(
        &mut recipient,
        vec![(&get_base_address_macro!(faucet, "sapling"), big, None)],
    )
    .await
    .unwrap();
    recipient.sync_and_await().await.unwrap();
    // One orchard spend, one sapling output, orchard change: the
    // ZIP-317 fee the former test pinned implicitly via its
    // 880_000 post-state.
    let big_fee = 2 * u64::from(MINIMUM_FEE);
    let after_big = after_small - (big + big_fee);
    assert_ironwood_split(&recipient, after_big, 0, after_big).await;
    assert_ironwood_note_statuses(
        &recipient,
        &[
            (funded, SpendStatus::Spent(*small_txids.first()), None),
            (
                after_small,
                SpendStatus::MempoolSpent(*big_txids.first()),
                None,
            ),
            (after_big, SpendStatus::Unspent, Some(false)),
        ],
    )
    .await;

    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    assert_ironwood_split(&recipient, after_big, after_big, 0).await;
    let settled = [
        (funded, SpendStatus::Spent(*small_txids.first()), None),
        (after_small, SpendStatus::Spent(*big_txids.first()), None),
        (after_big, SpendStatus::Unspent, Some(true)),
    ];
    assert_ironwood_note_statuses(&recipient, &settled).await;

    // Stability window: nine further blocks change nothing at
    // either granularity.
    increase_height_and_wait_for_client(&local_net, &mut recipient, 9)
        .await
        .unwrap();
    assert_ironwood_split(&recipient, after_big, after_big, 0).await;
    assert_ironwood_note_statuses(&recipient, &settled).await;
}

#[tokio::test]
async fn send_pre_ironwood() {
    let fixture = scenarios::wallet_activation_heights(
        &zcash_local_net::validator::regtest_test_activation_heights(),
    );
    let activation_heights = zingolib::ActivationHeights::builder()
        .set_overwinter(fixture.overwinter())
        .set_sapling(fixture.sapling())
        .set_blossom(fixture.blossom())
        .set_heartwood(fixture.heartwood())
        .set_canopy(fixture.canopy())
        .set_nu5(fixture.nu5())
        .set_nu6(fixture.nu6())
        .set_nu6_1(fixture.nu6_1())
        .set_nu6_2(fixture.nu6_2())
        .set_nu6_3(None)
        .set_nu7(None)
        .build();
    let (_local_net, _faucet, recipient, _, _, _) = scenarios::faucet_funded_recipient(
        Some(100_000),
        None,
        None,
        PoolType::ORCHARD,
        activation_heights,
        scenarios::ChainCachePolicy::PerTest,
    )
    .await;
    check_client_balances!(recipient, i: 0 o: 100_000 s: 0 t: 0);
}

#[tokio::test]
async fn send_post_ironwood() {
    let (_local_net, _faucet, recipient, _) =
        scenarios::faucet_funded_recipient_default(100_000).await;
    check_client_balances!(recipient, i: 100_000 o: 0 s: 0 t: 0);
}

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

/// Tests that transparent coinbases mature after `COINBASE_MATURITY_BLOCKS`.
#[tokio::test]
async fn mine_to_transparent_coinbase_maturity() {
    let (local_net, mut faucet, _recipient) = scenarios::faucet_recipient(
        PoolType::Transparent,
        scenarios::default_test_activation_heights(),
        scenarios::ChainCachePolicy::PerTest,
    )
    .await;

    // After 3 blocks...
    check_client_balances!(faucet, i: 0 o: 0 s: 0 t: 0);

    // Balance should be 0 because coinbase needs COINBASE_MATURITY_BLOCKS confirmations
    assert_eq!(
        faucet
            .wallet()
            .read()
            .await
            .confirmed_balance_excluding_dust::<TransparentCoin>(zip32::AccountId::ZERO)
            .unwrap()
            .into_u64(),
        0
    );

    increase_height_and_wait_for_client(&local_net, &mut faucet, COINBASE_MATURITY_BLOCKS)
        .await
        .unwrap();

    let mature_balance = faucet
        .wallet()
        .read()
        .await
        .confirmed_balance_excluding_dust::<TransparentCoin>(zip32::AccountId::ZERO)
        .unwrap()
        .into_u64();

    // Should have 3 blocks worth of rewards
    assert_eq!(mature_balance, scenarios::mined_block_rewards_total(3));
}

mod testnet_test {
    use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
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
            while lightclient
                .latest_sync_status()
                .is_none_or(|status| status.percentage_total_outputs_scanned < 1.0)
            {
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
