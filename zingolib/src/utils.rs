//! General library utilities such as parsing and conversions.

pub mod conversion;
pub mod error;

macro_rules! build_method {
    ($name:ident, $localtype:ty) => {
        #[doc = "Set the $name field of the builder."]
        pub fn $name(&mut self, $name: $localtype) -> &mut Self {
            self.$name = Some($name);
            self
        }
    };
}
#[cfg(test)] // temporary test gate as no production builders use this macros yet
macro_rules! build_method_push {
    ($name:ident, $localtype:ty) => {
        #[doc = "Push a $ty to the builder."]
        pub fn $name(&mut self, $name: $localtype) -> &mut Self {
            self.$name.push($name);
            self
        }
    };
}
#[cfg(test)] // temporary test gate as no production builders use this macros yet
macro_rules! build_push_list {
    ($name:ident, $builder:ident, $struct:ident) => {
        for i in &$builder.$name {
            $struct.$name.push(i.build());
        }
    };
}

pub(crate) use build_method;
#[cfg(test)]
pub(crate) use build_method_push;
#[cfg(test)]
pub(crate) use build_push_list;

/// mod
pub mod txid {
    use log::error;
    use zcash_primitives::transaction::TxId;

    /// used when the server reports a string txid
    pub fn compare_txid_to_string(
        txid: TxId,
        reported_txid_string: String,
        prefer_reported: bool,
    ) -> TxId {
        match crate::utils::conversion::txid_from_hex_encoded_str(reported_txid_string.as_str()) {
            Ok(reported_txid) => {
                if txid != reported_txid {
                    // happens during darkside tests
                    error!(
                        "served txid {} does not match calulated txid {}",
                        reported_txid, txid,
                    );
                };
                if prefer_reported {
                    reported_txid
                } else {
                    txid
                }
            }
            Err(e) => {
                error!("server returned invalid txid {}", e);
                txid
            }
        }
    }
}
