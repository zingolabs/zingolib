#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireParts {
    pub height: u64,
    pub hash_hex: String,
    pub sapling_tree_hex: String,
    pub orchard_tree_hex: String,
    pub ironwood_tree_hex: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Failure<Invalid> {
    #[error("the tree state request failed")]
    Request(#[source] tonic::Status),
    #[error("the indexer sent an invalid tree state: {0}")]
    Invalid(Invalid),
}
