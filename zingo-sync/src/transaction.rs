//! Data structures for transactions.
//!
//! This is a placeholder for specification/comments and does not belong in this crate
//!
//! - Includes a basic transaction for holding references to nullifiers and notes during scanning.
//! - Includes an enhanced transaction which can be created (or change state from basic?) when all
//! the neccessary information has been collected.
//!
//! An enhanced transaction includes references to all notes consumed in the transaction and
//! therefore cannot be created until these notes have been created by scanning previous
//! transactions
