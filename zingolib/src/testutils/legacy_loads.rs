use crate::wallet::LightWallet;

/// an enumeration of cases to test
pub enum LegacyWalletCase {
    /// at this version, legacy testing began
    ZingoV26,
}

/// loads test wallets
pub async fn load_legacy_wallet(case: LegacyWalletCase) -> LightWallet {
    let wallet_data = match case {
        LegacyWalletCase::ZingoV26 => include_bytes!("../../test-data/zingo-wallet-v26.dat"),
    };
    LightWallet::unsafe_from_buffer_testnet(wallet_data).await
}
