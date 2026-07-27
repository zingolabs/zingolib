//! Integration tests that exercise the wallet against a live regtest network.
//!
//! Every test in this crate launches a real Validator (`zebrad`) and a real
//! Indexer (`zainod`) through `zcash_local_net`, then drives a `LightClient`
//! against them, so the wallet meets the same protocol surface it meets on
//! mainnet. The test binaries under `tests/` hold the cases themselves.
//!
//! The library half exists to serve those binaries. It publishes
//! [`chain_generics`], which binds `zingolib`'s chain-generic test bodies to
//! this regtest network by implementing `ConductChain` for it.

#![forbid(unsafe_code)]

pub mod chain_generics;
