#![forbid(unsafe_code)]
#[macro_use]
extern crate rust_embed;
mod macros;

pub mod blaze;
pub mod commands;
pub mod compact_formats;
pub mod grpc_connector;
pub mod lightclient;
pub mod wallet;

#[cfg(feature = "embed_params")]
#[derive(RustEmbed)]
#[folder = "zcash-params/"]
pub struct SaplingParams;
use std::{
    io::{ErrorKind, Result},
    path::PathBuf,
    sync::{Arc, RwLock},
};
use zingoconfig::{ChainType, ZingoConfig};

pub fn load_clientconfig(
    lightwallet_uri: http::Uri,
    data_dir: Option<PathBuf>,
    chain: ChainType,
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
        monitor_mempool: true,
        reorg_buffer_offset: zingoconfig::REORG_BUFFER_OFFSET,
        zingo_wallet_dir: data_dir,
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
