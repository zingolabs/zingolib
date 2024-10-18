//! This is a mod for data structs that will be used across all sections of zingolib.
pub mod proposal;
pub mod witness_trees;

/// transforming data related to the destination of a send.
pub mod receivers {
    use zcash_address::ZcashAddress;
    use zcash_client_backend::zip321::Payment;
    use zcash_client_backend::zip321::TransactionRequest;
    use zcash_client_backend::zip321::Zip321Error;
    use zcash_primitives::memo::MemoBytes;
    use zcash_primitives::transaction::components::amount::NonNegativeAmount;

    /// A list of Receivers
    pub type Receivers = Vec<Receiver>;

    /// The superficial representation of the the consumer's intended receiver
    #[derive(Clone, Debug, PartialEq)]
    pub struct Receiver {
        pub(crate) recipient_address: ZcashAddress,
        pub(crate) amount: NonNegativeAmount,
        pub(crate) memo: Option<MemoBytes>,
    }
    impl Receiver {
        /// Create a new Receiver
        pub fn new(
            recipient_address: ZcashAddress,
            amount: NonNegativeAmount,
            memo: Option<MemoBytes>,
        ) -> Self {
            Self {
                recipient_address,
                amount,
                memo,
            }
        }
    }
    impl From<Receiver> for Payment {
        fn from(receiver: Receiver) -> Self {
            Payment::new(
                receiver.recipient_address,
                receiver.amount,
                receiver.memo,
                None,
                None,
                vec![],
            )
            .expect("memo compatability checked in 'parse_send_args'")
        }
    }

    /// Creates a [`zcash_client_backend::zip321::TransactionRequest`] from receivers.
    /// Note this fn is called to calculate the spendable_shielded balance
    /// shielding and TEX should be handled mutually exclusively
    pub fn transaction_request_from_receivers(
        receivers: Receivers,
    ) -> Result<TransactionRequest, Zip321Error> {
        // If this succeeds:
        //  * zingolib learns whether there is a TEX address
        //  * if there's a TEX address it's readable.
        let payments = receivers
            .into_iter()
            .map(|receiver| receiver.into())
            .collect();

        TransactionRequest::new(payments)
    }
}
