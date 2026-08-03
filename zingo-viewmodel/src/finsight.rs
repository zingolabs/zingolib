//! The financial-insight rollup types, relocated from zingolib's summary
//! data module when their derivation moved to this crate.

/// TODO: Add Doc Comment Here!
pub struct ValuesSentToAddress(pub std::collections::HashMap<String, Vec<u64>>);
/// TODO: Add Doc Comment Here!
pub struct TotalValueToAddress(pub std::collections::HashMap<String, u64>);
/// TODO: Add Doc Comment Here!
pub struct TotalSendsToAddress(pub std::collections::HashMap<String, u64>);
/// TODO: Add Doc Comment Here!
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
