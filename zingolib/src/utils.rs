//! General library utilities such as parsing and conversions.

use std::fmt;

use zcash_client_backend::address::Address;
use zcash_primitives::transaction::{components::amount::NonNegativeAmount, TxId};
use zingoconfig::ChainType;

/// Converts txid from hex-encoded `&str` to `zcash_primitives::transaction::TxId`.
///
/// TxId byte order is displayed in the reverse order to how it's encoded.
pub fn txid_from_hex_encoded_str(txid: &str) -> Result<TxId, ConversionError> {
    let txid_bytes = hex::decode(txid).unwrap();
    let mut txid_bytes = <[u8; 32]>::try_from(txid_bytes).unwrap();
    txid_bytes.reverse();
    Ok(TxId::from_bytes(txid_bytes))
}

pub(crate) fn address_from_str(
    address: &str,
    chain: &ChainType,
) -> Result<Address, ConversionError> {
    Address::decode(chain, address)
        .ok_or_else(|| ConversionError::InvalidAddress(address.to_string()))
}

pub(crate) fn zatoshis_from_u64(amount: u64) -> Result<NonNegativeAmount, ConversionError> {
    NonNegativeAmount::from_u64(amount).map_err(|e| ConversionError::OutsideValidRange)
}

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
        match *self {
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
