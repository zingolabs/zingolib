use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

pub const BLOCKHEIGHT_PLACEHOLDER_LOCAL: u32 = <u32>::max_value() - 32 - 1;
pub const BLOCKHEIGHT_PLACEHOLDER_INMEMPOOL: u32 = <u32>::max_value() - 32 - 2;

pub const BLOCKHEIGHT_PLACEHOLDER_NOKNOWNSPENDS: u32 = <u32>::max_value() - 48 - 1;
pub const BLOCKHEIGHT_PLACEHOLDER_PENDINGSPEND: u32 = <u32>::max_value() - 48 - 2;

fn u32_height_or_placeholder(option_blockheight: Option<BlockHeight>) -> u32 {
    match option_blockheight {
        Some(block_height) => u32::from(block_height),
        None => BLOCKHEIGHT_PLACEHOLDER_INMEMPOOL,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfirmationStatus {
    Local,
    // we may know when it entered the mempool.
    InMempool(Option<BlockHeight>),
    // confirmed on blockchain implies a height. this data piece will eventually be a block height
    ConfirmedOnChain(BlockHeight),
}

impl ConfirmationStatus {
    pub fn is_in_mempool(&self) -> bool {
        matches!(self, Self::InMempool(_))
    }
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::ConfirmedOnChain(_))
    }
    pub fn is_confirmed_after_or_at(&self, height: &BlockHeight) -> bool {
        match self {
            Self::ConfirmedOnChain(block_height) => block_height >= height,
            _ => false,
        }
    }
    pub fn is_confirmed_before_or_at(&self, height: &BlockHeight) -> bool {
        match self {
            Self::ConfirmedOnChain(block_height) => block_height <= height,
            _ => false,
        }
    }
    pub fn is_expired(&self, cutoff: &BlockHeight) -> bool {
        match self {
            Self::Local => true,
            Self::InMempool(option_blockheight) => match option_blockheight {
                None => true,
                Some(block_height) => block_height < cutoff,
            },
            Self::ConfirmedOnChain(_) => false,
        }
    }
    // this function and the placeholder is not a preferred pattern. please use match whenever possible.
    pub fn get_height_and_is_confirmed(&self) -> (u32, bool) {
        match self {
            Self::Local => (BLOCKHEIGHT_PLACEHOLDER_LOCAL, false),
            Self::InMempool(opt_block) => (u32_height_or_placeholder(*opt_block), false),
            Self::ConfirmedOnChain(block) => (u32::from(*block), true),
        }
    }
    // note, by making unconfirmed the true case, this does a potentially confusing boolean flip
    pub fn get_height_and_is_unconfirmed(&self) -> (u32, bool) {
        match self {
            Self::Local => (BLOCKHEIGHT_PLACEHOLDER_LOCAL, true),
            Self::InMempool(opt_block) => (u32_height_or_placeholder(*opt_block), true),
            Self::ConfirmedOnChain(block) => (u32::from(*block), false),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpendConfirmationStatus {
    NoKnownSpends,
    PendingSpend(TxId),
    ConfirmedSpent(TxId, BlockHeight),
}

impl SpendConfirmationStatus {
    pub fn from_txid_and_confirmation(
        spending_txid: TxId,
        confirmation_status: ConfirmationStatus,
    ) -> Self {
        match confirmation_status {
            ConfirmationStatus::Local | ConfirmationStatus::InMempool(_) => {
                Self::PendingSpend(spending_txid)
            }
            ConfirmationStatus::ConfirmedOnChain(confirmation_height) => {
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
