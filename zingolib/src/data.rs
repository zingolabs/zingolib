//! This is a mod for data structs that will be used across all sections of zingolib.
pub mod proposal;
pub mod witness_trees;

/// transforming data related to the destination of a send.
pub mod destinations {
    use zcash_address::ParseError;
    use zcash_address::ZcashAddress;
    use zcash_client_backend::zip321::Payment;
    use zcash_client_backend::zip321::TransactionRequest;
    use zcash_client_backend::zip321::Zip321Error;
    use zcash_primitives::memo::MemoBytes;
    use zcash_primitives::transaction::components::amount::NonNegativeAmount;

    /// A list of Receivers
    pub type Destinations = Vec<Destination>;

    /// The superficial representation of the the consumer's intended receiver
    #[derive(Clone, Debug, PartialEq)]
    pub struct Destination {
        pub(crate) recipient: String,
        pub(crate) amount: NonNegativeAmount,
        pub(crate) memo: Option<MemoBytes>,
    }
    impl Destination {
        /// Create a new Receiver
        pub fn new(recipient: String, amount: NonNegativeAmount, memo: Option<MemoBytes>) -> Self {
            Self {
                recipient,
                amount,
                memo,
            }
        }
    }

    /// anything that can go wrong parsing a TransactionRequest from a receiver
    #[derive(thiserror::Error, Debug)]
    pub enum DestinationParseError {
        /// see Debug
        #[error("Could not parse address: {0}")]
        AddressParse(#[from] ParseError),
        /// see Debug
        #[error("Cant send memo to transparent receiver")]
        MemoDisallowed,
        /// see Debug
        #[error("Could not build TransactionRequest: {0}")]
        Request(#[from] Zip321Error),
    }

    /// Creates a [`zcash_client_backend::zip321::TransactionRequest`] from receivers.
    pub fn transaction_request_from_destinations(
        destinations: Destinations,
    ) -> Result<TransactionRequest, DestinationParseError> {
        let payments = destinations
            .into_iter()
            .map(|receiver| {
                Payment::new(
                    ZcashAddress::try_from_encoded(receiver.recipient.as_str())
                        .map_err(DestinationParseError::AddressParse)?,
                    receiver.amount,
                    receiver.memo,
                    None,
                    None,
                    vec![],
                )
                .ok_or(DestinationParseError::MemoDisallowed)
            })
            .collect::<Result<Vec<Payment>, DestinationParseError>>()?;

        TransactionRequest::new(payments).map_err(DestinationParseError::Request)
    }
}
