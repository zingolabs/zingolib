//! General library utilities such as parsing and conversions.

use std::{path::Path, time::SystemTime};

use tokio::io::AsyncWriteExt as _;

pub mod conversion;
pub mod error;

/// Writes `bytes` to file at `path`.
pub(crate) async fn write_to_path(path: &Path, bytes: Vec<u8>) -> std::io::Result<()> {
    let mut file = tokio::fs::File::create(path).await?;
    file.write_all(&bytes).await
}

/// Returns number of seconds since unix epoch.
pub(crate) fn now() -> u32 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("should never fail when comparing with an instant so far in the past")
        .as_secs() as u32
}
