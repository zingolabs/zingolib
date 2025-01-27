//! This is a mod for data structs that will be used across all sections of zingolib.

use zcash_primitives::transaction::TxId;

use crate::utils::conversion::TxIdFromHexEncodedStrError;
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
            .expect("memo compatibility checked in 'parse_send_args'")
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

#[allow(missing_docs)] // error types document themselves
#[derive(Clone, Debug, thiserror::Error, PartialEq)]
pub enum TxIdComparisonError {
    #[error("Server returned TxId [{0}] which fails to decode with error: [{1:?}].")]
    InvalidTxId(String, TxIdFromHexEncodedStrError),
    /// please note that this error contains two arguments of the same type. Its possible to cause bugs by interpreting them in the opposite order.
    #[error("Server returned TxId: [{0:?}], which does not match expected [{1:?}].")]
    InconsistentTxId(TxId, TxId),
}

pub(crate) fn txid_comparison(
    reported_string_txid: String,
    original_txid: &TxId,
) -> Result<TxId, TxIdComparisonError> {
    match crate::utils::conversion::txid_from_hex_encoded_str(reported_string_txid.as_str()) {
        Ok(reported_txid) => {
            if *original_txid != reported_txid {
                {
                    Err(TxIdComparisonError::InconsistentTxId(
                        reported_txid,
                        *original_txid,
                    ))
                }
            } else {
                Ok(reported_txid)
            }
        }
        Err(e) => Err(TxIdComparisonError::InvalidTxId(reported_string_txid, e)),
    }
}
