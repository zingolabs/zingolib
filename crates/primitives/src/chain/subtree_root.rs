use crate::chain::block_hash::BlockHash;
use zcash_protocol::consensus::BlockHeight;

/// A completed shard's Merkle root, as served by an indexer: the raw 32-byte root and the
/// block that completed it, already validated rather than carried as unchecked wire bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreeRoot {
    /// The 32-byte Merkle root of the subtree. Not yet parsed into a pool-specific hash type,
    /// since that decoding depends on which shielded pool the caller requested roots for, which
    /// this wire message does not itself carry.
    pub root_hash: [u8; 32],
    /// The hash of the block that completed this subtree.
    pub completing_block_hash: BlockHash,
    /// The height of the block that completed this subtree in the main chain.
    pub completing_block_height: BlockHeight,
}

/// A [`SubtreeRoot`] field that failed to parse from its wire representation.
#[derive(Debug, thiserror::Error)]
pub enum InvalidSubtreeRoot {
    /// The root hash was not exactly 32 bytes.
    #[error("subtree root hash is not 32 bytes")]
    InvalidRootHashLength,
    /// The completing block hash was not exactly 32 bytes.
    #[error("completing block hash is not 32 bytes")]
    InvalidCompletingBlockHashLength,
    /// The completing block height does not fit a [`BlockHeight`].
    #[error("completing block height does not fit a block height")]
    InvalidCompletingBlockHeight,
}

impl SubtreeRoot {
    /// Parses a subtree root from its wire parts, rejecting a root hash or completing block
    /// hash that is not exactly 32 bytes, and a completing block height that does not fit a
    /// [`BlockHeight`].
    pub fn from_wire_parts(
        root_hash: Vec<u8>,
        completing_block_hash: Vec<u8>,
        completing_block_height: u64,
    ) -> Result<Self, InvalidSubtreeRoot> {
        let root_hash: [u8; 32] = root_hash
            .try_into()
            .map_err(|_| InvalidSubtreeRoot::InvalidRootHashLength)?;
        let completing_block_hash = BlockHash::try_from_slice(&completing_block_hash)
            .ok_or(InvalidSubtreeRoot::InvalidCompletingBlockHashLength)?;
        let completing_block_height = BlockHeight::try_from(completing_block_height)
            .map_err(|_| InvalidSubtreeRoot::InvalidCompletingBlockHeight)?;
        Ok(Self {
            root_hash,
            completing_block_hash,
            completing_block_height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{InvalidSubtreeRoot, SubtreeRoot};
    use zcash_protocol::consensus::BlockHeight;

    /// A height one past `u32::MAX`, the smallest value `BlockHeight::try_from` rejects.
    const TOO_TALL: u64 = u32::MAX as u64 + 1;

    #[test]
    fn valid_parts_build() {
        let subtree_root =
            SubtreeRoot::from_wire_parts(vec![0xaa; 32], vec![0xbb; 32], 100).unwrap();
        assert_eq!(subtree_root.root_hash, [0xaa; 32]);
        assert_eq!(
            subtree_root.completing_block_height,
            BlockHeight::from_u32(100)
        );
    }

    #[test]
    fn a_root_hash_not_32_bytes_is_rejected() {
        assert!(matches!(
            SubtreeRoot::from_wire_parts(vec![0xaa; 31], vec![0xbb; 32], 100),
            Err(InvalidSubtreeRoot::InvalidRootHashLength)
        ));
    }

    #[test]
    fn a_completing_block_hash_not_32_bytes_is_rejected() {
        assert!(matches!(
            SubtreeRoot::from_wire_parts(vec![0xaa; 32], vec![0xbb; 31], 100),
            Err(InvalidSubtreeRoot::InvalidCompletingBlockHashLength)
        ));
    }

    #[test]
    fn a_completing_block_height_past_u32_max_is_rejected() {
        assert!(matches!(
            SubtreeRoot::from_wire_parts(vec![0xaa; 32], vec![0xbb; 32], TOO_TALL),
            Err(InvalidSubtreeRoot::InvalidCompletingBlockHeight)
        ));
    }
}
