//! Conversion specific utilities

use thiserror::Error;

use zcash_client_backend::address::Address;
use zcash_primitives::transaction::{components::amount::NonNegativeAmount, TxId};

use crate::config::ChainType;

use super::error::ConversionError;

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, Error)]
pub enum TxIdFromHexEncodedStrError {
    #[error("{0}")]
    Decode(hex::FromHexError),
    #[error("{0:?}")]
    Code(Vec<u8>),
}

/// Converts txid from hex-encoded `&str` to `zcash_primitives::transaction::TxId`.
///
/// TxId byte order is displayed in the reverse order to how it's encoded.
pub fn txid_from_hex_encoded_str(txid: &str) -> Result<TxId, TxIdFromHexEncodedStrError> {
    let txid_bytes = hex::decode(txid).map_err(TxIdFromHexEncodedStrError::Decode)?;
    let mut txid_bytes =
        <[u8; 32]>::try_from(txid_bytes).map_err(TxIdFromHexEncodedStrError::Code)?;
    txid_bytes.reverse();
    Ok(TxId::from_bytes(txid_bytes))
}

/// Convert a &str to an Address
pub fn address_from_str(address: &str, chain: &ChainType) -> Result<Address, ConversionError> {
    Address::decode(chain, address)
        .ok_or_else(|| ConversionError::InvalidAddress(address.to_string()))
}

/// Convert a valid u64 into Zatoshis.
pub fn zatoshis_from_u64(amount: u64) -> Result<NonNegativeAmount, ConversionError> {
    NonNegativeAmount::from_u64(amount).map_err(|_e| ConversionError::OutsideValidRange)
}
