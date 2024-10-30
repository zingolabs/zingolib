//! Errors for [`crate::wallet`] and sub-modules

use zcash_keys::keys::DerivationError;

use crate::wallet::data::OutgoingTxData;

/// Errors associated with transaction fee calculation
#[derive(Debug, thiserror::Error)]
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
#[derive(Debug, thiserror::Error)]
pub enum BalanceError {
    /// failed to retrieve full viewing key
    #[error("failed to retrieve full viewing key.")]
    NoFullViewingKey,
    /// conversion failed
    #[error("conversion failed. {0}")]
    ConversionFailed(#[from] crate::utils::error::ConversionError),
}

/// Errors associated with balance key derivation
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// Error asociated with standard IO
    #[error("{0}")]
    IoError(#[from] std::io::Error),
    /// Invalid account ID
    #[error("Account ID should be at most 31 bits")]
    InvalidAccountId(#[from] zip32::TryFromIntError),
    /// Key derivation failed
    // TODO: add std::Error to zcash_keys::keys::DerivationError in LRZ fork and add thiserror #[from] macro
    #[error("Key derivation failed")]
    KeyDerivationError(DerivationError),
    /// Key decoding failed
    // TODO: add std::Error to zcash_keys::keys::DecodingError in LRZ fork and add thiserror #[from] macro
    #[error("Key decoding failed")]
    KeyDecodingError,
    /// Key parsing failed
    #[error("Key parsing failed. {0}")]
    KeyParseError(#[from] zcash_address::unified::ParseError),
    /// No spend capability
    #[error("No spend capability")]
    NoSpendCapability,
    /// No view capability
    #[error("No view capability")]
    NoViewCapability,
    /// Invalid non-hardened child indexes
    #[error("Outside range of non-hardened child indexes")]
    InvalidNonHardenedChildIndex,
    /// Network mismatch
    #[error("Decoded unified full viewing key does not match current network")]
    NetworkMismatch,
    /// Invalid format
    #[error("Viewing keys must be imported in the unified format")]
    InvalidFormat,
}
