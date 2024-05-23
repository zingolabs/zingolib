//! Errors for [`crate::wallet`] and sub-modules

use std::fmt;

use crate::wallet::data::OutgoingTxData;

/// Errors associated with calculating transaction fees
#[derive(Debug)]
pub enum FeeError {
    /// Sapling notes spent in a transaction not found in the wallet
    SaplingSpendNotFound(sapling_crypto::Nullifier),
    /// Orchard notes spent in a transaction not found in the wallet
    OrchardSpendNotFound(orchard::note::Nullifier),
    /// Attempted to calculate a fee for a transaction received and not created by the wallet's spend capability
    ReceivedTransaction,
    /// Outgoing tx data, but no spends found!
    OutgoingWithoutSpends(Vec<OutgoingTxData>),
    /// Total output value is larger than total spend value causing the unsigned integer to underflow
    FeeUnderflow,
}

impl fmt::Display for FeeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use FeeError::*;

        match self {
            OrchardSpendNotFound(n) => write!(f, "Orchard nullifier(s) {:?} for this transaction not found in wallet. check wallet is fully synced.", n),
            SaplingSpendNotFound(n) => write!(f, "Sapling nullifier(s) {:?} for this transaction not found in wallet. check wallet is fully synced.", n),
            ReceivedTransaction => write!(f, "no spends or outgoing transaction data found, indicating this transaction was received and not sent by this capability. check wallet is fully synced."),
            FeeUnderflow => write!(f, "total output value is larger than total spend value indicating transparent spends not found in the wallet. check wallet is fully synced."),
            OutgoingWithoutSpends(ov) =>  write!(f, "No inputs funded this transaction, but it has outgoing data: {:?}", ov),
        }
    }
}

impl std::error::Error for FeeError {}
