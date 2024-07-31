use std::collections::HashSet;

use orchard::keys::Scope;
use tokio::sync::mpsc::UnboundedSender;
use zcash_client_backend::{data_api::scanning::ScanRange, scanning::ScanningKeys};
use zcash_primitives::{consensus::Parameters, transaction::TxId, zip32::AccountId};

use crate::client::{get_compact_block_range, get_frontiers, FetchRequest};

use self::runners::BatchRunners;

pub(crate) mod runners;

pub(crate) async fn scanner<P>(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    parameters: &P,
    scanning_keys: &ScanningKeys<AccountId, (AccountId, Scope)>,
    scan_range: ScanRange,
) -> Result<(), ()>
where
    P: Parameters + Send + 'static,
{
    let compact_blocks = get_compact_block_range(
        fetch_request_sender.clone(),
        scan_range.block_range().clone(),
    )
    .await
    .unwrap();

    // FIXME: panics when less than 0 for regtest or less than sapling epoch for mainnet
    // let _frontiers = get_frontiers(fetch_request_sender, scan_range.block_range().start - 1)
    //     .await
    //     .unwrap();

    let mut runners = BatchRunners::<_, (), ()>::for_keys(100, &scanning_keys);
    for block in &compact_blocks {
        runners.add_block(parameters, block.clone()).unwrap();
    }
    runners.flush();

    // TODO: store wallet compact blocks and nullifier map
    // compact_blocks.iter().map(|block| {});

    // TODO: build shard tree

    let mut decrypted_txids = HashSet::<TxId>::new();
    for block in &compact_blocks {
        for transaction in block.vtx.iter() {
            let decrypted_sapling_outputs = runners
                .orchard
                .collect_results(block.hash(), transaction.txid());
            decrypted_sapling_outputs.into_keys().for_each(|(txid, _)| {
                decrypted_txids.insert(txid);
            });

            let decrypted_orchard_outputs = runners
                .orchard
                .collect_results(block.hash(), transaction.txid());
            decrypted_orchard_outputs.into_keys().for_each(|(txid, _)| {
                decrypted_txids.insert(txid);
            });
        }
    }

    Ok(())
}
