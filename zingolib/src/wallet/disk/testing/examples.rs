use super::super::LightWallet;

/// as opposed to [LegacyWalletCase], which enumerates test cases compiled from the history of zingo wallt tests, this ExampleWalletNetworkCase is meant to fully organize the set of test cases.
#[non_exhaustive]
#[derive(Clone)]
pub enum ExampleWalletNetworkCase {
    /// /
    Mainnet(ExampleMainnetWalletSeedCase),
    /// /
    Testnet(ExampleTestnetWalletSeedCase),
    /// /
    Regtest(ExampleRegtestWalletSeedCase),
    /// /
    Legacy(LegacyWalletCase),
}

/// /
#[non_exhaustive]
#[derive(Clone)]
pub enum ExampleMainnetWalletSeedCase {}
/// /
#[non_exhaustive]
#[derive(Clone)]
pub enum ExampleTestnetWalletSeedCase {}
/// /
#[non_exhaustive]
#[derive(Clone)]
pub enum ExampleRegtestWalletSeedCase {}

/// i do not know the difference between these wallets but i will find out soon
/// what can these files do?
#[non_exhaustive]
#[derive(Clone)]
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
#[derive(Clone)]
pub enum LegacyWalletCase {
    /// at this version, legacy testing began
    ZingoV26(LegacyWalletCaseZingoV26),
    /// ?
    ZingoV28,
    /// ...
    OldWalletReorgTestWallet,
}

/// loads test wallets
impl LightWallet {
    /// loads any one of the test wallets included in the examples
    pub async fn load_example_wallet(case: LegacyWalletCase) -> Self {
        match case {
            LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::One) => {
                LightWallet::unsafe_from_buffer_testnet(include_bytes!(
                    "examples/v26-1/zingo-wallet.dat"
                ))
                .await
            }
            LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::Two) => {
                LightWallet::unsafe_from_buffer_testnet(include_bytes!(
                    "examples/v26-2/zingo-wallet.dat"
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
            LegacyWalletCase::OldWalletReorgTestWallet => {
                LightWallet::unsafe_from_buffer_regtest(include_bytes!(
                    "examples/old_wallet_reorg_test_wallet/zingo-wallet.dat"
                ))
                .await
            }
        }
    }

    /// each wallet file has a saved balance
    pub fn example_expected_balance(case: LegacyWalletCase) -> u64 {
        match case {
            LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::One) => 0,
            LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::Two) => 10177826,
            LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::RegtestSapOnly) => todo!(),
            LegacyWalletCase::ZingoV28 => 10342837,
            LegacyWalletCase::OldWalletReorgTestWallet => todo!(),
        }
    }

    /// each wallet file has a saved balance
    pub fn example_expected_num_addresses(case: LegacyWalletCase) -> usize {
        match case {
            LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::One) => 3,
            LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::Two) => 1,
            LegacyWalletCase::ZingoV26(LegacyWalletCaseZingoV26::RegtestSapOnly) => todo!(),
            LegacyWalletCase::ZingoV28 => 3,
            LegacyWalletCase::OldWalletReorgTestWallet => todo!(),
        }
    }
}
