use super::{InvalidCompactBlock, fixed_bytes};

/// A Sapling Spend Description's nullifier, as carried in a compact block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactSaplingSpend {
    /// The nullifier of the note being spent.
    pub nullifier: [u8; 32],
}

impl CompactSaplingSpend {
    /// Parses a compact Sapling spend from its wire nullifier bytes.
    pub fn from_wire(nullifier: Vec<u8>) -> Result<Self, InvalidCompactBlock> {
        Ok(Self {
            nullifier: fixed_bytes("CompactSaplingSpend.nf", nullifier)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CompactSaplingSpend;

    #[test]
    fn a_32_byte_nullifier_is_accepted() {
        assert_eq!(
            CompactSaplingSpend::from_wire(vec![0xaa; 32])
                .unwrap()
                .nullifier,
            [0xaa; 32]
        );
    }

    #[test]
    fn a_wrong_length_nullifier_is_rejected() {
        for len in [0, 31, 33] {
            assert!(
                CompactSaplingSpend::from_wire(vec![0xaa; len]).is_err(),
                "len {len}"
            );
        }
    }
}
