use pepper_sync::wallet::{IronwoodNote, OrchardNote, SaplingNote};
use zcash_address::unified::Fvk;
use zcash_protocol::value::Zatoshis;
use zcash_protocol::{PoolType, ShieldedPool};
use zip32::AccountId;

use super::{HOSPITAL_MUSEUM, fund};
use crate::check_client_balances;
use crate::config::{ClientConfig, WalletConfig};
use crate::lightclient::LightClient;
use crate::lightclient::error::{LightClientError, SendError};
use crate::testutils::lightclient::{from_inputs, get_base_address};
use crate::testutils::mock_indexer::MockNet;
use crate::testutils::{build_fvks_from_unified_keystore, default_test_wallet_settings, tempfile};
use crate::wallet::balance::AccountBalance;
use crate::wallet::error::CalculateTransactionError;
use crate::wallet::keys::unified::UnifiedKeyStore;
use crate::wallet::summary::data::{CoinSummary, NoteSummary};

const TRANSPARENT_FUNDING: u64 = 10_000;
const SAPLING_FUNDING: u64 = 20_000;
const UNIFIED_FUNDING: u64 = 30_000;
const WATCH_ONLY_BIRTHDAY: u32 = 1;

fn check_expected_balance_with_fvks(
    fvks: &[&Fvk],
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
            Fvk::Sapling(_) => {
                assert_eq!(zatoshis(balance.total_sapling_balance), s_expect);
                assert_eq!(zatoshis(balance.confirmed_sapling_balance), s_expect);
                assert_eq!(zatoshis(balance.unconfirmed_sapling_balance), s_expect);
            }
            Fvk::Orchard(_) => {
                assert_eq!(zatoshis(balance.total_orchard_balance), o_expect);
                assert_eq!(zatoshis(balance.confirmed_orchard_balance), o_expect);
                assert_eq!(zatoshis(balance.unconfirmed_orchard_balance), o_expect);
            }
            Fvk::P2pkh(_) => {
                assert_eq!(zatoshis(balance.confirmed_transparent_balance), t_expect);
            }
            _ => panic!("the keystore builds orchard, sapling and transparent keys only"),
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
        assert!(ufvk.p2pkh().is_some());
        assert_eq!(balance.confirmed_transparent_balance, sent_t_value);
        assert_eq!(transparent_coins.len(), 1);
    } else {
        assert!(ufvk.p2pkh().is_none());
        assert_eq!(balance.confirmed_transparent_balance, None);
        assert_eq!(transparent_coins.len(), 0);
    }
}

fn zatoshis(value: u64) -> Zatoshis {
    Zatoshis::from_u64(value).expect("test values stay within the money range")
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
    let mut original_recipient = net.client(HOSPITAL_MUSEUM, None).await;

    let (recipient_taddr, recipient_sapling, recipient_unified) = (
        get_base_address(&original_recipient, PoolType::Transparent).await,
        get_base_address(
            &original_recipient,
            PoolType::Shielded(ShieldedPool::Sapling),
        )
        .await,
        get_base_address(
            &original_recipient,
            PoolType::Shielded(ShieldedPool::Orchard),
        )
        .await,
    );
    // 1. and 2. one funding transaction containing all types of outputs
    net.chain.write().await.mine_empty_blocks(1);
    fund(
        &net,
        vec![
            (recipient_taddr.as_str(), TRANSPARENT_FUNDING, None),
            (recipient_sapling.as_str(), SAPLING_FUNDING, None),
            (recipient_unified.as_str(), UNIFIED_FUNDING, None),
        ],
        1,
    )
    .await;
    original_recipient
        .sync_and_await()
        .await
        .expect("the original recipient syncs its funding");
    let original_recipient_balance = original_recipient
        .account_balance(AccountId::ZERO)
        .await
        .expect("a synced client reports a balance");
    let sent_t_value = zatoshis(TRANSPARENT_FUNDING);
    let sent_s_value = zatoshis(SAPLING_FUNDING);
    // The unified-address payment lands in the Ironwood pool (ADR 0009).
    let sent_o_value = zatoshis(0);
    let sent_i_value = zatoshis(UNIFIED_FUNDING);
    assert_eq!(
        original_recipient_balance.confirmed_transparent_balance,
        Some(sent_t_value)
    );
    assert_eq!(
        original_recipient_balance.total_sapling_balance,
        Some(sent_s_value)
    );
    assert_eq!(
        original_recipient_balance.total_orchard_balance,
        Some(sent_o_value)
    );
    assert_eq!(
        original_recipient_balance.total_ironwood_balance,
        Some(sent_i_value)
    );

    // check that do_rescan works
    original_recipient
        .rescan_and_await()
        .await
        .expect("a rescan from the birthday completes");
    check_client_balances!(original_recipient, i: UNIFIED_FUNDING o: 0 s: SAPLING_FUNDING t: TRANSPARENT_FUNDING);

    // Extract viewing keys
    let original_wallet = original_recipient.wallet().read().await;
    let [o_fvk, s_fvk, t_fvk] = build_fvks_from_unified_keystore(
        original_wallet
            .unified_key_store
            .get(&AccountId::ZERO)
            .expect("the seed wallet holds account zero"),
    );
    let fvks_sets = [
        vec![&o_fvk],
        vec![&s_fvk],
        vec![&o_fvk, &s_fvk],
        vec![&o_fvk, &t_fvk],
        vec![&s_fvk, &t_fvk],
        vec![&o_fvk, &s_fvk, &t_fvk],
    ];
    let mut wallet_dirs = Vec::new();
    for fvks in &fvks_sets {
        tracing::info!("testing UFVK containing:");
        tracing::info!("    orchard fvk: {}", fvks.contains(&&o_fvk));
        tracing::info!("    sapling fvk: {}", fvks.contains(&&s_fvk));
        tracing::info!("    transparent fvk: {}", fvks.contains(&&t_fvk));

        let ufvk = zcash_address::unified::Encoding::encode(
            &<zcash_address::unified::Ufvk as zcash_address::unified::Encoding>::try_from_items(
                zcash_address::unified::Revision::R0,
                fvks.iter()
                    .copied()
                    .cloned()
                    .map(zcash_address::unified::Uitem::Data)
                    .collect(),
            )
            .expect("every subset of the keystore's keys forms a unified full viewing key"),
            &zcash_protocol::consensus::NetworkType::Regtest,
        );
        let wallet_dir =
            tempfile::TempDir::new().expect("the host provides a temporary wallet directory");
        let zingo_config = ClientConfig::builder()
            .set_indexer_uri(net.indexer_uri())
            .set_chain_type(net.chain_type())
            .set_wallet_dir(wallet_dir.path().to_path_buf())
            .set_wallet_config(WalletConfig::Ufvk {
                ufvk,
                birthday: WATCH_ONLY_BIRTHDAY,
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
            .expect("a fresh watch-only client reports a balance");
        check_expected_balance_with_fvks(fvks, balance, 0, 0, 0);
        watch_client
            .rescan_and_await()
            .await
            .expect("a watch-only rescan from the birthday completes");
        let balance = watch_client
            .account_balance(AccountId::ZERO)
            .await
            .expect("a rescanned watch-only client reports a balance");
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
                    .get(&AccountId::ZERO)
                    .expect("the watch-only wallet holds account zero"),
                fvks,
                &o_fvk,
                &s_fvk,
                &t_fvk,
                Some(sent_i_value),
                Some(sent_o_value),
                Some(sent_s_value),
                Some(sent_t_value),
                &ironwood_notes,
                &orchard_notes,
                &sapling_notes,
                &transparent_coin,
            );
        }

        watch_client
            .rescan_and_await()
            .await
            .expect("a second watch-only rescan completes");
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
