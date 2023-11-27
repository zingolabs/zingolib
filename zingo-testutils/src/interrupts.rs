use zingolib::lightclient::LightClient;

pub async fn sync_with_timeout_millis(lightclient: &LightClient, timeout: u64) -> Result<(), ()> {
    tokio::select!(
        _ = tokio::time::sleep(std::time::Duration::from_millis(timeout)) => Err(()),
        _ = lightclient.do_sync(false) => Ok(()),
    )
}
