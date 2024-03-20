use zcash_primitives::consensus::BlockHeight;
/// Transaction confirmation states. Every transaction record includes exactly one of these variants.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmationStatus {
    /// The transaction has been broadcast to the zcash blockchain. It may be waiting in the mempool.
    /// The height of the chain as the transaction was broadcast.
    Broadcast(BlockHeight),
    /// The transaction has been included in at-least one block mined to the zcash blockchain.
    /// The height of a confirmed block that contains the transaction.
    Confirmed(BlockHeight),
}

impl ConfirmationStatus {
    /// Converts from a blockheight and `unconfirmed`. unconfirmed is deprecated and is only needed in loading from save.
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
    /// A wrapper matching the Broadcast case.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Broadcast(10.into());
    /// assert_eq!(status.is_broadcast(), true);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_broadcast(), false);
    /// ```
    pub fn is_broadcast(&self) -> bool {
        matches!(self, Self::Broadcast(_))
    }
    /// A wrapper matching the Confirmed case.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Broadcast(10.into());
    /// assert_eq!(status.is_confirmed(), false);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed(), true);
    /// ```
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed(_))
    }
    /// To return true, the status must be confirmed and no earlier than specified height.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed_after_or_at(&8.into()), true);
    ///
    /// let status = ConfirmationStatus::Broadcast(10.into());
    /// assert_eq!(status.is_confirmed_after_or_at(&10.into()), false);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed_after_or_at(&12.into()), false);
    /// ```
    pub fn is_confirmed_after_or_at(&self, comparison_height: &BlockHeight) -> bool {
        match self {
            Self::Confirmed(self_height) => self_height >= comparison_height,
            _ => false,
        }
    }
    /// To return true, the status must be confirmed and no later than specified height.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed_before_or_at(&8.into()), false);
    ///
    /// let status = ConfirmationStatus::Broadcast(10.into());
    /// assert_eq!(status.is_confirmed_before_or_at(&10.into()), false);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed_before_or_at(&12.into()), true);
    /// ```
    pub fn is_confirmed_before_or_at(&self, comparison_height: &BlockHeight) -> bool {
        match self {
            Self::Confirmed(self_height) => self_height <= comparison_height,
            _ => false,
        }
    }
    pub fn is_broadcast_after_or_at(&self, comparison_height: &BlockHeight) -> bool {
        match self {
            Self::Broadcast(self_height) => self_height >= comparison_height,
            _ => false,
        }
    }
    /// To return true, the status must not be confirmed and it must have been submitted sufficiently far in the past. This allows deduction of expired transactions.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Confirmed(16.into());
    /// assert_eq!(status.is_broadcast_before(&14.into()), false);
    ///
    /// let status = ConfirmationStatus::Broadcast(12.into());
    /// assert_eq!(status.is_broadcast_before(&14.into()), true);
    ///
    /// let status = ConfirmationStatus::Broadcast(14.into());
    /// assert_eq!(status.is_broadcast_before(&14.into()), false);
    /// ```
    pub fn is_broadcast_before(&self, comparison_height: &BlockHeight) -> bool {
        match self {
            Self::Broadcast(self_height) => self_height < comparison_height,
            Self::Confirmed(_) => false,
        }
    }
    /// Returns none if transaction is not confirmed, otherwise returns the height it was confirmed at.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Confirmed(16.into());
    /// assert_eq!(status.get_confirmed_height(), Some(16.into()));
    ///
    /// let status = ConfirmationStatus::Broadcast(15.into());
    /// assert_eq!(status.get_confirmed_height(), None);
    /// ```
    pub fn get_confirmed_height(&self) -> Option<BlockHeight> {
        match self {
            Self::Confirmed(self_height) => Some(*self_height),
            _ => None,
        }
    }
    /// Returns if transaction is confirmed, otherwise returns the height it was broadcast to the mempool.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Confirmed(16.into());
    /// assert_eq!(status.get_broadcast_height(), None);
    ///
    /// let status = ConfirmationStatus::Broadcast(15.into());
    /// assert_eq!(status.get_broadcast_height(), Some(15.into()));
    /// ```
    pub fn get_broadcast_height(&self) -> Option<BlockHeight> {
        match self {
            Self::Broadcast(self_height) => Some(*self_height),
            _ => None,
        }
    }
    /// this function and the placeholder is not a preferred pattern. please use match whenever possible.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Confirmed(15.into());
    /// assert_eq!(status.get_height(), 15.into());
    /// ```
    pub fn get_height(&self) -> BlockHeight {
        match self {
            Self::Broadcast(self_height) => *self_height,
            Self::Confirmed(self_height) => *self_height,
        }
    }
}

impl std::fmt::Display for ConfirmationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Broadcast(self_height) => {
                write!(f, "Transaction sent to mempool at height {}.", self_height)
            }
            Self::Confirmed(self_height) => {
                write!(
                    f,
                    "Transaction confirmed on chain at height {}.",
                    self_height
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
