use zcash_primitives::consensus::BlockHeight;

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
        Some(blockheight) => u32::from(blockheight),
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
        blockheight: BlockHeight,
        unconfirmed: bool,
    ) -> Self {
        if unconfirmed {
            Self::Local
        } else {
            Self::ConfirmedOnChain(blockheight)
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
            Self::ConfirmedOnChain(blockheight) => blockheight >= height,
            _ => false,
        }
    }
    pub fn is_confirmed_before_or_at(&self, height: &BlockHeight) -> bool {
        match self {
            Self::ConfirmedOnChain(blockheight) => blockheight <= height,
            _ => false,
        }
    }
    pub fn is_expired(&self, cutoff: &BlockHeight) -> bool {
        match self {
            Self::Local => true,
            Self::InMempool(option_blockheight) => match option_blockheight {
                None => true,
                Some(blockheight) => blockheight < cutoff,
            },
            Self::ConfirmedOnChain(_) => false,
        }
    }
    // temporary. fixing this should fix the confirmation bug. please use match whenever possible.
    pub fn get_height(&self) -> BlockHeight {
        match self {
            Self::Local => BlockHeight::from_u32(BLOCKHEIGHT_PLACEHOLDER_LOCAL),
            Self::InMempool(option_blockheight) => {
                option_blockheight.unwrap_or(BlockHeight::from_u32(BLOCKHEIGHT_PLACEHOLDER_LOCAL))
            }
            Self::ConfirmedOnChain(blockheight) => *blockheight,
        }
    }
    // this function and the placeholder is not a preferred pattern. please use match whenever possible.
    pub fn get_height_u32_and_is_confirmed(&self) -> (u32, bool) {
        match self {
            Self::Local => (BLOCKHEIGHT_PLACEHOLDER_LOCAL, false),
            Self::InMempool(option_blockheight) => {
                (u32_height_or_placeholder(*option_blockheight), false)
            }
            Self::ConfirmedOnChain(blockheight) => (u32::from(*blockheight), true),
        }
    }
    // note, by making unconfirmed the true case, this does a potentially confusing boolean flip
    pub fn get_height_u32_and_is_unconfirmed(&self) -> (u32, bool) {
        match self {
            Self::Local => (BLOCKHEIGHT_PLACEHOLDER_LOCAL, true),
            Self::InMempool(option_blockheight) => {
                (u32_height_or_placeholder(*option_blockheight), true)
            }
            Self::ConfirmedOnChain(blockheight) => (u32::from(*blockheight), false),
        }
    }
}

impl std::fmt::Display for ConfirmationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ConfirmationStatus::*;
        match self {
            Local => write!(f, "Transaction not published."),
            InMempool(option_blockheight) => match option_blockheight {
                None => write!(f, "Transaction sent to mempool at unknown height.",),
                Some(blockheight) => {
                    write!(f, "Transaction sent to mempool at height {}.", blockheight)
                }
            },
            ConfirmedOnChain(blockheight) => {
                write!(
                    f,
                    "Transaction confirmed on chain at height {}.",
                    blockheight
                )
            }
        }
    }
}

impl From<ConfirmationStatus> for String {
    fn from(value: ConfirmationStatus) -> Self {
        format!("{value}")
    }
}
