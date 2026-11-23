use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::BlockHeight;

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

        let decode_tree =
            |field: &'static str, hex_str: &str| -> Result<Vec<u8>, InvalidTreeState> {
                hex::decode(hex_str)
                    .map_err(|source| InvalidTreeState::InvalidTreeHex { field, source })
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
    use super::{InvalidTreeState, TreeState};
    use zcash_primitives::block::BlockHash;
    use zcash_protocol::consensus::BlockHeight;

    /// A height one past `u32::MAX`, the smallest value `BlockHeight::try_from` rejects.
    const TOO_TALL: u64 = u32::MAX as u64 + 1;

    /// Hex for a valid, arbitrary 32-byte hash. Not used to assert anything about byte order;
    /// see `valid_parts_decode_hex_and_reverse_the_hash` for that.
    fn some_hash_hex() -> String {
        hex::encode([0xaa; 32])
    }

    #[test]
    fn valid_parts_decode_hex_and_reverse_the_hash() {
        // chosen so reversal is visible: only the first byte is non-zero on the wire, so only
        // the LAST byte of the decoded hash should be non-zero.
        let mut wire_bytes = [0u8; 32];
        wire_bytes[0] = 0x01;
        let hash_hex = hex::encode(wire_bytes);

        let tree_state = TreeState::from_wire_parts(100, &hash_hex, "aa", "bb", "cc").unwrap();
        assert_eq!(tree_state.height, BlockHeight::from_u32(100));
        let mut expected = [0u8; 32];
        expected[31] = 0x01;
        assert_eq!(tree_state.hash, BlockHash(expected));
        assert_eq!(tree_state.sapling_tree, vec![0xaa]);
        assert_eq!(tree_state.orchard_tree, vec![0xbb]);
        assert_eq!(tree_state.ironwood_tree, vec![0xcc]);
    }

    #[test]
    fn a_height_past_u32_max_is_rejected() {
        assert!(matches!(
            TreeState::from_wire_parts(TOO_TALL, &some_hash_hex(), "", "", ""),
            Err(InvalidTreeState::InvalidHeight)
        ));
    }

    #[test]
    fn non_hex_hash_is_rejected() {
        assert!(matches!(
            TreeState::from_wire_parts(0, "not hex", "", "", ""),
            Err(InvalidTreeState::InvalidHashHex(_))
        ));
    }

    #[test]
    fn a_hash_not_exactly_32_bytes_is_rejected() {
        assert!(matches!(
            TreeState::from_wire_parts(0, "aabb", "", "", ""),
            Err(InvalidTreeState::InvalidHashLength)
        ));
    }

    #[test]
    fn non_hex_tree_is_rejected_and_names_the_pool() {
        for (sapling, orchard, ironwood) in [
            ("not hex", "", ""),
            ("", "not hex", ""),
            ("", "", "not hex"),
        ] {
            let err = TreeState::from_wire_parts(0, &some_hash_hex(), sapling, orchard, ironwood)
                .unwrap_err();
            assert!(
                matches!(err, InvalidTreeState::InvalidTreeHex { .. }),
                "{err}"
            );
        }
    }
}
