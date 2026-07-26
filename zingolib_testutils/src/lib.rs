//! Test infrastructure shared by the crates that exercise `zingolib` against
//! a live regtest network.
//!
//! These modules build and observe that network; they do not test it.
//! [`scenarios`] assembles the clients and funding a test starts from, and
//! [`chain_cache`] records the blocks a setup generated so a later run
//! replays them instead of regenerating them. [`setup_metrics`] meters what
//! each setup costs in time and disk.
//!
//! The remaining modules answer questions the wallet's own view cannot.
//! [`validator_rpc`] speaks JSON-RPC to the Validator directly, bypassing the
//! Indexer; [`attribution`] uses that direct channel to charge a send failure
//! to the wallet, the Indexer, or the Validator; and [`observability`] taps
//! the whole pipeline so every change to the chain can be accounted for.

#![forbid(unsafe_code)]

pub mod attribution;
pub mod chain_cache;
pub mod observability;
pub mod scenarios;
pub mod setup_metrics;
pub mod validator_rpc;
