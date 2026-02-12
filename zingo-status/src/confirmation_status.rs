//! If a note is confirmed, it is:
//!  Confirmed === on-record on-chain at `BlockHeight`

use std::io::{Read, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use zcash_primitives::consensus::BlockHeight;

/// Transaction confirmation states. Every transaction record includes exactly one of these variants.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConfirmationStatus {
    /// The transaction has been included in at-least one block mined to the zcash blockchain.
    /// The height of a confirmed block that contains the transaction.
    Confirmed(BlockHeight),
    /// The transaction is known to be or have been in the mempool.
    /// The `BlockHeight` is the 1 + the height of the chain as the transaction entered the mempool, i.e. the target height.
    Mempool(BlockHeight),
    /// The transaction has been sent to the zcash blockchain. It could be in the mempool.
    /// The `BlockHeight` is the 1 + the height of the chain as the transaction was broadcast, i.e. the target height.
    Transmitted(BlockHeight),
    /// The transaction has been calculated but not yet broadcast to the chain.
    /// The `BlockHeight` is the 1 + the height of the chain as the transaction was broadcast, i.e. the target height.
    Calculated(BlockHeight),
}

impl ConfirmationStatus {
    /// Converts from a blockheight and `pending`. pending is deprecated and is only needed in loading from save.
    #[must_use]
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
    #[must_use]
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
    #[must_use]
    pub fn is_confirmed_after_or_at(&self, comparison_height: &BlockHeight) -> bool {
        matches!(self, Self::Confirmed(self_height) if self_height >= comparison_height)
    }

    /// To return true, the status must be confirmed and no earlier than specified height.
    /// # Examples
    ///
    /// ```
    /// use zingo_status::confirmation_status::ConfirmationStatus;
    /// use zcash_primitives::consensus::BlockHeight;
    ///
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed_after(&9.into()));
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed_after(&10.into()));
    /// assert!(!ConfirmationStatus::Calculated(10.into()).is_confirmed_after(&11.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed_after(&9.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed_after(&10.into()));
    /// assert!(!ConfirmationStatus::Transmitted(10.into()).is_confirmed_after(&11.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed_after(&9.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed_after(&10.into()));
    /// assert!(!ConfirmationStatus::Mempool(10.into()).is_confirmed_after(&11.into()));
    /// assert!(ConfirmationStatus::Confirmed(10.into()).is_confirmed_after(&9.into()));
    /// assert!(!ConfirmationStatus::Confirmed(10.into()).is_confirmed_after(&10.into()));
    /// assert!(!ConfirmationStatus::Confirmed(10.into()).is_confirmed_after(&11.into()));
    /// ```
    #[must_use]
    pub fn is_confirmed_after(&self, comparison_height: &BlockHeight) -> bool {
        matches!(self, Self::Confirmed(self_height) if self_height > comparison_height)
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
    // TODO: blockheight impls copy so remove ref
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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
    #[must_use]
    pub fn get_height(&self) -> BlockHeight {
        match self {
            Self::Calculated(self_height) => *self_height,
            Self::Mempool(self_height) => *self_height,
            Self::Transmitted(self_height) => *self_height,
            Self::Confirmed(self_height) => *self_height,
        }
    }

    fn serialized_version() -> u8 {
        0
    }

    /// Deserialize into `reader`
    pub fn read<R: Read>(mut reader: R) -> std::io::Result<Self> {
        let _version = reader.read_u8()?;
        let status = reader.read_u8()?;
        let block_height = BlockHeight::from_u32(reader.read_u32::<LittleEndian>()?);

        match status {
            0 => Ok(Self::Calculated(block_height)),
            1 => Ok(Self::Transmitted(block_height)),
            2 => Ok(Self::Mempool(block_height)),
            3 => Ok(Self::Confirmed(block_height)),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "failed to read status",
            )),
        }
    }

    /// Serialize into `writer`
    pub fn write<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_u8(Self::serialized_version())?;
        writer.write_u8(match self {
            Self::Calculated(_) => 0,
            Self::Transmitted(_) => 1,
            Self::Mempool(_) => 2,
            Self::Confirmed(_) => 3,
        })?;
        writer.write_u32::<LittleEndian>(self.get_height().into())
    }
}

/// a public interface, writ in stone
impl std::fmt::Display for ConfirmationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Calculated(_h) => {
                write!(f, "calculated")
            }
            Self::Transmitted(_h) => {
                write!(f, "transmitted")
            }
            Self::Mempool(_h) => {
                write!(f, "mempool")
            }
            Self::Confirmed(_h) => {
                write!(f, "confirmed")
            }
        }
    }
}
#[test]
fn stringify_display() {
    let status = ConfirmationStatus::Transmitted(BlockHeight::from_u32(16_000));
    let string = format!("{status}");
    assert_eq!(string, "transmitted");
}

/// a more complete stringification
impl std::fmt::Debug for ConfirmationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Calculated(h) => {
                let hi = u32::from(*h);
                write!(f, "Calculated for {hi}")
            }
            Self::Transmitted(h) => {
                let hi = u32::from(*h);
                write!(f, "Transmitted for {hi}")
            }
            Self::Mempool(h) => {
                let hi = u32::from(*h);
                write!(f, "Mempool for {hi}")
            }
            Self::Confirmed(h) => {
                let hi = u32::from(*h);
                write!(f, "Confirmed at {hi}")
            }
        }
    }
}
#[test]
fn stringify_debug() {
    let status = ConfirmationStatus::Transmitted(BlockHeight::from_u32(16_000));
    let string = format!("{status:?}");
    assert_eq!(string, "Transmitted for 16000");
}

impl From<ConfirmationStatus> for String {
    fn from(value: ConfirmationStatus) -> Self {
        format!("{value}")
    }
}
