use super::{data::WitnessTrees, record_book::RecordBook};
use zcash_keys::keys::UnifiedSpendingKey;

pub mod trait_inputsource;
pub mod trait_walletcommitmenttrees;
pub mod trait_walletread;
pub mod trait_walletwrite;

pub struct SpendKit<'a> {
    pub key: &'a UnifiedSpendingKey,
    pub record_book: &'a RecordBook<'a>,
    pub trees: &'a WitnessTrees,
}
