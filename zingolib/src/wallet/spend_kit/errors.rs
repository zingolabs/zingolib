use std::{convert::Infallible, fmt};

use crate::error::ZingoLibError;

#[derive(Debug)]
pub enum CreateTransactionsError {
    NoProposal,
    CannotSpend(String),
    CreateTransactions(
        zcash_client_backend::data_api::error::Error<
            ZingoLibError,
            Infallible,
            ZingoLibError,
            zcash_primitives::transaction::fees::zip317::FeeError,
        >,
    ),
}
impl std::fmt::Display for CreateTransactionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use CreateTransactionsError::*;
        write!(
            f,
            "CreateTransactionsError - {}",
            match self {
                NoProposal => "No proposal! First propose a transfer.".to_string(),
                CreateTransactions(string) =>
                    format!("Could not calculate transaction: {}", string,),
                CannotSpend(e) => format!("no spend key: {e}, could not sign"),
            }
        )
    }
}
