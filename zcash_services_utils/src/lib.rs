#![warn(missing_docs)]
//! # Overview
//!
//! Utilities that launch and manage Zcash processes. This is used for integration
//! testing in the development of:
//!
//!   - lightclients
//!   - indexers
//!   - validators
//!
//!
//! # List of Managed Processes
//! - Zebrad
//! - Zcashd
//! - Zainod
//! - Lightwalletd
//!
//! # Prerequisites
//!
//! An internet connection will be needed (during the fist build at least) in order to fetch the required testing binaries.
//! The binaries will be automagically checked and downloaded on `cargo build/check/test`. If you specify `None` in a process `launch` config, these binaries will be used.
//! The path to the binaries can be specified when launching a process. In that case, you are responsible for compiling the needed binaries.
//! Each processes `launch` fn and [`crate::LocalNet::launch`] take config structs for defining parameters such as path
//! locations.
//! See the config structs for each process in validator.rs and indexer.rs for more details.
//!
//! ## Launching multiple processes
//!
//! See [`crate::LocalNet`].
//!
//! # Testing
//!
//! See [`crate::test_fixtures`] doc comments for running client rpc tests from external crates for indexer/validator development.
//!
//! The `test_fixtures` feature is enabled by default to allow tests to run.
//!
pub mod client;
pub mod test_fixtures;

/// Offer internal "service" logic via a pub interface
pub mod services {
    pub use zcash_services::error;
    pub use zcash_services::indexer;
    pub use zcash_services::network;
    pub use zcash_services::validator;
    pub use zcash_services::LocalNet;
}
