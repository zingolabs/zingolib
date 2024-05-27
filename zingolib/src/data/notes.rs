//! contains representations of notes

/// contains 1 note, either sapling or orchard
pub enum Note {
    /// sapling
    Sapling(sapling_crypto::note::Note),
    /// orchard
    Orchard(orchard::note::Note),
}
