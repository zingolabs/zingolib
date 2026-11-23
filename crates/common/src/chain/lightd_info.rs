/// An indexer's self-reported identity and chain diagnostics, as returned by `GetLightdInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightdInfo {
    /// The server's software version.
    pub version: String,
    /// The server's vendor string.
    pub vendor: String,
    /// Whether the server supports transparent addresses.
    pub taddr_support: bool,
    /// The chain name ("main", "test", "regtest").
    pub chain_name: String,
    /// The Sapling activation height of the chain.
    pub sapling_activation_height: u64,
    /// The consensus branch ID the server reports.
    pub consensus_branch_id: String,
    /// The server's view of the chain tip height.
    pub block_height: u64,
    /// The git commit the server was built from.
    pub git_commit: String,
    /// The version of the lightwallet protocol served by this server.
    pub lightwallet_protocol_version: String,
}
