//! the counterpart to mod save, these functions find a LightWallet and convert it to a LightClient using methods in instantiation.

use log::debug;
use std::{
    fs::File,
    io::{self, BufReader, Error, ErrorKind, Read},
};
use tokio::runtime::Runtime;

use crate::config::ZingoConfig;

use super::LightClient;
use crate::wallet::LightWallet;

impl LightClient {
    /// TODO: Add Doc Comment Here!
    pub async fn read_wallet_from_buffer_async<R: Read>(
        config: &ZingoConfig,
        mut reader: R,
    ) -> io::Result<Self> {
        let wallet = LightWallet::read_internal(&mut reader, config).await?;

        let lc = LightClient::create_from_wallet_async(wallet).await?;

        debug!(
            "Read wallet with birthday {}",
            lc.wallet.get_birthday().await
        );
        debug!("Created LightClient to {}", &config.get_lightwalletd_uri());

        Ok(lc)
    }

    /// This constructor depends on a wallet that's read from a buffer.
    /// It is used internally by read_from_disk, and directly called by
    /// zingo-mobile.
    pub fn read_wallet_from_buffer_runtime<R: Read>(
        config: &ZingoConfig,
        reader: R,
    ) -> io::Result<Self> {
        Runtime::new()
            .unwrap()
            .block_on(async move { Self::read_wallet_from_buffer_async(config, reader).await })
    }

    /// TODO: Add Doc Comment Here!
    pub fn read_wallet_from_disk(config: &ZingoConfig) -> io::Result<Self> {
        let wallet_path = if config.wallet_path_exists() {
            config.get_wallet_path()
        } else {
            return Err(Error::new(
                ErrorKind::NotFound,
                format!(
                    "Cannot read wallet. No file at {}",
                    config.get_wallet_path().display()
                ),
            ));
        };
        LightClient::read_wallet_from_buffer_runtime(
            config,
            BufReader::new(File::open(wallet_path)?),
        )
    }
}
