use zcash_protocol::consensus::BlockHeight;

/// Where a transaction stands relative to the main chain, as reported by a [`RawTransaction`]'s
/// wire height field. The wire field overloads one `u64` with three meanings (mempool, mined on
/// a fork, mined at a height); this type makes each one explicit instead of leaving every
/// consumer to re-derive the sentinel values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawTransactionStatus {
    /// The transaction is in the mempool, not yet mined.
    Mempool,
    /// The transaction was mined, but on a fork that is not the main chain.
    MinedOffMainChain,
    /// The transaction was mined on the main chain at this height.
    Mined(BlockHeight),
}

/// A [`RawTransaction`] that failed to parse from its wire representation: a mined height that
/// is neither of the wire format's two sentinel values, nor fits a [`BlockHeight`].
#[derive(Debug, thiserror::Error)]
#[error("mined height does not fit a block height")]
pub struct InvalidRawTransaction;

/// A full serialized transaction, as served by an indexer, with its mined status already
/// parsed out of the wire format's overloaded height field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTransaction {
    /// The serialized representation of the transaction.
    pub data: Vec<u8>,
    /// Where this transaction stands relative to the main chain.
    pub status: RawTransactionStatus,
}

impl RawTransaction {
    /// Parses a raw transaction from its wire parts, decoding the wire height's sentinel
    /// values (`0` for mempool, `u64::MAX` for mined-off-main-chain) into [`RawTransactionStatus`].
    pub fn from_wire_parts(data: Vec<u8>, height: u64) -> Result<Self, InvalidRawTransaction> {
        let status = match height {
            0 => RawTransactionStatus::Mempool,
            u64::MAX => RawTransactionStatus::MinedOffMainChain,
            height => RawTransactionStatus::Mined(
                BlockHeight::try_from(height).map_err(|_| InvalidRawTransaction)?,
            ),
        };
        Ok(Self { data, status })
    }
}

/// [`RawTransaction`]'s wire `height` overloads one `u64` with three meanings. This is the
/// exact shape that used to panic (`u32::try_from(height).expect(...)`) on a server-sent `0`
/// or `u64::MAX`; it is now a total, type-safe parse.
#[cfg(test)]
mod tests {
    use super::{RawTransaction, RawTransactionStatus};
    use zcash_protocol::consensus::BlockHeight;

    /// A height one past `u32::MAX`, the smallest value `BlockHeight::try_from` rejects.
    const TOO_TALL: u64 = u32::MAX as u64 + 1;

    #[test]
    fn zero_height_means_mempool() {
        let tx = RawTransaction::from_wire_parts(vec![1, 2, 3], 0).unwrap();
        assert_eq!(tx.status, RawTransactionStatus::Mempool);
        assert_eq!(tx.data, vec![1, 2, 3]);
    }

    #[test]
    fn u64_max_means_mined_off_main_chain() {
        let tx = RawTransaction::from_wire_parts(vec![], u64::MAX).unwrap();
        assert_eq!(tx.status, RawTransactionStatus::MinedOffMainChain);
    }

    #[test]
    fn any_other_height_means_mined_at_that_height() {
        let tx = RawTransaction::from_wire_parts(vec![], 12345).unwrap();
        assert_eq!(
            tx.status,
            RawTransactionStatus::Mined(BlockHeight::from_u32(12345))
        );
    }

    /// A height that is neither sentinel and does not fit a `BlockHeight` is a genuine parse
    /// failure, not a silent wraparound or a panic.
    #[test]
    fn a_mined_height_past_u32_max_is_rejected() {
        assert!(RawTransaction::from_wire_parts(vec![], TOO_TALL).is_err());
    }
}
