//! Scans a transaction.
//!
//! Takes a TXID from `scan director`
//! Requests the transaction from the `proxy connector`
//! Decrypts all the outputs and creates shielded notes
//! Creates transparent notes for the wallet's transparent addresses
//! Creates a basic transaction structure with references to all nullifiers and notes
//! Returns success result

mod fees;
