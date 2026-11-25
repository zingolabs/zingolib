// Built by the caller, not received from a server; nothing to validate. See the note in
// `block_range`.

/// Requests a shielded pool's subtree roots, from a starting index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GetSubtreeRootsArg {
    /// The index to start returning subtree roots from.
    pub start_index: u32,
    /// The shielded pool to return subtree roots for, as the wire format's shielded-protocol
    /// encoding.
    pub shielded_protocol: i32,
    /// The maximum number of entries to return, or zero for unlimited.
    pub max_entries: u32,
}
