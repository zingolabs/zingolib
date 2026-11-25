#![forbid(unsafe_code)]

use std::ops::Range;

use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zl_netutils::request;

pub(crate) use zl_netutils::lightwallet_protocol::{
    BlockId, BlockRange, CompactBlock, CompactOrchardAction, CompactSaplingOutput, CompactTx,
    GetAddressUtxosArg, GetAddressUtxosReply, GetSubtreeRootsArg, LightdInfo, PoolType,
    RawTransaction, SubtreeRoot, TransparentAddressBlockFilter, TreeState, TxFilter,
};

#[cfg(test)]
pub(crate) use zl_netutils::lightwallet_protocol::{
    ChainMetadata, CompactSaplingSpend, CompactTxIn, TxOut,
};

pub(crate) use tonic::{Code, Status, Streaming, TimeoutExpired, transport};

pub(crate) type CompactBlocks = Streaming<CompactBlock>;
pub(crate) type RawTransactions = Streaming<RawTransaction>;
pub(crate) type SubtreeRoots = Streaming<SubtreeRoot>;

pub(crate) const UNLIMITED_ENTRIES: u32 = 0;

const SHIELDED_POOL_TYPES: [PoolType; 3] =
    [PoolType::Sapling, PoolType::Orchard, PoolType::Ironwood];
const ALL_POOL_TYPES: [PoolType; 4] = [
    PoolType::Transparent,
    PoolType::Sapling,
    PoolType::Orchard,
    PoolType::Ironwood,
];
const EVERY_POOL_BY_DEFAULT: [PoolType; 0] = [];

const EXCLUSIVE_END: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSpan {
    first: BlockHeight,
    last: BlockHeight,
}

impl BlockSpan {
    pub(crate) fn of(range: &Range<BlockHeight>) -> Option<Self> {
        (range.start < range.end).then(|| Self {
            first: range.start,
            last: range.end - EXCLUSIVE_END,
        })
    }
}

pub(crate) fn pool_types(include_transparent: bool) -> &'static [PoolType] {
    if include_transparent {
        &ALL_POOL_TYPES
    } else {
        &SHIELDED_POOL_TYPES
    }
}

pub(crate) fn block_id_at(height: BlockHeight) -> BlockId {
    request::block_id(u64::from(height))
}

pub(crate) fn block_range(span: BlockSpan, pool_types: &[PoolType]) -> BlockRange {
    request::block_range(u64::from(span.first), u64::from(span.last), pool_types)
}

pub(crate) fn nullifier_range(span: BlockSpan) -> BlockRange {
    block_range(span, &EVERY_POOL_BY_DEFAULT)
}

pub(crate) fn tx_filter(txid: TxId) -> TxFilter {
    request::tx_filter(txid.as_ref().to_vec())
}

pub(crate) fn subtree_roots_arg(
    start_index: u32,
    shielded_protocol: i32,
    max_entries: u32,
) -> GetSubtreeRootsArg {
    GetSubtreeRootsArg {
        start_index,
        shielded_protocol,
        max_entries,
    }
}

pub(crate) fn address_utxos_arg(
    addresses: Vec<String>,
    start_height: BlockHeight,
    max_entries: u32,
) -> GetAddressUtxosArg {
    GetAddressUtxosArg {
        addresses,
        start_height: u64::from(start_height),
        max_entries,
    }
}

pub(crate) fn address_block_filter(
    address: String,
    span: BlockSpan,
) -> TransparentAddressBlockFilter {
    TransparentAddressBlockFilter {
        address,
        range: Some(nullifier_range(span)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: u32 = 0;
    const FIRST: u32 = 3;
    const END: u32 = 6;
    const LAST: u32 = END - EXCLUSIVE_END;

    fn span(start: u32, end: u32) -> Option<BlockSpan> {
        BlockSpan::of(&(start.into()..end.into()))
    }

    #[test]
    fn a_block_range_ends_at_the_block_below_its_exclusive_end() {
        let range = span(FIRST, END).map(|span| block_range(span, pool_types(false)));

        assert_eq!(
            range,
            Some(request::block_range(
                u64::from(FIRST),
                u64::from(LAST),
                &SHIELDED_POOL_TYPES
            ))
        );
    }

    #[test]
    fn an_empty_block_range_builds_no_request() {
        assert_eq!(span(FIRST, FIRST), None);
    }

    #[test]
    fn an_inverted_block_range_builds_no_request() {
        assert_eq!(span(END, FIRST), None);
    }

    #[test]
    fn a_block_range_that_ends_at_genesis_builds_no_request() {
        assert_eq!(span(GENESIS, GENESIS), None);
    }
}
