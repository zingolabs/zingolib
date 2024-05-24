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
    FeeUnderflow(u64),
}

impl fmt::Display for FeeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use FeeError::*;

        match self {
            OrchardSpendNotFound(n) => write!(f, "Orchard nullifier(s) {:?} for this transaction not found in wallet. Is the wallet fully synced?", n),
            SaplingSpendNotFound(n) => write!(f, "Sapling nullifier(s) {:?} for this transaction not found in wallet. Is the wallet fully synced?", n),
            ReceivedTransaction => write!(f, "No inputs or outgoing transaction data found, indicating this transaction was received and not sent by this capability"),
            FeeUnderflow => write!(f, "total output value is larger than total spend value indicating transparent spends not found in the wallet. Is the wallet fully synced?"),
            OutgoingWithoutSpends(ov) =>  write!(f, "No inputs funded this transaction, but it has outgoing data! Is the wallet fully synced? {:?}", ov),
        }
    }
}

impl std::error::Error for FeeError {}
