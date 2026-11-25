#![forbid(unsafe_code)]

use zcash_protocol::consensus::BlockHeight;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawTransactionStatus {
    Mempool,
    MinedOffMainChain,
    Mined(BlockHeight),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Transaction height {0} is neither a block height nor a sentinel.")]
pub struct InvalidRawTransaction(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTransaction {
    pub data: Vec<u8>,
    pub status: RawTransactionStatus,
}

impl RawTransactionStatus {
    pub const MEMPOOL_HEIGHT: u64 = 0;
    pub const OFF_MAIN_CHAIN_HEIGHT: u64 = u64::MAX;

    pub fn of_height(height: u64) -> Result<Self, InvalidRawTransaction> {
        match height {
            Self::MEMPOOL_HEIGHT => Ok(Self::Mempool),
            Self::OFF_MAIN_CHAIN_HEIGHT => Ok(Self::MinedOffMainChain),
            mined => BlockHeight::try_from(mined)
                .map(Self::Mined)
                .map_err(|_| InvalidRawTransaction(mined)),
        }
    }
}

#[cfg(test)]
mod tests {
    use zcash_protocol::consensus::BlockHeight;

    use super::{InvalidRawTransaction, RawTransactionStatus};

    const MINED_HEIGHT: u32 = 12_345;
    const TOO_TALL: u64 = u32::MAX as u64 + 1;

    #[test]
    fn the_mempool_sentinel_is_the_mempool() {
        assert_eq!(
            RawTransactionStatus::of_height(RawTransactionStatus::MEMPOOL_HEIGHT),
            Ok(RawTransactionStatus::Mempool)
        );
    }

    #[test]
    fn the_off_main_chain_sentinel_is_mined_off_the_main_chain() {
        assert_eq!(
            RawTransactionStatus::of_height(RawTransactionStatus::OFF_MAIN_CHAIN_HEIGHT),
            Ok(RawTransactionStatus::MinedOffMainChain)
        );
    }

    #[test]
    fn a_block_height_is_mined_at_that_height() {
        assert_eq!(
            RawTransactionStatus::of_height(u64::from(MINED_HEIGHT)),
            Ok(RawTransactionStatus::Mined(BlockHeight::from_u32(
                MINED_HEIGHT
            )))
        );
    }

    #[test]
    fn a_height_above_the_block_height_range_is_invalid() {
        assert_eq!(
            RawTransactionStatus::of_height(TOO_TALL),
            Err(InvalidRawTransaction(TOO_TALL))
        );
    }
}
