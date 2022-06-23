use zcash_primitives::{
    consensus::{BlockHeight, NetworkUpgrade, Parameters, MAIN_NETWORK, TEST_NETWORK},
    constants,
};
pub const MAX_REORG: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Network {
    Testnet,
    Mainnet,
    FakeMainnet,
}

impl Network {
    pub fn hrp_orchard_spending_key(&self) -> &str {
        match self {
            Network::Mainnet => "secret-orchard-sk-main",
            Network::Testnet => "secret-orchard-sk-test",
            Network::FakeMainnet => "secret-orchard-sk-main",
        }
    }
    pub fn hrp_unified_full_viewing_key(&self) -> &str {
        match self {
            Network::Mainnet => "uview",
            Network::Testnet => "uviewtest",
            Network::FakeMainnet => "uview",
        }
    }
    pub fn to_zcash_address_network(&self) -> zcash_address::Network {
        match self {
            Network::Testnet => zcash_address::Network::Test,
            _ => zcash_address::Network::Main,
        }
    }
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use Network::*;
        let name = match self {
            Mainnet => "main",
            Testnet => "test",
            FakeMainnet => "regtest",
        };
        write!(f, "{name}")
    }
}

impl Parameters for Network {
    fn activation_height(&self, nu: NetworkUpgrade) -> Option<zcash_primitives::consensus::BlockHeight> {
        use Network::*;
        match self {
            Mainnet => MAIN_NETWORK.activation_height(nu),
            Testnet => TEST_NETWORK.activation_height(nu),
            FakeMainnet => {
                //Tests don't need to worry about NU5 yet
                match nu {
                    NetworkUpgrade::Nu5 => None,
                    _ => Some(BlockHeight::from_u32(1)),
                }
            }
        }
    }

    fn coin_type(&self) -> u32 {
        use Network::*;
        match self {
            Mainnet => constants::mainnet::COIN_TYPE,
            Testnet => constants::testnet::COIN_TYPE,
            FakeMainnet => constants::mainnet::COIN_TYPE,
        }
    }

    fn hrp_sapling_extended_spending_key(&self) -> &str {
        use Network::*;
        match self {
            Mainnet => constants::mainnet::HRP_SAPLING_EXTENDED_SPENDING_KEY,
            Testnet => constants::testnet::HRP_SAPLING_EXTENDED_SPENDING_KEY,
            FakeMainnet => constants::mainnet::HRP_SAPLING_EXTENDED_SPENDING_KEY,
        }
    }

    fn hrp_sapling_extended_full_viewing_key(&self) -> &str {
        use Network::*;
        match self {
            Mainnet => constants::mainnet::HRP_SAPLING_EXTENDED_FULL_VIEWING_KEY,
            Testnet => constants::testnet::HRP_SAPLING_EXTENDED_FULL_VIEWING_KEY,
            FakeMainnet => constants::mainnet::HRP_SAPLING_EXTENDED_FULL_VIEWING_KEY,
        }
    }

    fn hrp_sapling_payment_address(&self) -> &str {
        use Network::*;
        match self {
            Mainnet => constants::mainnet::HRP_SAPLING_PAYMENT_ADDRESS,
            Testnet => constants::testnet::HRP_SAPLING_PAYMENT_ADDRESS,
            FakeMainnet => constants::mainnet::HRP_SAPLING_PAYMENT_ADDRESS,
        }
    }

    fn b58_pubkey_address_prefix(&self) -> [u8; 2] {
        use Network::*;
        match self {
            Mainnet => constants::mainnet::B58_PUBKEY_ADDRESS_PREFIX,
            Testnet => constants::testnet::B58_PUBKEY_ADDRESS_PREFIX,
            FakeMainnet => constants::mainnet::B58_PUBKEY_ADDRESS_PREFIX,
        }
    }

    fn b58_script_address_prefix(&self) -> [u8; 2] {
        use Network::*;
        match self {
            Mainnet => constants::mainnet::B58_SCRIPT_ADDRESS_PREFIX,
            Testnet => constants::testnet::B58_SCRIPT_ADDRESS_PREFIX,
            FakeMainnet => constants::mainnet::B58_SCRIPT_ADDRESS_PREFIX,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
