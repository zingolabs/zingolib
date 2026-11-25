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

pub(crate) type CompactBlocks = tonic::Streaming<CompactBlock>;
pub(crate) type RawTransactions = tonic::Streaming<RawTransaction>;
pub(crate) type SubtreeRoots = tonic::Streaming<SubtreeRoot>;

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

const EXCLUSIVE_END: u64 = 1;
const RANGE_ENDS_AT_GENESIS: &str = "a block range must end above height 0";

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

pub(crate) fn block_range(
    range: Range<BlockHeight>,
    pool_types: &[PoolType],
) -> Result<BlockRange, tonic::Status> {
    let last = u64::from(range.end)
        .checked_sub(EXCLUSIVE_END)
        .ok_or_else(|| tonic::Status::invalid_argument(RANGE_ENDS_AT_GENESIS))?;

    Ok(request::block_range(
        u64::from(range.start),
        last,
        pool_types,
    ))
}

pub(crate) fn nullifier_range(range: Range<BlockHeight>) -> Result<BlockRange, tonic::Status> {
    block_range(range, &EVERY_POOL_BY_DEFAULT)
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
    range: Range<BlockHeight>,
) -> Result<TransparentAddressBlockFilter, tonic::Status> {
    Ok(TransparentAddressBlockFilter {
        address,
        range: Some(nullifier_range(range)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: u32 = 0;
    const FIRST: u32 = 3;
    const END: u32 = 6;
    const LAST: u64 = 5;

    #[test]
    fn a_block_range_ends_at_the_block_below_its_exclusive_end() {
        let range = block_range(FIRST.into()..END.into(), pool_types(false));

        assert_eq!(
            range.map_err(|refusal| refusal.to_string()),
            Ok(request::block_range(
                u64::from(FIRST),
                LAST,
                &SHIELDED_POOL_TYPES
            ))
        );
    }

    #[test]
    fn a_block_range_that_ends_at_genesis_is_refused() {
        let range = block_range(GENESIS.into()..GENESIS.into(), pool_types(true));

        assert!(matches!(
            range,
            Err(refusal) if refusal.code() == tonic::Code::InvalidArgument
        ));
    }
}
