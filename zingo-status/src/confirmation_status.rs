use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

/// A 32 wide bitmask with 0 in the last 5 places
pub const BLOCKHEIGHT_PLACEHOLDER_LOCAL: u32 = <u32>::max_value() - (16 + 8 + 4 + 2 + 1);
/// A 32 wide bitmask with 1 in the least significant place, and 0 inn each of the next 4
pub const BLOCKHEIGHT_PLACEHOLDER_INMEMPOOL: u32 = <u32>::max_value() - (16 + 8 + 4 + 2);

/// A 32 wide bitmask with 0 at 2^5, 2^3, 2^2, 2^1, and 2^0
pub const BLOCKHEIGHT_PLACEHOLDER_NOKNOWNSPENDS: u32 = <u32>::max_value() - (32 + 8 + 4 + 2 + 1);
/// A 32 wide bitmask with 0 at 2^5, 2^3, 2^2, and 2^1
pub const BLOCKHEIGHT_PLACEHOLDER_PENDINGSPEND: u32 = <u32>::max_value() - (32 + 8 + 4 + 2);

fn u32_height_or_placeholder(option_blockheight: Option<BlockHeight>) -> u32 {
    match option_blockheight {
        Some(block_height) => u32::from(block_height),
        None => BLOCKHEIGHT_PLACEHOLDER_INMEMPOOL,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmationStatus {
    Local,
    /// we may know when it entered the mempool.
    InMempool(Option<BlockHeight>),
    /// confirmed on blockchain implies a height. this data piece will eventually be a block height
    ConfirmedOnChain(BlockHeight),
}

impl ConfirmationStatus {
    pub fn from_blockheight_and_unconfirmed_bool(
        block_height: BlockHeight,
        unconfirmed: bool,
    ) -> Self {
        if unconfirmed {
            Self::Local
        } else {
            Self::ConfirmedOnChain(block_height)
        }
    }
    pub fn is_in_mempool(&self) -> bool {
        matches!(self, Self::InMempool(_))
    }
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::ConfirmedOnChain(_))
    }
    pub fn is_confirmed_after_or_at(&self, height: &BlockHeight) -> bool {
        match self {
            Self::ConfirmedOnChain(block_height) => block_height >= height,
            _ => false,
        }
    }
    pub fn is_confirmed_before_or_at(&self, height: &BlockHeight) -> bool {
        match self {
            Self::ConfirmedOnChain(block_height) => block_height <= height,
            _ => false,
        }
    }
    pub fn is_expired(&self, cutoff: &BlockHeight) -> bool {
        match self {
            Self::Local => true,
            Self::InMempool(option_blockheight) => match option_blockheight {
                None => true,
                Some(block_height) => block_height < cutoff,
            },
            Self::ConfirmedOnChain(_) => false,
        }
    }
    // temporary. fixing this should fix the confirmation bug. please use match whenever possible.
    pub fn get_height(&self) -> BlockHeight {
        match self {
            Self::Local => BlockHeight::from_u32(BLOCKHEIGHT_PLACEHOLDER_LOCAL),
            Self::InMempool(opt_block) => {
                opt_block.unwrap_or(BlockHeight::from_u32(BLOCKHEIGHT_PLACEHOLDER_LOCAL))
            }
            Self::ConfirmedOnChain(block) => *block,
        }
    }
    // this function and the placeholder is not a preferred pattern. please use match whenever possible.
    pub fn get_height_u32_and_is_confirmed(&self) -> (u32, bool) {
        match self {
            Self::Local => (BLOCKHEIGHT_PLACEHOLDER_LOCAL, false),
            Self::InMempool(opt_block) => (u32_height_or_placeholder(*opt_block), false),
            Self::ConfirmedOnChain(block) => (u32::from(*block), true),
        }
    }
    // note, by making unconfirmed the true case, this does a potentially confusing boolean flip
    pub fn get_height_u32_and_is_unconfirmed(&self) -> (u32, bool) {
        match self {
            Self::Local => (BLOCKHEIGHT_PLACEHOLDER_LOCAL, true),
            Self::InMempool(opt_block) => (u32_height_or_placeholder(*opt_block), true),
            Self::ConfirmedOnChain(block) => (u32::from(*block), false),
        }
    }
}
