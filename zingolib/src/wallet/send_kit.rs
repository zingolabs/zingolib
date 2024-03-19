use super::{data::WitnessTrees, record_book::RecordBook};
use zcash_keys::keys::UnifiedSpendingKey;

pub struct SendKit<'a> {
    pub key: &'a UnifiedSpendingKey,
    pub record_book: &'a RecordBook<'a>,
    pub trees: &'a WitnessTrees,
}
