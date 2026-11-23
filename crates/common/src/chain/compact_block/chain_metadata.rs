/// Information about the state of the chain as of the end of a block: each shielded pool's
/// note commitment tree size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainMetadata {
    /// The size of the Sapling note commitment tree as of the end of this block.
    pub sapling_commitment_tree_size: u32,
    /// The size of the Orchard note commitment tree as of the end of this block.
    pub orchard_commitment_tree_size: u32,
    /// The size of the Ironwood note commitment tree as of the end of this block.
    pub ironwood_commitment_tree_size: u32,
}
