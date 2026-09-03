//! OP_RETURN (null-data) payload support for the spend path.
//!
//! Lets a caller attach a single OP_RETURN (null-data) output to the
//! transaction a send produces, so a THORChain/MAYAChain swap memo can be
//! written on chain.

use thiserror::Error;

/// The maximum size, in bytes, of an OP_RETURN payload that will relay on
/// the network. Mirrors the limit the transaction builder enforces, so an
/// oversized payload is rejected at construction with a typed error rather
/// than at build time.
pub const MAX_OP_RETURN_BYTES: usize = 80;

/// A validated OP_RETURN payload: at most [`MAX_OP_RETURN_BYTES`] bytes.
///
/// Construct via [`OpReturnData::new`]; the length invariant is enforced
/// once, at the boundary, so the build layer can treat the bytes as safe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpReturnData(Vec<u8>);

impl OpReturnData {
    /// Validate `data` and wrap it. Fails if it exceeds the relay limit.
    pub fn new(data: Vec<u8>) -> Result<Self, OpReturnDataError> {
        if data.len() > MAX_OP_RETURN_BYTES {
            return Err(OpReturnDataError::TooLong {
                len: data.len(),
                max: MAX_OP_RETURN_BYTES,
            });
        }
        Ok(Self(data))
    }

    /// The validated payload bytes, ready to hand to
    /// `Builder::add_transparent_null_data_output`.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Error constructing an [`OpReturnData`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpReturnDataError {
    /// Payload exceeds the OP_RETURN relay limit.
    #[error("OP_RETURN payload is {len} bytes, exceeds the {max}-byte limit")]
    TooLong { len: usize, max: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_payload_at_the_limit() {
        let data = vec![0u8; MAX_OP_RETURN_BYTES];
        assert!(OpReturnData::new(data).is_ok());
    }

    #[test]
    fn rejects_payload_over_the_limit() {
        let data = vec![0u8; MAX_OP_RETURN_BYTES + 1];
        assert_eq!(
            OpReturnData::new(data),
            Err(OpReturnDataError::TooLong {
                len: MAX_OP_RETURN_BYTES + 1,
                max: MAX_OP_RETURN_BYTES,
            })
        );
    }

    #[test]
    fn accepts_empty_payload() {
        assert_eq!(OpReturnData::new(vec![]).unwrap().as_bytes(), b"");
    }

    #[test]
    fn preserves_the_payload_bytes() {
        let memo = b"=:ZEC.ZEC:tzabc:0/1/0".to_vec();
        let data = OpReturnData::new(memo.clone()).unwrap();
        assert_eq!(data.as_bytes(), memo.as_slice());
    }
}
