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
    /// Total explicit receiver value is larger than input value causing the unsigned integer to underflow
    FeeUnderflow((u64, u64)),
}

impl fmt::Display for FeeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FeeError::OrchardSpendNotFound(n) => write!(f, "Orchard nullifier(s) {:?} for this transaction not found in wallet. Is the wallet fully synced?", n),
            FeeError::SaplingSpendNotFound(n) => write!(f, "Sapling nullifier(s) {:?} for this transaction not found in wallet. Is the wallet fully synced?", n),
            FeeError::ReceivedTransaction => write!(f, "No inputs or outgoing transaction data found, indicating this transaction was received and not sent by this capability"),
            FeeError::FeeUnderflow((total_out, total_in)) => write!(f, "Output value: {} is larger than total input value: {} Is the wallet fully synced?", total_out, total_in),
            FeeError::OutgoingWithoutSpends(ov) =>  write!(f, "No inputs funded this transaction, but it has outgoing data! Is the wallet fully synced? {:?}", ov),
        }
    }
}

impl std::error::Error for FeeError {}
