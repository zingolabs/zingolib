#![forbid(unsafe_code)]
#[macro_use]
extern crate rust_embed;
mod test_framework;

pub mod blaze;
pub mod commands;
pub mod compact_formats;
pub mod grpc_connector;
pub mod lightclient;
pub mod wallet;

pub use blaze::block_witness_data::BATCHSIZE;

// This line includes the generated `git_description()` function directly into this scope.
include!(concat!(env!("OUT_DIR"), "/git_description.rs"));

#[cfg(feature = "embed_params")]
#[derive(RustEmbed)]
#[folder = "zcash-params/"]
pub struct SaplingParams;
use std::{
    io::{ErrorKind, Result},
    path::PathBuf,
    sync::{Arc, RwLock},
};
use zingoconfig::{ChainType, ZingoConfig, DEFAULT_LOGFILE_NAME, DEFAULT_WALLET_NAME};

pub fn load_clientconfig(
    lightwallet_uri: http::Uri,
    data_dir: Option<PathBuf>,
    chain: ChainType,
    monitor_mempool: bool,
    regtest_orchard_activation_height: Option<zcash_primitives::consensus::BlockHeight>,
) -> Result<ZingoConfig> {
    use std::net::ToSocketAddrs;
    format!(
        "{}:{}",
        lightwallet_uri.host().unwrap(),
        lightwallet_uri.port().unwrap()
    )
    .to_socket_addrs()?
    .next()
    .ok_or(std::io::Error::new(
        ErrorKind::ConnectionRefused,
        "Couldn't resolve server!",
    ))?;

    // Create a Light Client Config
    let config = ZingoConfig {
        lightwalletd_uri: Arc::new(RwLock::new(lightwallet_uri)),
        chain,
        monitor_mempool,
        reorg_buffer_offset: zingoconfig::REORG_BUFFER_OFFSET,
        wallet_dir: data_dir,
        wallet_name: DEFAULT_WALLET_NAME.into(),
        logfile_name: DEFAULT_LOGFILE_NAME.into(),
        regtest_orchard_activation_height,
    };

    Ok(config)
}
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
