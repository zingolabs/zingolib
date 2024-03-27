use std::fmt;

use zcash_primitives::transaction::TxId;

pub enum SendToAddressesError {
    NoSpendCapability,
    NoProposal,
    Decode(String),
    NoBroadcast(String),
    PartialBroadcast(Vec<TxId>, String),
}
impl std::fmt::Display for SendToAddressesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use SendToAddressesError::*;
        write!(
            f,
            "SendToAddressesError: {}",
            match self {
                NoSpendCapability =>
                    format!("This wallet has no spend capability. It is a viewkey only wallet.",),
                NoProposal => format!("No proposal! First propose a transfer.",),
                Decode(string) => format!("Cannot decode created transaction: {}", string,),
                NoBroadcast(string) => format!("Could not broadcast transaction: {}", string,),
                PartialBroadcast(_vec, string) =>
                    format!("Could not complete broadcast: {}", string,),
            }
        )
    }
}
