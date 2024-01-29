//! Map nullifiers to TXID and block height for a range of block heights.
//!
//! Takes a range of block heights from `scan director`
//! Requests the range of compact blocks from `proxy connector` (or requests nullifiers, TXIDs and
//! block heights directly if available)
//! Creates a map with each nullifier as the key and the corresponding txid and blockheight
//! as the value
//! Scans from highest to lowest so its clear of the wallet state on reload
//! Returns success result
