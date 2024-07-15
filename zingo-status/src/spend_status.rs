//! A note optionally may have up to one spend.

use zcash_primitives::transaction::TxId;

use super::confirmation_status::ConfirmationStatus;

/// a note may be spent. we enumerate the relationship
/// of the spending transaction to the blockchain
/// i.e. ConfirmationStatus, including height.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SpendStatus {
    /// the output has never been known to be spent.
    Unspent,
    /// the output has been spent somewhere. it may be confirmed or not
    SpendExists((TxId, ConfirmationStatus)),
}
impl std::fmt::Display for SpendStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpendStatus::Unspent => write!(f, "unspent"),
            SpendStatus::SpendExists((txid, ConfirmationStatus::Pending(_))) => {
                write!(f, "pending spent in {}", txid)
            }
            SpendStatus::SpendExists((txid, ConfirmationStatus::Confirmed(_))) => {
                write!(f, "spent in {}", txid)
            }
        }
    }
}
