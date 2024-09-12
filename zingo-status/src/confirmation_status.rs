//! If a note is confirmed, it is:
//!  Confirmed === on-record on-chain at BlockHeight

use zcash_primitives::consensus::BlockHeight;
/// Transaction confirmation states. Every transaction record includes exactly one of these variants.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmationStatus {
    /// the transaction has been calculated but not yet broadcast to the chain.
    Calculated(BlockHeight),

    /// The transaction has been sent to the zcash blockchain. It could be in the mempool, but if
    /// it's known to be, it will be Mempool instead,
    /// The BlockHeight is the 1 + the height of the chain as the transaction was broadcast, i.e. the target height.
    Transmitted(BlockHeight),

    /// The transaction is known to be or have been in the mempool.
    /// The BlockHeight is the 1 + the height of the chain as the transaction entered the mempool, i.e. the target height.
    Mempool(BlockHeight),

    /// The transaction has been included in at-least one block mined to the zcash blockchain.
    /// The height of a confirmed block that contains the transaction.
    Confirmed(BlockHeight),
}

impl ConfirmationStatus {
    /// Converts from a blockheight and `pending`. pending is deprecated and is only needed in loading from save.
    pub fn from_blockheight_and_pending_bool(blockheight: BlockHeight, pending: bool) -> Self {
        if pending {
            Self::Transmitted(blockheight)
        } else {
            Self::Confirmed(blockheight)
        }
    }

    /// Is pending/unconfirmed. Use is_transmitted/is_mempool where possible
    /// pending is used to mean either Transmitted or in mempool. transactions yet to be broadcast are NOT considered pending
    /// TOdo! this may create misunderstanding errors, that may be difficult to understand. we must replace this function with is_confirmed wherever possible. also, use matches!
    pub fn is_pending(&self) -> bool {
        match self {
            ConfirmationStatus::Transmitted(_) | ConfirmationStatus::Mempool(_) => true,
            _ => false,
        }
    }

    /// A wrapper matching the Transmitted case.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Transmitted(10.into());
    /// assert_eq!(status.is_transmitted(), true);
    /// assert_eq!(status.is_confirmed(), false);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_transmitted(), false);
    /// assert_eq!(status.is_confirmed(), true);
    /// ```
    pub fn is_transmitted(&self) -> bool {
        matches!(self, Self::Transmitted(_))
    }
    /// A wrapper matching the Mempool case.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Mempool(10.into());
    /// assert_eq!(status.is_mempool(), true);
    /// assert_eq!(status.is_confirmed(), false);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_mempool(), false);
    /// assert_eq!(status.is_confirmed(), true);
    /// ```
    pub fn is_mempool(&self) -> bool {
        matches!(self, Self::Mempool(_))
    }

    /// A wrapper matching the Confirmed case.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Mempool(10.into());
    /// assert_eq!(status.is_confirmed(), false);
    /// assert_eq!(status.is_pending(), true);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed(), true);
    /// assert_eq!(status.is_pending(), false);
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
    /// assert_eq!(status.is_confirmed_after_or_at(&9.into()), true);
    ///
    /// let status = ConfirmationStatus::Mempool(10.into());
    /// assert_eq!(status.is_confirmed_after_or_at(&10.into()), false);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed_after_or_at(&11.into()), false);
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
    /// assert_eq!(status.is_confirmed_before_or_at(&9.into()), false);
    ///
    /// let status = ConfirmationStatus::Mempool(10.into());
    /// assert_eq!(status.is_confirmed_before_or_at(&10.into()), false);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed_before_or_at(&11.into()), true);
    /// ```
    pub fn is_confirmed_before_or_at(&self, comparison_height: &BlockHeight) -> bool {
        match self {
            Self::Confirmed(self_height) => {
                self.is_confirmed_before(comparison_height) || self_height == comparison_height
            }
            _ => false,
        }
    }

    /// To return true, the status must be confirmed earlier than specified height.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed_before(&9.into()), false);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed_before(&10.into()), false);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed_before(&11.into()), true);
    /// ```
    pub fn is_confirmed_before(&self, comparison_height: &BlockHeight) -> bool {
        match self {
            Self::Confirmed(self_height) => self_height < comparison_height,
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
    /// assert_eq!(status.is_pending_before(&15.into()), false);
    ///
    /// let status = ConfirmationStatus::Mempool(12.into());
    /// assert_eq!(status.is_pending_before(&13.into()), true);
    ///
    /// let status = ConfirmationStatus::Mempool(14.into());
    /// assert_eq!(status.is_pending_before(&14.into()), false);
    /// ```
    // TODO remove 'pending' and fix spend status.
    pub fn is_pending_before(&self, comparison_height: &BlockHeight) -> bool {
        match self {
            Self::Transmitted(self_height) | Self::Mempool(self_height) => {
                self_height < comparison_height
            }
            _ => false,
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
    /// let status = ConfirmationStatus::Mempool(15.into());
    /// assert_eq!(status.get_confirmed_height(), None);
    /// ```
    pub fn get_confirmed_height(&self) -> Option<BlockHeight> {
        match self {
            Self::Confirmed(self_height) => Some(*self_height),
            _ => None,
        }
    }

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
            Self::Calculated(self_height) => *self_height,
            Self::Mempool(self_height) => *self_height,
            Self::Transmitted(self_height) => *self_height,
            Self::Confirmed(self_height) => *self_height,
        }
    }
}

impl std::fmt::Display for ConfirmationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Calculated(_) => {
                write!(f, "transmitted")
            }
            Self::Transmitted(_) => {
                write!(f, "transmitted")
            }
            Self::Mempool(_) => {
                write!(f, "mempool")
            }
            Self::Confirmed(_) => {
                write!(f, "confirmed")
            }
        }
    }
}

impl From<ConfirmationStatus> for String {
    fn from(value: ConfirmationStatus) -> Self {
        format!("{value}")
    }
}
