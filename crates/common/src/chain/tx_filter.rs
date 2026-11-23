use super::BlockId;
use zcash_protocol::TxId;

// Built by the caller, not received from a server; nothing to validate. See the note in
// `block_range`.

/// Identifies a transaction to fetch from a server, by its position in a block or by txid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxFilter {
    /// The block the transaction is in, when identifying it by position.
    pub block: Option<BlockId>,
    /// The transaction's index within that block.
    pub index: u64,
    /// The transaction's id, when identifying it by txid.
    pub hash: Option<TxId>,
}
