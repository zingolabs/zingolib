//! A transaction can be:
//!  ClientOnly === not broadcast in-band to the mempool or blockchain
//!  FromMempool === not on-record on-chain, but in band, in the mempool (gossip)
//!  OnChain === on-record on-chain at BlockHeight

use zcash_primitives::consensus::BlockHeight;
/// Transaction confirmation states. Every transaction record includes exactly one of these variants.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransactionSource {
    /// If the transaction has not been broadcast (in band) then it is known only to the client
    ClientOnly(BlockHeight),
    /// The transaction is pending confirmation to the zcash blockchain. It may be waiting in the mempool.
    /// The BlockHeight is the 1 + the height of the chain as the transaction was broadcast, i.e. the target height.
    FromMempool(BlockHeight),
    /// The transaction has been included in at-least one block mined to the zcash blockchain.
    /// The height of a confirmed block that contains the transaction.
    OnChain(BlockHeight),
}

impl TransactionSource {
    /// Converts from a blockheight and `pending`. pending is deprecated and is only needed in loading from save.
    pub fn from_blockheight_and_pending_bool(blockheight: BlockHeight, pending: bool) -> Self {
        if pending {
            Self::FromMempool(blockheight)
        } else {
            Self::OnChain(blockheight)
        }
    }

    /// A wrapper matching the Pending case.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Pending(10.into());
    /// assert_eq!(status.is_pending(), true);
    /// assert_eq!(status.is_confirmed(), false);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_pending(), false);
    /// assert_eq!(status.is_confirmed(), true);
    /// ```
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::FromMempool(_))
    }

    /// A wrapper matching the Confirmed case.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Pending(10.into());
    /// assert_eq!(status.is_confirmed(), false);
    /// assert_eq!(status.is_pending(), true);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed(), true);
    /// assert_eq!(status.is_pending(), false);
    /// ```
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::OnChain(_))
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
    /// let status = ConfirmationStatus::Pending(10.into());
    /// assert_eq!(status.is_confirmed_after_or_at(&10.into()), false);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed_after_or_at(&11.into()), false);
    /// ```
    pub fn is_confirmed_after_or_at(&self, comparison_height: &BlockHeight) -> bool {
        match self {
            Self::OnChain(self_height) => self_height >= comparison_height,
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
    /// let status = ConfirmationStatus::Pending(10.into());
    /// assert_eq!(status.is_confirmed_before_or_at(&10.into()), false);
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_confirmed_before_or_at(&11.into()), true);
    /// ```
    pub fn is_confirmed_before_or_at(&self, comparison_height: &BlockHeight) -> bool {
        match self {
            Self::OnChain(self_height) => {
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
            Self::OnChain(self_height) => self_height < comparison_height,
            _ => false,
        }
    }

    /// To return true, the status must have broadcast at or later than specified height.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// let status = ConfirmationStatus::Confirmed(10.into());
    /// assert_eq!(status.is_pending_after_or_at(&9.into()), false);
    ///
    /// let status = ConfirmationStatus::Pending(10.into());
    /// assert_eq!(status.is_pending_after_or_at(&10.into()), true);
    ///
    /// let status = ConfirmationStatus::Pending(10.into());
    /// assert_eq!(status.is_pending_after_or_at(&11.into()), false);
    /// ```
    pub fn is_pending_after_or_at(&self, comparison_height: &BlockHeight) -> bool {
        match self {
            Self::FromMempool(self_height) => self_height >= comparison_height,
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
    /// let status = ConfirmationStatus::Pending(12.into());
    /// assert_eq!(status.is_pending_before(&13.into()), true);
    ///
    /// let status = ConfirmationStatus::Pending(14.into());
    /// assert_eq!(status.is_pending_before(&14.into()), false);
    /// ```
    pub fn is_pending_before(&self, comparison_height: &BlockHeight) -> bool {
        match self {
            Self::FromMempool(self_height) => self_height < comparison_height,
            Self::OnChain(_) => false,
            Self::ClientOnly(_) => false,
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
    /// let status = ConfirmationStatus::Pending(15.into());
    /// assert_eq!(status.get_confirmed_height(), None);
    /// ```
    pub fn get_confirmed_height(&self) -> Option<BlockHeight> {
        match self {
            Self::OnChain(self_height) => Some(*self_height),
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
    /// assert_eq!(status.get_pending_height(), None);
    ///
    /// let status = ConfirmationStatus::Pending(15.into());
    /// assert_eq!(status.get_pending_height(), Some(15.into()));
    /// ```
    pub fn get_pending_height(&self) -> Option<BlockHeight> {
        match self {
            Self::FromMempool(self_height) => Some(*self_height),
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
            Self::FromMempool(self_height) => *self_height,
            Self::OnChain(self_height) => *self_height,
            Self::ClientOnly(self_height) => *self_height,
        }
    }
}

impl std::fmt::Display for TransactionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FromMempool(_) => {
                write!(f, "mempool")
            }
            Self::OnChain(_) => {
                write!(f, "onchain")
            }
            Self::ClientOnly(_) => {
                write!(f, "clientonly")
            }
        }
    }
}

impl From<TransactionSource> for String {
    fn from(value: TransactionSource) -> Self {
        format!("{value}")
    }
}
