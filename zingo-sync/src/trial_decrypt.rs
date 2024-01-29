//! Trial decrypt a range of compact blocks.
//!
//! - Takes a range of block heights from the scan director
//! - Requests the range of compact blocks from the proxy connector
//! - Trial decrypts each output within each compact transaction
//! - Sends the TXID of any compact transaction with successfully decrypted outputs to `scan
//! director` in realtime
//! - returns success result
