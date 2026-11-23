use zcash_protocol::consensus::BlockHeight;

// Built by the caller, not received from a server; nothing to validate. See the note in
// `block_range`.

/// Requests unspent transparent outputs for a set of addresses, from a starting height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetAddressUtxosArg {
    /// The transparent addresses to look up.
    pub addresses: Vec<String>,
    /// The height to start returning UTXOs from.
    pub start_height: BlockHeight,
    /// The maximum number of entries to return, or zero for unlimited.
    pub max_entries: u32,
}
