//! This is a mod for data structs that will be used across all sections of zingolib.
#[cfg(feature = "zip317")]
pub mod proposal;
pub mod witness_trees;

/// transforming data related to the destination of a send.
pub mod receivers {
    use zcash_client_backend::zip321::Payment;
    use zcash_client_backend::zip321::TransactionRequest;
    use zcash_client_backend::zip321::Zip321Error;
    use zcash_keys::address;
    use zcash_primitives::memo::MemoBytes;
    use zcash_primitives::transaction::components::amount::NonNegativeAmount;

    pub(crate) type Receivers = Vec<(address::Address, NonNegativeAmount, Option<MemoBytes>)>;

    /// Creates a [`zcash_client_backend::zip321::TransactionRequest`] from receivers.
    pub(crate) fn transaction_request_from_receivers(
        receivers: Receivers,
    ) -> Result<TransactionRequest, Zip321Error> {
        let payments = receivers
            .into_iter()
            .map(|receiver| Payment {
                recipient_address: receiver.0,
                amount: receiver.1,
                memo: receiver.2,
                label: None,
                message: None,
                other_params: vec![],
            })
            .collect();

        TransactionRequest::new(payments)
    }
}
