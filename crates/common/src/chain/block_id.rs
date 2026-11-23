use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::BlockHeight;

// This type is built by the caller as well as received from a server (see `from_height` vs.
// `from_wire_parts`), unlike the request-only types in sibling modules.

/// Identifies a block to a server, by height and, optionally, hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockId {
    /// The block's height.
    pub height: BlockHeight,
    /// The block's hash, when the caller wants to identify the block by hash instead of (or as
    /// well as) height.
    pub hash: Option<BlockHash>,
}

/// A [`BlockId`] received from a server that failed to parse: its height does not fit a
/// [`BlockHeight`].
#[derive(Debug, thiserror::Error)]
#[error("invalid block height")]
pub struct InvalidBlockId;

impl BlockId {
    /// A [`BlockId`] that identifies a block by height alone.
    pub fn from_height(height: BlockHeight) -> Self {
        Self { height, hash: None }
    }

    /// Parses a block id received from a server. A hash that is not exactly 32 bytes is
    /// treated as absent rather than rejected, since `hash` is an optional secondary
    /// identifier here, not the field this value is primarily validated on.
    pub fn from_wire_parts(height: u64, hash: Vec<u8>) -> Result<Self, InvalidBlockId> {
        Ok(Self {
            height: BlockHeight::try_from(height).map_err(|_| InvalidBlockId)?,
            hash: BlockHash::try_from_slice(&hash),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::BlockId;
    use zcash_primitives::block::BlockHash;
    use zcash_protocol::consensus::BlockHeight;

    /// A height one past `u32::MAX`, the smallest value `BlockHeight::try_from` rejects.
    const TOO_TALL: u64 = u32::MAX as u64 + 1;

    #[test]
    fn from_height_carries_no_hash() {
        let id = BlockId::from_height(BlockHeight::from_u32(5));
        assert_eq!(id.height, BlockHeight::from_u32(5));
        assert_eq!(id.hash, None);
    }

    #[test]
    fn a_32_byte_hash_is_kept() {
        let id = BlockId::from_wire_parts(5, vec![0xaa; 32]).unwrap();
        assert_eq!(id.hash, Some(BlockHash([0xaa; 32])));
    }

    /// `hash` is a secondary, optional identifier here: a malformed one does not reject the
    /// whole value, it is simply treated as absent. Only `height` is load-bearing.
    #[test]
    fn a_wrong_length_hash_is_treated_as_absent_not_rejected() {
        let id = BlockId::from_wire_parts(5, vec![0xaa; 20]).unwrap();
        assert_eq!(id.hash, None);
    }

    #[test]
    fn a_height_past_u32_max_is_rejected() {
        assert!(BlockId::from_wire_parts(TOO_TALL, vec![]).is_err());
    }
}
