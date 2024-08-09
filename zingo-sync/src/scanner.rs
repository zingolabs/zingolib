use std::collections::HashSet;

use orchard::keys::Scope;
use tokio::sync::mpsc::UnboundedSender;
use zcash_client_backend::{
    data_api::scanning::ScanRange, proto::compact_formats::CompactBlock, scanning::ScanningKeys,
    PoolType, ShieldedProtocol,
};
use zcash_primitives::{consensus::Parameters, transaction::TxId, zip32::AccountId};

use crate::{
    client::{get_compact_block_range, FetchRequest},
    primitives::OutputId,
};

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

    // TODO: check continuity

    let (incoming_output_ids, outgoing_output_ids) =
        trial_decrypt(parameters, scanning_keys, &compact_blocks).unwrap();

    // gather the IDs of all transactions relevent to the wallet
    // the edge case of transactions that this capability created but did not receive change
    // or create outgoing data is handled when the nullifiers are added and linked
    let mut relevent_txids: HashSet<TxId> = HashSet::new();
    incoming_output_ids
        .iter()
        .chain(outgoing_output_ids.iter())
        .for_each(|output_id| {
            relevent_txids.insert(*output_id.txid());
        });

    // TODO: build shard tree, calculate nullifiers and positions for incoming outputs, map nullifiers and write compact blocks
    // for compact_block in compact_blocks {
    // let zip212_enforcement = zip212_enforcement(parameters, compact_block.height());
    // }

    // FIXME: panics when less than 0 for regtest or less than sapling epoch for mainnet
    // let _frontiers = get_frontiers(fetch_request_sender, scan_range.block_range().start - 1)
    //     .await
    //     .unwrap();

    // let mut sapling_nullifiers_and_positions: HashMap<
    //     OutputId,
    //     (sapling_crypto::Nullifier, Position),
    // > = HashMap::new();
    // let mut orchard_nullifiers_and_positions: HashMap<
    //     OutputId,
    //     (orchard::note::Nullifier, Position),
    // > = HashMap::new();

    Ok(())
}

fn trial_decrypt<P>(
    parameters: &P,
    scanning_keys: &ScanningKeys<AccountId, (AccountId, Scope)>,
    compact_blocks: &[CompactBlock],
) -> Result<(Vec<OutputId>, Vec<OutputId>), ()>
where
    P: Parameters + Send + 'static,
{
    let mut runners = BatchRunners::<_, (), ()>::for_keys(100, scanning_keys);
    for block in compact_blocks {
        runners.add_block(parameters, block.clone()).unwrap();
    }
    runners.flush();

    let mut incoming_output_ids: Vec<OutputId> = Vec::new();
    let outgoing_output_ids: Vec<OutputId> = Vec::new(); // TODO: add outgoing decryption
    for block in compact_blocks {
        for transaction in block.vtx.iter() {
            let decrypted_sapling_outputs = runners
                .sapling
                .collect_results(block.hash(), transaction.txid());
            decrypted_sapling_outputs
                .into_keys()
                .for_each(|(txid, output_index)| {
                    incoming_output_ids.push(OutputId::from_parts(
                        txid,
                        output_index,
                        PoolType::Shielded(ShieldedProtocol::Sapling),
                    ));
                });

            let decrypted_orchard_outputs = runners
                .orchard
                .collect_results(block.hash(), transaction.txid());
            decrypted_orchard_outputs
                .into_keys()
                .for_each(|(txid, output_index)| {
                    incoming_output_ids.push(OutputId::from_parts(
                        txid,
                        output_index,
                        PoolType::Shielded(ShieldedProtocol::Orchard),
                    ));
                });
        }
    }

    Ok((incoming_output_ids, outgoing_output_ids))
}
