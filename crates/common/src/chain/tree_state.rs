use zcash_primitives::block::BlockHash;
use zcash_protocol::{PoolType, ShieldedPool, consensus::BlockHeight};

#[derive(Debug, Clone, PartialEq)]
pub enum Tree {
    Omitted,
    Served(Vec<u8>),
    Malformed(hex::FromHexError),
}

impl Tree {
    #[must_use]
    pub fn from_hex(tree_hex: &str) -> Self {
        if tree_hex.is_empty() {
            Self::Omitted
        } else {
            hex::decode(tree_hex).map_or_else(Self::Malformed, Self::Served)
        }
    }

    pub fn bytes(&self, pool: ShieldedPool) -> Result<&[u8], InvalidTreeState> {
        match self {
            Self::Omitted => Ok(&[]),
            Self::Served(bytes) => Ok(bytes),
            Self::Malformed(source) => Err(InvalidTreeState::InvalidTreeHex {
                pool,
                source: *source,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TreeStateReply {
    pub height: Result<BlockHeight, InvalidTreeState>,
    pub hash: Result<BlockHash, InvalidTreeState>,
    pub sapling_tree: Tree,
    pub orchard_tree: Tree,
    pub ironwood_tree: Tree,
}

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidTreeState {
    #[error("invalid block height")]
    InvalidHeight,
    #[error("block hash is not valid hex: {0}")]
    InvalidHashHex(hex::FromHexError),
    #[error("invalid block hash length")]
    InvalidHashLength,
    #[error("{} tree is not valid hex: {source}", PoolType::Shielded(*pool))]
    InvalidTreeHex {
        pool: ShieldedPool,
        source: hex::FromHexError,
    },
}

impl TreeStateReply {
    pub fn block_height(height: u64) -> Result<BlockHeight, InvalidTreeState> {
        BlockHeight::try_from(height).map_err(|_| InvalidTreeState::InvalidHeight)
    }

    pub fn block_hash(hash_hex: &str) -> Result<BlockHash, InvalidTreeState> {
        let mut hash_bytes = hex::decode(hash_hex).map_err(InvalidTreeState::InvalidHashHex)?;
        // Block-hash hex strings are byte-reversed on the wire.
        hash_bytes.reverse();
        BlockHash::try_from_slice(&hash_bytes).ok_or(InvalidTreeState::InvalidHashLength)
    }

    pub fn trees(&self) -> impl Iterator<Item = (ShieldedPool, &Tree)> {
        [
            (ShieldedPool::Sapling, &self.sapling_tree),
            (ShieldedPool::Orchard, &self.orchard_tree),
            (ShieldedPool::Ironwood, &self.ironwood_tree),
        ]
        .into_iter()
    }
}

#[cfg(test)]
mod tests {
    use zcash_primitives::block::BlockHash;
    use zcash_protocol::{ShieldedPool, consensus::BlockHeight};

    use super::{InvalidTreeState, Tree, TreeStateReply};

    const HEIGHT: u32 = 100;
    const TOO_TALL: u64 = u32::MAX as u64 + 1;
    const HASH_LENGTH: usize = 32;
    const MARKED_BYTE: u8 = 0x01;
    const TREE_BYTE: u8 = 0xaa;
    const TREE_HEX: &str = "aa";
    const OMITTED_TREE_HEX: &str = "";
    const NOT_HEX: &str = "not hex";
    const SHORT_HASH_HEX: &str = "aabb";

    #[test]
    fn a_height_that_fits_is_a_block_height() {
        assert_eq!(
            TreeStateReply::block_height(u64::from(HEIGHT)),
            Ok(BlockHeight::from_u32(HEIGHT))
        );
    }

    #[test]
    fn a_height_past_u32_max_is_invalid() {
        assert_eq!(
            TreeStateReply::block_height(TOO_TALL),
            Err(InvalidTreeState::InvalidHeight)
        );
    }

    #[test]
    fn a_block_hash_is_reversed_from_its_wire_order() {
        let mut wire_bytes = [0; HASH_LENGTH];
        wire_bytes[0] = MARKED_BYTE;
        let mut reversed_bytes = [0; HASH_LENGTH];
        reversed_bytes[HASH_LENGTH - 1] = MARKED_BYTE;

        assert_eq!(
            TreeStateReply::block_hash(&hex::encode(wire_bytes)),
            Ok(BlockHash(reversed_bytes))
        );
    }

    #[test]
    fn a_block_hash_that_is_not_hex_is_invalid() {
        assert!(matches!(
            TreeStateReply::block_hash(NOT_HEX),
            Err(InvalidTreeState::InvalidHashHex(_))
        ));
    }

    #[test]
    fn a_block_hash_of_the_wrong_length_is_invalid() {
        assert_eq!(
            TreeStateReply::block_hash(SHORT_HASH_HEX),
            Err(InvalidTreeState::InvalidHashLength)
        );
    }

    #[test]
    fn each_tree_hex_is_omitted_served_or_malformed() {
        assert_eq!(Tree::from_hex(OMITTED_TREE_HEX), Tree::Omitted);
        assert_eq!(Tree::from_hex(TREE_HEX), Tree::Served(vec![TREE_BYTE]));
        assert!(matches!(Tree::from_hex(NOT_HEX), Tree::Malformed(_)));
    }

    #[test]
    fn the_bytes_of_a_malformed_tree_name_its_pool() {
        for pool in [
            ShieldedPool::Sapling,
            ShieldedPool::Orchard,
            ShieldedPool::Ironwood,
        ] {
            assert!(matches!(
                Tree::from_hex(NOT_HEX).bytes(pool),
                Err(InvalidTreeState::InvalidTreeHex { pool: named, .. }) if named == pool
            ));
            assert_eq!(Tree::from_hex(OMITTED_TREE_HEX).bytes(pool), Ok(&[][..]));
            assert_eq!(Tree::from_hex(TREE_HEX).bytes(pool), Ok(&[TREE_BYTE][..]));
        }
    }

    #[test]
    fn the_trees_are_listed_with_their_pools() {
        let tree_state = TreeStateReply {
            height: TreeStateReply::block_height(u64::from(HEIGHT)),
            hash: TreeStateReply::block_hash(NOT_HEX),
            sapling_tree: Tree::from_hex(TREE_HEX),
            orchard_tree: Tree::from_hex(OMITTED_TREE_HEX),
            ironwood_tree: Tree::from_hex(NOT_HEX),
        };

        assert_eq!(
            tree_state.trees().collect::<Vec<_>>(),
            [
                (ShieldedPool::Sapling, &tree_state.sapling_tree),
                (ShieldedPool::Orchard, &tree_state.orchard_tree),
                (ShieldedPool::Ironwood, &tree_state.ironwood_tree),
            ]
        );
    }
}
