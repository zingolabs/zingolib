use bip0039::Mnemonic;

use zcash_client_backend::PoolType;
use zcash_client_backend::ShieldedProtocol;
use zcash_keys::keys::Era;

use crate::lightclient::LightClient;
use crate::wallet::keys::unified::UnifiedKeyStore;

use super::super::LightWallet;
use super::assert_wallet_capability_matches_seed;

use super::examples::NetworkSeedVersion;
use super::examples::NetworkSeedVersion::Mainnet;
use super::examples::NetworkSeedVersion::Regtest;
use super::examples::NetworkSeedVersion::Testnet;

use super::examples::MainnetSeedVersion::HHCCLALTPCCKCSSLPCNETBLR;
use super::examples::MainnetSeedVersion::VTFCORFBCBPCTCFUPMEGMWBP;
use super::examples::RegtestSeedVersion::AAAAAAAAAAAAAAAAAAAAAAAA;
use super::examples::RegtestSeedVersion::AADAALACAADAALACAADAALAC;
use super::examples::RegtestSeedVersion::HMVASMUVWMSSVICHCARBPOCT;
use super::examples::TestnetSeedVersion::CBBHRWIILGBRABABSSHSMTPR;
use super::examples::TestnetSeedVersion::MSKMGDBHOTBPETCJWCSPGOPP;

use super::examples::AAAAAAAAAAAAAAAAAAAAAAAAVersion;
use super::examples::AADAALACAADAALACAADAALACVersion;
use super::examples::CBBHRWIILGBRABABSSHSMTPRVersion;
use super::examples::HHCCLALTPCCKCSSLPCNETBLRVersion;
use super::examples::HMVASMUVWMSSVICHCARBPOCTVersion;
use super::examples::MSKMGDBHOTBPETCJWCSPGOPPVersion;
use super::examples::VTFCORFBCBPCTCFUPMEGMWBPVersion;

// moving toward completeness: each of these tests should assert everything known about the LightWallet without network.

impl NetworkSeedVersion {
    /// this is enough data to restore wallet from! thus, it is the bronze test for backward compatibility
    async fn load_example_wallet_with_verification(&self) -> LightWallet {
        let wallet = self.load_example_wallet().await;
        assert_wallet_capability_matches_seed(&wallet, self.example_wallet_base()).await;
        for pool in [
            PoolType::Transparent,
            PoolType::Shielded(ShieldedProtocol::Sapling),
            PoolType::Shielded(ShieldedProtocol::Orchard),
        ] {
            assert_eq!(
                wallet
                    .get_first_address(pool)
                    .expect("can find the first address"),
                self.example_wallet_address(pool)
            );
        }
        wallet
    }
}

#[tokio::test]
async fn verify_example_wallet_regtest_aaaaaaaaaaaaaaaaaaaaaaaa_v26() {
    Regtest(AAAAAAAAAAAAAAAAAAAAAAAA(
        AAAAAAAAAAAAAAAAAAAAAAAAVersion::V26,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_regtest_aadaalacaadaalacaadaalac_orch_and_sapl() {
    Regtest(AADAALACAADAALACAADAALAC(
        AADAALACAADAALACAADAALACVersion::OrchAndSapl,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_regtest_aadaalacaadaalacaadaalac_orch_only() {
    Regtest(AADAALACAADAALACAADAALAC(
        AADAALACAADAALACAADAALACVersion::OrchOnly,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_regtest_hmvasmuvwmssvichcarbpoct_v27() {
    Regtest(HMVASMUVWMSSVICHCARBPOCT(
        HMVASMUVWMSSVICHCARBPOCTVersion::V27,
    ))
    .load_example_wallet_with_verification()
    .await;
}
/// unlike other, more basic tests, this test also checks number of addresses and balance
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_v26() {
    let wallet = Testnet(CBBHRWIILGBRABABSSHSMTPR(
        CBBHRWIILGBRABABSSHSMTPRVersion::V26,
    ))
    .load_example_wallet_with_verification()
    .await;

    loaded_wallet_assert(
        wallet,
        crate::testvectors::seeds::CHIMNEY_BETTER_SEED.to_string(),
        0,
        3,
    )
    .await;
}
/// unlike other, more basic tests, this test also checks number of addresses and balance
#[ignore = "test proves note has no index bug is a breaker"]
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_v27() {
    let wallet = Testnet(CBBHRWIILGBRABABSSHSMTPR(
        CBBHRWIILGBRABABSSHSMTPRVersion::V27,
    ))
    .load_example_wallet_with_verification()
    .await;

    loaded_wallet_assert(
        wallet,
        crate::testvectors::seeds::CHIMNEY_BETTER_SEED.to_string(),
        10177826,
        1,
    )
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_v28() {
    Testnet(CBBHRWIILGBRABABSSHSMTPR(
        CBBHRWIILGBRABABSSHSMTPRVersion::V28,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_g2f3830058() {
    Testnet(CBBHRWIILGBRABABSSHSMTPR(
        CBBHRWIILGBRABABSSHSMTPRVersion::G2f3830058,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_mskmgdbhotbpetcjwcspgopp_gab72a38b() {
    Testnet(MSKMGDBHOTBPETCJWCSPGOPP(
        MSKMGDBHOTBPETCJWCSPGOPPVersion::Gab72a38b,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_mskmgdbhotbpetcjwcspgopp_g93738061a() {
    Testnet(MSKMGDBHOTBPETCJWCSPGOPP(
        MSKMGDBHOTBPETCJWCSPGOPPVersion::G93738061a,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_mskmgdbhotbpetcjwcspgopp_ga74fed621() {
    Testnet(MSKMGDBHOTBPETCJWCSPGOPP(
        MSKMGDBHOTBPETCJWCSPGOPPVersion::Ga74fed621,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_mainnet_vtfcorfbcbpctcfupmegmwbp_v28() {
    Mainnet(VTFCORFBCBPCTCFUPMEGMWBP(
        VTFCORFBCBPCTCFUPMEGMWBPVersion::V28,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_mainnet_hhcclaltpcckcsslpcnetblr_gf0aaf9347() {
    Mainnet(HHCCLALTPCCKCSSLPCNETBLR(
        HHCCLALTPCCKCSSLPCNETBLRVersion::Gf0aaf9347,
    ))
    .load_example_wallet_with_verification()
    .await;
}

async fn loaded_wallet_assert(
    wallet: LightWallet,
    expected_seed_phrase: String,
    expected_balance: u64,
    expected_num_addresses: usize,
) {
    assert_wallet_capability_matches_seed(&wallet, expected_seed_phrase).await;

    let wc = wallet.wallet_capability();
    assert_eq!(wc.addresses().len(), expected_num_addresses);
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
        crate::testutils::lightclient::from_inputs::quick_send(
            &client,
            vec![(
                &crate::get_base_address_macro!(client, "sapling"),
                11011,
                None,
            )],
        )
        .await
        .unwrap();
        client.do_sync(true).await.unwrap();
        crate::testutils::lightclient::from_inputs::quick_send(
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

// todo: proptest enum
#[tokio::test]
async fn reload_wallet_from_buffer() {
    use crate::testvectors::seeds::CHIMNEY_BETTER_SEED;
    use crate::wallet::WalletBase;
    use crate::wallet::WalletCapability;

    let mid_wallet = Testnet(CBBHRWIILGBRABABSSHSMTPR(
        CBBHRWIILGBRABABSSHSMTPRVersion::V28,
    ))
    .load_example_wallet_with_verification()
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

    let expected_wc = WalletCapability::new_from_phrase(
        &mid_client.wallet.transaction_context.config,
        &expected_mnemonic.0,
        expected_mnemonic.1,
    )
    .unwrap();
    let wc = wallet.wallet_capability();

    let UnifiedKeyStore::Spend(usk) = wc.unified_key_store() else {
        panic!("should be spending key!")
    };
    let UnifiedKeyStore::Spend(expected_usk) = expected_wc.unified_key_store() else {
        panic!("should be spending key!")
    };

    assert_eq!(
        usk.to_bytes(Era::Orchard),
        expected_usk.to_bytes(Era::Orchard)
    );
    assert_eq!(usk.orchard().to_bytes(), expected_usk.orchard().to_bytes());
    assert_eq!(usk.sapling().to_bytes(), expected_usk.sapling().to_bytes());
    assert_eq!(
        usk.transparent().to_bytes(),
        expected_usk.transparent().to_bytes()
    );

    assert_eq!(wc.addresses().len(), 3);
    for addr in wc.addresses().iter() {
        assert!(addr.orchard().is_some());
        assert!(addr.sapling().is_some());
        assert!(addr.transparent().is_some());
    }

    let ufvk = usk.to_unified_full_viewing_key();
    let ufvk_string = ufvk.encode(&wallet.transaction_context.config.chain);
    let ufvk_base = WalletBase::Ufvk(ufvk_string.clone());
    let view_wallet = LightWallet::new(
        wallet.transaction_context.config.clone(),
        ufvk_base,
        wallet.get_birthday().await,
    )
    .unwrap();
    let v_wc = view_wallet.wallet_capability();
    let UnifiedKeyStore::View(v_ufvk) = v_wc.unified_key_store() else {
        panic!("should be viewing key!");
    };
    let v_ufvk_string = v_ufvk.encode(&wallet.transaction_context.config.chain);
    assert_eq!(ufvk_string, v_ufvk_string);

    let client = LightClient::create_from_wallet_async(wallet).await.unwrap();
    let balance = client.do_balance().await;
    assert_eq!(balance.orchard_balance, Some(10342837));
}
