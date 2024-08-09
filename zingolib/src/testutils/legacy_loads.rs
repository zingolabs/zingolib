use crate::wallet::LightWallet;

/// i do not know the difference between these wallets but i will find out soon
/// what can these files do?
pub enum LegacyWalletCaseZingoV26 {
    /// /
    One,
    /// /
    Two,
}
/// an enumeration of cases to test
pub enum LegacyWalletCase {
    /// at this version, legacy testing began
    ZingoV26(LegacyWalletCaseZingoV26),
    /// ?
    ZingoV28,
}

/// loads test wallets
pub async fn load_legacy_wallet(case: LegacyWalletCase) -> LightWallet {
    match case {
        LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::One) => {
            LightWallet::unsafe_from_buffer_testnet(include_bytes!(
                "../../test-data/zingo-wallet-v26.dat"
            ))
        }
        LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::Two) => {
            LightWallet::unsafe_from_buffer_testnet(include_bytes!(
                "../../test-data/zingo-wallet-v26-2.dat"
            ))
        }
        LegacyWalletCase::ZingoV28 => LightWallet::unsafe_from_buffer_testnet(include_bytes!(
            "../../test-data/zingo-wallet-v28.dat"
        )),
    }
    .await
}
