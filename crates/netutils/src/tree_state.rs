#![forbid(unsafe_code)]

const BLOCK_HASH_BYTES: usize = 32;
const SAPLING: &str = "Sapling";
const ORCHARD: &str = "Orchard";
const IRONWOOD: &str = "Ironwood";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireParts {
    pub height: u64,
    pub hash_hex: String,
    pub sapling_tree_hex: String,
    pub orchard_tree_hex: String,
    pub ironwood_tree_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeState(WireParts);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validated {
    pub height: u32,
    pub hash: [u8; BLOCK_HASH_BYTES],
    pub sapling_tree: Vec<u8>,
    pub orchard_tree: Vec<u8>,
    pub ironwood_tree: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum Invalid {
    #[error("Invalid block height")]
    Height(u64),
    #[error("Block hash is not valid hex: {0:?}")]
    HashHex(hex::FromHexError),
    #[error("Invalid block hash length.")]
    HashLength(usize),
    #[error("Hex decoding of {pool} tree bytes failed: {source:?}")]
    TreeHex {
        pool: &'static str,
        source: hex::FromHexError,
    },
}

impl From<WireParts> for TreeState {
    fn from(wire_parts: WireParts) -> Self {
        Self(wire_parts)
    }
}

impl TreeState {
    pub fn sapling_tree_is_omitted(&self) -> bool {
        self.0.sapling_tree_hex.is_empty()
    }

    pub fn orchard_tree_is_omitted(&self) -> bool {
        self.0.orchard_tree_hex.is_empty()
    }

    pub fn ironwood_tree_is_omitted(&self) -> bool {
        self.0.ironwood_tree_hex.is_empty()
    }

    pub fn validate(self) -> Result<Validated, Invalid> {
        let Self(wire_parts) = self;
        let mut hash_bytes = hex::decode(&wire_parts.hash_hex).map_err(Invalid::HashHex)?;
        hash_bytes.reverse();
        let height =
            u32::try_from(wire_parts.height).map_err(|_| Invalid::Height(wire_parts.height))?;
        let hash = <[u8; BLOCK_HASH_BYTES]>::try_from(hash_bytes)
            .map_err(|hash_bytes| Invalid::HashLength(hash_bytes.len()))?;

        let decode_tree = |pool, tree_hex: &str| {
            hex::decode(tree_hex).map_err(|source| Invalid::TreeHex { pool, source })
        };

        Ok(Validated {
            height,
            hash,
            sapling_tree: decode_tree(SAPLING, &wire_parts.sapling_tree_hex)?,
            orchard_tree: decode_tree(ORCHARD, &wire_parts.orchard_tree_hex)?,
            ironwood_tree: decode_tree(IRONWOOD, &wire_parts.ironwood_tree_hex)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEIGHT: u32 = 2_000_000;
    const HASH_HEX: &str = "00000000000000000000000000000000000000000000000000000000000000ff";
    const TREE_HEX: &str = "000000";
    const TREE_BYTES: [u8; 3] = [0, 0, 0];
    const NOT_HEX: &str = "zz";
    const SHORT_HASH_HEX: &str = "00ff";
    const SHORT_HASH_BYTES: usize = 2;

    fn tree_state(height: u64, hash_hex: &str, ironwood_tree_hex: &str) -> TreeState {
        TreeState::from(WireParts {
            height,
            hash_hex: hash_hex.to_string(),
            sapling_tree_hex: TREE_HEX.to_string(),
            orchard_tree_hex: TREE_HEX.to_string(),
            ironwood_tree_hex: ironwood_tree_hex.to_string(),
        })
    }

    #[test]
    fn a_well_formed_tree_state_validates_with_its_hash_reversed() {
        let mut reversed_hash = [0; BLOCK_HASH_BYTES];
        reversed_hash[0] = u8::MAX;

        assert_eq!(
            tree_state(u64::from(HEIGHT), HASH_HEX, "")
                .validate()
                .map_err(|invalid| invalid.to_string()),
            Ok(Validated {
                height: HEIGHT,
                hash: reversed_hash,
                sapling_tree: TREE_BYTES.to_vec(),
                orchard_tree: TREE_BYTES.to_vec(),
                ironwood_tree: Vec::new(),
            })
        );
    }

    #[test]
    fn only_an_empty_tree_is_omitted() {
        let tree_state = tree_state(u64::from(HEIGHT), HASH_HEX, "");

        assert!(!tree_state.sapling_tree_is_omitted());
        assert!(!tree_state.orchard_tree_is_omitted());
        assert!(tree_state.ironwood_tree_is_omitted());
    }

    #[test]
    fn each_malformed_field_is_named() {
        let beyond_32_bits = u64::from(u32::MAX) + 1;

        assert!(matches!(
            tree_state(beyond_32_bits, HASH_HEX, TREE_HEX).validate(),
            Err(Invalid::Height(height)) if height == beyond_32_bits
        ));
        assert!(matches!(
            tree_state(u64::from(HEIGHT), NOT_HEX, TREE_HEX).validate(),
            Err(Invalid::HashHex(_))
        ));
        assert!(matches!(
            tree_state(u64::from(HEIGHT), SHORT_HASH_HEX, TREE_HEX).validate(),
            Err(Invalid::HashLength(SHORT_HASH_BYTES))
        ));
        assert!(matches!(
            tree_state(u64::from(HEIGHT), HASH_HEX, NOT_HEX).validate(),
            Err(Invalid::TreeHex { pool: IRONWOOD, .. })
        ));
    }

    #[test]
    fn each_report_keeps_its_text() {
        let source = hex::FromHexError::OddLength;

        assert_eq!(
            Invalid::Height(u64::from(HEIGHT)).to_string(),
            "Invalid block height"
        );
        assert_eq!(
            Invalid::HashHex(source).to_string(),
            "Block hash is not valid hex: OddLength"
        );
        assert_eq!(
            Invalid::HashLength(SHORT_HASH_BYTES).to_string(),
            "Invalid block hash length."
        );
        assert_eq!(
            Invalid::TreeHex {
                pool: SAPLING,
                source
            }
            .to_string(),
            "Hex decoding of Sapling tree bytes failed: OddLength"
        );
    }
}
