//! LightClient saves internally when it gets to a checkpoint. If has filesystem access, it saves to file at those points. otherwise, it passes the save buffer to the FFI.

use log::error;

use std::{
    fs::{remove_file, File},
    io::Write,
    path::PathBuf,
};

use super::LightClient;
use crate::error::ZingoLibError;

impl LightClient {
    /// If the wallet state has changed since last save, serializes the wallet and returns the wallet bytes.
    /// Returns `Ok(None)` if the wallet state has not changed and save is not required.
    /// Returns error if serialization fails.
    ///
    /// Intended to be called from a save task which calls `save` in a loop, awaiting the wallet lock and checking
    /// `save_required` status, writing the returned wallet bytes to persistance.
    // FIXME: zingo-cli needs a save task
    pub async fn save(&self) -> std::io::Result<Option<Vec<u8>>> {
        let mut wallet_bytes: Vec<u8> = vec![];
        {
            let mut wallet = self.wallet.lock().await;
            if wallet.save_required {
                let network = wallet.network;
                wallet.write(&mut wallet_bytes, &network).await?;
                wallet.save_required = false;
            }
        }

        if wallet_bytes.is_empty() {
            Ok(None)
        } else {
            Ok(Some(wallet_bytes))
        }
    }

    /// Persists the `wallet_bytes` returned from [`crate::lightclient::LightClient::save`] to the wallet path specified
    /// in `self.config`.
    pub async fn persist_wallet_bytes(&self, wallet_bytes: Vec<u8>) -> std::io::Result<()> {
        let mut file = File::create(self.config.get_wallet_path())?;
        file.write_all(&wallet_bytes)
    }

    /// Calls `save` in a runtime and returns an empty buffer in the case save was not required.
    // FIXME: zingo2, this is kept in to make zingomobile integration easier but should be moved into zingo-mobile
    pub fn export_save_buffer_runtime(&mut self) -> Result<Vec<u8>, String> {
        crate::commands::RT.block_on(async move {
            match self.save().await {
                Ok(Some(wallet_bytes)) => Ok(wallet_bytes),
                Ok(None) => Ok(vec![]),
                Err(e) => Err(e.to_string()),
            }
        })
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
