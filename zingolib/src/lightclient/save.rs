/// LightClient saves internally when it gets to a checkpoint. If has filesystem access, it saves to file at those points. otherwise, it passes the save buffer to the FFI.
use log::error;

use std::{
    fs::{remove_file, File},
    io::{self, Write},
    path::{Path, PathBuf},
};
use tokio::runtime::Runtime;

use super::LightClient;
use crate::error::{ZingoLibError, ZingoLibResult};

impl LightClient {
    //        SAVE METHODS

    /// Called internally at sync checkpoints to save state. Should not be called midway through sync.
    pub(super) async fn save_internal_rust(&self) -> ZingoLibResult<bool> {
        match self.save_internal_buffer().await {
            Ok(()) => self.rust_write_save_buffer_to_file().await,
            Err(err) => {
                error!("{}", err);
                Err(err)
            }
        }
    }

    pub(super) async fn save_internal_buffer(&self) -> ZingoLibResult<()> {
        let mut buffer: Vec<u8> = vec![];
        self.wallet
            .write(&mut buffer)
            .await
            .map_err(ZingoLibError::InternalWriteBufferError)?;
        *self.save_buffer.buffer.write().await = buffer;
        Ok(())
    }

    /// If possible, write to disk. todo conditionally compile this function
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
                    .map_err(ZingoLibError::WriteFileError)?;
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

    pub(super) fn write_file_if_not_exists(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
        let mut file_path = dir.to_path_buf();
        file_path.push(name);
        if !file_path.exists() {
            let mut file = File::create(&file_path)?;
            file.write_all(bytes)?;
        }

        Ok(())
    }

    /// Only relevant in non-mobile, this function removes the save file.
    // TodO: can we shred it?
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
