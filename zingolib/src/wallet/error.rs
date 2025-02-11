//! Errors for [`crate::wallet`] and sub-modules

use zcash_keys::keys::DerivationError;
use zcash_primitives::transaction::TxId;

/// Top level wallet errors
#[derive(Debug, thiserror::Error)]
pub enum WalletError {
    /// Key error
    #[error("{0}")]
    KeyError(#[from] KeyError),
    /// Mnemonic error
    #[error("{0}")]
    MnemonicError(#[from] bip0039::Error),
    /// Value outside the valid range of zatoshis
    #[error("Value outside valid range of zatoshis. {0:?}")]
    InvalidValue(#[from] zcash_primitives::transaction::components::amount::BalanceError),
    /// Failed to write transaxction.
    #[error("Failed to write transaction. {0:?}")]
    TransactionWrite(#[from] std::io::Error),
}

/// Errors associated with calculating transaction fee
#[derive(Debug)]
pub enum FeeError {
    /// Transparent spend not found in wallet
    SpendNotFound { txid: TxId, spend: String },
    /// Balance error
    BalanceError(zcash_primitives::transaction::components::amount::BalanceError),
}

impl std::error::Error for FeeError {}

impl std::fmt::Display for FeeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match &self {
            Self::SpendNotFound { txid, spend } => {
                write!(
                    f,
                    "Transparent spend not found for transaction id {txid}. Is the wallet fully synced? \nMissing spend: {spend}"
                )
            }
            Self::BalanceError(e) => write!(f, "{}", e),
        }
    }
}

impl From<zcash_primitives::transaction::components::amount::BalanceError> for FeeError {
    fn from(value: zcash_primitives::transaction::components::amount::BalanceError) -> Self {
        Self::BalanceError(value)
    }
}

/// Errors associated with determining transaction kind
#[derive(Debug, thiserror::Error)]
pub enum KindError {
    // TODO: add pool info to missing spend
    /// Transaction spends not found in wallet
    #[error("Spend not found for transaction id {txid}. Is the wallet fully synced? \nMissing spend: {spend}")]
    SpendNotFound { txid: TxId, spend: String },
    /// Attempted to calculate a fee for a transaction received and not created by the wallet's spend capability
    #[error("No inputs or outgoing transaction data found, indicating this transaction was received and not sent by this capability. Is the wallet fully synced?")]
    ReceivedTransaction,
    /// Outgoing notes, but no spends found!
    #[error("This transaction has outgoing notes but not all spends were found! Is the wallet fully synced?")]
    OutgoingWithoutSpends,
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

/// Errors associated with key and address derivation
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// Error associated with standard IO
    #[error("{0}")]
    IoError(#[from] std::io::Error),
    /// Invalid account ID
    #[error("Account ID should be at most 31 bits")]
    InvalidAccountId(#[from] zip32::TryFromIntError),
    /// Key derivation failed
    // TODO: add std::Error to zcash_keys::keys::DerivationError in LRZ fork and add thiserror #[from] macro
    #[error("Key derivation failed")]
    KeyDerivationError(#[from] DerivationError),
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
    /// Unified address missing shielded receiver
    #[error("Unified address must contain a shielded receiver")]
    UnifiedAddressError,
}

impl From<bip32::Error> for KeyError {
    fn from(value: bip32::Error) -> Self {
        Self::KeyDerivationError(DerivationError::Transparent(value))
    }
}
