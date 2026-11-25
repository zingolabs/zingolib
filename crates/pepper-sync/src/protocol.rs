#![forbid(unsafe_code)]

use std::ops::Range;

use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

pub(crate) use zl_netutils::lightwallet_protocol::{
    BlockId, BlockRange, CompactBlock, CompactOrchardAction, CompactSaplingOutput, CompactTx,
    GetAddressUtxosArg, GetAddressUtxosReply, GetSubtreeRootsArg, LightdInfo, PoolType,
    RawTransaction, SubtreeRoot, TransparentAddressBlockFilter, TreeState, TxFilter,
};

#[cfg(test)]
pub(crate) use zl_netutils::lightwallet_protocol::{
    ChainMetadata, CompactSaplingSpend, CompactTxIn, TxOut,
};

pub(crate) type CompactBlocks = tonic::Streaming<CompactBlock>;
pub(crate) type RawTransactions = tonic::Streaming<RawTransaction>;
pub(crate) type SubtreeRoots = tonic::Streaming<SubtreeRoot>;

pub(crate) const SHIELDED_POOL_TYPES: [PoolType; 3] =
    [PoolType::Sapling, PoolType::Orchard, PoolType::Ironwood];
pub(crate) const ALL_POOL_TYPES: [PoolType; 4] = [
    PoolType::Transparent,
    PoolType::Sapling,
    PoolType::Orchard,
    PoolType::Ironwood,
];

const EXCLUSIVE_END: u64 = 1;

fn block_id(height: u64) -> BlockId {
    BlockId {
        height,
        hash: Vec::new(),
    }
}

pub(crate) fn block_id_at(height: BlockHeight) -> BlockId {
    block_id(u64::from(height))
}

pub(crate) fn block_range(range: Range<BlockHeight>, pool_types: &[PoolType]) -> BlockRange {
    BlockRange {
        start: Some(block_id_at(range.start)),
        end: Some(block_id(u64::from(range.end) - EXCLUSIVE_END)),
        pool_types: pool_types.iter().map(|pool| *pool as i32).collect(),
    }
}

pub(crate) fn tx_filter(txid: TxId) -> TxFilter {
    TxFilter {
        block: None,
        index: 0,
        hash: txid.as_ref().to_vec(),
    }
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
    range: Range<BlockHeight>,
) -> TransparentAddressBlockFilter {
    TransparentAddressBlockFilter {
        address,
        range: Some(block_range(range, &[])),
    }
}
