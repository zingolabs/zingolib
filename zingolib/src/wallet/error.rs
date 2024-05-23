//! Errors for [`crate::wallet`] and sub-modules

use std::fmt;

/// Errors associated with calculating transaction fees
#[derive(Debug)]
pub enum FeeError {
    /// Notes spent in a transaction not found in the wallet
    SpendNotFound,
    /// Attempted to calculate a fee for a transaction received and not created by the wallet's spend capability
    ReceivedTransaction,
    /// Total output value is larger than total spend value causing the unsigned integer to underflow
    FeeUnderflow,
}

impl fmt::Display for FeeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use FeeError::*;

        match self {
            SpendNotFound => write!(f, "spend(s) for this transaction not found in wallet. check wallet is fully synced."),
            ReceivedTransaction => write!(f, "no spends or outgoing transaction data found, indicating this transaction was received and not sent by this capability. check wallet is fully synced."),
            FeeUnderflow => write!(f, "total output value is larger than total spend value indicating transparent spends not found in the wallet. check wallet is fully synced."),
        }
    }
}

impl std::error::Error for FeeError {}
