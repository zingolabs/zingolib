//! Error sub-module for utils module.

use std::fmt;

/// The error type for conversion errors.
#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum ConversionError {
    /// Failed to decode hex
    DecodeHexFailed(hex::FromHexError),
    /// Invalid string length
    InvalidStringLength,
    /// Invalid recipient address
    InvalidAddress(#[from] zcash_address::ParseError),
    /// Amount is outside the valid range of zatoshis
    OutsideValidRange,
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConversionError::DecodeHexFailed(e) => write!(f, "failed to decode hex. {}", e),
            ConversionError::InvalidStringLength => write!(f, "invalid string length"),
            ConversionError::InvalidAddress(e) => {
                write!(f, "invalid recipient address. {}", e)
            }
            ConversionError::OutsideValidRange => {
                write!(f, "amount is outside the valid range of zatoshis")
            }
        }
    }
}
