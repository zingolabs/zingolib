use zcash_primitives::block::{BlockHash, BlockHeader};
use zcash_protocol::{ShieldedPool, consensus::BlockHeight};
use zingo_netutils::lightwallet_protocol::CompactBlock;

use super::transaction;

/// Returns the [`BlockHash`] for this block.
///
/// # Panics
///
/// This function will panic if `compact_block.header` is not set and
/// `compact_block.hash` is not exactly 32 bytes.
pub(crate) fn get_compact_hash(compact_block: &CompactBlock) -> BlockHash {
    if let Some(header) = get_compact_header(compact_block) {
        header.hash()
    } else {
        BlockHash::from_slice(&compact_block.hash)
    }
}

/// Returns the [`BlockHash`] for this block's parent.
///
/// # Panics
///
/// This function will panic if `compact_block.header` is not set and
/// `compact_block.hash` is not exactly 32 bytes.
pub(crate) fn get_compact_prev_hash(compact_block: &CompactBlock) -> BlockHash {
    if let Some(header) = get_compact_header(compact_block) {
        header.prev_block
    } else {
        BlockHash::from_slice(&compact_block.prev_hash)
    }
}

/// Returns the [`BlockHeight`] value for this block
///
/// # Panics
///
/// This function will panic if `compact_block.height` is not representable within a
/// `u32`.
pub(crate) fn get_compact_height(compact_block: &CompactBlock) -> BlockHeight {
    compact_block.height.try_into().unwrap()
}

/// Returns the [`BlockHeader`] for this block if present.
///
/// A convenience method that parses `compact_block.height` if present.
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
    use crate::utils::transaction::{ALL_POOLS, compact_tx};

    #[test]
    fn counts_sum_across_transactions() {
        let block = CompactBlock {
            vtx: vec![compact_tx(1, 2, 3, 4), compact_tx(5, 6, 7, 8)],
            ..Default::default()
        };
        assert_eq!(shielded_output_count(&block, ShieldedPool::Sapling), 8);
        assert_eq!(shielded_output_count(&block, ShieldedPool::Orchard), 10);
        assert_eq!(shielded_output_count(&block, ShieldedPool::Ironwood), 12);
        assert_eq!(shielded_input_count(&block, ShieldedPool::Sapling), 6);
        assert_eq!(shielded_input_count(&block, ShieldedPool::Orchard), 10);
        assert_eq!(shielded_input_count(&block, ShieldedPool::Ironwood), 12);
    }

    #[test]
    fn empty_block_counts_zero_for_every_pool() {
        let block = CompactBlock::default();
        for pool in ALL_POOLS {
            assert_eq!(shielded_output_count(&block, pool), 0);
            assert_eq!(shielded_input_count(&block, pool), 0);
        }
    }
}
