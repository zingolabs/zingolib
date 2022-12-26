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
pub mod wallet_internal_memo_handling;

#[cfg(test)]
pub(crate) mod test_vectors;

#[cfg(feature = "embed_params")]
#[derive(RustEmbed)]
#[folder = "zcash-params/"]
pub struct SaplingParams;
use std::{
    io::{ErrorKind, Result},
    sync::{Arc, RwLock},
};
use tokio::runtime::Runtime;
use zingoconfig::{ChainType, ZingoConfig};

pub fn create_zingoconf_from_datadir(
    server: http::Uri,
    data_dir: Option<String>,
) -> Result<(ZingoConfig, u64)> {
    //! This call depends on a running lightwalletd it uses the ligthtwalletd
    //! to find out what kind of chain it's running against.
    use std::net::ToSocketAddrs;

    Runtime::new().unwrap().block_on(async move {
        // Test for a connection first

        format!("{}:{}", server.host().unwrap(), server.port().unwrap())
            .to_socket_addrs()?
            .next()
            .ok_or(std::io::Error::new(
                ErrorKind::ConnectionRefused,
                "Couldn't resolve server!",
            ))?;

        // Do a getinfo first, before opening the wallet
        let info = grpc_connector::GrpcConnector::get_info(server.clone())
            .await
            .map_err(|e| std::io::Error::new(ErrorKind::ConnectionRefused, e))?;

        // Create a Light Client Config
        let config = ZingoConfig {
            server_uri: Arc::new(RwLock::new(server)),
            chain: match info.chain_name.as_str() {
                "main" => ChainType::Mainnet,
                "test" => ChainType::Testnet,
                "regtest" => ChainType::Regtest,
                "fakemainnet" => ChainType::FakeMainnet,
                _ => panic!("Unknown network"),
            },
            monitor_mempool: true,
            reorg_buffer_offset: zingoconfig::REORG_BUFFER_OFFSET,
            data_dir,
        };

        Ok((config, info.block_height))
    })
}
