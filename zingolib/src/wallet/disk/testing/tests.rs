use zcash_address::unified::Encoding;
use zcash_primitives::zip339::Mnemonic;

use crate::get_base_address_macro;
use crate::lightclient::LightClient;

use super::super::LightWallet;

use super::examples::load_legacy_wallet;
use super::examples::LegacyWalletCase;
use super::examples::LegacyWalletCaseZingoV26;

async fn loaded_wallet_assert(wallet: LightWallet, expected_balance: u64, num_addresses: usize) {
    let expected_mnemonic = (
        zcash_primitives::zip339::Mnemonic::from_phrase(
            crate::testvectors::seeds::CHIMNEY_BETTER_SEED.to_string(),
        )
        .unwrap(),
        0,
    );
    assert_eq!(wallet.mnemonic(), Some(&expected_mnemonic));

    let expected_wc = crate::wallet::keys::unified::WalletCapability::new_from_phrase(
        &wallet.transaction_context.config,
        &expected_mnemonic.0,
        expected_mnemonic.1,
    )
    .unwrap();
    let wc = wallet.wallet_capability();

    // We don't want the WalletCapability to impl. `Eq` (because it stores secret keys)
    // so we have to compare each component instead

    // Compare Orchard
    let crate::wallet::keys::unified::Capability::Spend(orchard_sk) = &wc.orchard else {
        panic!("Expected Orchard Spending Key");
    };
    assert_eq!(
        orchard_sk.to_bytes(),
        orchard::keys::SpendingKey::try_from(&expected_wc)
            .unwrap()
            .to_bytes()
    );

    // Compare Sapling
    let crate::wallet::keys::unified::Capability::Spend(sapling_sk) = &wc.sapling else {
        panic!("Expected Sapling Spending Key");
    };
    assert_eq!(
        sapling_sk,
        &zcash_client_backend::keys::sapling::ExtendedSpendingKey::try_from(&expected_wc).unwrap()
    );

    // Compare transparent
    let crate::wallet::keys::unified::Capability::Spend(transparent_sk) = &wc.transparent else {
        panic!("Expected transparent extended private key");
    };
    assert_eq!(
        transparent_sk,
        &crate::wallet::keys::extended_transparent::ExtendedPrivKey::try_from(&expected_wc)
            .unwrap()
    );

    assert_eq!(wc.addresses().len(), num_addresses);
    for addr in wc.addresses().iter() {
        assert!(addr.orchard().is_some());
        assert!(addr.sapling().is_some());
        assert!(addr.transparent().is_some());
    }

    let client = crate::lightclient::LightClient::create_from_wallet_async(wallet)
        .await
        .unwrap();
    let balance = client.do_balance().await;
    assert_eq!(balance.orchard_balance, Some(expected_balance));
    if expected_balance > 0 {
        let _ = crate::testutils::lightclient::from_inputs::quick_send(
            &client,
            vec![(&get_base_address_macro!(client, "sapling"), 11011, None)],
        )
        .await
        .unwrap();
        let _ = client.do_sync(true).await.unwrap();
        let _ = crate::testutils::lightclient::from_inputs::quick_send(
            &client,
            vec![(
                &crate::get_base_address_macro!(client, "transparent"),
                28000,
                None,
            )],
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn load_and_parse_different_wallet_versions() {
    let _loaded_wallet = load_legacy_wallet(LegacyWalletCase::ZingoV26(
        LegacyWalletCaseZingoV26::RegtestSapOnly,
    ))
    .await;
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
    let wallet =
        load_legacy_wallet(LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::One)).await;

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
    let wallet =
        load_legacy_wallet(LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::Two)).await;

    loaded_wallet_assert(wallet, 10177826, 1).await;
}

#[ignore = "flakey test"]
#[tokio::test]
async fn load_wallet_from_v28_dat_file() {
    // We test that the LightWallet can be read from v28 .dat file
    // --seed "chimney better bulb horror rebuild whisper improve intact letter giraffe brave rib appear bulk aim burst snap salt hill sad merge tennis phrase raise"
    // with 3 addresses containing all receivers.
    let wallet = load_legacy_wallet(LegacyWalletCase::ZingoV28).await;

    loaded_wallet_assert(wallet, 10342837, 3).await;
}

#[tokio::test]
async fn reload_wallet_from_buffer() {
    use zcash_primitives::consensus::Parameters;

    use crate::testvectors::seeds::CHIMNEY_BETTER_SEED;
    use crate::wallet::disk::Capability;
    use crate::wallet::keys::extended_transparent::ExtendedPrivKey;
    use crate::wallet::WalletBase;
    use crate::wallet::WalletCapability;

    // We test that the LightWallet can be read from v28 .dat file
    // A testnet wallet initiated with
    // --seed "chimney better bulb horror rebuild whisper improve intact letter giraffe brave rib appear bulk aim burst snap salt hill sad merge tennis phrase raise"
    // --birthday 0
    // --nosync
    // with 3 addresses containing all receivers.
    let mid_wallet = load_legacy_wallet(LegacyWalletCase::ZingoV28).await;

    let mid_client = LightClient::create_from_wallet_async(mid_wallet)
        .await
        .unwrap();
    let mid_buffer = mid_client.export_save_buffer_async().await.unwrap();
    let wallet = LightWallet::read_internal(
        &mid_buffer[..],
        &mid_client.wallet.transaction_context.config,
    )
    .await
    .map_err(|e| format!("Cannot deserialize rebuffered LightWallet: {}", e))
    .unwrap();
    let expected_mnemonic = (
        Mnemonic::from_phrase(CHIMNEY_BETTER_SEED.to_string()).unwrap(),
        0,
    );
    assert_eq!(wallet.mnemonic(), Some(&expected_mnemonic));

    let expected_wc = WalletCapability::new_from_phrase(
        &mid_client.wallet.transaction_context.config,
        &expected_mnemonic.0,
        expected_mnemonic.1,
    )
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
        &zcash_client_backend::keys::sapling::ExtendedSpendingKey::try_from(&expected_wc).unwrap()
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
    let ufvk_string = ufvk.encode(&wallet.transaction_context.config.chain.network_type());
    let ufvk_base = WalletBase::Ufvk(ufvk_string.clone());
    let view_wallet = LightWallet::new(
        wallet.transaction_context.config.clone(),
        ufvk_base,
        wallet.get_birthday().await,
    )
    .unwrap();
    let v_wc = view_wallet.wallet_capability();
    let vv = v_wc.ufvk().unwrap();
    let vv_string = vv.encode(&wallet.transaction_context.config.chain.network_type());
    assert_eq!(ufvk_string, vv_string);

    let client = LightClient::create_from_wallet_async(wallet).await.unwrap();
    let balance = client.do_balance().await;
    assert_eq!(balance.orchard_balance, Some(10342837));
}
