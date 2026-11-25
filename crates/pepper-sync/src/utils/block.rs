use zcash_primitives::block::{BlockHash, BlockHeader};
use zcash_protocol::{ShieldedPool, consensus::BlockHeight};
use zl_common::chain::CompactBlock;

use super::transaction;

/// `zl_common::chain::BlockHash` mirrors `zcash_primitives::block::BlockHash` byte for byte
/// without depending on it. Everything downstream of the wire boundary (witness trees, wallet
/// scanning, block headers) still speaks the latter, so this is the one place that bridges the
/// two.
pub(crate) fn to_zcash_primitives(hash: zl_common::chain::BlockHash) -> BlockHash {
    BlockHash(hash.0)
}

/// Returns the [`BlockHash`] for this block.
pub(crate) fn get_compact_hash(compact_block: &CompactBlock) -> BlockHash {
    get_compact_header(compact_block)
        .map(|header| header.hash())
        .unwrap_or_else(|| to_zcash_primitives(compact_block.hash))
}

/// Returns the [`BlockHash`] for this block's parent.
pub(crate) fn get_compact_prev_hash(compact_block: &CompactBlock) -> BlockHash {
    get_compact_header(compact_block)
        .map(|header| header.prev_block)
        .unwrap_or_else(|| to_zcash_primitives(compact_block.prev_hash))
}

/// Returns the [`BlockHeight`] value for this block.
pub(crate) fn get_compact_height(compact_block: &CompactBlock) -> BlockHeight {
    compact_block.height
}

/// Returns the [`BlockHeader`] for this block if present.
///
/// A convenience method that parses `compact_block.header` if present.
pub(crate) fn get_compact_header(compact_block: &CompactBlock) -> Option<BlockHeader> {
    if compact_block.header.is_empty() {
        None
    } else {
        BlockHeader::read(&compact_block.header[..]).ok()
    }
}

pub(crate) fn shielded_output_count(compact_block: &CompactBlock, pool: ShieldedPool) -> u32 {
    compact_block
        .vtx
        .iter()
        .map(|compact_tx| u64::from(transaction::shielded_output_count(compact_tx, pool)))
        .sum::<u64>()
        .try_into()
        .expect("note commitment tree sizes are u32; a block cannot carry more outputs")
}

pub(crate) fn shielded_input_count(compact_block: &CompactBlock, pool: ShieldedPool) -> u32 {
    compact_block
        .vtx
        .iter()
        .map(|compact_tx| u64::from(transaction::shielded_input_count(compact_tx, pool)))
        .sum::<u64>()
        .try_into()
        .expect("note commitment tree sizes are u32; a block cannot carry more inputs")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::transaction::{ALL_POOLS, compact_tx, empty_compact_block};

    #[test]
    fn counts_sum_across_transactions() {
        let block = empty_compact_block(vec![compact_tx(1, 2, 3, 4), compact_tx(5, 6, 7, 8)]);
        assert_eq!(shielded_output_count(&block, ShieldedPool::Sapling), 8);
        assert_eq!(shielded_output_count(&block, ShieldedPool::Orchard), 10);
        assert_eq!(shielded_output_count(&block, ShieldedPool::Ironwood), 12);
        assert_eq!(shielded_input_count(&block, ShieldedPool::Sapling), 6);
        assert_eq!(shielded_input_count(&block, ShieldedPool::Orchard), 10);
        assert_eq!(shielded_input_count(&block, ShieldedPool::Ironwood), 12);
    }

    #[test]
    fn empty_block_counts_zero_for_every_pool() {
        let block = empty_compact_block(vec![]);
        for pool in ALL_POOLS {
            assert_eq!(shielded_output_count(&block, pool), 0);
            assert_eq!(shielded_input_count(&block, pool), 0);
        }
    }
}
