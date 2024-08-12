use super::super::LightWallet;

/// i do not know the difference between these wallets but i will find out soon
/// what can these files do?
#[non_exhaustive]
pub enum LegacyWalletCaseZingoV26 {
    /// /
    One,
    /// /
    Two,
    /// regtest sap only wallet
    RegtestSapOnly,
}
/// an enumeration of cases to test
#[non_exhaustive]
pub enum LegacyWalletCase {
    /// at this version, legacy testing began
    ZingoV26(LegacyWalletCaseZingoV26),
    /// ?
    ZingoV28,
}

/// loads test wallets
impl LightWallet {
    /// loads any one of the test wallets included in the examples
    pub async fn load_example_wallet(case: LegacyWalletCase) -> Self {
        match case {
            LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::One) => {
                LightWallet::unsafe_from_buffer_testnet(include_bytes!(
                    "examples/zingo-wallet-v26.dat"
                ))
                .await
            }
            LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::Two) => {
                LightWallet::unsafe_from_buffer_testnet(include_bytes!(
                    "examples/zingo-wallet-v26.dat"
                ))
                .await
            }
            LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::RegtestSapOnly) => {
                LightWallet::unsafe_from_buffer_regtest(include_bytes!(
                    "examples/v26/202302_release/regtest/sap_only/zingo-wallet.dat"
                ))
                .await
            }
            LegacyWalletCase::ZingoV28 => {
                LightWallet::unsafe_from_buffer_testnet(include_bytes!(
                    "examples/zingo-wallet-v28.dat"
                ))
                .await
            }
        }
    }
}
