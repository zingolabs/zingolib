//! Map nullifiers to block height for a range of block heights.
//!
//! - Takes a range of block heights from `scan director`
//! - Requests the range of compact blocks from `proxy connector` (or requests nullifiers and
//! block heights [and TXIDs?] directly if available)
//! - Creates a list of nullifiers ordered by block height
//! - Creates a map with each nullifier as the key and the corresponding blockheight
//! as the value
//! - Scans from highest to lowest so its clear of the wallet state on reload
//! - Returns success result
