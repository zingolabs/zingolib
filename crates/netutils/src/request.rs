#![forbid(unsafe_code)]

use lightwallet_protocol::{BlockId, BlockRange, PoolType, TxFilter};

const NO_INDEX: u64 = 0;

/// ```
/// use zl_netutils::request;
///
/// let block_id = request::block_id(7);
/// assert_eq!(block_id.height, 7);
/// assert!(block_id.hash.is_empty());
/// ```
pub fn block_id(height: u64) -> BlockId {
    BlockId {
        height,
        hash: Vec::new(),
    }
}

/// ```
/// use zl_netutils::lightwallet_protocol::PoolType;
/// use zl_netutils::request;
///
/// let range = request::block_range(3, 5, &[PoolType::Orchard]);
/// assert_eq!(range.start, Some(request::block_id(3)));
/// assert_eq!(range.end, Some(request::block_id(5)));
/// assert_eq!(range.pool_types, [PoolType::Orchard as i32]);
/// ```
pub fn block_range(first: u64, last: u64, pool_types: &[PoolType]) -> BlockRange {
    BlockRange {
        start: Some(block_id(first)),
        end: Some(block_id(last)),
        pool_types: pool_types.iter().map(|pool| *pool as i32).collect(),
    }
}

/// ```
/// use zl_netutils::request;
///
/// let filter = request::tx_filter(vec![1, 2, 3]);
/// assert_eq!(filter.hash, [1, 2, 3]);
/// assert_eq!(filter.block, None);
/// ```
pub fn tx_filter(txid: Vec<u8>) -> TxFilter {
    TxFilter {
        block: None,
        index: NO_INDEX,
        hash: txid,
    }
}
