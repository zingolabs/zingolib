use zcash_address::unified::Encoding;
use zcash_primitives::zip339::Mnemonic;

use crate::get_base_address_macro;
use crate::lightclient::LightClient;

use super::super::LightWallet;

use super::examples::ExampleWalletNetwork::Mainnet;
use super::examples::ExampleWalletNetwork::Regtest;
use super::examples::ExampleWalletNetwork::Testnet;

use super::examples::ExampleMainnetWalletSeed::VTFCORFBCBPCTCFUPMEGMWBP;
// use super::examples::ExampleRegtestWalletSeed::AAAAAAAAAAAAAAAAAAAAAAAA;
use super::examples::ExampleRegtestWalletSeed::HMVASMUVWMSSVICHCARBPOCT;
use super::examples::ExampleTestnetWalletSeed::CBBHRWIILGBRABABSSHSMTPR;
use super::examples::ExampleTestnetWalletSeed::MSKMGDBHOTBPETCJWCSPGOPP;

// use super::examples::ExampleAAAAAAAAAAAAAAAAAAAAAAAAWalletVersion;
use super::examples::ExampleCBBHRWIILGBRABABSSHSMTPRWalletVersion;
use super::examples::ExampleHMVASMUVWMSSVICHCARBPOCTWalletVersion;
use super::examples::ExampleMSKMGDBHOTBPETCJWCSPGOPPWalletVersion;
use super::examples::ExampleVTFCORFBCBPCTCFUPMEGMWBPWalletVersion;

// moving toward completeness: each of these tests should assert everything known about the LightWallet without network.

#[tokio::test]
async fn verify_example_wallet_regtest_hmvasmuvwmssvichcarbpoct_v27() {
    let _wallet = LightWallet::load_example_wallet(Regtest(HMVASMUVWMSSVICHCARBPOCT(
        ExampleHMVASMUVWMSSVICHCARBPOCTWalletVersion::V27,
    )))
    .await;
}
// #[ignore = "test fails because ZFZ panics in regtest"]
// #[tokio::test]
// async fn verify_example_wallet_regtest_aaaaaaaaaaaaaaaaaaaaaaaa_v26() {
//     let wallet = LightWallet::load_example_wallet(Regtest(AAAAAAAAAAAAAAAAAAAAAAAA(
//         ExampleAAAAAAAAAAAAAAAAAAAAAAAAWalletVersion::V26,
//     )))
//     .await;

//     loaded_wallet_assert(
//         wallet,
//         crate::testvectors::seeds::ABANDON_ART_SEED.to_string(),
//         10342837,
//         3,
//     )
//     .await;
// }

#[tokio::test]
async fn verify_example_wallet_testnet_mskmgdbhotbpetcjwcspgopp_gab72a38b() {
    let _wallet = LightWallet::load_example_wallet(Testnet(MSKMGDBHOTBPETCJWCSPGOPP(
        ExampleMSKMGDBHOTBPETCJWCSPGOPPWalletVersion::Gab72a38b,
    )))
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_v26() {
    let wallet = LightWallet::load_example_wallet(Testnet(CBBHRWIILGBRABABSSHSMTPR(
        ExampleCBBHRWIILGBRABABSSHSMTPRWalletVersion::V26,
    )))
    .await;

    assert_wallet_capability_matches_seed_address_number(
        &wallet,
        crate::testvectors::seeds::CHIMNEY_BETTER_SEED.to_string(),
        3,
    )
    .await;

    loaded_wallet_assert(wallet, 0).await;
}
// #[ignore = "test proves note has no index bug is a breaker"]
// #[tokio::test]
// async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_v27() {
//     let wallet = LightWallet::load_example_wallet(Testnet(CBBHRWIILGBRABABSSHSMTPR(
//         ExampleCBBHRWIILGBRABABSSHSMTPRWalletVersion::V27,
//     )))
//     .await;

//     loaded_wallet_assert(
//         wallet,
//         crate::testvectors::seeds::CHIMNEY_BETTER_SEED.to_string(),
//         10177826,
//         1,
//     )
//     .await;
// }
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_v28() {
    let _wallet = LightWallet::load_example_wallet(Testnet(CBBHRWIILGBRABABSSHSMTPR(
        ExampleCBBHRWIILGBRABABSSHSMTPRWalletVersion::V28,
    )))
    .await;
}
#[tokio::test]
async fn verify_example_wallet_mainnet_vtfcorfbcbpctcfupmegmwbp_v28() {
    let _wallet = LightWallet::load_example_wallet(Mainnet(VTFCORFBCBPCTCFUPMEGMWBP(
        ExampleVTFCORFBCBPCTCFUPMEGMWBPWalletVersion::V28,
    )))
    .await;
}

async fn assert_wallet_capability_matches_seed_address_number(
    wallet: &LightWallet,
    expected_seed_phrase: String,
    expected_num_addresses: usize,
) {
    let expected_mnemonic = (
        zcash_primitives::zip339::Mnemonic::from_phrase(expected_seed_phrase).unwrap(),
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

    assert_eq!(wc.addresses().len(), expected_num_addresses);
    for addr in wc.addresses().iter() {
        assert!(addr.orchard().is_some());
        assert!(addr.sapling().is_some());
        assert!(addr.transparent().is_some());
    }
}

async fn loaded_wallet_assert(wallet: LightWallet, expected_balance: u64) {
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
    dbg!(client.do_seed_phrase().await.unwrap());
}

#[tokio::test]
async fn reload_wallet_from_buffer() {
    use zcash_primitives::consensus::Parameters;

    use crate::testvectors::seeds::CHIMNEY_BETTER_SEED;
    use crate::wallet::disk::Capability;
    use crate::wallet::keys::extended_transparent::ExtendedPrivKey;
    use crate::wallet::WalletBase;
    use crate::wallet::WalletCapability;

    let mid_wallet = LightWallet::load_example_wallet(Testnet(CBBHRWIILGBRABABSSHSMTPR(
        ExampleCBBHRWIILGBRABABSSHSMTPRWalletVersion::V28,
    )))
    .await;

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
