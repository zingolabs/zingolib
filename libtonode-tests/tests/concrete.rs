#![forbid(unsafe_code)]
use json::JsonValue;

use zcash_address::unified::Fvk;
use zcash_primitives::transaction::fees::zip317::MINIMUM_FEE;

use pepper_sync::wallet::TransparentCoin;
use zcash_protocol::PoolType;
use zcash_protocol::value::Zatoshis;
use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingolib::utils::conversion::address_from_str;
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
    sent_o_value: Option<Zatoshis>,
    sent_s_value: Option<Zatoshis>,
    sent_t_value: Option<Zatoshis>,
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
        assert_eq!(balance.unconfirmed_orchard_balance, Some(Zatoshis::ZERO));
        // assert 1 Orchard note, or 2 notes if a dummy output is included
        let orchard_notes_count = orchard_notes
            .iter()
            .filter(|note| note.spend_status.is_unspent())
            .count();
        assert!((1..=2).contains(&orchard_notes_count));
    } else {
        assert!(ufvk.orchard().is_none());
        assert_eq!(balance.total_orchard_balance, None);
        assert_eq!(balance.confirmed_orchard_balance, None);
        assert_eq!(balance.unconfirmed_orchard_balance, None);
        assert_eq!(orchard_notes.len(), 0);
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

mod fast {

    use pepper_sync::wallet::{OutputInterface, TransparentCoin};
    use zcash_address::ZcashAddress;
    use zcash_client_backend::{
        encoding::encode_payment_address_p,
        zip321::{Payment, TransactionRequest},
    };
    use zcash_local_net::validator::Validator;
    use zcash_protocol::consensus::BlockHeight;
    use zcash_protocol::{PoolType, ShieldedProtocol, value::Zatoshis};
    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingolib::{
        config::WalletConfig,
        testutils::{
            chain_generics::conduct_chain::ConductChain, default_test_wallet_settings,
            lightclient::from_inputs,
        },
        wallet::keys::unified::{ReceiverSelection, UnifiedAddressId},
    };
    use zingolib_testutils::scenarios::increase_height_and_wait_for_client;
    use zip32::AccountId;

    use super::*;
    use libtonode_tests::chain_generics::LibtonodeEnvironment;


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
        if let Some(_ua) =
            recipient
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
        if let Some(_ua) =
            recipient
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
        if let Some(_ua) =
            recipient
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

        // sync recipient (to the Validator's tip — a bare sync races the
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
    ///   ORCHARD_COINBASE_START_HEIGHT is wrong (flip 2 <-> 3);
    /// - sapling delta of BLOCK_ONE_SAPLING_COINBASE (625_000_000): the
    ///   block-1-pays-the-sapling-receiver rule is wrong;
    /// - transparent nonzero: pre-NU5 or activation-block coinbase pays a
    ///   transparent output instead;
    /// - balances short by whole blocks: the deterministic
    ///   sync_client_to_validator_tip is not actually deterministic.
    #[tokio::test]
    async fn orchard_miner_coinbase_distribution() {
        let mut environment = LibtonodeEnvironment::setup().await;
        let mut faucet = environment.create_faucet().await;
        environment.increase_chain_height().await;
        scenarios::sync_client_to_validator_tip(&environment.local_net, &mut faucet).await;

        // Tip is height 4: launch block + 2 setup blocks + 1 above.
        check_client_balances!(
            faucet,
            o: (scenarios::orchard_coinbase_total(4)) s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0u64
        );
    }

    #[tokio::test]
    async fn send_not_fully_synced() {
        let (local_net, _faucet, mut recipient, _, _, _) = scenarios::faucet_funded_recipient(
            Some(200_000),
            Some(100_000),
            None,
            PoolType::Shielded(ShieldedProtocol::Orchard),
            scenarios::default_test_activation_heights(),
            None,
        )
        .await;

        local_net.validator().generate_blocks(5).await.unwrap();

        recipient
            .propose_send_all(
                address_from_str(&get_base_address_macro!(&recipient, "sapling")).unwrap(),
                false,
                None,
                zip32::AccountId::ZERO,
            )
            .await
            .unwrap();

        recipient.send_stored_proposal(true).await.unwrap();
    }

    pub mod tex {
        use pepper_sync::keys::decode_address;
        use zcash_client_backend::address::Address;
        use zcash_primitives::transaction::TxId;
        use zcash_transparent::address::TransparentAddress;
        use zingolib::{testutils, wallet::LightWallet};

        use super::*;

        fn first_taddr_to_tex(wallet: &LightWallet) -> ZcashAddress {
            let taddr = wallet.transparent_addresses().values().next().unwrap();
            let Address::Transparent(taddr) =
                decode_address(&wallet.chain_type(), taddr.as_str()).unwrap()
            else {
                panic!("not t addr")
            };

            let taddr_bytes = match taddr {
                TransparentAddress::PublicKeyHash(taddr_bytes) => taddr_bytes,
                TransparentAddress::ScriptHash(_) => panic!(),
            };
            let tex_string =
                testutils::interpret_taddr_as_tex_addr(taddr_bytes, &wallet.chain_type());

            ZcashAddress::try_from_encoded(&tex_string).unwrap()
        }
        #[tokio::test]
        async fn send_to_tex() {
            let (ref local_net, ref faucet, mut sender, _txid) =
                scenarios::faucet_funded_recipient_default(5_000_000).await;

            let tex_addr_from_first = first_taddr_to_tex(&*faucet.wallet().read().await);
            let payment = vec![Payment::without_memo(
                tex_addr_from_first.clone(),
                Zatoshis::from_u64(100_000).unwrap(),
            )];

            let transaction_request = TransactionRequest::new(payment).unwrap();

            let proposal = sender
                .propose_send(transaction_request, zip32::AccountId::ZERO)
                .await
                .unwrap();
            assert_eq!(proposal.steps().len(), 2usize);
            let _sent_txids_according_to_broadcast =
                sender.send_stored_proposal(true).await.unwrap();
            let _txids = sender
                .wallet()
                .read()
                .await
                .wallet_transactions
                .keys()
                .copied()
                .collect::<Vec<TxId>>();
            increase_height_and_wait_for_client(local_net, &mut sender, 1)
                .await
                .unwrap();
            assert_eq!(
                sender.wallet().read().await.wallet_transactions.len(),
                3usize
            );

            // FIXME: add tex addresses to encoded memos
            // let val_tranfers = sender.value_transfers(true).await.unwrap();
            // assert_eq!(
            //     val_tranfers[0].recipient_address().unwrap(),
            //     tex_addr_from_first.encode()
            // );
        }
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
        let mempool_target_height =
            BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 3);
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
    async fn mine_to_orchard() {
        let (local_net, mut faucet) = scenarios::faucet(
            PoolType::ORCHARD,
            scenarios::default_test_activation_heights(),
            None,
        )
        .await;
        check_client_balances!(
            faucet,
            o: (scenarios::funded_faucet_orchard_balance()) s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0
        );
        increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
            .await
            .unwrap();
        check_client_balances!(
            faucet,
            o: (scenarios::funded_faucet_orchard_balance() + scenarios::POST_STREAM_BLOCK_REWARD) s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0
        );
    }

    /// Tests that the miner's address receives (immature) rewards from mining to the transparent pool.
    #[tokio::test]
    async fn mine_to_transparent() {
        let (local_net, mut faucet, _recipient) = scenarios::faucet_recipient(
            PoolType::Transparent,
            scenarios::default_test_activation_heights(),
            None,
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
        // with the fixture shape — so the expressible epoch boundaries are
        // NU5/NU6 at height 2 and NU6.1/NU6.2 at height 5. Sync across all
        // of them with room to spare.
        let (local_net, mut lightclient) =
            scenarios::unfunded_client(scenarios::default_test_activation_heights(), None).await;
        increase_height_and_wait_for_client(&local_net, &mut lightclient, 12)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn mine_to_transparent_and_shield() {
        let activation_heights = scenarios::default_test_activation_heights();
        let (local_net, mut faucet, _recipient) =
            scenarios::faucet_recipient(PoolType::Transparent, activation_heights, None).await;
        increase_height_and_wait_for_client(&local_net, &mut faucet, 100)
            .await
            .unwrap();
        faucet.quick_shield(zip32::AccountId::ZERO).await.unwrap();
        increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
            .await
            .unwrap();

        assert_eq!(
            faucet
                .account_balance(zip32::AccountId::ZERO)
                .await
                .unwrap()
                .confirmed_orchard_balance
                .unwrap()
                .into_u64(),
            // 4 mature coinbases shielded in one step, minus the shield fee.
            scenarios::mined_block_rewards_total(4) - 30_000
        );
    }
}
mod slow {
    use pepper_sync::wallet::{OrchardNote, OutputInterface, SaplingNote, TransparentCoin};
    use zcash_local_net::validator::Validator;
    use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
    use zcash_protocol::consensus::BlockHeight;

    use zcash_protocol::PoolType;
    use zcash_protocol::value::Zatoshis;
    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingo_test_vectors::TEST_TXID;
    use zingolib::config::{ChainType, ClientConfig, WalletConfig};
    use zingolib::lightclient::LightClient;
    use zingolib::lightclient::error::{LightClientError, SendError};
    use zingolib::testutils::lightclient::{from_inputs, get_fees_paid_by_client};
    use zingolib::testutils::{
        assert_transaction_summary_equality, assert_transaction_summary_exists,
        build_fvks_from_unified_keystore, default_test_wallet_settings,
    };
    use zingolib::utils;
    use zingolib::utils::conversion::txid_from_hex_encoded_str;
    use zingolib::wallet::error::{CalculateTransactionError, ProposeSendError};

    use zingolib::wallet::output::SpendStatus;
    use zingolib::wallet::summary;
    use zingolib::wallet::summary::data::{
        BasicNoteSummary, OutgoingNoteSummary, SendType, TransactionKind, TransactionSummary,
    };
    use zingolib_testutils::scenarios::increase_height_and_wait_for_client;
    use zip32::AccountId;

    use super::*;

    #[tokio::test]
    async fn zero_value_receipts() {
        let (local_net, mut faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        let sent_value = 0;
        let _sent_transaction_id = from_inputs::quick_send(
            &mut faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                sent_value,
                None,
            )],
        )
        .await
        .unwrap();

        increase_height_and_wait_for_client(&local_net, &mut recipient, 5)
            .await
            .unwrap();
        let _sent_transaction_id = from_inputs::quick_send(
            &mut recipient,
            vec![(&get_base_address_macro!(faucet, "unified"), 1000, None)],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut recipient, 5)
            .await
            .unwrap();

        tracing::info!(
            "{}",
            &recipient
                .account_balance(zip32::AccountId::ZERO)
                .await
                .unwrap()
        );
        tracing::info!(
            "{}",
            JsonValue::from(recipient.value_transfers(true).await.unwrap()).pretty(4)
        );
    }
    #[tokio::test]
    async fn zero_value_change() {
        let value = 100_000;
        let (local_net, faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(value).await;

        let sent_value = value - u64::from(MINIMUM_FEE);
        let sent_transaction_id = from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                sent_value,
                None,
            )],
        )
        .await
        .unwrap()
        .first()
        .to_string();

        increase_height_and_wait_for_client(&local_net, &mut recipient, 5)
            .await
            .unwrap();

        let recipient_wallet = recipient.wallet().read().await;
        let transparent_coins = recipient_wallet.wallet_outputs::<TransparentCoin>();
        assert_eq!(transparent_coins.len(), 0);
        let sapling_notes = recipient_wallet.wallet_outputs::<SaplingNote>();
        assert_eq!(sapling_notes.len(), 0);
        let orchard_notes = recipient_wallet.wallet_outputs::<OrchardNote>();
        let unspent_orchard_notes = orchard_notes
            .iter()
            .filter(|&&note| recipient_wallet.output_spend_status(note).is_unspent())
            .collect::<Vec<_>>();
        let spent_orchard_notes = orchard_notes
            .iter()
            .filter(|&&note| {
                recipient_wallet
                    .output_spend_status(note)
                    .is_confirmed_spent()
            })
            .collect::<Vec<_>>();

        assert_eq!(unspent_orchard_notes.len(), 1);
        assert_eq!(
            orchard_notes
                .iter()
                .filter(|&&note| recipient_wallet
                    .output_spend_status(note)
                    .is_pending_spent())
                .count(),
            0
        );
        assert_eq!(spent_orchard_notes.len(), 1);

        assert_eq!(unspent_orchard_notes.first().unwrap().value(), 0);
        assert_eq!(
            spent_orchard_notes
                .first()
                .unwrap()
                .spending_transaction()
                .unwrap()
                .to_string(),
            sent_transaction_id
        );
        drop(recipient_wallet);

        check_client_balances!(recipient, o: 0 s: 0 t: 0);
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
        assert_eq!(sent_t_value, 10_000u64);
        assert_eq!(sent_s_value, 20_000u64);
        assert_eq!(sent_o_value, 30_000u64);

        // check that do_rescan works
        original_recipient.rescan_and_await().await.unwrap();
        check_client_balances!(original_recipient, o: sent_o_value s: sent_s_value t: sent_t_value);

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
                .build();
            let mut watch_client = LightClient::new(zingo_config, false).await.unwrap();
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
                    Some(sent_o_value.try_into().unwrap()),
                    Some(sent_s_value.try_into().unwrap()),
                    Some(sent_t_value.try_into().unwrap()),
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
        let (ref local_net, mut faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;
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
        tracing::info!(
            "{}",
            &recipient
                .account_balance(zip32::AccountId::ZERO)
                .await
                .unwrap()
        );
        tracing::info!("{}", recipient.transaction_summaries(false).await.unwrap());
        tracing::info!(
            "{}",
            JsonValue::from(recipient.value_transfers(true).await.unwrap()).pretty(2)
        );
        recipient.rescan_and_await().await.unwrap();
        tracing::info!(
            "{}",
            &recipient
                .account_balance(zip32::AccountId::ZERO)
                .await
                .unwrap()
        );
        tracing::info!("{}", recipient.transaction_summaries(false).await.unwrap());
        tracing::info!(
            "{}",
            JsonValue::from(recipient.value_transfers(true).await.unwrap()).pretty(2)
        );
        // TODO: Add asserts!
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
                .outgoing_orchard_notes
                .iter()
                .chain(transaction.outgoing_sapling_notes.iter())
                .any(|note| {
                    note.recipient_unified_address == Some(recipient_unified_address.clone())
                })
        }));
        faucet.rescan_and_await().await.unwrap();
        let rescanned_transactions = faucet.transaction_summaries(false).await.unwrap().0;
        assert!(rescanned_transactions.iter().any(|transaction| {
            transaction
                .outgoing_orchard_notes
                .iter()
                .chain(transaction.outgoing_sapling_notes.iter())
                .any(|note| {
                    note.recipient_unified_address == Some(recipient_unified_address.clone())
                })
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
    async fn send_to_transparent_and_sapling_maintain_balance() {
        // Receipt of orchard funds
        let recipient_initial_funds = 100_000_000;
        let (ref local_net, mut faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(recipient_initial_funds).await;

        let summary_orchard_receipt = TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 2,
            )),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 2),
            kind: TransactionKind::Received,
            value: recipient_initial_funds,
            fee: Some(10_000),
            zec_price: None,
            orchard_notes: vec![BasicNoteSummary::from_parts(
                recipient_initial_funds,
                SpendStatus::Spent(
                    utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
                ),
                0,
                None,
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![],
            outgoing_transparent_coins: vec![],
        };

        // Send to faucet (external) sapling
        let first_send_to_sapling = 20_000;
        from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                first_send_to_sapling,
                None,
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(local_net, &mut recipient, 1)
            .await
            .unwrap();
        let summary_external_sapling = TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 3)),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 3),
            kind: TransactionKind::Sent(SendType::Send),
            value: first_send_to_sapling,
            fee: Some(20_000),
            zec_price: None,
            orchard_notes: vec![BasicNoteSummary::from_parts(
                99_960_000,
                SpendStatus::TransmittedSpent(
                    utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
                ),
                0,
                None,
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![OutgoingNoteSummary {
                 output_index: 0,
                 value: first_send_to_sapling,
                 memo: None,
                 recipient: "zregtestsapling1sa4rckrf4zs6ny3l3ljnezupacvxfnjjn90lpeaa4ddtjeyww2ypzqr3jxfsta3t8dn3jk8cm4f".to_string(),
                 recipient_unified_address: Some("uregtest183rtm3qhxxermx3nxwa706va0xnypt3td648tayetchlp28hue08vrcnwq02ryyk5rh3y0xhftay8a5ynjdg8kr3juq5x0d9ygd5ffht".to_string()),
                 account_id: AccountId::ZERO,
                 scope: summary::data::Scope::from(zip32::Scope::External),
             }],
            outgoing_transparent_coins: vec![],
        };

        // Send to faucet (external) transparent
        let first_send_to_transparent = 20_000;
        let summary_external_transparent = TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Transmitted(BlockHeight::from_u32(
                scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 4,
            )),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 4),
            kind: TransactionKind::Sent(SendType::Send),
            value: first_send_to_transparent,
            fee: Some(15_000),
            zec_price: None,
            orchard_notes: vec![BasicNoteSummary::from_parts(
                99_925_000,
                SpendStatus::Unspent,
                0,
                None,
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![],
            outgoing_transparent_coins: vec![],
        };

        from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "transparent"),
                first_send_to_transparent,
                None,
            )],
        )
        .await
        .unwrap();

        // Assert transactions are as expected
        assert_transaction_summary_equality(
            &recipient.transaction_summaries(false).await.unwrap().0[0],
            &summary_orchard_receipt,
        );
        assert_transaction_summary_equality(
            &recipient.transaction_summaries(false).await.unwrap().0[1],
            &summary_external_sapling,
        );
        assert_transaction_summary_equality(
            &recipient.transaction_summaries(false).await.unwrap().0[2],
            &summary_external_transparent,
        );

        // Check several expectations about recipient wallet state:
        //  (1) shielded balance total is expected amount
        let expected_funds = recipient_initial_funds
            - first_send_to_sapling
            - (4 * u64::from(MARGINAL_FEE))
            - first_send_to_transparent
            - (3 * u64::from(MARGINAL_FEE));

        {
            let recipient_wallet = recipient.wallet().read().await;
            assert_eq!(
                recipient_wallet
                    .unconfirmed_balance::<OrchardNote>(zip32::AccountId::ZERO)
                    .unwrap(),
                expected_funds.try_into().unwrap()
            );
            //  (2) The balance is not yet verified
            assert_eq!(
                recipient_wallet
                    .confirmed_balance::<OrchardNote>(zip32::AccountId::ZERO)
                    .unwrap(),
                0.try_into().unwrap()
            );
        }

        increase_height_and_wait_for_client(local_net, &mut faucet, 1)
            .await
            .unwrap();

        let recipient_second_funding = 1_000_000;
        let summary_orchard_receipt_2 = TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 5,
            )),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 5),
            kind: TransactionKind::Received,
            value: recipient_second_funding,
            // The observed zip317 fee of the faucet's second-wave funding
            // send on the Core stack: by this point the faucet's note pool
            // is fragmented by the earlier waves (and selection is
            // smallest-first), making this a four-logical-action
            // transaction. Under the old monolithic zcashd-era funding it
            // was two actions (10_000).
            fee: Some(20_000),
            zec_price: None,
            orchard_notes: vec![BasicNoteSummary::from_parts(
                recipient_second_funding,
                SpendStatus::Spent(
                    utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
                ),
                0,
                Some("Second wave incoming".to_string()),
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![],
            outgoing_transparent_coins: vec![],
        };
        from_inputs::quick_send(
            &mut faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                recipient_second_funding,
                Some("Second wave incoming"),
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(local_net, &mut recipient, 1)
            .await
            .unwrap();

        // Send to external (faucet) transparent
        let second_send_to_transparent = 20_000;
        let summary_external_transparent_2 = TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 6,
            )),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 6),
            kind: TransactionKind::Sent(SendType::Send),
            value: second_send_to_transparent,
            fee: Some(15_000),
            zec_price: None,
            orchard_notes: vec![BasicNoteSummary::from_parts(
                965_000,
                SpendStatus::Spent(
                    utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
                ),
                0,
                None,
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![],
            outgoing_transparent_coins: vec![],
        };
        from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "transparent"),
                second_send_to_transparent,
                None,
            )],
        )
        .await
        .unwrap();

        // Send to faucet (external) sapling 2
        let second_send_to_sapling = 20_000;
        let summary_external_sapling_2 =

TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 6)),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 6),
            kind: TransactionKind::Sent(SendType::Send),
            value: second_send_to_sapling,
            fee: Some(20_000),
            zec_price: None,
            orchard_notes: vec![BasicNoteSummary::from_parts(
                99_885_000,
                SpendStatus::Unspent,
                0,
                None,
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![OutgoingNoteSummary {
                output_index: 0,
                 value: second_send_to_sapling,
                memo: None,
                 recipient: "zregtestsapling1sa4rckrf4zs6ny3l3ljnezupacvxfnjjn90lpeaa4ddtjeyww2ypzqr3jxfsta3t8dn3jk8cm4f".to_string(),
                 recipient_unified_address: Some("uregtest183rtm3qhxxermx3nxwa706va0xnypt3td648tayetchlp28hue08vrcnwq02ryyk5rh3y0xhftay8a5ynjdg8kr3juq5x0d9ygd5ffht".to_string()),
                 account_id: AccountId::ZERO,
                 scope: summary::data::Scope::from(zip32::Scope::External),
            }],
            outgoing_transparent_coins: vec![],
        };
        from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                second_send_to_sapling,
                None,
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(local_net, &mut recipient, 1)
            .await
            .unwrap();

        // Third external transparent
        let external_transparent_3 = 20_000;
        let summary_external_transparent_3 = TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 7,
            )),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 7),
            kind: TransactionKind::Sent(SendType::Send),
            value: external_transparent_3,
            fee: Some(15_000),
            zec_price: None,
            orchard_notes: vec![BasicNoteSummary::from_parts(
                930_000,
                SpendStatus::Unspent,
                0,
                None,
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![],
            outgoing_transparent_coins: vec![],
        };
        from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "transparent"),
                external_transparent_3,
                None,
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(local_net, &mut recipient, 1)
            .await
            .unwrap();

        // Final check
        assert_transaction_summary_equality(
            &recipient.transaction_summaries(false).await.unwrap().0[3],
            &summary_orchard_receipt_2,
        );
        assert_transaction_summary_exists(&recipient, &summary_external_transparent_2).await; // due to summaries of the same blockheight changing order
        assert_transaction_summary_exists(&recipient, &summary_external_sapling_2).await; // we check all summaries for these expected transactions
        assert_transaction_summary_equality(
            &recipient.transaction_summaries(false).await.unwrap().0[6],
            &summary_external_transparent_3,
        );
        let second_wave_expected_funds = expected_funds + recipient_second_funding
            - second_send_to_sapling
            - second_send_to_transparent
            - external_transparent_3
            - (5 * u64::from(MINIMUM_FEE));
        assert_eq!(
            recipient
                .wallet()
                .read()
                .await
                .confirmed_balance::<OrchardNote>(zip32::AccountId::ZERO)
                .unwrap(),
            second_wave_expected_funds.try_into().unwrap(),
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
        let setup_reward = scenarios::funded_faucet_orchard_balance();
        check_client_balances!(faucet, o: setup_reward s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0);

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
        let orch_change = scenarios::POST_STREAM_BLOCK_REWARD
            - (faucet_to_recipient_amount + u64::from(MINIMUM_FEE));
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        faucet.sync_and_await().await.unwrap();
        let faucet_orch = setup_reward + orch_change + u64::from(MINIMUM_FEE);

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

        check_client_balances!(faucet, o: faucet_orch s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0);
        check_client_balances!(recipient, o: faucet_to_recipient_amount s: 0 t: 0);

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

        let faucet_final_orch = faucet_orch
            + recipient_to_faucet_amount
            + scenarios::POST_STREAM_BLOCK_REWARD
            + u64::from(MINIMUM_FEE);
        let recipient_final_orch =
            faucet_to_recipient_amount - (u64::from(MINIMUM_FEE) + recipient_to_faucet_amount);
        check_client_balances!(
            faucet,
            o: faucet_final_orch s: (scenarios::BLOCK_ONE_SAPLING_COINBASE) t: 0
        );
        check_client_balances!(recipient, o: recipient_final_orch s: 0 t: 0);
    }

    #[tokio::test]
    async fn send_mined_orchard_to_orchard() {
        // This test shows a confirmation changing the state of balance by
        // debiting unverified_orchard_balance and crediting verified_orchard_balance.  The debit amount is
        // consistent with all the notes in the relevant block changing state.
        // NOTE that the balance doesn't give insight into the distribution across notes.
        let (local_net, mut faucet) = scenarios::faucet(
            PoolType::ORCHARD,
            scenarios::default_test_activation_heights(),
            None,
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
            balance.unconfirmed_orchard_balance,
            Some(0.try_into().unwrap())
        );
        // The send is to self, so only the fee leaves the wallet — and the
        // faucet mines the confirming block, collecting a fresh coinbase
        // reward plus that same fee back.
        assert_eq!(
            balance.confirmed_orchard_balance.unwrap().into_u64(),
            scenarios::funded_faucet_orchard_balance() + scenarios::POST_STREAM_BLOCK_REWARD
        );
    }

    #[tokio::test]
    async fn self_send_to_t_displays_as_one_transaction() {
        let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
        let recipient_unified_address = get_base_address_macro!(recipient, "unified");
        let sent_value = 80_000;
        from_inputs::quick_send(
            &mut faucet,
            vec![(recipient_unified_address.as_str(), sent_value, None)],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        let recipient_taddr = get_base_address_macro!(recipient, "transparent");
        let recipient_zaddr = get_base_address_macro!(recipient, "sapling");
        let sent_to_taddr_value = 5_000;
        let sent_to_zaddr_value = 11_000;
        let sent_to_self_orchard_value = 1_000;
        from_inputs::quick_send(
            &mut recipient,
            vec![(recipient_taddr.as_str(), sent_to_taddr_value, None)],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        from_inputs::quick_send(
            &mut recipient,
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
        faucet.sync_and_await().await.unwrap();
        from_inputs::quick_send(
            &mut faucet,
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
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        tracing::info!(
            "{}",
            json::stringify_pretty(recipient.transaction_summaries(false).await.unwrap(), 4)
        );
        let mut txids = recipient
            .transaction_summaries(false)
            .await
            .unwrap()
            .txids()
            .into_iter();
        assert!(itertools::Itertools::all_unique(&mut txids));
    }

    #[tokio::test]
    async fn sapling_dust_fee_collection() {
        let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
        let recipient_sapling = get_base_address_macro!(recipient, "sapling");
        let recipient_unified = get_base_address_macro!(recipient, "unified");
        check_client_balances!(recipient, o: 0 s: 0 t: 0);
        let fee = u64::from(MINIMUM_FEE);
        let for_orchard = dbg!(fee * 10);
        let for_sapling = dbg!(fee / 10);
        from_inputs::quick_send(
            &mut faucet,
            vec![
                (&recipient_unified, for_orchard, Some("Plenty for orchard.")),
                (&recipient_sapling, for_sapling, Some("Dust for sapling.")),
            ],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        check_client_balances!(recipient, o: for_orchard s: 0 t: 0 );

        from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                fee * 5,
                Some("Five times fee."),
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        let remaining_orchard = for_orchard - (6 * fee);
        check_client_balances!(recipient, o: remaining_orchard s: 0 t: 0);
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
        #[tokio::test]
        async fn external_send() {
            let (local_net, mut faucet, recipient) = scenarios::faucet_recipient_default().await;
            let _external_send_txid_with_memo = *from_inputs::quick_send(
                &mut faucet,
                vec![(
                    get_base_address_macro!(recipient, "sapling").as_str(),
                    1_000,
                    Some("foo"),
                )],
            )
            .await
            .unwrap()
            .first();
            let _external_send_txid_no_memo = *from_inputs::quick_send(
                &mut faucet,
                vec![(
                    get_base_address_macro!(recipient, "sapling").as_str(),
                    1_000,
                    None,
                )],
            )
            .await
            .unwrap()
            .first();
            // TODO:  This chain height bump should be unnecessary. I think removing
            // this increase_height call reveals a bug!
            increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
                .await
                .unwrap();

            let pre_rescan_summaries = faucet.transaction_summaries(false).await.unwrap();
            faucet.rescan_and_await().await.unwrap();
            let post_rescan_summaries = faucet.transaction_summaries(false).await.unwrap();
            assert_eq!(pre_rescan_summaries, post_rescan_summaries);
        }
        #[tokio::test]
        async fn check_list_value_transfers_across_rescan() {
            let inital_value = 100_000;
            let (ref local_net, faucet, mut recipient, _txid) =
                scenarios::faucet_funded_recipient_default(inital_value).await;
            from_inputs::quick_send(
                &mut recipient,
                vec![(&get_base_address_macro!(faucet, "unified"), 10_000, None); 2],
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
    /// spend with cross-pool (orchard) change. The unwraps are the
    /// assertions: proposal, proving, and the validator's acceptance of the
    /// bundle. Note-selection ordering itself is asserted offline by
    /// `note_selection_covers_target_with_minimal_change` in
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
    }

    // FIXME: it seems this test makes assertions on mempool but mempool monitoring is off?
    #[tokio::test]
    async fn mempool_and_balance() {
        let value = 100_000;
        let (local_net, faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(value).await;

        let bal = recipient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();
        tracing::info!("{bal}");
        assert_eq!(bal.total_orchard_balance.unwrap().into_u64(), value);
        assert_eq!(bal.confirmed_orchard_balance.unwrap().into_u64(), value);
        assert_eq!(bal.unconfirmed_orchard_balance.unwrap().into_u64(), 0);

        // 3. Mine 10 blocks
        increase_height_and_wait_for_client(&local_net, &mut recipient, 10)
            .await
            .unwrap();
        let bal = recipient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert_eq!(bal.total_orchard_balance.unwrap().into_u64(), value);
        assert_eq!(bal.confirmed_orchard_balance.unwrap().into_u64(), value);
        assert_eq!(bal.unconfirmed_orchard_balance.unwrap().into_u64(), 0);

        // 4. Spend the funds
        let sent_value = 2000;
        let outgoing_memo = "Outgoing Memo";

        let _sent_transaction_id = from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                sent_value,
                Some(outgoing_memo),
            )],
        )
        .await
        .unwrap();

        let bal = recipient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();

        // Even though the transaction is not mined (in the mempool) the balances should be updated to reflect the spent funds
        let new_bal = value - (sent_value + u64::from(MINIMUM_FEE));
        assert_eq!(bal.total_orchard_balance.unwrap().into_u64(), new_bal);
        assert_eq!(bal.confirmed_orchard_balance.unwrap().into_u64(), 0);
        assert_eq!(bal.unconfirmed_orchard_balance.unwrap().into_u64(), new_bal);

        // 5. Mine the pending block, making the funds verified and spendable.
        increase_height_and_wait_for_client(&local_net, &mut recipient, 10)
            .await
            .unwrap();

        let bal = recipient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();

        assert_eq!(bal.total_orchard_balance.unwrap().into_u64(), new_bal);
        assert_eq!(bal.confirmed_orchard_balance.unwrap().into_u64(), new_bal);
        assert_eq!(bal.unconfirmed_orchard_balance.unwrap().into_u64(), 0);
    }

    // FIXME: add unified address discovery to pepper sync and add a test here

    #[tokio::test]
    async fn list_value_transfers_check_fees() {
        // Check that list_value_transfers behaves correctly given different fee scenarios
        let (local_net, mut client_builder) = scenarios::custom_clients_default().await;
        let mut faucet = client_builder.build_faucet(false).await;
        let mut pool_migration_client = client_builder
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
        let pmc_taddr = get_base_address_macro!(pool_migration_client, "transparent");
        let pmc_sapling = get_base_address_macro!(pool_migration_client, "sapling");
        let pmc_unified = get_base_address_macro!(pool_migration_client, "unified");
        // Ensure that the client has confirmed spendable funds
        increase_height_and_wait_for_client(&local_net, &mut faucet, 3)
            .await
            .unwrap();
        macro_rules! bump_and_check_pmc {
            (o: $o:tt s: $s:tt t: $t:tt) => {
                increase_height_and_wait_for_client(&local_net, &mut pool_migration_client, 1).await.unwrap();
                check_client_balances!(pool_migration_client, o:$o s:$s t:$t);
            };
        }

        // pmc receives 100_000 orchard
        from_inputs::quick_send(&mut faucet, vec![(&pmc_unified, 100_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 100_000 s: 0 t: 0);

        // to transparent and sapling from orchard
        //
        // Expected Fees:
        // 5_000 for transparent + 10_000 for orchard + 10_000 for sapling == 25_000
        from_inputs::quick_send(
            &mut pool_migration_client,
            vec![(&pmc_taddr, 30_000, None), (&pmc_sapling, 30_000, None)],
        )
        .await
        .unwrap();
        bump_and_check_pmc!(o: 15_000 s: 30_000 t: 30_000);
    }

    #[tokio::test]
    async fn from_t_z_o_tz_to_zo_tzo_to_orchard() {
        // Test all possible promoting note source combinations
        let (local_net, mut client_builder) = scenarios::custom_clients_default().await;
        let mut faucet = client_builder.build_faucet(false).await;
        let mut client = client_builder
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
        let pmc_taddr = get_base_address_macro!(client, "transparent");
        let pmc_sapling = get_base_address_macro!(client, "sapling");
        let pmc_unified = get_base_address_macro!(client, "unified");

        // Ensure that the faucet has confirmed spendable funds
        increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
            .await
            .unwrap();

        macro_rules! bump_and_check {
            (o: $o:tt s: $s:tt t: $t:tt) => {
                increase_height_and_wait_for_client(&local_net, &mut client, 1).await.unwrap();
                check_client_balances!(client, o:$o s:$s t:$t);
            };
        }

        let mut test_dev_total_expected_fee = 0;
        // 1 pmc receives 50_000 transparent
        //  # Expected Fees to recipient:
        //    - legacy: 0
        //    - 317:    0
        from_inputs::quick_send(&mut faucet, vec![(&pmc_taddr, 50_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 s: 0 t: 50_000);
        assert_eq!(test_dev_total_expected_fee, 0);

        // 2 pmc shields 50_000 transparent, to orchard paying fee
        //  t -> o
        //  # Expected Fees to recipient:
        //    - legacy: 10_000
        //    - 317:    15_000 1-orchard + 1-dummy + 1-transparent in
        client.quick_shield(zip32::AccountId::ZERO).await.unwrap();
        bump_and_check!(o: 35_000 s: 0 t: 0);
        test_dev_total_expected_fee += 15_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 3 pmc receives 50_000 sapling
        //  # Expected Fees to recipient:
        //    - legacy: 0
        //    - 317:    0
        from_inputs::quick_send(&mut faucet, vec![(&pmc_sapling, 50_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 35_000 s: 50_000 t: 0);
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 4 pmc migrates 40_000 from sapling to orchard plus fee
        //  z -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000
        from_inputs::quick_send(&mut client, vec![(&pmc_unified, 30_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 65_000 s: 0 t: 0);
        test_dev_total_expected_fee += 20_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 5 Self send of 55_000 paying 10_000 fee
        //  o -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000
        from_inputs::quick_send(&mut client, vec![(&pmc_unified, 55_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 55_000 s: 0 t: 0);
        test_dev_total_expected_fee += 10_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 6 to transparent and sapling from orchard
        //  o -> tz
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    5_000 for transparent out + 10_000 for orchard + 10_000 for sapling == 25_000
        from_inputs::quick_send(
            &mut client,
            vec![(&pmc_taddr, 10_000, None), (&pmc_sapling, 10_000, None)],
        )
        .await
        .unwrap();
        bump_and_check!(o: 10_000 s: 10_000 t: 10_000);
        test_dev_total_expected_fee += 25_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 7 Receive 500_000 to transparent
        from_inputs::quick_send(&mut faucet, vec![(&pmc_taddr, 500_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 10_000 s: 10_000 t: 510_000);
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 8 Shield transparent to orchard
        //  t -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000 = 10_000 orchard and o-dummy + 10_000 (2 t-notes)
        client.quick_shield(zip32::AccountId::ZERO).await.unwrap();
        bump_and_check!(o: 500_000 s: 10_000 t: 0);
        test_dev_total_expected_fee += 20_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 9 self o send orchard to orchard
        // TODO: already tested!?
        //  o -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000
        from_inputs::quick_send(&mut client, vec![(&pmc_unified, 30_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 490_000 s: 10_000 t: 0);
        test_dev_total_expected_fee += 10_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 10 Orchard and Sapling demote all to transparent self-send
        //  oz -> t
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    15_000 5-o (3 dust)- 10_000 orchard, 1 utxo 5_000 transparent
        from_inputs::quick_send(&mut client, vec![(&pmc_taddr, 470_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 s: 0 t: 470_000);
        test_dev_total_expected_fee += 30_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 10 transparent to transparent
        // Very explicit catch of reject sending from transparent
        match from_inputs::quick_send(&mut client, vec![(&pmc_taddr, 10_000, None)]).await {
            Ok(_) => panic!(),
            Err(LightClientError::SendError(SendError::ProposeSendError(e))) => match e {
                ProposeSendError::Proposal(insufficient) => {
                    if let zcash_client_backend::data_api::error::Error::InsufficientFunds {
                        available,
                        required,
                    } = insufficient
                    {
                        assert_eq!(available, Zatoshis::from_u64(0).unwrap());
                        assert_eq!(required, Zatoshis::from_u64(20_000).unwrap());
                    } else {
                        panic!()
                    }
                }
                ProposeSendError::TransactionRequestFailed(_) => panic!(),
                ProposeSendError::ZeroValueSendAll => panic!(),
                ProposeSendError::BalanceError(_) => panic!(),
            },
            _ => panic!(),
        }
        bump_and_check!(o: 0 s: 0 t: 470_000);
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 11 transparent to sapling
        //  t -> z
        match from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 50_000, None)]).await {
            Ok(_) => panic!(),
            Err(LightClientError::SendError(SendError::ProposeSendError(e))) => {
                if let ProposeSendError::Proposal(insufficient_funds) = e {
                    match insufficient_funds {
                        zcash_client_backend::data_api::error::Error::InsufficientFunds {
                            available,
                            required,
                        } => {
                            assert_eq!(available, Zatoshis::from_u64(0).unwrap());
                            assert_eq!(required, Zatoshis::from_u64(60_000).unwrap());
                        }
                        _ => {
                            panic!()
                        }
                    }
                } else {
                    panic!()
                }
            }
            _ => panic!(),
        }
        bump_and_check!(o: 0 s: 0 t: 470_000);
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 12 Shield
        //  t -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    15_000 1t and 2o
        client.quick_shield(zip32::AccountId::ZERO).await.unwrap();
        bump_and_check!(o: 455_000 s: 0 t: 0);
        test_dev_total_expected_fee += 15_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 13 Orchard to Sapling
        //  o -> z
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000 2o and 2s
        from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 10_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 425_000 s: 10_000 t: 0);
        test_dev_total_expected_fee += 20_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 14 Orchard self-send
        //  o -> o
        // TODO: already tested!?
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000
        from_inputs::quick_send(&mut client, vec![(&pmc_unified, 20_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 415_000 s: 10_000 t: 0);
        test_dev_total_expected_fee += 10_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 14 Orchard and Sapling to Sapling
        //  zo -> z
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000
        from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 405_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 s: 405_000 t: 0);
        test_dev_total_expected_fee += 20_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 15 Sapling self-send
        //  z -> z
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000
        from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 380_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 s: 395_000 t: 0);
        test_dev_total_expected_fee += 10_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );
    }

    #[tokio::test]
    async fn factor_do_shield_to_call_do_send() {
        let (local_net, mut faucet, recipient) = scenarios::faucet_recipient_default().await;
        increase_height_and_wait_for_client(&local_net, &mut faucet, 2)
            .await
            .unwrap();
        from_inputs::quick_send(
            &mut faucet,
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
        let (_local_net, faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        // Send of less that transaction fee
        let sent_value = 1_000;
        let _sent_transaction_id = from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                sent_value,
                None,
            )],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn zero_value_change_to_orchard_created() {
        let (local_net, faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();

        // 1. Send a transaction to an external z addr
        let sent_zvalue = 80_000;
        let sent_zmemo = "Ext z";
        let sent_transaction_id = from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                sent_zvalue,
                Some(sent_zmemo),
            )],
        )
        .await
        .unwrap()
        .first()
        .to_string();

        // Validate transaction
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();

        let sent_txid = txid_from_hex_encoded_str(&sent_transaction_id).unwrap();
        let orchard_note = recipient
            .wallet()
            .read()
            .await
            .wallet_transactions
            .get(&sent_txid)
            .unwrap()
            .orchard_notes()
            .first()
            .unwrap()
            .clone();
        assert_eq!(orchard_note.value(), 0);
    }
    #[tokio::test]
    async fn mempool_spends_correctly_marked_pending_spent() {
        let (local_net, faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(1_000_000).await;
        let sent_txids = from_inputs::quick_send(
            &mut recipient,
            vec![(&get_base_address_macro!(faucet, "sapling"), 100_000, None)],
        )
        .await
        .unwrap();
        recipient.sync_and_await().await.unwrap();
        {
            let recipient_wallet = recipient.wallet().read().await;
            let sapling_notes = recipient_wallet.wallet_outputs::<SaplingNote>();
            assert_eq!(sapling_notes.len(), 0);
            let orchard_notes = recipient_wallet.wallet_outputs::<OrchardNote>();
            assert_eq!(orchard_notes.len(), 2);
            let spent_orchard_note = (*orchard_notes
                .iter()
                .find(|&&note| note.value() == 1_000_000)
                .unwrap())
            .clone();
            assert_eq!(
                recipient_wallet.output_spend_status(&spent_orchard_note),
                SpendStatus::MempoolSpent(*sent_txids.first())
            );
            let orchard_change_note = (*orchard_notes
                .iter()
                .find(|&&note| note.value() == 880_000)
                .unwrap())
            .clone();
            assert_eq!(
                recipient_wallet.output_spend_status(&orchard_change_note),
                SpendStatus::Unspent
            );
            assert!(
                !recipient_wallet
                    .output_transaction(&orchard_change_note)
                    .status()
                    .is_confirmed()
            );
        }
        let balance = recipient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert_eq!(balance.total_orchard_balance.unwrap().into_u64(), 880_000);
        assert_eq!(balance.confirmed_orchard_balance.unwrap().into_u64(), 0);
        assert_eq!(
            balance.unconfirmed_orchard_balance.unwrap().into_u64(),
            880_000
        );
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        {
            let recipient_wallet = recipient.wallet().read().await;
            let sapling_notes = recipient_wallet.wallet_outputs::<SaplingNote>();
            assert_eq!(sapling_notes.len(), 0);
            let orchard_notes = recipient_wallet.wallet_outputs::<OrchardNote>();
            assert_eq!(orchard_notes.len(), 2);
            let spent_orchard_note = (*orchard_notes
                .iter()
                .find(|&&note| note.value() == 1_000_000)
                .unwrap())
            .clone();
            assert_eq!(
                recipient_wallet.output_spend_status(&spent_orchard_note),
                SpendStatus::Spent(*sent_txids.first())
            );
            let orchard_change_note = (*orchard_notes
                .iter()
                .find(|&&note| note.value() == 880_000)
                .unwrap())
            .clone();
            assert_eq!(
                recipient_wallet.output_spend_status(&orchard_change_note),
                SpendStatus::Unspent
            );
            assert!(
                recipient_wallet
                    .output_transaction(&orchard_change_note)
                    .status()
                    .is_confirmed()
            );
        }
        let balance = recipient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert_eq!(balance.total_orchard_balance.unwrap().into_u64(), 880_000);
        assert_eq!(
            balance.confirmed_orchard_balance.unwrap().into_u64(),
            880_000
        );
        assert_eq!(balance.unconfirmed_orchard_balance.unwrap().into_u64(), 0);
    }
}

mod basic_transactions {
    use zingolib::{get_base_address_macro, testutils::lightclient::from_inputs};
    use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};

    #[tokio::test]
    async fn send_and_sync_with_multiple_notes_no_panic() {
        let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;

        let recipient_addr_ua = get_base_address_macro!(recipient, "unified");
        let faucet_addr_ua = get_base_address_macro!(faucet, "unified");

        increase_height_and_wait_for_client(&local_net, &mut recipient, 2)
            .await
            .unwrap();
        scenarios::sync_client_to_validator_tip(&local_net, &mut faucet).await;

        for _ in 0..2 {
            from_inputs::quick_send(
                &mut faucet,
                vec![(recipient_addr_ua.as_str(), 40_000, None)],
            )
            .await
            .unwrap();
        }

        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        scenarios::sync_client_to_validator_tip(&local_net, &mut faucet).await;

        from_inputs::quick_send(
            &mut recipient,
            vec![(faucet_addr_ua.as_str(), 50_000, None)],
        )
        .await
        .unwrap();

        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        scenarios::sync_client_to_validator_tip(&local_net, &mut faucet).await;
    }

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

/// Tests that transparent coinbases are matured after 100 blocks.
#[tokio::test]
async fn mine_to_transparent_coinbase_maturity() {
    let (local_net, mut faucet, _recipient) = scenarios::faucet_recipient(
        PoolType::Transparent,
        scenarios::default_test_activation_heights(),
        None,
    )
    .await;

    // After 3 blocks...
    check_client_balances!(faucet, o: 0 s: 0 t: 0);

    // Balance should be 0 because coinbase needs 100 confirmations
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

    increase_height_and_wait_for_client(&local_net, &mut faucet, 100)
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

mod send_all {

    use pepper_sync::wallet::{OrchardNote, SaplingNote};

    use zingolib::testutils::lightclient::from_inputs;

    use super::*;
    #[tokio::test]
    async fn ptfm_general() {
        let (local_net, mut faucet, mut recipient, _) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        from_inputs::quick_send(
            &mut faucet,
            vec![(&get_base_address_macro!(&recipient, "unified"), 5_000, None)],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
            .await
            .unwrap();
        from_inputs::quick_send(
            &mut faucet,
            vec![(
                &get_base_address_macro!(&recipient, "sapling"),
                50_000,
                None,
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
            .await
            .unwrap();
        from_inputs::quick_send(
            &mut faucet,
            vec![(&get_base_address_macro!(&recipient, "sapling"), 4_000, None)],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
            .await
            .unwrap();
        from_inputs::quick_send(
            &mut faucet,
            vec![(&get_base_address_macro!(&recipient, "unified"), 4_000, None)],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
            .await
            .unwrap();
        recipient.sync_and_await().await.unwrap();

        recipient
            .propose_send_all(
                address_from_str(&get_base_address_macro!(faucet, "sapling")).unwrap(),
                false,
                None,
                zip32::AccountId::ZERO,
            )
            .await
            .unwrap();
        recipient.send_stored_proposal(true).await.unwrap();
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        faucet.sync_and_await().await.unwrap();

        assert_eq!(
            recipient
                .wallet()
                .read()
                .await
                .confirmed_balance_excluding_dust::<SaplingNote>(zip32::AccountId::ZERO)
                .unwrap()
                .into_u64(),
            0
        );
        assert_eq!(
            recipient
                .wallet()
                .read()
                .await
                .confirmed_balance_excluding_dust::<OrchardNote>(zip32::AccountId::ZERO)
                .unwrap()
                .into_u64(),
            0
        );
    }
}

mod testnet_test {
    use pepper_sync::sync_status;
    use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
    use zingolib::{
        config::{ChainType, ClientConfig, DEFAULT_INDEXER_URI_TESTNET, WalletConfig},
        lightclient::LightClient,
        testutils::{default_test_wallet_settings, tempfile::TempDir},
    };

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
                .set_indexer_uri((DEFAULT_INDEXER_URI_TESTNET).parse::<http::Uri>().unwrap())
                .set_wallet_config(WalletConfig::MnemonicPhrase {
                    mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
                    no_of_accounts: 1.try_into().unwrap(),
                    birthday: 2_000_000,
                    wallet_settings: default_test_wallet_settings(),
                })
                .set_wallet_dir(wallet_dir.path().to_path_buf())
                .build();

            let mut lightclient = LightClient::new(config, true).await.unwrap();
            lightclient.save_task().await;
            lightclient.sync().await.unwrap();
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            while sync_status(&*lightclient.wallet().read().await)
                .await
                .unwrap()
                .percentage_total_outputs_scanned
                > 1.0
            {
                interval.tick().await;
            }
            lightclient.stop_sync().unwrap();
            lightclient.await_sync().await.unwrap();
            lightclient.shutdown_save_task().await.unwrap();

            // will fail if there were any reload errors due to bad file write code i.e. no flushing or file syncing
            let config = ClientConfig::builder()
                .set_chain_type(ChainType::Testnet)
                .set_indexer_uri((DEFAULT_INDEXER_URI_TESTNET).parse::<http::Uri>().unwrap())
                .set_wallet_config(WalletConfig::Read)
                .set_wallet_dir(wallet_dir.path().to_path_buf())
                .build();
            LightClient::new(config, true).await.unwrap();

            test_count += 1;
        }
    }
}
