use json::JsonValue;
use serde::Serialize;
use zcash_primitives::consensus::BlockHeight;

pub const UNCONFIRMED_BLOCKHEIGHT_PLACEHOLDER: u32 = <u32>::max_value() - 33;

#[derive(Clone, Copy, Debug, Serialize)]
pub enum ConfirmationStatus {
    Unconfirmed,
    // confirmed on blockchain implies a height. this data piece will eventually be a block height
    #[serde(with = "SerdeBlockHeight")]
    Confirmed(BlockHeight),
}

impl ConfirmationStatus {
    // this function and the placeholder is not a preferred pattern. please use match whenever possible.
    pub fn height_andor_is_confirmed(&self) -> (u32, bool) {
        match self {
            ConfirmationStatus::Unconfirmed => (UNCONFIRMED_BLOCKHEIGHT_PLACEHOLDER, false),
            ConfirmationStatus::Confirmed(block) => (u32::from(*block), true),
        }
    }
    // note, by making unconfirmed the true case, this does a potentially confusing boolean flip
    pub fn height_andor_is_unconfirmed(&self) -> (u32, bool) {
        match self {
            ConfirmationStatus::Unconfirmed => (UNCONFIRMED_BLOCKHEIGHT_PLACEHOLDER, true),
            ConfirmationStatus::Confirmed(block) => (u32::from(*block), false),
        }
    }
}
impl Into<json::JsonValue> for ConfirmationStatus {
    fn into(self) -> json::JsonValue {
        JsonValue("Confirmations")
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(remote = "BlockHeight")]
struct SerdeBlockHeight(#[serde(getter = "ref_blockheight_to_u32")] u32);

fn ref_blockheight_to_u32(height: &BlockHeight) -> u32 {
    u32::from(*height)
}
