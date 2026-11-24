use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::BlockHeight;
use zingo_common_components::protocol::ActivationHeights;

/// The network types a lightclient can connect to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainType {
    /// Mainnet
    Mainnet,
    /// Testnet
    Testnet,
    /// Regtest
    Regtest(ActivationHeights),
}

impl std::fmt::Display for ChainType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let chain = match self {
            ChainType::Mainnet => "mainnet",
            ChainType::Testnet => "testnet",
            ChainType::Regtest(_) => "regtest",
        };
        write!(f, "{chain}")
    }
}

impl TryFrom<&str> for ChainType {
    type Error = InvalidChainType;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "mainnet" | "main" => Ok(ChainType::Mainnet),
            "testnet" | "test" => Ok(ChainType::Testnet),
            "regtest" => Ok(ChainType::Regtest(ActivationHeights::default())),
            _ => Err(InvalidChainType(value.to_string())),
        }
    }
}

pub(crate) mod consealed {
    use zcash_protocol::consensus::{
        BlockHeight, MAIN_NETWORK, NetworkType, NetworkUpgrade, Parameters, TEST_NETWORK,
    };

    use super::ChainType;

    impl Parameters for ChainType {
        fn network_type(&self) -> NetworkType {
            match self {
                ChainType::Mainnet => NetworkType::Main,
                ChainType::Testnet => NetworkType::Test,
                ChainType::Regtest(_) => NetworkType::Regtest,
            }
        }

        fn activation_height(&self, nu: NetworkUpgrade) -> Option<BlockHeight> {
            match self {
                ChainType::Mainnet => MAIN_NETWORK.activation_height(nu),
                ChainType::Testnet => TEST_NETWORK.activation_height(nu),
                ChainType::Regtest(activation_heights) => match nu {
                    NetworkUpgrade::Overwinter => {
                        activation_heights.overwinter().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Sapling => {
                        activation_heights.sapling().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Blossom => {
                        activation_heights.blossom().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Heartwood => {
                        activation_heights.heartwood().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Canopy => {
                        activation_heights.canopy().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Nu5 => activation_heights.nu5().map(BlockHeight::from_u32),
                    NetworkUpgrade::Nu6 => activation_heights.nu6().map(BlockHeight::from_u32),
                    NetworkUpgrade::Nu6_1 => activation_heights.nu6_1().map(BlockHeight::from_u32),
                    NetworkUpgrade::Nu6_2 => activation_heights.nu6_2().map(BlockHeight::from_u32),
                    NetworkUpgrade::Nu6_3 => activation_heights.nu6_3().map(BlockHeight::from_u32),
                    NetworkUpgrade::Nu7 => activation_heights.nu7().map(BlockHeight::from_u32),
                },
            }
        }
    }
}

/// Invalid chain type.
#[derive(thiserror::Error, Debug)]
#[error("Invalid chain type '{0}'. Expected one of: 'mainnet', 'testnet' or 'regtest'.")]
pub struct InvalidChainType(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeState {
    pub height: BlockHeight,
    pub hash: BlockHash,
    pub sapling_tree: Vec<u8>,
    pub orchard_tree: Vec<u8>,
    pub ironwood_tree: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum InvalidTreeState {
    #[error("the block height {0} does not fit a block height")]
    Height(u64),
    #[error("the block hash is not valid hex: {0}")]
    HashHex(hex::FromHexError),
    #[error("the block hash has {0} bytes")]
    HashLength(usize),
    #[error("the {pool} tree is not valid hex: {source}")]
    TreeHex {
        pool: &'static str,
        source: hex::FromHexError,
    },
}

impl TreeState {
    pub const SAPLING: &str = "sapling";
    pub const ORCHARD: &str = "orchard";
    pub const IRONWOOD: &str = "ironwood";

    pub fn from_wire_parts(
        height: u64,
        hash_hex: &str,
        sapling_tree_hex: &str,
        orchard_tree_hex: &str,
        ironwood_tree_hex: &str,
    ) -> Result<Self, InvalidTreeState> {
        let block_height =
            BlockHeight::try_from(height).map_err(|_| InvalidTreeState::Height(height))?;

        let mut hash_bytes = hex::decode(hash_hex).map_err(InvalidTreeState::HashHex)?;
        hash_bytes.reverse();
        let hash = BlockHash::try_from_slice(&hash_bytes)
            .ok_or(InvalidTreeState::HashLength(hash_bytes.len()))?;

        let decode_tree = |pool, tree_hex: &str| {
            hex::decode(tree_hex).map_err(|source| InvalidTreeState::TreeHex { pool, source })
        };

        Ok(Self {
            height: block_height,
            hash,
            sapling_tree: decode_tree(Self::SAPLING, sapling_tree_hex)?,
            orchard_tree: decode_tree(Self::ORCHARD, orchard_tree_hex)?,
            ironwood_tree: decode_tree(Self::IRONWOOD, ironwood_tree_hex)?,
        })
    }
}

#[cfg(test)]
mod tree_state_tests {
    use super::*;

    const HEIGHT: u32 = 2_000_000;
    const HASH_HEX: &str = "00000000000000000000000000000000000000000000000000000000000000ff";
    const HASH_BYTES: usize = 32;
    const TREE_HEX: &str = "000000";
    const TREE_BYTES: [u8; 3] = [0, 0, 0];
    const OMITTED_TREE_HEX: &str = "";
    const NOT_HEX: &str = "zz";
    const SHORT_HASH_HEX: &str = "00ff";
    const SHORT_HASH_BYTES: usize = 2;

    fn tree_state(
        height: u64,
        hash_hex: &str,
        ironwood_tree_hex: &str,
    ) -> Result<TreeState, InvalidTreeState> {
        TreeState::from_wire_parts(height, hash_hex, TREE_HEX, TREE_HEX, ironwood_tree_hex)
    }

    #[test]
    fn a_well_formed_tree_state_parses_with_its_hash_reversed() {
        let mut reversed_hash = [0; HASH_BYTES];
        reversed_hash[0] = u8::MAX;

        assert_eq!(
            tree_state(u64::from(HEIGHT), HASH_HEX, OMITTED_TREE_HEX),
            Ok(TreeState {
                height: BlockHeight::from_u32(HEIGHT),
                hash: BlockHash(reversed_hash),
                sapling_tree: TREE_BYTES.to_vec(),
                orchard_tree: TREE_BYTES.to_vec(),
                ironwood_tree: Vec::new(),
            })
        );
    }

    #[test]
    fn each_malformed_field_is_named() {
        let beyond_32_bits = u64::from(u32::MAX) + 1;

        assert_eq!(
            tree_state(beyond_32_bits, HASH_HEX, TREE_HEX),
            Err(InvalidTreeState::Height(beyond_32_bits))
        );
        assert!(matches!(
            tree_state(u64::from(HEIGHT), NOT_HEX, TREE_HEX),
            Err(InvalidTreeState::HashHex(_))
        ));
        assert_eq!(
            tree_state(u64::from(HEIGHT), SHORT_HASH_HEX, TREE_HEX),
            Err(InvalidTreeState::HashLength(SHORT_HASH_BYTES))
        );
        assert!(matches!(
            tree_state(u64::from(HEIGHT), HASH_HEX, NOT_HEX),
            Err(InvalidTreeState::TreeHex {
                pool: TreeState::IRONWOOD,
                ..
            })
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::ChainType;
    use zingo_common_components::protocol::ActivationHeights;

    #[test]
    fn test_chaintypes_from_strings() {
        assert_eq!(
            ChainType::try_from("mainnet").expect("'mainnet' names the main network"),
            ChainType::Mainnet
        );
        assert_eq!(
            ChainType::try_from("testnet").expect("'testnet' names the test network"),
            ChainType::Testnet
        );
        // the following 2 variants match the string from get_lightd_info, from the LightdInfo struct
        // as referenced in librustzcash/zcash_client_backend/src/proto/service.rs 17a4830c70
        // and provided by an indexer (previously called lightwallet)
        assert_eq!(
            ChainType::try_from("main").expect("'main' names the main network"),
            ChainType::Mainnet
        );
        assert_eq!(
            ChainType::try_from("test").expect("'test' names the test network"),
            ChainType::Testnet
        );
        assert_eq!(
            ChainType::try_from("regtest").expect("'regtest' names the regtest network"),
            ChainType::Regtest(ActivationHeights::default())
        );
        assert!(ChainType::try_from("invalid").is_err());
        let err = ChainType::try_from("badvalue").expect_err("'badvalue' names no network");
        assert_eq!(err.0, "badvalue");
        assert!(ChainType::try_from("").is_err());
        assert!(ChainType::try_from("Mainnet").is_err());
        assert!(ChainType::try_from("MAINNET").is_err());
        assert!(ChainType::try_from("Testnet").is_err());
        assert!(ChainType::try_from("Regtest").is_err());
        assert!(ChainType::try_from("mainnet ").is_err()); // trailing space
        assert!(ChainType::try_from(" mainnet").is_err()); // leading space
        assert!(ChainType::try_from("regtест").is_err()); // lookalike chars
    }
}
