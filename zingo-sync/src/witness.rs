//! Shardtree witness construction.
//!
//! Undefined.
//! - May use the compact blocks from `trial decrypt` batches or fetch it's own batches
//! - Marks all leaves for retention until later updated to ephemeral if not needed
//! - Optimal place to mark ephemeral? `trial decrypt`?
//! - Spend before sync? fetching shards?
