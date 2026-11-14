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
