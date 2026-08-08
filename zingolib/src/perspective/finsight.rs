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

/// Every finsight rollup, built from a single pass over the value
/// transfers, for consumers that want more than one rollup without
/// re-deriving the value transfers for each.
pub struct Finsight {
    /// Every external send's value, grouped by recipient address.
    pub values_sent_to_address: ValuesSentToAddress,
    /// The total memo bytes sent to each recipient address.
    pub total_memobytes_to_address: TotalMemoBytesToAddress,
}

impl ValuesSentToAddress {
    /// The total value sent to each address.
    ///
    /// ```
    /// use std::collections::HashMap;
    /// use zingolib::perspective::finsight::ValuesSentToAddress;
    ///
    /// let values = ValuesSentToAddress(HashMap::from([("zs1x".to_string(), vec![100, 250])]));
    /// assert_eq!(values.total_value().0["zs1x"], 350);
    /// ```
    pub fn total_value(&self) -> TotalValueToAddress {
        TotalValueToAddress(
            self.0
                .iter()
                .map(|(address, values)| (address.clone(), values.iter().sum()))
                .collect(),
        )
    }

    /// The number of sends to each address.
    ///
    /// ```
    /// use std::collections::HashMap;
    /// use zingolib::perspective::finsight::ValuesSentToAddress;
    ///
    /// let values = ValuesSentToAddress(HashMap::from([("zs1x".to_string(), vec![100, 250])]));
    /// assert_eq!(values.total_sends().0["zs1x"], 2);
    /// ```
    pub fn total_sends(&self) -> TotalSendsToAddress {
        TotalSendsToAddress(
            self.0
                .iter()
                .map(|(address, values)| (address.clone(), values.len() as u64))
                .collect(),
        )
    }
}

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
