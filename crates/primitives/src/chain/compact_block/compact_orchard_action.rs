use super::{InvalidCompactBlock, fixed_bytes};

/// An Orchard (or Ironwood) Action's compact fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactOrchardAction {
    /// The nullifier of the input note. Always populated: this is the one field the
    /// deprecated nullifiers-only block fetch exists to serve.
    pub nullifier: [u8; 32],
    /// The x-coordinate of the note commitment for the output note. Not fixed-length-validated
    /// here: an Orchard action carries both spend and output data in one message, so the
    /// nullifiers-only fetch serves this (and the two fields below) empty rather than omitting
    /// the action outright. The consumer validates the length only when it actually needs to
    /// decrypt the output.
    pub cmx: Vec<u8>,
    /// An encoding of an ephemeral Pallas public key. See the note on [`Self::cmx`].
    pub ephemeral_key: Vec<u8>,
    /// The first 52 bytes of the encrypted ciphertext. See the note on [`Self::cmx`].
    pub ciphertext: Vec<u8>,
}

impl CompactOrchardAction {
    /// Parses a compact Orchard (or Ironwood) action from its wire fields, validating only the
    /// nullifier: see the field docs for why the rest are not validated here.
    pub fn from_wire(
        nullifier: Vec<u8>,
        cmx: Vec<u8>,
        ephemeral_key: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Result<Self, InvalidCompactBlock> {
        Ok(Self {
            nullifier: fixed_bytes("CompactOrchardAction.nullifier", nullifier)?,
            cmx,
            ephemeral_key,
            ciphertext,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CompactOrchardAction;

    /// The nullifier is the one field the deprecated nullifiers-only fetch exists to serve, so
    /// it alone is validated.
    #[test]
    fn a_32_byte_nullifier_is_accepted() {
        let action =
            CompactOrchardAction::from_wire(vec![0xaa; 32], vec![], vec![], vec![]).unwrap();
        assert_eq!(action.nullifier, [0xaa; 32]);
    }

    #[test]
    fn a_wrong_length_nullifier_is_rejected() {
        assert!(CompactOrchardAction::from_wire(vec![0xaa; 31], vec![], vec![], vec![]).is_err());
    }

    /// A nullifiers-only fetch serves an Orchard action with its output data hollowed out
    /// (empty, not omitted): regression coverage for the eager-validation bug that broke this
    /// exact shape. See crate-level notes on [`super::CompactOrchardAction`].
    #[test]
    fn empty_output_payload_is_accepted_unvalidated() {
        let action =
            CompactOrchardAction::from_wire(vec![0xaa; 32], vec![], vec![], vec![]).unwrap();
        assert!(action.cmx.is_empty());
        assert!(action.ephemeral_key.is_empty());
        assert!(action.ciphertext.is_empty());
    }

    /// The payload fields pass through byte-for-byte, whatever their length: only the nullifier
    /// is shape-checked.
    #[test]
    fn arbitrary_output_payload_passes_through_unchanged() {
        let action = CompactOrchardAction::from_wire(
            vec![0xaa; 32],
            vec![1, 2, 3],
            vec![4; 32],
            vec![5; 52],
        )
        .unwrap();
        assert_eq!(action.cmx, vec![1, 2, 3]);
        assert_eq!(action.ephemeral_key, vec![4; 32]);
        assert_eq!(action.ciphertext, vec![5; 52]);
    }
}
