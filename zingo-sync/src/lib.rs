#![warn(missing_docs)]
//! Zingo sync engine prototype
//!
//! Entrypoint: [`crate::sync::sync`]
//!
//! Terminology:
//! Chain height - highest block height of best chain from the server
//! Wallet height - highest block height of blockchain known to the wallet. Commonly used, to determine the chain height
//!                 of the previous sync, before the server is contacted to update the wallet height to the new chain height.
//! Fully scanned height - block height in which the wallet has completed scanning all blocks equal to and below this height.

pub mod client;
pub mod error;
pub mod keys;
#[allow(missing_docs)]
pub mod primitives;
pub(crate) mod scan;
pub mod sync;
pub mod traits;
pub(crate) mod utils;
pub mod witness;
