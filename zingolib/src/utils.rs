//! General library utilities such as parsing and conversions.

use std::fmt;

use zcash_primitives::transaction::TxId;

/// Converts txid from hex-encoded `&str` to `zcash_primitives::transaction::TxId`.
///
/// TxId byte order is displayed in the reverse order to how it's encoded.
pub fn txid_from_hex_encoded_str(txid: &str) -> Result<TxId, ConversionError> {
    let txid_bytes = hex::decode(txid).unwrap();
    let mut txid_bytes = <[u8; 32]>::try_from(txid_bytes).unwrap();
    txid_bytes.reverse();
    Ok(TxId::from_bytes(txid_bytes))
}

/// The error type for conversion errors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConversionError {
    DecodeHexFailed(hex::FromHexError),
    InvalidStringLength,
}

impl std::error::Error for ConversionError {}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ConversionError::DecodeHexFailed(e) => write!(f, "failed to decode hex. {}", e),
            ConversionError::InvalidStringLength => write!(f, "invalid string length"),
        }
    }
}
