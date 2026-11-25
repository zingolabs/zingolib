/// A Sapling Output Description's compact fields: the note commitment, the ephemeral public
/// key, and the first 52 bytes of the encrypted ciphertext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactSaplingOutput {
    /// The note commitment u-coordinate. Not fixed-length-validated here: the deprecated
    /// nullifiers-only block fetch legitimately omits output data, so a server may send this
    /// empty. [`crate::chain`]'s consumer validates the length only when it actually needs to
    /// decrypt the output.
    pub cmu: Vec<u8>,
    /// The ephemeral public key. See the note on [`Self::cmu`].
    pub ephemeral_key: Vec<u8>,
    /// The first 52 bytes of the encrypted ciphertext. See the note on [`Self::cmu`].
    pub ciphertext: Vec<u8>,
}
