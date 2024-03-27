use std::fmt;

use zcash_client_backend::zip321::Zip321Error;
use zcash_primitives::transaction::TxId;

use crate::{error::ZingoLibError, wallet::spend_kit::errors::CreateTransactionsError};

#[derive(Debug)]
pub enum DoProposeError {
    RequestConstruction(Zip321Error),
    Proposing(ZingoLibError),
}
impl std::fmt::Display for DoProposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use DoProposeError::*;
        write!(
            f,
            "DoPropose failed - {}",
            match self {
                RequestConstruction(err) => format!("Could not parse transaction request {}", err,),
                Proposing(err) => format!("Could not create proposal {}", err,),
            }
        )
    }
}
impl From<DoProposeError> for String {
    fn from(value: DoProposeError) -> Self {
        format!("{value}")
    }
}

#[derive(Debug)]
pub enum SendProposedTransferError {
    NoSpendCapability,
    NoProposal,
    AssembleSpendKit(ZingoLibError),
    CreateTransactions(CreateTransactionsError),
    Decode(String),
    NoBroadcast(String),
    PartialBroadcast(Vec<TxId>, String),
}
impl std::fmt::Display for SendProposedTransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use SendProposedTransferError::*;
        write!(
            f,
            "SendToAddressesError - {}",
            match self {
                NoSpendCapability =>
                    "This wallet has no spend capability. It is a viewkey only wallet.".to_string(),
                NoProposal => "No proposal! First propose a transfer.".to_string(),
                AssembleSpendKit(string) => format!("Cannot assemble spend kit: {}", string,),
                CreateTransactions(string) =>
                    format!("Could not calculate created transaction: {}", string,),
                Decode(string) => format!("Cannot decode created transaction: {}", string,),
                NoBroadcast(string) => format!("Could not broadcast transaction: {}", string,),
                PartialBroadcast(_vec, string) =>
                    format!("Could not complete broadcast: {}", string,),
            }
        )
    }
}
