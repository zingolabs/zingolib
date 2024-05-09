//! This is a mod for data structs that will be used across all sections of zingolib.
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

    /// TODO: Add Doc Comment Here!
    // review! unit test this
    pub fn build_transaction_request_from_receivers(
        receivers: Receivers,
    ) -> Result<TransactionRequest, Zip321Error> {
        let mut payments = vec![];
        for out in receivers.clone() {
            payments.push(Payment {
                recipient_address: out.0,
                amount: out.1,
                memo: out.2,
                label: None,
                message: None,
                other_params: vec![],
            });
        }

        TransactionRequest::new(payments)
    }
}
