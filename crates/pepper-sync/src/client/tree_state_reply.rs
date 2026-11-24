use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::BlockHeight;
use zl_common::chain;
use zl_netutils::tree_state;

const SAPLING: &str = "sapling";
const ORCHARD: &str = "orchard";
const IRONWOOD: &str = "ironwood";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OmittedTrees {
    pub sapling: bool,
    pub orchard: bool,
    pub ironwood: bool,
}

impl OmittedTrees {
    pub(crate) fn of(tree_state: &chain::TreeState) -> Self {
        Self {
            sapling: tree_state.sapling_tree.is_empty(),
            orchard: tree_state.orchard_tree.is_empty(),
            ironwood: tree_state.ironwood_tree.is_empty(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{field}")]
pub struct Invalid {
    pub omitted_trees: OmittedTrees,
    pub field: InvalidField,
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidField {
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

pub(crate) fn parse(wire_parts: tree_state::WireParts) -> Result<chain::TreeState, Invalid> {
    let omitted_trees = OmittedTrees {
        sapling: wire_parts.sapling_tree_hex.is_empty(),
        orchard: wire_parts.orchard_tree_hex.is_empty(),
        ironwood: wire_parts.ironwood_tree_hex.is_empty(),
    };

    parse_fields(wire_parts).map_err(|field| Invalid {
        omitted_trees,
        field,
    })
}

fn parse_fields(wire_parts: tree_state::WireParts) -> Result<chain::TreeState, InvalidField> {
    let height = BlockHeight::try_from(wire_parts.height)
        .map_err(|_| InvalidField::Height(wire_parts.height))?;

    let mut hash_bytes = hex::decode(&wire_parts.hash_hex).map_err(InvalidField::HashHex)?;
    hash_bytes.reverse();
    let hash =
        BlockHash::try_from_slice(&hash_bytes).ok_or(InvalidField::HashLength(hash_bytes.len()))?;

    let decode_tree = |pool, tree_hex: &str| {
        hex::decode(tree_hex).map_err(|source| InvalidField::TreeHex { pool, source })
    };

    Ok(chain::TreeState {
        height,
        hash,
        sapling_tree: decode_tree(SAPLING, &wire_parts.sapling_tree_hex)?,
        orchard_tree: decode_tree(ORCHARD, &wire_parts.orchard_tree_hex)?,
        ironwood_tree: decode_tree(IRONWOOD, &wire_parts.ironwood_tree_hex)?,
    })
}

#[cfg(test)]
pub(crate) fn fetched(
    wire_parts: tree_state::WireParts,
) -> Result<chain::TreeState, tree_state::Failure<Invalid>> {
    parse(wire_parts).map_err(tree_state::Failure::Invalid)
}

#[cfg(test)]
mod tests {
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

    fn wire_parts(height: u64, hash_hex: &str, ironwood_tree_hex: &str) -> tree_state::WireParts {
        tree_state::WireParts {
            height,
            hash_hex: hash_hex.to_string(),
            sapling_tree_hex: TREE_HEX.to_string(),
            orchard_tree_hex: TREE_HEX.to_string(),
            ironwood_tree_hex: ironwood_tree_hex.to_string(),
        }
    }

    #[test]
    fn a_well_formed_tree_state_parses_with_its_hash_reversed() {
        let mut reversed_hash = [0; HASH_BYTES];
        reversed_hash[0] = u8::MAX;

        assert_eq!(
            parse(wire_parts(u64::from(HEIGHT), HASH_HEX, OMITTED_TREE_HEX))
                .map_err(|invalid| invalid.to_string()),
            Ok(chain::TreeState {
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

        assert!(matches!(
            parse(wire_parts(beyond_32_bits, HASH_HEX, TREE_HEX)),
            Err(Invalid { field: InvalidField::Height(height), .. }) if height == beyond_32_bits
        ));
        assert!(matches!(
            parse(wire_parts(u64::from(HEIGHT), NOT_HEX, TREE_HEX)),
            Err(Invalid {
                field: InvalidField::HashHex(_),
                ..
            })
        ));
        assert!(matches!(
            parse(wire_parts(u64::from(HEIGHT), SHORT_HASH_HEX, TREE_HEX)),
            Err(Invalid {
                field: InvalidField::HashLength(SHORT_HASH_BYTES),
                ..
            })
        ));
        assert!(matches!(
            parse(wire_parts(u64::from(HEIGHT), HASH_HEX, NOT_HEX)),
            Err(Invalid {
                field: InvalidField::TreeHex { pool: IRONWOOD, .. },
                ..
            })
        ));
    }

    #[test]
    fn an_invalid_tree_state_still_reports_its_omitted_trees() {
        assert!(matches!(
            parse(wire_parts(u64::from(HEIGHT), NOT_HEX, OMITTED_TREE_HEX)),
            Err(Invalid {
                omitted_trees: OmittedTrees {
                    sapling: false,
                    orchard: false,
                    ironwood: true,
                },
                ..
            })
        ));
    }
}
