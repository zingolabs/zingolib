//! functionality for testing the save and load functions of LightWallet.
//! do not compile test-elevation feature for production.

use super::LightWallet;

impl LightWallet {
    /// parses a wallet as an testnet wallet, aimed at a default testnet server
    pub async fn unsafe_from_buffer_testnet(data: &[u8]) -> Self {
        let config = crate::config::ZingoConfig::create_testnet();
        Self::read_internal(data, &config)
            .await
            .map_err(|e| format!("Cannot deserialize LightWallet file!: {}", e))
            .unwrap()
    }
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
