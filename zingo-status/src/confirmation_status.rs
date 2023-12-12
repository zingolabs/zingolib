use zcash_primitives::consensus::BlockHeight;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmationStatus {
    /// we may know when it entered the mempool.
    Broadcast(BlockHeight),
    /// confirmed on blockchain implies a height. this data piece will eventually be a block height
    Confirmed(BlockHeight),
}

impl ConfirmationStatus {
    pub fn from_blockheight_and_unconfirmed_bool(
        blockheight: BlockHeight,
        unconfirmed: bool,
    ) -> Self {
        if unconfirmed {
            Self::Broadcast(blockheight)
        } else {
            Self::Confirmed(blockheight)
        }
    }
    pub fn is_broadcast(&self) -> bool {
        matches!(self, Self::Broadcast(_))
    }
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed(_))
    }
    pub fn is_confirmed_after_or_at(&self, height: &BlockHeight) -> bool {
        match self {
            Self::Confirmed(blockheight) => blockheight >= height,
            _ => false,
        }
    }
    pub fn is_confirmed_before_or_at(&self, height: &BlockHeight) -> bool {
        match self {
            Self::Confirmed(blockheight) => blockheight <= height,
            _ => false,
        }
    }
    pub fn is_broadcast_unconfirmed_after(&self, height: &BlockHeight) -> bool {
        match self {
            Self::Broadcast(blockheight) => blockheight <= height,
            _ => false,
        }
    }
    pub fn is_expired(&self, cutoff: &BlockHeight) -> bool {
        match self {
            Self::Broadcast(blockheight) => blockheight < cutoff,
            Self::Confirmed(_) => false,
        }
    }
    pub fn get_confirmed_height(&self) -> Option<BlockHeight> {
        match self {
            Self::Confirmed(blockheight) => Some(*blockheight),
            _ => None,
        }
    }
    pub fn get_broadcast_unconfirmed_height(&self) -> Option<BlockHeight> {
        match self {
            Self::Broadcast(blockheight) => Some(*blockheight),
            _ => None,
        }
    }
    // this function and the placeholder is not a preferred pattern. please use match whenever possible.
    pub fn get_height(&self) -> BlockHeight {
        match self {
            Self::Broadcast(blockheight) => *blockheight,
            Self::Confirmed(blockheight) => *blockheight,
        }
    }
}

impl std::fmt::Display for ConfirmationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ConfirmationStatus::*;
        match self {
            Broadcast(blockheight) => {
                write!(f, "Transaction sent to mempool at height {}.", blockheight)
            }
            Confirmed(blockheight) => {
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
