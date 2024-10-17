//! If a note is confirmed, it is:
//!  Confirmed === on-record on-chain at BlockHeight

use zcash_primitives::consensus::BlockHeight;
/// Transaction confirmation states. Every transaction record includes exactly one of these variants.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmationStatus {
    /// the transaction has been calculated but not yet broadcast to the chain.
    Calculated(BlockHeight),

    /// The transaction has been sent to the zcash blockchain. It could be in the mempool.
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

    /// A wrapper matching the Confirmed case.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed());
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed());
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed());
    /// assert!(ConfirmationStatus::Confirmed(10.into()).is_confirmed());
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
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed_after_or_at(&9.into()));
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed_after_or_at(&10.into()));
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed_after_or_at(&11.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed_after_or_at(&9.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed_after_or_at(&10.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed_after_or_at(&11.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed_after_or_at(&9.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed_after_or_at(&10.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed_after_or_at(&11.into()));
    /// assert!(ConfirmationStatus::Confirmed(10.into()).is_confirmed_after_or_at(&9.into()));
    /// assert!(ConfirmationStatus::Confirmed(10.into()).is_confirmed_after_or_at(&10.into()));
    /// assert!(!ConfirmationStatus::Confirmed(10.into()).is_confirmed_after_or_at(&11.into()));
    /// ```
    pub fn is_confirmed_after_or_at(&self, comparison_height: &BlockHeight) -> bool {
        matches!(self, Self::Confirmed(self_height) if self_height >= comparison_height)
    }

    /// To return true, the status must be confirmed and no later than specified height.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed_before_or_at(&9.into()));
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed_before_or_at(&10.into()));
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed_before_or_at(&11.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed_before_or_at(&9.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed_before_or_at(&10.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed_before_or_at(&11.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed_before_or_at(&9.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed_before_or_at(&10.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed_before_or_at(&11.into()));
    /// assert!(!ConfirmationStatus::Confirmed(10.into()).is_confirmed_before_or_at(&9.into()));
    /// assert!(ConfirmationStatus::Confirmed(10.into()).is_confirmed_before_or_at(&10.into()));
    /// assert!(ConfirmationStatus::Confirmed(10.into()).is_confirmed_before_or_at(&11.into()));
    /// ```
    pub fn is_confirmed_before_or_at(&self, comparison_height: &BlockHeight) -> bool {
        matches!(self, Self::Confirmed(self_height) if self_height <= comparison_height)
    }

    /// To return true, the status must be confirmed earlier than specified height.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed_before(&9.into()));
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed_before(&10.into()));
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed_before(&11.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed_before(&9.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed_before(&10.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed_before(&11.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed_before(&9.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed_before(&10.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed_before(&11.into()));
    /// assert!(!ConfirmationStatus::Confirmed(10.into()).is_confirmed_before(&9.into()));
    /// assert!(!ConfirmationStatus::Confirmed(10.into()).is_confirmed_before(&10.into()));
    /// assert!(ConfirmationStatus::Confirmed(10.into()).is_confirmed_before(&11.into()));
    /// ```
    pub fn is_confirmed_before(&self, comparison_height: &BlockHeight) -> bool {
        matches!(self, Self::Confirmed(self_height) if self_height < comparison_height)
    }

    /// To return true, the status must not be confirmed and it must have been submitted sufficiently far in the past. This allows deduction of expired transactions.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_pending_before(&9.into()));
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_pending_before(&10.into()));
    /// assert!(ConfirmationStatus::Calculated(10.into()).is_pending_before(&11.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_pending_before(&9.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_pending_before(&10.into()));
    /// assert!(ConfirmationStatus::Transmitted(10.into()).is_pending_before(&11.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_pending_before(&9.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_pending_before(&10.into()));
    /// assert!(ConfirmationStatus::Mempool(10.into()).is_pending_before(&11.into()));
    /// assert!(!ConfirmationStatus::Confirmed(10.into()).is_pending_before(&9.into()));
    /// assert!(!ConfirmationStatus::Confirmed(10.into()).is_pending_before(&10.into()));
    /// assert!(!ConfirmationStatus::Confirmed(10.into()).is_pending_before(&11.into()));
    /// ```
    // TODO remove 'pending' and fix spend status.
    pub fn is_pending_before(&self, comparison_height: &BlockHeight) -> bool {
        match self {
            Self::Calculated(self_height)
            | Self::Transmitted(self_height)
            | Self::Mempool(self_height) => self_height < comparison_height,
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
                write!(f, "calculated")
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
