#![forbid(unsafe_code)]
#[macro_use]
extern crate rust_embed;
mod test_framework;

pub mod blaze;
pub mod commands;
pub mod error;
pub mod grpc_connector;
pub mod lightclient;
pub mod wallet;

// This line includes the generated `git_description()` function directly into this scope.
include!(concat!(env!("OUT_DIR"), "/git_description.rs"));

#[cfg(feature = "embed_params")]
#[derive(RustEmbed)]
#[folder = "zcash-params/"]
pub struct SaplingParams;

pub fn get_latest_block_height(lightwalletd_uri: http::Uri) -> std::io::Result<u64> {
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async move {
            crate::grpc_connector::GrpcConnector::get_info(lightwalletd_uri)
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::ConnectionRefused, e))
        })
        .map(|ld_info| ld_info.block_height)
}
