//! Trial decrypt a range of compact blocks.
//!
//! Takes a range of block heights from the scan director
//! Requests the range of compact blocks from the proxy connector
//! Trial decrypts each output within each compact transaction
//! Sends the TXID of any compact transaction with successfully decrypted outputs to the scan
//! director in realtime (channel?)
//! Returns a list of TXIDs with successfully decrytped outputs to the scan director for verifying
//! correct wallet load state. This allows a new trial decryptor to start scanning the next batch
//! without losing track of scan state
