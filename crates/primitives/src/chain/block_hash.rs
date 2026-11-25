use std::fmt;

/// The identifier for a Zcash block: the SHA-256d hash of its encoded header, byte-reversed for
/// display (matching `zcash_primitives::block::BlockHash`, which this type mirrors without
/// depending on `zcash_primitives` and the shielded-protocol crates it pulls in).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockHash(pub [u8; 32]);

impl fmt::Debug for BlockHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("BlockHash")
            .field(&self.to_string())
            .finish()
    }
}

impl fmt::Display for BlockHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut data = self.0;
        data.reverse();
        formatter.write_str(&hex::encode(data))
    }
}

impl BlockHash {
    /// Constructs a [`BlockHash`] from `bytes`, or `None` when its length is not 32.
    pub fn try_from_slice(bytes: &[u8]) -> Option<Self> {
        <[u8; 32]>::try_from(bytes).ok().map(BlockHash)
    }
}

#[cfg(test)]
mod tests {
    use super::BlockHash;

    #[test]
    fn a_32_byte_slice_round_trips() {
        let bytes = [0xaa; 32];
        assert_eq!(BlockHash::try_from_slice(&bytes), Some(BlockHash(bytes)));
    }

    #[test]
    fn a_wrong_length_slice_is_rejected() {
        assert_eq!(BlockHash::try_from_slice(&[0xaa; 31]), None);
        assert_eq!(BlockHash::try_from_slice(&[0xaa; 33]), None);
    }

    #[test]
    fn display_byte_reverses_before_hex_encoding() {
        let mut bytes = [0u8; 32];
        bytes[0] = 0x01;
        assert_eq!(
            BlockHash(bytes).to_string(),
            "0000000000000000000000000000000000000000000000000000000000000001"
        );
    }
}
