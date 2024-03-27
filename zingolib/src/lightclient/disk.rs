use crate::{
    blaze::syncdata::BlazeSyncData,
    error::{ZingoLibError, ZingoLibResult},
    wallet::{LightWallet, WalletBase},
};

use log::{debug, error};

use std::{
    fs::{remove_file, File},
    io::{self, BufReader, Error, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    runtime::Runtime,
    sync::{Mutex, RwLock},
};

use zingoconfig::ZingoConfig;

use super::LightClient;

static LOG_INIT: std::sync::Once = std::sync::Once::new();

const MARGINAL_FEE: u64 = 5_000; // From ZIP-317

pub(super) struct ZingoSaveBuffer {
    pub buffer: Arc<RwLock<Vec<u8>>>,
}

impl ZingoSaveBuffer {
    fn new(buffer: Vec<u8>) -> Self {
        ZingoSaveBuffer {
            buffer: Arc::new(RwLock::new(buffer)),
        }
    }
}

impl LightClient {
    // initializers

    /// this is the standard initializer for a LightClient.
    pub async fn create_from_wallet_async(
        wallet: LightWallet,
        config: ZingoConfig,
    ) -> io::Result<Self> {
        let mut buffer: Vec<u8> = vec![];
        wallet.write(&mut buffer).await?;
        Ok(LightClient {
            wallet,
            config: config.clone(),
            mempool_monitor: std::sync::RwLock::new(None),
            sync_lock: Mutex::new(()),
            bsync_data: Arc::new(RwLock::new(BlazeSyncData::new(&config))),
            interrupt_sync: Arc::new(RwLock::new(false)),
            save_buffer: ZingoSaveBuffer::new(buffer),
        })
    }
    /// The wallet this fn associates with the lightclient is specifically derived from
    /// a spend authority.
    /// this pubfn is consumed in zingocli, zingo-mobile, and ZingoPC
    pub fn create_from_wallet_base(
        wallet_base: WalletBase,
        config: &ZingoConfig,
        birthday: u64,
        overwrite: bool,
    ) -> io::Result<Self> {
        Runtime::new().unwrap().block_on(async move {
            LightClient::create_from_wallet_base_async(wallet_base, config, birthday, overwrite)
                .await
        })
    }
    /// The wallet this fn associates with the lightclient is specifically derived from
    /// a spend authority.
    pub async fn create_from_wallet_base_async(
        wallet_base: WalletBase,
        config: &ZingoConfig,
        birthday: u64,
        overwrite: bool,
    ) -> io::Result<Self> {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            if !overwrite && config.wallet_path_exists() {
                return Err(Error::new(
                    ErrorKind::AlreadyExists,
                    format!(
                        "Cannot create a new wallet from seed, because a wallet already exists at:\n{:?}",
                        config.get_wallet_path().as_os_str()
                    ),
                ));
            }
        }
        let lightclient = LightClient::create_from_wallet_async(
            LightWallet::new(config.clone(), wallet_base, birthday)?,
            config.clone(),
        )
        .await?;

        lightclient.set_wallet_initial_state(birthday).await;
        lightclient
            .save_internal_rust()
            .await
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;

        debug!("Created new wallet!");

        Ok(lightclient)
    }
    pub async fn create_unconnected(
        config: &ZingoConfig,
        wallet_base: WalletBase,
        height: u64,
    ) -> io::Result<Self> {
        let lightclient = LightClient::create_from_wallet_async(
            LightWallet::new(config.clone(), wallet_base, height)?,
            config.clone(),
        )
        .await?;
        Ok(lightclient)
    }

    fn create_with_new_wallet(config: &ZingoConfig, height: u64) -> io::Result<Self> {
        Runtime::new().unwrap().block_on(async move {
            let l =
                LightClient::create_unconnected(config, WalletBase::FreshEntropy, height).await?;
            l.set_wallet_initial_state(height).await;

            debug!("Created new wallet with a new seed!");
            debug!("Created LightClient to {}", &config.get_lightwalletd_uri());

            // Save
            l.save_internal_rust()
                .await
                .map_err(|s| io::Error::new(ErrorKind::PermissionDenied, s))?;

            Ok(l)
        })
    }

    /// Create a brand new wallet with a new seed phrase. Will fail if a wallet file
    /// already exists on disk
    pub fn new(config: &ZingoConfig, latest_block: u64) -> io::Result<Self> {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            if config.wallet_path_exists() {
                return Err(Error::new(
                    ErrorKind::AlreadyExists,
                    "Cannot create a new wallet from seed, because a wallet already exists",
                ));
            }
        }

        Self::create_with_new_wallet(config, latest_block)
    }

    // read

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

    pub async fn read_wallet_from_buffer_async<R: Read>(
        config: &ZingoConfig,
        mut reader: R,
    ) -> io::Result<Self> {
        let wallet = LightWallet::read_internal(&mut reader, config).await?;

        let lc = LightClient::create_from_wallet_async(wallet, config.clone()).await?;

        debug!(
            "Read wallet with birthday {}",
            lc.wallet.get_birthday().await
        );
        debug!("Created LightClient to {}", &config.get_lightwalletd_uri());

        Ok(lc)
    }

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

    //        SAVE METHODS

    pub(super) async fn save_internal_rust(&self) -> ZingoLibResult<bool> {
        match self.save_internal_buffer().await {
            Ok(()) => self.rust_write_save_buffer_to_file().await,
            Err(err) => {
                error!("{}", err);
                Err(err)
            }
        }
    }

    pub(crate) async fn save_internal_buffer(&self) -> ZingoLibResult<()> {
        let mut buffer: Vec<u8> = vec![];
        self.wallet
            .write(&mut buffer)
            .await
            .map_err(ZingoLibError::InternalWriteBuffer)?;
        *self.save_buffer.buffer.write().await = buffer;
        Ok(())
    }

    async fn rust_write_save_buffer_to_file(&self) -> ZingoLibResult<bool> {
        #[cfg(any(target_os = "ios", target_os = "android"))]
        // on mobile platforms, saving from this buffer will be handled by the native layer
        {
            // on ios and android just return ok
            return Ok(false);
        }

        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let read_buffer = self.save_buffer.buffer.read().await;
            if !read_buffer.is_empty() {
                LightClient::write_to_file(self.config.get_wallet_path(), &read_buffer)
                    .map_err(ZingoLibError::WriteFile)?;
                Ok(true)
            } else {
                ZingoLibError::EmptySaveBuffer.handle()
            }
        }
    }

    fn write_to_file(path: Box<Path>, buffer: &[u8]) -> std::io::Result<()> {
        let mut file = File::create(path)?;
        file.write_all(buffer)?;
        Ok(())
    }

    pub async fn export_save_buffer_async(&self) -> ZingoLibResult<Vec<u8>> {
        // self.save_internal_rust().await?;
        let read_buffer = self.save_buffer.buffer.read().await;
        if !read_buffer.is_empty() {
            Ok(read_buffer.clone())
        } else {
            ZingoLibError::EmptySaveBuffer.handle()
        }
    }

    /// This function is the sole correct way to ask LightClient to save.
    pub fn export_save_buffer_runtime(&self) -> Result<Vec<u8>, String> {
        Runtime::new()
            .unwrap()
            .block_on(async move { self.export_save_buffer_async().await })
            .map_err(String::from)
    }

    pub async fn do_delete(&self) -> Result<(), String> {
        // Check if the file exists before attempting to delete
        if self.config.wallet_path_exists() {
            match remove_file(self.config.get_wallet_path()) {
                Ok(_) => {
                    log::debug!("File deleted successfully!");
                    Ok(())
                }
                Err(e) => {
                    let err = format!("ERR: {}", e);
                    error!("{}", err);
                    log::debug!("DELETE FAIL ON FILE!");
                    Err(e.to_string())
                }
            }
        } else {
            let err = "Error: File does not exist, nothing to delete.".to_string();
            error!("{}", err);
            log::debug!("File does not exist, nothing to delete.");
            Err(err)
        }
    }
    pub(super) fn write_file_if_not_exists(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
        let mut file_path = dir.to_path_buf();
        file_path.push(name);
        if !file_path.exists() {
            let mut file = File::create(&file_path)?;
            file.write_all(bytes)?;
        }

        Ok(())
    }

    /// Some LightClients have a data dir in state. Mobile versions instead rely on a buffer and will return an error if this function is called.
    /// ZingoConfig specifies both a wallet file and a directory containing it.
    /// This function returns a PathBuf, the absolute path of the wallet file typically named zingo-wallet.dat
    pub fn get_wallet_file_location(&self) -> Result<PathBuf, ZingoLibError> {
        if let Some(mut loc) = self.config.wallet_dir.clone() {
            loc.push(self.config.wallet_name.clone());
            Ok(loc)
        } else {
            Err(ZingoLibError::NoWalletLocation)
        }
    }

    /// Some LightClients have a data dir in state. Mobile versions instead rely on a buffer and will return an error if this function is called.
    /// ZingoConfig specifies both a wallet file and a directory containing it.
    /// This function returns a PathBuf, the absolute path of a directory which typically contains a wallet.dat file
    pub fn get_wallet_dir_location(&self) -> Result<PathBuf, ZingoLibError> {
        if let Some(loc) = self.config.wallet_dir.clone() {
            Ok(loc)
        } else {
            Err(ZingoLibError::NoWalletLocation)
        }
    }
}
