use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

use crate::confirmation_status::ConfirmationStatus;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpendStatus {
    NoKnownSpends,
    PendingSpend(TxId),
    ConfirmedSpent(TxId, BlockHeight),
}

impl SpendStatus {
    pub fn from_txid_and_confirmation(
        spending_txid: TxId,
        confirmation_status: ConfirmationStatus,
    ) -> Self {
        match confirmation_status {
            ConfirmationStatus::Local | ConfirmationStatus::Broadcast(_) => {
                Self::PendingSpend(spending_txid)
            }
            ConfirmationStatus::Confirmed(confirmation_height) => {
                Self::ConfirmedSpent(spending_txid, confirmation_height)
            }
        }
    }
    pub fn from_opt_txidandu32(option_txidandu32: Option<(TxId, u32)>) -> Self {
        match option_txidandu32 {
            None => Self::NoKnownSpends,
            Some((txid, confirmed_height)) => {
                Self::ConfirmedSpent(txid, BlockHeight::from_u32(confirmed_height))
            }
        }
    }
    pub fn from_opt_i32_and_option_txid(
        option_height: Option<i32>,
        option_txid: Option<TxId>,
    ) -> Self {
        match option_txid {
            None => Self::NoKnownSpends,
            Some(txid) => match option_height {
                None => Self::PendingSpend(txid),
                Some(integer) => match u32::try_from(integer) {
                    Err(_) => Self::PendingSpend(txid),
                    Ok(height) => Self::ConfirmedSpent(txid, BlockHeight::from_u32(height)),
                },
            },
        }
    }
    pub fn is_unspent(&self) -> bool {
        matches!(self, Self::NoKnownSpends)
    }
    pub fn is_pending_spend(&self) -> bool {
        matches!(self, Self::PendingSpend(_))
    }
    pub fn is_pending_spend_or_confirmed_spent(&self) -> bool {
        matches!(self, Self::PendingSpend(_) | Self::ConfirmedSpent(_, _))
    }
    pub fn is_confirmed_spent(&self) -> bool {
        matches!(self, Self::ConfirmedSpent(_, _))
    }
    pub fn is_not_confirmed_spent(&self) -> bool {
        !matches!(self, Self::ConfirmedSpent(_, _))
    }
    pub fn erase_spent_in_txids(&mut self, txids: &[TxId]) {
        match self {
            Self::NoKnownSpends => (),
            Self::PendingSpend(txid) => {
                if txids.contains(txid) {
                    *self = Self::NoKnownSpends;
                }
            }
            Self::ConfirmedSpent(txid, _) => {
                if txids.contains(txid) {
                    *self = Self::NoKnownSpends;
                }
            }
        }
    }
    // this function and seperate enum possibilities is not a preferred pattern. please use match whenever possible.
    pub fn get_option_i32_and_option_txid(&self) -> (Option<i32>, Option<TxId>) {
        match self {
            Self::NoKnownSpends => (None, None),
            Self::PendingSpend(_) => (None, None),
            Self::ConfirmedSpent(txid, block) => (Some(u32::from(*block) as i32), Some(*txid)),
        }
    }
    pub fn to_opt_txidandu32(&self) -> Option<(TxId, u32)> {
        match self {
            Self::ConfirmedSpent(txid, confirmation_height) => {
                Some((*txid, u32::from(*confirmation_height)))
            }
            _ => None,
        }
    }
    pub fn to_serde_json(&self) -> serde_json::Value {
        match self {
            Self::NoKnownSpends => serde_json::Value::from("no known spends"),
            Self::PendingSpend(spent_txid) => serde_json::json!({
                "pending_spend_at_txid": format!("{}",spent_txid),}),
            Self::ConfirmedSpent(spent_txid, block_height) => serde_json::json!({
                "spent_at_txid": format!("{}",spent_txid),
                "spend_at_block_height": u32::from(*block_height),}),
        }
    }
}
