use tokio::sync::mpsc::UnboundedSender;
use zcash_client_backend::{data_api::scanning::ScanRange, scanning::ScanningKeys};

use crate::{
    client::{get_compact_block_range, get_frontiers, FetchRequest},
    interface::SyncWallet,
};

pub(crate) async fn scanner<W>(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    wallet: &W,
    scan_range: ScanRange,
) -> Result<(), ()>
where
    W: SyncWallet,
{
    let _compact_blocks = get_compact_block_range(
        fetch_request_sender.clone(),
        scan_range.block_range().clone(),
    )
    .await
    .unwrap();

    let _frontiers = get_frontiers(fetch_request_sender, scan_range.block_range().start - 1)
        .await
        .unwrap();

    let account_ufvks = wallet.get_unified_full_viewing_keys().unwrap();
    let _scanning_keys = ScanningKeys::from_account_ufvks(account_ufvks);

    Ok(())
}
