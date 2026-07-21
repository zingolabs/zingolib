//! Summary types specifically for providing financial insight (a.k.a
//! finsight): per-recipient rollups over the value-transfer view, moved
//! verbatim from zingolib's summary layer.

/// The values of every send to each recipient address.
pub struct ValuesSentToAddress(pub std::collections::HashMap<String, Vec<u64>>);

/// The total value sent to each recipient address.
pub struct TotalValueToAddress(pub std::collections::HashMap<String, u64>);

/// The number of sends to each recipient address.
pub struct TotalSendsToAddress(pub std::collections::HashMap<String, u64>);

/// The total outgoing-memo bytes sent to each recipient address.
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
