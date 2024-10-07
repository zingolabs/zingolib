//! functionality for testing the save and load functions of LightWallet.
//! do not compile test-elevation feature for production.

use bip0039::Mnemonic;
use zcash_keys::keys::{Era, UnifiedSpendingKey};

use crate::wallet::keys::unified::UnifiedKeyStore;

use super::LightWallet;

impl LightWallet {
    /// connects a wallet to a local regtest node.
    pub async fn unsafe_from_buffer_regtest(data: &[u8]) -> Self {
        // this step starts a TestEnvironment and picks a new port!
        let lightwalletd_uri =
            crate::testutils::scenarios::setup::TestEnvironmentGenerator::new(None)
                .get_lightwalletd_uri();
        let config = crate::config::load_clientconfig(
            lightwalletd_uri,
            None,
            crate::config::ChainType::Regtest(crate::config::RegtestNetwork::all_upgrades_active()),
            true,
        )
        .unwrap();
        Self::read_internal(data, &config)
            .await
            .map_err(|e| format!("Cannot deserialize LightWallet file!: {}", e))
            .unwrap()
    }
    /// parses a wallet as an testnet wallet, aimed at a default testnet server
    pub async fn unsafe_from_buffer_testnet(data: &[u8]) -> Self {
        let config = crate::config::ZingoConfig::create_testnet();
        Self::read_internal(data, &config)
            .await
            .map_err(|e| format!("Cannot deserialize LightWallet file!: {}", e))
            .unwrap()
    }
    /// parses a wallet as an testnet wallet, aimed at a default testnet server
    pub async fn unsafe_from_buffer_mainnet(data: &[u8]) -> Self {
        let config = crate::config::ZingoConfig::create_mainnet();
        Self::read_internal(data, &config)
            .await
            .map_err(|e| format!("Cannot deserialize LightWallet file!: {}", e))
            .unwrap()
    }
}

// async fn assert_test_wallet(case: examples::LegacyWalletCase) {
//     let wallet = LightWallet::load_example_wallet(case).await;
// }

/// example wallets
/// including from different versions of the software.
pub mod examples;

/// tests
#[cfg(test)]
pub mod tests;

// test helper functions

/// asserts that a fresh capability generated with the seed matches the extant capability, which also can export the seed
pub async fn assert_wallet_capability_matches_seed(
    wallet: &LightWallet,
    expected_seed_phrase: String,
) {
    let actual_seed_phrase = wallet.get_seed_phrase().await.unwrap();
    assert_eq!(expected_seed_phrase, actual_seed_phrase);

    let expected_mnemonic = (
        Mnemonic::<bip0039::English>::from_phrase(expected_seed_phrase).unwrap(),
        0,
    );

    let expected_wc = crate::wallet::keys::unified::WalletCapability::new_from_phrase(
        &wallet.transaction_context.config,
        &expected_mnemonic.0,
        expected_mnemonic.1,
    )
    .unwrap();
    let wc = wallet.wallet_capability();

    // Compare USK
    let UnifiedKeyStore::Spend(usk) = &wc.unified_key_store() else {
        panic!("Expected Unified Spending Key");
    };
    assert_eq!(
        usk.to_bytes(Era::Orchard),
        UnifiedSpendingKey::try_from(expected_wc.unified_key_store())
            .unwrap()
            .to_bytes(Era::Orchard)
    );
}

/// basically does what it says on the tin
pub async fn assert_wallet_capability_contains_n_triple_pool_receivers(
    wallet: &LightWallet,
    expected_num_addresses: usize,
) {
    let wc = wallet.wallet_capability();

    assert_eq!(wc.addresses().len(), expected_num_addresses);
    for addr in wc.addresses().iter() {
        assert!(addr.orchard().is_some());
        assert!(addr.sapling().is_some());
        assert!(addr.transparent().is_some());
    }
}
