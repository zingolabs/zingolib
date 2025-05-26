#![allow(missing_docs)]
#![forbid(unsafe_code)]
//! ZingoLib
//! Zingo backend library

pub mod commands;
pub mod config;
pub mod data;
pub mod error;
pub mod grpc_connector;
pub mod lightclient;
pub mod utils;
pub mod wallet;

#[cfg(any(test, feature = "test-elevation"))]
pub mod mocks;
#[cfg(any(test, feature = "test-elevation"))]
pub mod testutils;

// This line includes the generated `git_description()` function directly into this scope.
include!(concat!(env!("OUT_DIR"), "/git_description.rs"));

/// TODO: Add Doc Comment Here!
// TODO:  use `get_latest_block` gRPC, also should be removed from zingolib, runtimes should be handled by consumers
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
