//! Errors for [`crate::wallet`] and sub-modules

use thiserror::Error;

use crate::wallet::data::OutgoingTxData;

/// Errors associated with transaction fee calculation
#[derive(Debug, Error)]
pub enum FeeError {
    /// Sapling notes spent in a transaction not found in the wallet
    #[error("Sapling nullifier(s) {0:?} for this transaction not found in wallet. Is the wallet fully synced?")]
    SaplingSpendNotFound(sapling_crypto::Nullifier),
    /// Orchard notes spent in a transaction not found in the wallet
    #[error("Orchard nullifier(s) {0:?} for this transaction not found in wallet. Is the wallet fully synced?")]
    OrchardSpendNotFound(orchard::note::Nullifier),
    /// Attempted to calculate a fee for a transaction received and not created by the wallet's spend capability
    #[error("No inputs or outgoing transaction data found, indicating this transaction was received and not sent by this capability. Is the wallet fully synced?")]
    ReceivedTransaction,
    /// Outgoing tx data, but no spends found!
    #[error("No inputs funded this transaction, but it has outgoing data! Is the wallet fully synced? {0:?}")]
    OutgoingWithoutSpends(Vec<OutgoingTxData>),
    /// Total explicit receiver value is larger than input value causing the unsigned integer to underflow
    #[error(
        "Output value {explicit_output_value} is larger than total input value {input_value}. Is the wallet fully synced?"
    )]
    FeeUnderflow {
        /// total value of all shielded notes and transparent outputs spent in a transaction
        input_value: u64,
        /// total value of all outputs to receivers including change
        explicit_output_value: u64,
    },
}

/// Errors associated with balance calculation
#[derive(Debug, Error)]
pub enum BalanceError {
    /// failed to retrieve full viewing key
    #[error("failed to retrieve full viewing key.")]
    NoFullViewingKey,
    /// conversion failed
    #[error("conversion failed. {0}")]
    ConversionFailed(crate::utils::error::ConversionError),
}
