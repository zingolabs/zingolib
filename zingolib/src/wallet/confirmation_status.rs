use json::JsonValue;
use serde::Serialize;
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

pub const UNCONFIRMED_BLOCKHEIGHT_PLACEHOLDER: u32 = <u32>::max_value() - 33;

#[derive(Clone, Copy, Debug, Serialize)]
pub enum ConfirmationStatus {
    Unconfirmed,
    // confirmed on blockchain implies a height. this data piece will eventually be a block height
    #[serde(with = "SerdeBlockHeight")]
    Confirmed(BlockHeight),
}

impl ConfirmationStatus {
    pub fn is_confirmed(&self) -> bool {
        match self {
            Self::Unconfirmed => false,
            Self::Confirmed(_) => true,
        }
    }
    pub fn could_be_spent_at_anchor_height(&self, chain_height: &BlockHeight) -> bool {
        match self {
            Self::Unconfirmed => false,
            Self::Confirmed(block_height) => block_height <= chain_height,
        }
    }
    // this function and the placeholder is not a preferred pattern. please use match whenever possible.
    pub fn get_height_and_is_confirmed(&self) -> (u32, bool) {
        match self {
            Self::Unconfirmed => (UNCONFIRMED_BLOCKHEIGHT_PLACEHOLDER, false),
            Self::Confirmed(block) => (u32::from(*block), true),
        }
    }
    // note, by making unconfirmed the true case, this does a potentially confusing boolean flip
    pub fn get_height_andor_is_unconfirmed(&self) -> (u32, bool) {
        match self {
            Self::Unconfirmed => (UNCONFIRMED_BLOCKHEIGHT_PLACEHOLDER, true),
            Self::Confirmed(block) => (u32::from(*block), false),
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(remote = "BlockHeight")]
struct SerdeBlockHeight(#[serde(getter = "ref_blockheight_to_u32")] u32);

fn ref_blockheight_to_u32(height: &BlockHeight) -> u32 {
    u32::from(*height)
}

#[derive(Clone, Copy, Debug, Serialize)]
pub enum SpendConfirmationStatus {
    NoKnownSpends,
    #[serde(with = "SerdeTxId")]
    PendingSpend(TxId),
    #[serde(with = "SerdeBlockHeight")]
    ConfirmedSpent(TxId, BlockHeight),
}

impl SpendConfirmationStatus {
    pub fn is_unspent(&self) -> bool {
        match self {
            Self::NoKnownSpends => true,
            _ => false,
        }
    }
    pub fn is_pending_spend(&self) -> bool {
        match self {
            Self::PendingSpend(_) => true,
            _ => false,
        }
    }
    pub fn is_pending_spend_or_confirmed_spent(&self) -> bool {
        match self {
            Self::PendingSpend(_) => true,
            Self::ConfirmedSpent(_, _) => true,
            _ => false,
        }
    }
    pub fn is_confirmed_spent(&self) -> bool {
        match self {
            Self::ConfirmedSpent(_, _) => true,
            _ => false,
        }
    }
    pub fn to_serde_json(&self) -> serde_json::Value {
        match self {
            Self::NoKnownSpends => serde_json::Value::from("no known spends"),
            Self::PendingSpend(spent_txid) => serde_json::json!({
                "pending_spend_at_txid": format!("{}",spent_txid),}),
            Self::ConfirmedSpent(spent_txid, block_height) => serde_json::json!({
                "spent_at_txid": format!("{}",spent_txid),
                "spend_at_block_height": u32::from(block_height),}),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(remote = "TxId")]
struct SerdeTxId(#[serde(getter = "ref_txid_to_u8_32")] [u8; 32]);

fn ref_txid_to_u8_32(txid: &TxId) -> [u8; 32] {
    //
    <[u8; 32]>::from(*txid)
}

#[derive(Clone, Serialize)]
#[serde(remote = "TxId", "BlockHeight")]
struct SerdeTxIdAndBlockHeight(
    #[serde(getter = "ref_txid_and_blockheight_to_u8_32_and_u32")] ([u8; 32], u32),
);

fn ref_txid_and_blockheight_to_u8_32_and_u32(pair: (&TxId, &BlockHeight)) -> ([u8; 32], u32) {
    let (txid, height) = pair;
    (ref_txid_to_u8_32(txid), ref_blockheight_to_u32(height))
}
