//! Shardtree witness construction.
//!
//! Undefined.
//! - May use the compact blocks from `trial decrypt` batches or fetch it's own batches
//! - Marks all leaves for retention until later updated to ephemeral if not needed
//! - Optimal place to mark ephemeral? `trial decrypt`?
//! - Spend before sync? fetching shards?
//! - Need to ensure if a commitment is marked ephemeral when a note changes state from receipt to
//! spend that it is also correctly marked for retention if a re-org causes a note to change back
//! to a receipt
