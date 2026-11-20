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

/// A note commitment tree state as served by an indexer for a block: each shielded pool's
/// serialized commitment tree, already hex-decoded, with a validated height and hash.
///
/// Carrying wire bytes unvalidated this far would mean every consumer re-derives validity
/// (decodable hex? right-sized hash? a height that fits?) on every touch; this type validates
/// once, at the boundary, so a `TreeState` is trusted structurally wherever it is passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeState {
    /// The height of the block this tree state is as of.
    pub height: BlockHeight,
    /// The hash of the block this tree state is as of.
    pub hash: BlockHash,
    /// The Sapling note commitment tree, hex-decoded. Empty below Sapling activation, or when
    /// the server omits it.
    pub sapling_tree: Vec<u8>,
    /// The Orchard note commitment tree, hex-decoded. Empty below Orchard activation, or when
    /// the server omits it.
    pub orchard_tree: Vec<u8>,
    /// The Ironwood note commitment tree, hex-decoded. Empty below Ironwood activation, or when
    /// the server omits it.
    pub ironwood_tree: Vec<u8>,
}

/// A [`TreeState`] field that failed to parse from its wire representation.
#[derive(Debug, thiserror::Error)]
pub enum InvalidTreeState {
    /// The block height does not fit a [`BlockHeight`].
    #[error("invalid block height")]
    InvalidHeight,
    /// The block hash is not valid hex.
    #[error("block hash is not valid hex: {0}")]
    InvalidHashHex(hex::FromHexError),
    /// The block hash did not decode to exactly 32 bytes.
    #[error("invalid block hash length")]
    InvalidHashLength,
    /// A shielded pool's tree is not valid hex.
    #[error("{field} tree is not valid hex: {source}")]
    InvalidTreeHex {
        /// The pool whose tree failed to decode.
        field: &'static str,
        /// The underlying hex decode error.
        source: hex::FromHexError,
    },
}

impl TreeState {
    /// Parses a tree state from its wire parts: a block height, a byte-reversed hex block
    /// hash, and three hex-encoded (possibly empty) serialized commitment trees.
    ///
    /// Rejects a height that does not fit a [`BlockHeight`], hex that fails to decode, and a
    /// hash that does not decode to exactly 32 bytes.
    pub fn from_wire_parts(
        height: u64,
        hash_hex: &str,
        sapling_tree_hex: &str,
        orchard_tree_hex: &str,
        ironwood_tree_hex: &str,
    ) -> Result<Self, InvalidTreeState> {
        let height = BlockHeight::try_from(height).map_err(|_| InvalidTreeState::InvalidHeight)?;

        let mut hash_bytes = hex::decode(hash_hex).map_err(InvalidTreeState::InvalidHashHex)?;
        // Block-hash hex strings are byte-reversed on the wire.
        hash_bytes.reverse();
        let hash =
            BlockHash::try_from_slice(&hash_bytes).ok_or(InvalidTreeState::InvalidHashLength)?;

        let decode_tree = |field: &'static str, hex_str: &str| -> Result<Vec<u8>, InvalidTreeState> {
            hex::decode(hex_str).map_err(|source| InvalidTreeState::InvalidTreeHex { field, source })
        };

        Ok(Self {
            height,
            hash,
            sapling_tree: decode_tree("sapling", sapling_tree_hex)?,
            orchard_tree: decode_tree("orchard", orchard_tree_hex)?,
            ironwood_tree: decode_tree("ironwood", ironwood_tree_hex)?,
        })
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
