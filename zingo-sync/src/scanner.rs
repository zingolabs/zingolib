use std::collections::HashSet;

use tokio::sync::mpsc::UnboundedSender;
use zcash_client_backend::{data_api::scanning::ScanRange, scanning::ScanningKeys};
use zcash_primitives::{consensus::Parameters, transaction::TxId};

use crate::{
    client::{get_compact_block_range, get_frontiers, FetchRequest},
    interface::SyncWallet,
};

use self::runners::BatchRunners;

pub(crate) mod runners;

pub(crate) async fn scanner<P, W>(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    parameters: &P,
    wallet: &W,
    scan_range: ScanRange,
) -> Result<(), ()>
where
    P: Parameters + Send + 'static,
    W: SyncWallet,
{
    let compact_blocks = get_compact_block_range(
        fetch_request_sender.clone(),
        scan_range.block_range().clone(),
    )
    .await
    .unwrap();

    // FIXME: panics
    // let _frontiers = get_frontiers(fetch_request_sender, scan_range.block_range().start - 1)
    //     .await
    //     .unwrap();

    let account_ufvks = wallet.get_unified_full_viewing_keys().unwrap();
    let scanning_keys = ScanningKeys::from_account_ufvks(account_ufvks);

    let mut runners = BatchRunners::<_, (), ()>::for_keys(100, &scanning_keys);

    for block in &compact_blocks {
        runners.add_block(parameters, block.clone()).unwrap();
    }
    runners.flush();

    let mut decrypted_txids = HashSet::<TxId>::new();
    for block in &compact_blocks {
        for transaction in block.vtx.iter() {
            let decrypted_outputs = runners
                .orchard
                .collect_results(block.hash(), transaction.txid());
            dbg!(&decrypted_outputs);
            decrypted_outputs.into_keys().for_each(|(txid, _)| {
                decrypted_txids.insert(txid);
            });
        }
    }
    dbg!(decrypted_txids);

    Ok(())
}
