//! functionality for testing the save and load functions of LightWallet.
//! do not compile test-elevation feature for production.

use bip0039::Mnemonic;
use zcash_keys::keys::{Era, UnifiedSpendingKey};

use crate::{config::ChainType, wallet::keys::unified::UnifiedKeyStore};

use super::LightWallet;

impl LightWallet {
    /// connects a wallet to a local regtest node.
    pub fn unsafe_from_buffer(network: ChainType, data: &[u8]) -> Self {
        Self::read(data, network)
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

    let expected_keys = crate::wallet::keys::unified::UnifiedKeyStore::new_from_mnemonic(
        &wallet.network,
        &expected_mnemonic.0,
        expected_mnemonic.1,
    )
    .unwrap();

    // Compare USK
    let UnifiedKeyStore::Spend(usk) = &wallet.unified_key_store else {
        panic!("Expected Unified Spending Key");
    };
    assert_eq!(
        usk.to_bytes(Era::Orchard),
        UnifiedSpendingKey::try_from(&expected_keys)
            .unwrap()
            .to_bytes(Era::Orchard)
    );
}

/// basically does what it says on the tin
pub async fn assert_wallet_capability_contains_n_triple_pool_receivers(
    wallet: &LightWallet,
    expected_num_addresses: usize,
) {
    assert_eq!(wallet.unified_addresses.len(), expected_num_addresses);
    for addr in wallet.unified_addresses.values() {
        assert!(addr.orchard().is_some());
        assert!(addr.sapling().is_some());
        assert!(addr.transparent().is_some());
    }
}
