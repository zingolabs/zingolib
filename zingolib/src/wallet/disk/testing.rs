//! functionality for testing the save and load functions of LightWallet.
//! do not compile test-elevation feature for production.

use super::LightWallet;

impl LightWallet {
    /// connects a wallet to TestNet server.
    pub async fn unsafe_from_buffer_testnet(data: &[u8]) -> Self {
        let config = crate::config::ZingoConfig::create_testnet();
        Self::read_internal(data, &config)
            .await
            .map_err(|e| format!("Cannot deserialize LightWallet file!: {}", e))
            .unwrap()
    }
}

/// example wallets
/// including from different versions of the software.
pub mod examples;

/// tests
#[cfg(test)]
pub mod tests;
