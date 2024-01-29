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
//!
//! Also includes functionality for iterating through all shielded receipts and checking if the
//! calculated nullifier matches a nullifier in the nullifier map.
//! If a match is found, send the block height (or TXID if available) of the transaction the note
//! was consumed to `scan director`, otherwise a sending transaction may not be created in the case
//! where no change was received. Also the transaction must be created to change state of receipt
//! to spend.
//!
//! This also has the added benefit of "rifts" where a newly created note is immediately linked
//! to the transaction it was consumed in via the nullifier map and can enable targetted
//! transaction scanning, similar to (or maybe _IS_) DAG sync.
