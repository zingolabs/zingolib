//! Errors for [`crate::wallet`] and sub-modules

use pepper_sync::{error::ScanError, wallet::OutputId};
use zcash_client_backend::PoolType;
use zcash_keys::keys::DerivationError;
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

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
    /// Failed to write transaction.
    #[error("Failed to write transaction. {0:?}")]
    TransactionWrite(#[from] std::io::Error),
    /// Wallet block not found in the wallet.
    #[error("Wallet block at height {0} not found in the wallet.")]
    BlockNotFound(BlockHeight),
    /// Minimum confirmations must be non-zero.
    #[error("Minimum confirmations must be non-zero.")]
    MinimumConfirmationError,
    /// Failed to scan calculated transaction.
    #[error("Failed to scan calculated transaction.")]
    CalculatedTxScanError(#[from] ScanError),
    /// Address parse error
    #[error("address parse error. {0}")]
    ParseError(#[from] zcash_address::ParseError),
}

/// Summary error
#[derive(Debug, thiserror::Error)]
pub enum SummaryError {
    /// Address parse error
    #[error("address parse error. {0}")]
    ParseError(#[from] zcash_address::ParseError),
    /// Std IO address parse or conversion error
    // TODO: temp while we fix `decode_address` error handling in pepper sync
    #[error("address parse error. {0}")]
    StdParseError(#[from] std::io::Error),
    /// Spend error
    #[error("spend error. {0}")]
    SpendError(#[from] SpendError),
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

/// Errors associated with spends
#[derive(Debug, thiserror::Error)]
pub enum SpendError {
    /// Transaction spends not found in wallet
    #[error("spend not found for transaction id {txid}. is the wallet fully synced?\nmissing spend: {spend}")]
    SpendNotFound {
        pool: PoolType,
        txid: TxId,
        spend: String,
    },
    /// Output has incorrect spending transaction id
    #[error("output has incorrect spending transaction id: {txid}.\noutput id: {output_id}")]
    IncorrectSpendingTransaction { output_id: OutputId, txid: TxId },
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
