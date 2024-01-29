//! Data structures for transactions.
//!
//! Includes a basic transaction for holdiong references to nullifiers and notes during scanning
//! Includes an enhanced transaction which can be created (or change state from basic?) when all
//! the neccessary information has been collected.
//!
//! An enhanced transaction includes references to all notes consumed in the transaction and
//! therefore cannot be created until all previous compact blocks have been trial decrypted and
//! transactions scanned from any successful decryption attempts.
