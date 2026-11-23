use super::BlockRange;

// Built by the caller, not received from a server; nothing to validate. See the note in
// `block_range`.

/// Requests a transparent address's transactions in a block range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransparentAddressBlockFilter {
    /// The transparent address to look up.
    pub address: String,
    /// The block range to search, or `None` for the whole chain.
    pub range: Option<BlockRange>,
}
