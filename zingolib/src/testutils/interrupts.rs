//! TODO: Add Mod Description Here!

use crate::lightclient::LightClient;

/// TODO: Add Doc Comment Here!
pub async fn sync_with_timeout_millis(lightclient: &LightClient, timeout: u64) -> Result<(), ()> {
    tokio::select!(
        biased;
        _ = tokio::time::sleep(std::time::Duration::from_millis(timeout)) => Err(()),
        _ = lightclient.do_sync(false) => Ok(()),
    )
}
