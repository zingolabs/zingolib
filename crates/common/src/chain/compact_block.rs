use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::BlockHeight;

pub mod chain_metadata;
pub mod compact_orchard_action;
pub mod compact_sapling_output;
pub mod compact_sapling_spend;
pub mod compact_tx;
pub mod compact_tx_in;
pub mod tx_out;

pub use chain_metadata::ChainMetadata;
pub use compact_orchard_action::CompactOrchardAction;
pub use compact_sapling_output::CompactSaplingOutput;
pub use compact_sapling_spend::CompactSaplingSpend;
pub use compact_tx::CompactTx;
pub use compact_tx_in::CompactTxIn;
pub use tx_out::TxOut;

/// A [`CompactBlock`] (or one of its nested parts) that failed to parse from its wire
/// representation: a byte field of the wrong length, or a height that does not fit a
/// [`BlockHeight`].
#[derive(Debug, thiserror::Error)]
pub enum InvalidCompactBlock {
    /// The block height does not fit a [`BlockHeight`].
    #[error("invalid block height")]
    InvalidHeight,
    /// A fixed-length field was not the expected number of bytes.
    #[error("{field} is not {expected} bytes (found {found})")]
    InvalidFieldLength {
        /// The dotted struct.field name of the offending field, e.g. `"CompactBlock.hash"`.
        field: &'static str,
        /// The length the field must have.
        expected: usize,
        /// The length actually found.
        found: usize,
    },
}

pub(crate) fn fixed_bytes<const N: usize>(
    field: &'static str,
    bytes: Vec<u8>,
) -> Result<[u8; N], InvalidCompactBlock> {
    let found = bytes.len();
    bytes
        .try_into()
        .map_err(|_| InvalidCompactBlock::InvalidFieldLength {
            field,
            expected: N,
            found,
        })
}

/// A compact block: the minimum data a wallet needs to detect payments, detect spends, update
/// witnesses, and spend transparent UTXOs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactBlock {
    /// The height of this block.
    pub height: BlockHeight,
    /// The hash of this block.
    pub hash: BlockHash,
    /// The hash of this block's predecessor.
    pub prev_hash: BlockHash,
    /// The Unix epoch time the block was mined, or zero if the server does not report it.
    pub time: u32,
    /// The full serialized block header, as returned by the `getblock` RPC, or empty if the
    /// server does not report it.
    pub header: Vec<u8>,
    /// The block's compact transactions.
    pub vtx: Vec<CompactTx>,
    /// The state of the chain as of the end of this block, when the server reports it.
    pub chain_metadata: Option<ChainMetadata>,
}

impl CompactBlock {
    /// Parses a compact block's own scalar fields, taking its transactions already parsed.
    pub fn from_wire_parts(
        height: u64,
        hash: Vec<u8>,
        prev_hash: Vec<u8>,
        time: u32,
        header: Vec<u8>,
        vtx: Vec<CompactTx>,
        chain_metadata: Option<ChainMetadata>,
    ) -> Result<Self, InvalidCompactBlock> {
        Ok(Self {
            height: BlockHeight::try_from(height)
                .map_err(|_| InvalidCompactBlock::InvalidHeight)?,
            hash: BlockHash::try_from_slice(&hash).ok_or(
                InvalidCompactBlock::InvalidFieldLength {
                    field: "CompactBlock.hash",
                    expected: 32,
                    found: hash.len(),
                },
            )?,
            prev_hash: BlockHash::try_from_slice(&prev_hash).ok_or(
                InvalidCompactBlock::InvalidFieldLength {
                    field: "CompactBlock.prev_hash",
                    expected: 32,
                    found: prev_hash.len(),
                },
            )?,
            time,
            header,
            vtx,
            chain_metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CompactBlock;
    use zcash_primitives::block::BlockHash;

    /// A height one past `u32::MAX`, the smallest value `BlockHeight::try_from` rejects.
    const TOO_TALL: u64 = u32::MAX as u64 + 1;

    fn parts() -> (u64, Vec<u8>, Vec<u8>) {
        (100, vec![0xaa; 32], vec![0xbb; 32])
    }

    #[test]
    fn valid_parts_build() {
        let (height, hash, prev_hash) = parts();
        let block = CompactBlock::from_wire_parts(height, hash, prev_hash, 0, vec![], vec![], None)
            .unwrap();
        assert_eq!(block.hash, BlockHash([0xaa; 32]));
        assert_eq!(block.prev_hash, BlockHash([0xbb; 32]));
    }

    #[test]
    fn a_height_past_u32_max_is_rejected() {
        let (_, hash, prev_hash) = parts();
        assert!(
            CompactBlock::from_wire_parts(TOO_TALL, hash, prev_hash, 0, vec![], vec![], None)
                .is_err()
        );
    }

    #[test]
    fn a_wrong_length_hash_is_rejected() {
        let (height, _, prev_hash) = parts();
        assert!(
            CompactBlock::from_wire_parts(
                height,
                vec![0xaa; 20],
                prev_hash,
                0,
                vec![],
                vec![],
                None
            )
            .is_err()
        );
    }

    #[test]
    fn a_wrong_length_prev_hash_is_rejected() {
        let (height, hash, _) = parts();
        assert!(
            CompactBlock::from_wire_parts(height, hash, vec![0xbb; 20], 0, vec![], vec![], None)
                .is_err()
        );
    }
}
