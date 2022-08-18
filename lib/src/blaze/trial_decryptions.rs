use crate::{
    compact_formats::CompactBlock,
    wallet::{
        data::{ChannelNullifier, TransactionMetadata},
        keys::Keys,
        transactions::TransactionMetadataSet,
        MemoDownloadOption,
    },
};
use futures::{stream::FuturesUnordered, StreamExt};
use log::info;
use orchard::{
    keys::{FullViewingKey as OrchardFvk, IncomingViewingKey as OrchardIvk},
    note_encryption::{CompactAction, OrchardDomain},
};
use std::sync::Arc;
use tokio::{
    sync::{
        mpsc::{unbounded_channel, UnboundedSender},
        oneshot, RwLock,
    },
    task::JoinHandle,
};
use zcash_primitives::{
    consensus::BlockHeight,
    sapling::{note_encryption::try_sapling_compact_note_decryption, SaplingIvk},
    transaction::{Transaction, TxId},
};

use super::syncdata::BlazeSyncData;

pub struct TrialDecryptions {
    keys: Arc<RwLock<Keys>>,
    transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
}

impl TrialDecryptions {
    pub fn new(
        keys: Arc<RwLock<Keys>>,
        transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
    ) -> Self {
        Self {
            keys,
            transaction_metadata_set,
        }
    }

    pub async fn start(
        &self,
        bsync_data: Arc<RwLock<BlazeSyncData>>,
        detected_transaction_id_sender: UnboundedSender<(
            TxId,
            ChannelNullifier,
            BlockHeight,
            Option<u32>,
        )>,
        full_transaction_fetcher: UnboundedSender<(
            TxId,
            oneshot::Sender<Result<Transaction, String>>,
        )>,
    ) -> (JoinHandle<()>, UnboundedSender<CompactBlock>) {
        //info!("Starting trial decrptions processor");

        // Create a new channel where we'll receive the blocks
        let (transmitter, mut receiver) = unbounded_channel::<CompactBlock>();

        let keys = self.keys.clone();
        let transaction_metadata_set = self.transaction_metadata_set.clone();

        let h = tokio::spawn(async move {
            let mut workers = FuturesUnordered::new();
            let mut cbs = vec![];

            let sapling_ivks = Arc::new(
                keys.read()
                    .await
                    .zkeys
                    .iter()
                    .map(|zk| zk.extfvk().fvk.vk.ivk())
                    .collect::<Vec<_>>(),
            );
            let orchard_ivks = Arc::new(keys.read().await.get_all_orchard_keys_of_type());

            while let Some(cb) = receiver.recv().await {
                cbs.push(cb);

                if cbs.len() >= 1_000 {
                    let keys = keys.clone();
                    let sapling_ivks = sapling_ivks.clone();
                    let orchard_ivks = orchard_ivks.clone();
                    let transaction_metadata_set = transaction_metadata_set.clone();
                    let bsync_data = bsync_data.clone();
                    let detected_transaction_id_sender = detected_transaction_id_sender.clone();

                    workers.push(tokio::spawn(Self::trial_decrypt_batch(
                        cbs.split_off(0),
                        keys,
                        bsync_data,
                        sapling_ivks,
                        orchard_ivks,
                        transaction_metadata_set,
                        detected_transaction_id_sender,
                        full_transaction_fetcher.clone(),
                    )));
                }
            }

            workers.push(tokio::spawn(Self::trial_decrypt_batch(
                cbs,
                keys,
                bsync_data,
                sapling_ivks,
                orchard_ivks,
                transaction_metadata_set,
                detected_transaction_id_sender,
                full_transaction_fetcher,
            )));

            while let Some(r) = workers.next().await {
                r.unwrap().unwrap();
            }

            //info!("Finished final trial decryptions");
        });

        return (h, transmitter);
    }

    /// Trial decryption is invoked by spend-or-view key holders to detect notes addressed to shielded addresses derived from
    /// the spend key.
    async fn trial_decrypt_batch(
        compact_blocks: Vec<CompactBlock>,
        keys: Arc<RwLock<Keys>>,
        bsync_data: Arc<RwLock<BlazeSyncData>>,
        sapling_ivks: Arc<Vec<SaplingIvk>>,
        orchard_ivks: Arc<Vec<OrchardIvk>>,
        transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
        detected_transaction_id_sender: UnboundedSender<(
            TxId,
            ChannelNullifier,
            BlockHeight,
            Option<u32>,
        )>,
        full_transaction_fetcher: UnboundedSender<(
            TxId,
            oneshot::Sender<Result<Transaction, String>>,
        )>,
    ) -> Result<(), String> {
        let config = keys.read().await.config().clone();
        let blk_count = compact_blocks.len();
        let mut workers = FuturesUnordered::new();

        let download_memos = bsync_data.read().await.wallet_options.download_memos;

        for compact_block in compact_blocks {
            let height = BlockHeight::from_u32(compact_block.height as u32);

            for (transaction_num, compact_transaction) in compact_block.vtx.iter().enumerate() {
                let mut transaction_metadata = false;

                for (output_num, compact_output) in compact_transaction.outputs.iter().enumerate() {
                    if let Err(_) = compact_output.epk() {
                        continue;
                    };

                    for (i, ivk) in sapling_ivks.iter().enumerate() {
                        if let Some((note, to)) = try_sapling_compact_note_decryption(
                            &config.chain,
                            height,
                            &ivk,
                            compact_output,
                        ) {
                            transaction_metadata = true;

                            let keys = keys.clone();
                            let bsync_data = bsync_data.clone();
                            let transaction_metadata_set = transaction_metadata_set.clone();
                            let detected_transaction_id_sender =
                                detected_transaction_id_sender.clone();
                            let timestamp = compact_block.time as u64;
                            let compact_transaction = compact_transaction.clone();

                            workers.push(tokio::spawn(async move {
                                let keys = keys.read().await;
                                let extfvk = keys.zkeys[i].extfvk();
                                let have_sapling_spending_key =
                                    keys.have_sapling_spending_key(extfvk);
                                let uri = bsync_data.read().await.uri().clone();

                                // Get the witness for the note
                                let witness = bsync_data
                                    .read()
                                    .await
                                    .block_data
                                    .get_sapling_note_witness(
                                        uri,
                                        height,
                                        transaction_num,
                                        output_num,
                                    )
                                    .await?;

                                let transaction_id =
                                    TransactionMetadata::new_txid(&compact_transaction.hash);
                                let nullifier = note.nf(&extfvk.fvk.vk, witness.position() as u64);

                                transaction_metadata_set.write().await.add_new_sapling_note(
                                    transaction_id.clone(),
                                    height,
                                    false,
                                    timestamp,
                                    note,
                                    to,
                                    &extfvk,
                                    have_sapling_spending_key,
                                    witness,
                                );

                                info!("Trial decrypt Detected txid {}", &transaction_id);

                                detected_transaction_id_sender
                                    .send((
                                        transaction_id,
                                        ChannelNullifier::Sapling(nullifier),
                                        height,
                                        Some(output_num as u32),
                                    ))
                                    .unwrap();

                                Ok::<_, String>(())
                            }));

                            // No need to try the other ivks if we found one
                            break;
                        }
                    }
                }
                for (action_num, action) in compact_transaction.actions.iter().enumerate() {
                    let action = match CompactAction::try_from(action) {
                        Ok(a) => a,
                        Err(_e) => {
                            todo!("Implement error handling for action parsing")
                        }
                    };
                    for (i, ivk) in orchard_ivks.iter().cloned().enumerate() {
                        if let Some((note, recipient)) =
                            zcash_note_encryption::try_compact_note_decryption(
                                &OrchardDomain::for_nullifier(action.nullifier()),
                                &ivk,
                                &action,
                            )
                        {
                            let keys = keys.clone();
                            let bsync_data = bsync_data.clone();
                            let transaction_metadata_set = transaction_metadata_set.clone();
                            let detected_transaction_id_sender =
                                detected_transaction_id_sender.clone();
                            let timestamp = compact_block.time as u64;
                            let compact_transaction = compact_transaction.clone();

                            workers.push(tokio::spawn(async move {
                                let keys = keys.read().await;
                                let fvk = OrchardFvk::try_from(&keys.okeys[i].key);
                                let have_orchard_spending_key =
                                    keys.have_orchard_spending_key(&ivk);
                                let uri = bsync_data.read().await.uri().clone();

                                // Get the witness for the note
                                let witness = bsync_data
                                    .read()
                                    .await
                                    .block_data
                                    .get_orchard_note_witness(
                                        uri,
                                        height,
                                        transaction_num,
                                        action_num,
                                    )
                                    .await?;

                                let transaction_id = TransactionMetadata::new_txid(&compact_transaction.hash);

                                if let Ok(ref fvk) = &fvk {
                                    let nullifier = note.nullifier(fvk);
                                    transaction_metadata_set.write().await.add_new_orchard_note(
                                        transaction_id.clone(),
                                        height,
                                        false,
                                        timestamp,
                                        note,
                                        recipient,
                                        &fvk,
                                        have_orchard_spending_key,
                                        witness,
                                    );

                                    info!("Trial decrypt Detected txid {}", &transaction_id);

                                    detected_transaction_id_sender
                                        .send((
                                            transaction_id,
                                            ChannelNullifier::Orchard(nullifier),
                                            height,
                                            Some(action_num as u32),
                                        ))
                                        .unwrap();
                                } else {
                                    eprintln!(
                                        "TODO: Transaction decryption without fvks not yet supported"
                                    );
                                }

                                Ok::<_, String>(())
                            }));

                            // No need to try the other ivks if we found one
                            break;
                        }
                    }
                }

                // Check option to see if we are fetching all transactions.
                if !transaction_metadata && download_memos == MemoDownloadOption::AllMemos {
                    let transaction_id = TransactionMetadata::new_txid(&compact_transaction.hash);
                    let (transmitter, receiver) = oneshot::channel();
                    full_transaction_fetcher
                        .send((transaction_id, transmitter))
                        .unwrap();

                    workers.push(tokio::spawn(async move {
                        // Discard the result, because this was not a wallet transaction.
                        receiver.await.unwrap().map(|_r| ())
                    }));
                }
            }
        }

        while let Some(r) = workers.next().await {
            r.map_err(|e| e.to_string())??;
        }

        // Update sync status
        bsync_data
            .read()
            .await
            .sync_status
            .write()
            .await
            .trial_dec_done += blk_count as u64;

        // Return a nothing-value
        Ok::<(), String>(())
    }
}
