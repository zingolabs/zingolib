use tokio::sync::mpsc::UnboundedSender;
use zcash_client_backend::data_api::scanning::ScanRange;

use crate::client::{get_compact_block_range, FetchRequest};

pub(crate) async fn scanner(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    scan_range: ScanRange,
) -> Result<(), ()> {
    let _compact_blocks =
        get_compact_block_range(fetch_request_sender, scan_range.block_range().clone())
            .await
            .unwrap();

    Ok(())
}
