//! Error sub-module for utils module.

use std::fmt;

/// The error type for conversion errors.
#[derive(Debug, Clone, PartialEq)]
pub enum ConversionError {
    /// Failed to decode hex
    DecodeHexFailed(hex::FromHexError),
    /// Invalid string length
    InvalidStringLength,
    /// Invalid recipient address
    InvalidAddress(String),
    /// Amount is outside the valid range of zatoshis
    OutsideValidRange,
}

impl std::error::Error for ConversionError {}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConversionError::DecodeHexFailed(e) => write!(f, "failed to decode hex. {}", e),
            ConversionError::InvalidStringLength => write!(f, "invalid string length"),
            ConversionError::InvalidAddress(address) => {
                write!(f, "invalid recipient address. {}", address)
            }
            ConversionError::OutsideValidRange => {
                write!(f, "amount is outside the valid range of zatoshis")
            }
        }
    }
}
