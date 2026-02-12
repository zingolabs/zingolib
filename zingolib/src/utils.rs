//! General library utilities such as parsing and conversions.

use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub mod conversion;
pub mod error;

/// Writes `bytes` to file at `path`.
pub(crate) fn write_to_path(wallet_path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // let mut file = tokio::fs::File::create(path).await?;
    // file.write_all(&bytes).await

    let temp_wallet_path: PathBuf = wallet_path.with_extension(
        wallet_path
            .extension()
            .map(|e| format!("{}.tmp", e.to_string_lossy()))
            .unwrap_or_else(|| "tmp".to_string()),
    );
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp_wallet_path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(bytes)?;
    writer.flush()?;

    let file = writer.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()?;
    std::fs::rename(&temp_wallet_path, wallet_path)?;
    if let Some(parent) = wallet_path.parent() {
        let wallet_dir = File::open(parent)?;
        let _ignore_error = wallet_dir.sync_all(); // NOTE: error is ignored as syncing dirs on windows OS may return an error
    }

    Ok(())
}

/// Returns number of seconds since unix epoch.
pub(crate) fn now() -> u32 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("should never fail when comparing with an instant so far in the past")
        .as_secs() as u32
}
