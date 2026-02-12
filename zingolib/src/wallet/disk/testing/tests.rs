use bip0039::Mnemonic;

use zcash_keys::keys::Era;
use zcash_protocol::{PoolType, ShieldedProtocol};

use crate::{
    config::ZingoConfig,
    lightclient::LightClient,
    wallet::{
        LightWallet,
        disk::testing::{
            assert_wallet_capability_matches_seed,
            examples::{
                AbandonAbandonVersion, AbsurdAmountVersion, ChimneyBetterVersion,
                HospitalMuseumVersion, HotelHumorVersion, MainnetSeedVersion, MobileShuffleVersion,
                NetworkSeedVersion, RegtestSeedVersion, TestnetSeedVersion, VillageTargetVersion,
            },
        },
        keys::unified::UnifiedKeyStore,
    },
};

// moving toward completeness: each of these tests should assert everything known about the LightWallet without network.

impl NetworkSeedVersion {
    /// this is enough data to restore wallet from! thus, it is the bronze test for backward compatibility
    async fn load_example_wallet_with_verification(&self) -> LightClient {
        let client = self.load_example_wallet_with_client().await;
        let wallet = client.wallet.read().await;

        assert_wallet_capability_matches_seed(&wallet, self.example_wallet_base()).await;
        for pool in [
            PoolType::Transparent,
            PoolType::Shielded(ShieldedProtocol::Orchard),
        ] {
            assert_eq!(wallet.get_address(pool), self.example_wallet_address(pool));
        }
        drop(wallet);

        client
    }
}

#[tokio::test]
async fn verify_example_wallet_regtest_aaaaaaaaaaaaaaaaaaaaaaaa_v26() {
    NetworkSeedVersion::Regtest(RegtestSeedVersion::AbandonAbandon(
        AbandonAbandonVersion::V26,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_regtest_aadaalacaadaalacaadaalac_orch_and_sapl() {
    NetworkSeedVersion::Regtest(RegtestSeedVersion::AbsurdAmount(
        AbsurdAmountVersion::OrchAndSapl,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_regtest_aadaalacaadaalacaadaalac_orch_only() {
    NetworkSeedVersion::Regtest(RegtestSeedVersion::AbsurdAmount(
        AbsurdAmountVersion::OrchOnly,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_regtest_hmvasmuvwmssvichcarbpoct_v27() {
    NetworkSeedVersion::Regtest(RegtestSeedVersion::HospitalMuseum(
        HospitalMuseumVersion::V27,
    ))
    .load_example_wallet_with_verification()
    .await;
}
/// unlike other, more basic tests, this test also checks number of addresses and balance
#[ignore = "FIXME pepper sync needs unified address discovery"]
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_v26() {
    let client =
        NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(ChimneyBetterVersion::V26))
            .load_example_wallet_with_verification()
            .await;

    loaded_wallet_assert(
        client,
        zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED.to_string(),
        0,
        3,
    )
    .await;
}
/// unlike other, more basic tests, this test also checks number of addresses and balance
#[ignore = "test proves note has no index bug is a breaker"]
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_v27() {
    let wallet =
        NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(ChimneyBetterVersion::V27))
            .load_example_wallet_with_verification()
            .await;

    loaded_wallet_assert(
        wallet,
        zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED.to_string(),
        10177826,
        1,
    )
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_v28() {
    NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(ChimneyBetterVersion::V28))
        .load_example_wallet_with_verification()
        .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_cbbhrwiilgbrababsshsmtpr_g2f3830058() {
    NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(
        ChimneyBetterVersion::Latest,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_mskmgdbhotbpetcjwcspgopp_gab72a38b() {
    NetworkSeedVersion::Testnet(TestnetSeedVersion::MobileShuffle(
        MobileShuffleVersion::Gab72a38b,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_mskmgdbhotbpetcjwcspgopp_g93738061a() {
    NetworkSeedVersion::Testnet(TestnetSeedVersion::MobileShuffle(
        MobileShuffleVersion::G93738061a,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_mskmgdbhotbpetcjwcspgopp_ga74fed621() {
    NetworkSeedVersion::Testnet(TestnetSeedVersion::MobileShuffle(
        MobileShuffleVersion::Latest,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_testnet_glorygoddess() {
    NetworkSeedVersion::Testnet(TestnetSeedVersion::GloryGoddess)
        .load_example_wallet_with_verification()
        .await;
}
#[tokio::test]
async fn verify_example_wallet_mainnet_vtfcorfbcbpctcfupmegmwbp_v28() {
    NetworkSeedVersion::Mainnet(MainnetSeedVersion::VillageTarget(VillageTargetVersion::V28))
        .load_example_wallet_with_verification()
        .await;
}
#[tokio::test]
async fn verify_example_wallet_mainnet_hhcclaltpcckcsslpcnetblr_gf0aaf9347() {
    NetworkSeedVersion::Mainnet(MainnetSeedVersion::HotelHumor(
        HotelHumorVersion::Gf0aaf9347,
    ))
    .load_example_wallet_with_verification()
    .await;
}
#[tokio::test]
async fn verify_example_wallet_mainnet_hhcclaltpcckcsslpcnetblr_latest() {
    NetworkSeedVersion::Mainnet(MainnetSeedVersion::HotelHumor(HotelHumorVersion::Latest))
        .load_example_wallet_with_verification()
        .await;
}

async fn loaded_wallet_assert(
    mut lightclient: LightClient,
    expected_seed_phrase: String,
    expected_balance: u64,
    expected_num_addresses: usize,
) {
    {
        let wallet = lightclient.wallet.read().await;
        assert_wallet_capability_matches_seed(&wallet, expected_seed_phrase).await;

        assert_eq!(wallet.unified_addresses.len(), expected_num_addresses);
        for addr in wallet.unified_addresses.values() {
            assert!(addr.orchard().is_some());
            assert!(addr.sapling().is_some());
            assert!(addr.transparent().is_some());
        }

        let balance = lightclient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap();
        assert_eq!(
            balance.total_orchard_balance,
            Some(expected_balance.try_into().unwrap())
        );
    }
    if expected_balance > 0 {
        let sapling_address = crate::get_base_address_macro!(lightclient, "sapling");
        crate::testutils::lightclient::from_inputs::quick_send(
            &mut lightclient,
            vec![(&sapling_address, 11011, None)],
        )
        .await
        .unwrap();
        lightclient.sync_and_await().await.unwrap();
        let transparent_address = crate::get_base_address_macro!(lightclient, "transparent");
        crate::testutils::lightclient::from_inputs::quick_send(
            &mut lightclient,
            vec![(&transparent_address, 28000, None)],
        )
        .await
        .unwrap();
    }
}

// todo: proptest enum
#[tokio::test]
async fn reload_wallet_from_buffer() {
    use crate::wallet::WalletBase;
    use zingo_test_vectors::seeds::CHIMNEY_BETTER_SEED;

    let mid_client =
        NetworkSeedVersion::Testnet(TestnetSeedVersion::ChimneyBetter(ChimneyBetterVersion::V28))
            .load_example_wallet_with_verification()
            .await;
    let mid_client_network = mid_client.wallet.read().await.network;

    let mut mid_buffer: Vec<u8> = vec![];
    mid_client
        .wallet
        .write()
        .await
        .write(&mut mid_buffer, &mid_client.config.chain)
        .unwrap();

    let config = ZingoConfig::create_testnet();
    let client = LightClient::create_from_wallet(
        LightWallet::read(&mid_buffer[..], config.chain).unwrap(),
        config,
        true,
    )
    .unwrap();
    let wallet = client.wallet.read().await;

    let expected_mnemonic = Mnemonic::from_phrase(CHIMNEY_BETTER_SEED.to_string()).unwrap();

    let expected_keys = UnifiedKeyStore::new_from_mnemonic(
        &mid_client_network,
        &expected_mnemonic,
        zip32::AccountId::ZERO,
    )
    .unwrap();

    let UnifiedKeyStore::Spend(usk) = &wallet
        .unified_key_store
        .get(&zip32::AccountId::ZERO)
        .unwrap()
    else {
        panic!("should be spending key!")
    };
    let UnifiedKeyStore::Spend(expected_usk) = &expected_keys else {
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

    // TODO: there were 3 UAs associated with this wallet, we reset to 1 to ensure index is upheld correctly and
    // should thoroughly test UA discovery when syncing which should find these UAs again
    assert_eq!(wallet.unified_addresses.len(), 1);
    for addr in wallet.unified_addresses.values() {
        assert!(addr.orchard().is_some());
        assert!(addr.sapling().is_none());
        assert!(addr.transparent().is_none());
    }

    let ufvk = usk.to_unified_full_viewing_key();
    let ufvk_string = ufvk.encode(&wallet.network);
    let ufvk_base = WalletBase::Ufvk(ufvk_string.clone());
    let view_wallet = LightWallet::new(
        wallet.network,
        ufvk_base,
        wallet.birthday,
        wallet.wallet_settings.clone(),
    )
    .unwrap();
    let UnifiedKeyStore::View(v_ufvk) = &view_wallet
        .unified_key_store
        .get(&zip32::AccountId::ZERO)
        .unwrap()
    else {
        panic!("should be viewing key!");
    };
    let v_ufvk_string = v_ufvk.encode(&view_wallet.network);
    assert_eq!(ufvk_string, v_ufvk_string);

    // NOTE: removed balance check as need to sync to restore transaction data.
}
