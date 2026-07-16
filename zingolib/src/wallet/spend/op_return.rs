//! OP_RETURN Data: caller-provided opaque bytes carried in a zero-value,
//! provably unspendable transparent output on the final transaction of a
//! send. The wallet never interprets the bytes; encoding a swap
//! instruction (THORChain, MAYAChain) is the caller's concern. See the
//! glossary entry in `zingolib/CONTEXT.md` — the term is never "memo".

/// The most bytes an OP_RETURN Data payload may carry.
///
/// Mirrors the standard relay rule that `zcash_transparent`'s
/// `add_null_data_output` enforces at build time (its
/// `MAX_OP_RETURN_RELAY_BYTES` is not exported); validating here fails a
/// too-long payload at proposal time, before any plan exists.
pub const MAX_OP_RETURN_DATA_BYTES: usize = 80;

/// Ways a byte payload can fail to be valid OP_RETURN Data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpReturnDataError {
    /// The payload exceeds the relay rule's ceiling.
    #[error("OP_RETURN data of {actual} bytes exceeds the {limit}-byte relay limit")]
    TooLong {
        /// The rejected payload's length in bytes.
        actual: usize,
        /// The ceiling it exceeded: [`MAX_OP_RETURN_DATA_BYTES`].
        limit: usize,
    },
}

/// A validated OP_RETURN Data payload.
///
/// Construction is the sole validation point: a value of this type is
/// always within the relay limit, so the plan and build layers carry it
/// without re-checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpReturnData(Vec<u8>);

impl OpReturnData {
    /// Validates and wraps a payload.
    ///
    /// # Errors
    ///
    /// Returns [`OpReturnDataError::TooLong`] if the payload exceeds
    /// [`MAX_OP_RETURN_DATA_BYTES`].
    pub fn new(bytes: Vec<u8>) -> Result<Self, OpReturnDataError> {
        if bytes.len() > MAX_OP_RETURN_DATA_BYTES {
            return Err(OpReturnDataError::TooLong {
                actual: bytes.len(),
                limit: MAX_OP_RETURN_DATA_BYTES,
            });
        }

        Ok(OpReturnData(bytes))
    }

    /// The validated payload bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl TryFrom<Vec<u8>> for OpReturnData {
    type Error = OpReturnDataError;

    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        OpReturnData::new(bytes)
    }
}

impl TryFrom<&[u8]> for OpReturnData {
    type Error = OpReturnDataError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        OpReturnData::new(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_OP_RETURN_DATA_BYTES, OpReturnData, OpReturnDataError};

    #[test]
    fn accepts_empty_payload() {
        assert!(OpReturnData::new(vec![]).unwrap().as_bytes().is_empty());
    }

    #[test]
    fn accepts_payload_at_the_relay_limit() {
        let payload = vec![0xAB; MAX_OP_RETURN_DATA_BYTES];
        assert_eq!(
            OpReturnData::new(payload.clone()).unwrap().as_bytes(),
            payload.as_slice()
        );
    }

    #[test]
    fn rejects_payload_over_the_relay_limit() {
        let payload = vec![0xAB; MAX_OP_RETURN_DATA_BYTES + 1];
        assert_eq!(
            OpReturnData::new(payload),
            Err(OpReturnDataError::TooLong {
                actual: MAX_OP_RETURN_DATA_BYTES + 1,
                limit: MAX_OP_RETURN_DATA_BYTES,
            })
        );
    }
}
