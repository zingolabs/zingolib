//! The financial-insight rollup types, relocated from zingolib's summary
//! data module when their derivation moved to this crate.

/// Every external send's value, grouped by recipient address: one entry
/// per address, holding the value of each Send value transfer made to it.
/// The other rollups are reductions of this map.
pub struct ValuesSentToAddress(pub std::collections::HashMap<String, Vec<u64>>);
/// The total value sent to each recipient address, summed over all of the
/// wallet's external sends to it.
pub struct TotalValueToAddress(pub std::collections::HashMap<String, u64>);
/// The number of external sends the wallet has made to each recipient
/// address.
pub struct TotalSendsToAddress(pub std::collections::HashMap<String, u64>);
/// The total bytes of memo text the wallet has sent to each recipient
/// address, summed over the memos of its external sends.
#[derive(Debug)]
pub struct TotalMemoBytesToAddress(pub std::collections::HashMap<String, usize>);

impl From<TotalMemoBytesToAddress> for json::JsonValue {
    fn from(value: TotalMemoBytesToAddress) -> Self {
        let mut jsonified = json::object!();
        let hm = value.0;
        for (key, val) in &hm {
            jsonified[key] = json::JsonValue::from(*val);
        }
        jsonified
    }
}

impl From<TotalValueToAddress> for json::JsonValue {
    fn from(value: TotalValueToAddress) -> Self {
        let mut jsonified = json::object!();
        let hm = value.0;
        for (key, val) in &hm {
            jsonified[key] = json::JsonValue::from(*val);
        }
        jsonified
    }
}

impl From<TotalSendsToAddress> for json::JsonValue {
    fn from(value: TotalSendsToAddress) -> Self {
        let mut jsonified = json::object!();
        let hm = value.0;
        for (key, val) in &hm {
            jsonified[key] = json::JsonValue::from(*val);
        }
        jsonified
    }
}
