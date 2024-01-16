use zcash_primitives::consensus::BlockHeight;
/// Transaction confirmation states. Every transaction maps to exactly one of these variants.
/// Transitions between states are enforced by ????
/// TODO: If we want an exhaustive partition of states, do we have all necessary variants?

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmationStatus {
    /// The value the client has for the height of the chain when the transaction is broadcast.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Broadcast(BlockHeight::from_u32(100));
    /// assert_eq!(status.is_broadcast(), true);
    /// assert_eq!(status.get_height(), BlockHeight::from_u32(100));
    /// ```
    Broadcast(BlockHeight),
    /// The height of a confirmed block that contains the transaction.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Confirmed(BlockHeight::from_u32(200));
    /// assert_eq!(status.is_confirmed(), true);
    /// assert_eq!(status.get_height(), BlockHeight::from_u32(200));
    /// ```
    Confirmed(BlockHeight),
}

impl ConfirmationStatus {
    /// Converts a blockheight and unconfirmed status into a `ConfirmationStatus`.
    /// TODO:  I think we are missing a state, what about after a record is created, but before it's broadcast?!
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::{ConfirmationStatus, BlockHeight};
    ///
    /// let status = ConfirmationStatus::from_blockheight_and_unconfirmed_bool(BlockHeight(10), false);
    /// assert_eq!(status, ConfirmationStatus::Confirmed(BlockHeight(10)));
    ///
    /// let status = ConfirmationStatus::from_blockheight_and_unconfirmed_bool(BlockHeight(10), true);
    /// assert_eq!(status, ConfirmationStatus::Broadcast(BlockHeight(10)));
    /// ```
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
