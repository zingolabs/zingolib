use pepper_sync::wallet::{IronwoodNote, OrchardNote, SaplingNote};
use zcash_address::unified;
use zcash_protocol::PoolType;
use zcash_protocol::consensus::Parameters;
use zcash_protocol::value::Zatoshis;
use zip32::AccountId;

use super::{external_transparent_address, funded_client, zatoshis};
use crate::check_client_balances;
use crate::config::{ClientConfig, WalletConfig};
use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, SendError};
use crate::testutils::lightclient::from_inputs;
use crate::testutils::mock_indexer::MockNet;
use crate::testutils::{build_fvks_from_unified_keystore, default_test_wallet_settings, tempfile};
use crate::wallet::balance::AccountBalance;
use crate::wallet::error::CalculateTransactionError;
use crate::wallet::keys::unified::UnifiedKeyStore;
use crate::wallet::summary::data::{CoinSummary, NoteSummary};

const TRANSPARENT_FUNDING: u64 = 10_000;
const SAPLING_FUNDING: u64 = 20_000;
const UNIFIED_FUNDING: u64 = 30_000;
const UNSIGNABLE_PAYMENT: u64 = 1_000;
const FUNDED_IRONWOOD_NOTES: usize = 1;
const DUMMY_OUTPUT_NOTES: usize = 1;

struct Funded {
    ironwood: Zatoshis,
    orchard: Zatoshis,
    sapling: Zatoshis,
    transparent: Zatoshis,
}

struct WatchWalletView<'a> {
    balance: &'a AccountBalance,
    unified_key_store: &'a UnifiedKeyStore,
    ironwood_notes: &'a [NoteSummary],
    orchard_notes: &'a [NoteSummary],
    sapling_notes: &'a [NoteSummary],
    transparent_coins: &'a [CoinSummary],
}

fn check_expected_balance_with_fvks(
    fvks: &[&unified::Fvk],
    balance: AccountBalance,
    o_expect: u64,
    s_expect: u64,
    t_expect: u64,
) {
    let zatoshis = |value: Option<Zatoshis>| {
        value
            .expect("a viewing key the wallet holds reports its pool's balance")
            .into_u64()
    };
    for fvk in fvks {
        match fvk {
            unified::Fvk::Sapling(_) => {
                assert_eq!(zatoshis(balance.total_sapling_balance), s_expect);
                assert_eq!(zatoshis(balance.confirmed_sapling_balance), s_expect);
                assert_eq!(zatoshis(balance.unconfirmed_sapling_balance), s_expect);
            }
            unified::Fvk::Orchard(_) => {
                assert_eq!(zatoshis(balance.total_orchard_balance), o_expect);
                assert_eq!(zatoshis(balance.confirmed_orchard_balance), o_expect);
                assert_eq!(zatoshis(balance.unconfirmed_orchard_balance), o_expect);
            }
            unified::Fvk::P2pkh(_) => {
                assert_eq!(zatoshis(balance.confirmed_transparent_balance), t_expect);
            }
            _ => panic!("the keystore builds orchard, sapling and transparent keys only"),
        }
    }
}

fn check_view_capability_bounds(
    view: &WatchWalletView<'_>,
    fvks: &[&unified::Fvk],
    all_fvks: &[unified::Fvk; 3],
    funded: &Funded,
) {
    let [orchard_fvk, sapling_fvk, transparent_fvk] = all_fvks;
    let UnifiedKeyStore::View(ufvk) = view.unified_key_store else {
        panic!("should be viewing key!")
    };
    //Orchard
    if fvks.contains(&orchard_fvk) {
        assert!(ufvk.orchard().is_some());
        assert_eq!(view.balance.total_orchard_balance, Some(funded.orchard));
        assert_eq!(view.balance.confirmed_orchard_balance, Some(funded.orchard));
        assert_eq!(view.balance.total_ironwood_balance, Some(funded.ironwood));
        assert_eq!(
            view.balance.confirmed_ironwood_balance,
            Some(funded.ironwood)
        );
        assert_eq!(
            view.balance.unconfirmed_ironwood_balance,
            Some(Zatoshis::ZERO)
        );
        // The funded note, or one more when the funding carried a dummy output.
        let ironwood_notes_count = view
            .ironwood_notes
            .iter()
            .filter(|note| note.spend_status.is_unspent())
            .count();
        assert!(
            (FUNDED_IRONWOOD_NOTES..=FUNDED_IRONWOOD_NOTES + DUMMY_OUTPUT_NOTES)
                .contains(&ironwood_notes_count)
        );
    } else {
        assert!(ufvk.orchard().is_none());
        assert_eq!(view.balance.total_orchard_balance, None);
        assert_eq!(view.balance.confirmed_orchard_balance, None);
        assert_eq!(view.balance.unconfirmed_orchard_balance, None);
        assert_eq!(view.orchard_notes.len(), 0);
        assert_eq!(view.balance.total_ironwood_balance, None);
        assert_eq!(view.balance.confirmed_ironwood_balance, None);
        assert_eq!(view.balance.unconfirmed_ironwood_balance, None);
        assert_eq!(view.ironwood_notes.len(), 0);
    }
    //Sapling
    if fvks.contains(&sapling_fvk) {
        assert!(ufvk.sapling().is_some());
        assert_eq!(view.balance.total_sapling_balance, Some(funded.sapling));
        assert_eq!(view.balance.confirmed_sapling_balance, Some(funded.sapling));
        assert_eq!(
            view.balance.unconfirmed_sapling_balance,
            Some(Zatoshis::ZERO)
        );
        assert_eq!(
            view.sapling_notes
                .iter()
                .filter(|note| note.spend_status.is_unspent())
                .count(),
            1
        );
    } else {
        assert!(ufvk.sapling().is_none());
        assert_eq!(view.balance.total_sapling_balance, None);
        assert_eq!(view.balance.confirmed_sapling_balance, None);
        assert_eq!(view.balance.unconfirmed_sapling_balance, None);
        assert_eq!(view.sapling_notes.len(), 0);
    }
    if fvks.contains(&transparent_fvk) {
        assert!(ufvk.p2pkh().is_some());
        assert_eq!(
            view.balance.confirmed_transparent_balance,
            Some(funded.transparent)
        );
        assert_eq!(view.transparent_coins.len(), 1);
    } else {
        assert!(ufvk.p2pkh().is_none());
        assert_eq!(view.balance.confirmed_transparent_balance, None);
        assert_eq!(view.transparent_coins.len(), 0);
    }
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

    let mut net = MockNet::launch().await;
    // 1. and 2. one funding transaction containing all types of outputs
    let mut original_recipient = funded_client(
        &mut net,
        &[
            (PoolType::TRANSPARENT, TRANSPARENT_FUNDING),
            (PoolType::SAPLING, SAPLING_FUNDING),
            (PoolType::ORCHARD, UNIFIED_FUNDING),
        ],
        1,
    )
    .await;
    let original_recipient_balance = original_recipient
        .account_balance(AccountId::ZERO)
        .await
        .expect("a seed wallet holds account zero");
    // The unified-address payment lands in the Ironwood pool (ADR 0009).
    let funded = Funded {
        ironwood: zatoshis(UNIFIED_FUNDING),
        orchard: zatoshis(0),
        sapling: zatoshis(SAPLING_FUNDING),
        transparent: zatoshis(TRANSPARENT_FUNDING),
    };
    assert_eq!(
        original_recipient_balance.confirmed_transparent_balance,
        Some(funded.transparent)
    );
    assert_eq!(
        original_recipient_balance.total_sapling_balance,
        Some(funded.sapling)
    );
    assert_eq!(
        original_recipient_balance.total_orchard_balance,
        Some(funded.orchard)
    );
    assert_eq!(
        original_recipient_balance.total_ironwood_balance,
        Some(funded.ironwood)
    );

    // check that do_rescan works
    original_recipient
        .rescan_and_await()
        .await
        .expect("the mock chain serves every block a rescan from the birthday requests");
    check_client_balances!(original_recipient, i: UNIFIED_FUNDING o: 0 s: SAPLING_FUNDING t: TRANSPARENT_FUNDING);

    // Extract viewing keys
    let original_wallet = original_recipient.wallet().read().await;
    let all_fvks = build_fvks_from_unified_keystore(
        original_wallet
            .unified_key_store
            .get(&AccountId::ZERO)
            .expect("a seed wallet holds account zero"),
    );
    let [o_fvk, s_fvk, t_fvk] = &all_fvks;
    let fvks_sets = [
        vec![o_fvk],
        vec![s_fvk],
        vec![o_fvk, s_fvk],
        vec![o_fvk, t_fvk],
        vec![s_fvk, t_fvk],
        vec![o_fvk, s_fvk, t_fvk],
    ];
    let network_type = net.chain_type().network_type();
    let external_taddr = external_transparent_address();
    let mut wallet_dirs = Vec::new();
    for fvks in &fvks_sets {
        tracing::info!("testing UFVK containing:");
        tracing::info!("    orchard fvk: {}", fvks.contains(&o_fvk));
        tracing::info!("    sapling fvk: {}", fvks.contains(&s_fvk));
        tracing::info!("    transparent fvk: {}", fvks.contains(&t_fvk));

        let ufvk = unified::Encoding::encode(
            &<unified::Ufvk as unified::Encoding>::try_from_items(
                unified::Revision::R0,
                fvks.iter()
                    .copied()
                    .cloned()
                    .map(unified::Uitem::Data)
                    .collect(),
            )
            .expect("every subset of the keystore's keys forms a unified full viewing key"),
            &network_type,
        );
        let wallet_dir =
            tempfile::TempDir::new().expect("the host provides a temporary wallet directory");
        let zingo_config = ClientConfig::builder()
            .set_indexer_uri(net.indexer_uri())
            .set_chain_type(net.chain_type())
            .set_wallet_dir(wallet_dir.path().to_path_buf())
            .set_wallet_config(WalletConfig::Ufvk {
                ufvk,
                birthday: MockNet::DEFAULT_BIRTHDAY,
                wallet_settings: default_test_wallet_settings(),
            })
            .build()
            .expect("every required field of the watch-only config is set above");
        wallet_dirs.push(wallet_dir);
        let mut watch_client = LightClient::new_nakednet_consented(zingo_config, false)
            .await
            .expect("a watch-only client opens from a unified full viewing key");
        // assert empty wallet before rescan
        let balance = watch_client
            .account_balance(AccountId::ZERO)
            .await
            .expect("a watch-only wallet holds account zero");
        check_expected_balance_with_fvks(fvks, balance, 0, 0, 0);
        watch_client
            .rescan_and_await()
            .await
            .expect("the mock chain serves every block a watch-only rescan requests");
        let balance = watch_client
            .account_balance(AccountId::ZERO)
            .await
            .expect("a watch-only wallet holds account zero");
        {
            let watch_wallet = watch_client.wallet().read().await;
            let ironwood_notes = watch_wallet.note_summaries::<IronwoodNote>(true);
            let orchard_notes = watch_wallet.note_summaries::<OrchardNote>(true);
            let sapling_notes = watch_wallet.note_summaries::<SaplingNote>(true);
            let transparent_coins = watch_wallet.coin_summaries(true);

            check_view_capability_bounds(
                &WatchWalletView {
                    balance: &balance,
                    unified_key_store: watch_wallet
                        .unified_key_store
                        .get(&AccountId::ZERO)
                        .expect("a watch-only wallet holds account zero"),
                    ironwood_notes: &ironwood_notes,
                    orchard_notes: &orchard_notes,
                    sapling_notes: &sapling_notes,
                    transparent_coins: &transparent_coins,
                },
                fvks,
                &all_fvks,
                &funded,
            );
        }

        watch_client
            .rescan_and_await()
            .await
            .expect("the mock chain serves every block a second watch-only rescan requests");
        assert!(matches!(
            from_inputs::quick_send(
                &mut watch_client,
                vec![(external_taddr.as_str(), UNSIGNABLE_PAYMENT, None)]
            )
            .await,
            Err(LightClientError::SendError(SendError::CalculateSendError(
                CalculateTransactionError::NoSpendingKey(_)
            )))
        ));
    }
}
