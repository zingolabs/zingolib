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

#[derive(Debug)]
pub enum AssembleSpendKitError {
    NoSpendCapability,
}
impl std::fmt::Display for AssembleSpendKitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use AssembleSpendKitError::*;
        write!(
            f,
            "{:#?} - {}",
            self,
            // the former script converts the error type to a string. the following dictionary provides explanations.
            match self {
                NoSpendCapability => "This is a view-only wallet.".to_string(),
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::wallet::spend_kit::errors::AssembleSpendKitError;

    #[tokio::test]
    async fn test_error_message() {
        dbg!(format!("{}", AssembleSpendKitError::NoSpendCapability));
    }
}
