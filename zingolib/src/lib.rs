#![warn(missing_docs)]
#![forbid(unsafe_code)]
//! ZingoLib
//! Zingo backend code base
//! Use this high level API to do things like submit transactions to the zcash blockchain

#[macro_use]
extern crate rust_embed;

pub mod blaze;
pub mod commands;
pub mod data;
pub mod grpc_connector;
pub mod lightclient;
pub mod utils;
pub mod wallet;
#[cfg(feature = "test-features")]
pub use zingo_testvectors as testvectors;

#[cfg(feature = "test-features")]
pub mod test_framework;

// This line includes the generated `git_description()` function directly into this scope.
include!(concat!(env!("OUT_DIR"), "/git_description.rs"));

/// TODO: Add Doc Comment Here!
#[cfg(feature = "embed_params")]
#[derive(RustEmbed)]
#[folder = "zcash-params/"]
pub struct SaplingParams;

/// TODO: Add Doc Comment Here!
pub fn get_latest_block_height(lightwalletd_uri: http::Uri) -> std::io::Result<u64> {
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async move {
            crate::grpc_connector::get_info(lightwalletd_uri)
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e))
        })
        .map(|ld_info| ld_info.block_height)
}
