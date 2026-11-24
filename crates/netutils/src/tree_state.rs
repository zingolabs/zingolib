#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireParts {
    pub height: u64,
    pub hash_hex: String,
    pub sapling_tree_hex: String,
    pub orchard_tree_hex: String,
    pub ironwood_tree_hex: String,
}
