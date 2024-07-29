//! functionality for testing the save and load functions of LightWallet.
//! do not compile test-elevation feature for production.

impl super::LightWallet {
    /// connects a wallet to TestNet server.
    pub async fn unsafe_from_buffer_testnet(data: &[u8]) -> Self {
        let config = zingoconfig::ZingoConfig::create_testnet();
        Self::read_internal(data, &config)
            .await
            .map_err(|e| format!("Cannot deserialize LightWallet file!: {}", e))
            .unwrap()
    }
}
