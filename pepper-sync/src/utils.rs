use zcash_primitives::block::{BlockHash, BlockHeader};
use zcash_protocol::{TxId, consensus::BlockHeight};
use zingo_netutils::lightwallet_protocol::{CompactBlock, CompactTx};

/// Returns the [`BlockHash`] for this block.
///
/// # Panics
///
/// This function will panic if [`field@Self::header`] is not set and
/// [`field@Self::hash`] is not exactly 32 bytes.
pub(crate) fn get_compact_block_hash(compact_block: &CompactBlock) -> BlockHash {
    if let Some(header) = get_compact_block_header(compact_block) {
        header.hash()
    } else {
        BlockHash::from_slice(&compact_block.hash)
    }
}

/// Returns the [`BlockHash`] for this block's parent.
///
/// # Panics
///
/// This function will panic if [`field@Self::header`] is not set and
/// [`field@Self::prev_hash`] is not exactly 32 bytes.
pub(crate) fn get_compact_block_prev_hash(compact_block: &CompactBlock) -> BlockHash {
    if let Some(header) = get_compact_block_header(compact_block) {
        header.prev_block
    } else {
        BlockHash::from_slice(&compact_block.prev_hash)
    }
}

/// Returns the [`BlockHeight`] value for this block
///
/// # Panics
///
/// This function will panic if [`field@Self::height`] is not representable within a
/// `u32`.
pub(crate) fn get_compact_block_height(compact_block: &CompactBlock) -> BlockHeight {
    compact_block.height.try_into().unwrap()
}

/// Returns the [`BlockHeader`] for this block if present.
///
/// A convenience method that parses [`field@Self::header`] if present.
pub(crate) fn get_compact_block_header(compact_block: &CompactBlock) -> Option<BlockHeader> {
    if compact_block.header.is_empty() {
        None
    } else {
        BlockHeader::read(&compact_block.header[..]).ok()
    }
}

/// Returns the transaction Id
pub(crate) fn get_compact_tx_txid(compact_tx: &CompactTx) -> TxId {
    let mut txid_bytes = [0u8; 32];
    txid_bytes.copy_from_slice(&compact_tx.txid);
    TxId::from_bytes(txid_bytes)
}
