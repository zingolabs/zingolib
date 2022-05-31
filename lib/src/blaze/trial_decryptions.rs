use crate::{
    compact_formats::CompactBlock,
    lightwallet::{data::WalletTx, keys::Keys, wallet_transactions::WalletTxns, MemoDownloadOption},
};
use futures::{stream::FuturesUnordered, StreamExt};
use log::info;
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
    sapling::{note_encryption::try_sapling_compact_note_decryption, Nullifier, SaplingIvk},
    transaction::{Transaction, TxId},
};

use super::syncdata::BlazeSyncData;

pub struct TrialDecryptions {
    keys: Arc<RwLock<Keys>>,
    wallet_transactions: Arc<RwLock<WalletTxns>>,
}

impl TrialDecryptions {
    pub fn new(keys: Arc<RwLock<Keys>>, wallet_transactions: Arc<RwLock<WalletTxns>>) -> Self {
        Self {
            keys,
            wallet_transactions,
        }
    }

    pub async fn start(
        &self,
        bsync_data: Arc<RwLock<BlazeSyncData>>,
        detected_transaction_id_sender: UnboundedSender<(TxId, Nullifier, BlockHeight, Option<u32>)>,
        full_transaction_fetcher: UnboundedSender<(TxId, oneshot::Sender<Result<Transaction, String>>)>,
    ) -> (JoinHandle<()>, UnboundedSender<CompactBlock>) {
        //info!("Starting trial decrptions processor");

        // Create a new channel where we'll receive the blocks
        let (transmitter, mut receiver) = unbounded_channel::<CompactBlock>();

        let keys = self.keys.clone();
        let wallet_transactions = self.wallet_transactions.clone();

        let h = tokio::spawn(async move {
            let mut workers = FuturesUnordered::new();
            let mut cbs = vec![];

            let ivks = Arc::new(
                keys.read()
                    .await
                    .zkeys
                    .iter()
                    .map(|zk| zk.extfvk().fvk.vk.ivk())
                    .collect::<Vec<_>>(),
            );

            while let Some(cb) = receiver.recv().await {
                cbs.push(cb);

                if cbs.len() >= 1_000 {
                    let keys = keys.clone();
                    let ivks = ivks.clone();
                    let wallet_transactions = wallet_transactions.clone();
                    let bsync_data = bsync_data.clone();
                    let detected_transaction_id_sender = detected_transaction_id_sender.clone();

                    workers.push(tokio::spawn(Self::trial_decrypt_batch(
                        cbs.split_off(0),
                        keys,
                        bsync_data,
                        ivks,
                        wallet_transactions,
                        detected_transaction_id_sender,
                        full_transaction_fetcher.clone(),
                    )));
                }
            }

            workers.push(tokio::spawn(Self::trial_decrypt_batch(
                cbs,
                keys,
                bsync_data,
                ivks,
                wallet_transactions,
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

    async fn trial_decrypt_batch(
        cbs: Vec<CompactBlock>,
        keys: Arc<RwLock<Keys>>,
        bsync_data: Arc<RwLock<BlazeSyncData>>,
        ivks: Arc<Vec<SaplingIvk>>,
        wallet_transactions: Arc<RwLock<WalletTxns>>,
        detected_transaction_id_sender: UnboundedSender<(TxId, Nullifier, BlockHeight, Option<u32>)>,
        full_transaction_fetcher: UnboundedSender<(TxId, oneshot::Sender<Result<Transaction, String>>)>,
    ) -> Result<(), String> {
        let config = keys.read().await.config().clone();
        let blk_count = cbs.len();
        let mut workers = FuturesUnordered::new();

        let download_memos = bsync_data.read().await.wallet_options.download_memos;

        for cb in cbs {
            let height = BlockHeight::from_u32(cb.height as u32);

            for (transaction_num, compact_transaction) in cb.vtx.iter().enumerate() {
                let mut wallet_transaction = false;

                for (output_num, co) in compact_transaction.outputs.iter().enumerate() {
                    if let Err(_) = co.epk() {
                        continue;
                    };

                    for (i, ivk) in ivks.iter().enumerate() {
                        if let Some((note, to)) =
                            try_sapling_compact_note_decryption(&config.get_params(), height, &ivk, co)
                        {
                            wallet_transaction = true;

                            let keys = keys.clone();
                            let bsync_data = bsync_data.clone();
                            let wallet_transactions = wallet_transactions.clone();
                            let detected_transaction_id_sender = detected_transaction_id_sender.clone();
                            let timestamp = cb.time as u64;
                            let compact_transaction = compact_transaction.clone();

                            workers.push(tokio::spawn(async move {
                                let keys = keys.read().await;
                                let extfvk = keys.zkeys[i].extfvk();
                                let have_spending_key = keys.have_spending_key(extfvk);
                                let uri = bsync_data.read().await.uri().clone();

                                // Get the witness for the note
                                let witness = bsync_data
                                    .read()
                                    .await
                                    .block_data
                                    .get_note_witness(uri, height, transaction_num, output_num)
                                    .await?;

                                let transaction_id = WalletTx::new_txid(&compact_transaction.hash);
                                let nullifier = note.nf(&extfvk.fvk.vk, witness.position() as u64);

                                wallet_transactions.write().await.add_new_note(
                                    transaction_id.clone(),
                                    height,
                                    false,
                                    timestamp,
                                    note,
                                    to,
                                    &extfvk,
                                    have_spending_key,
                                    witness,
                                );

                                info!("Trial decrypt Detected txid {}", &transaction_id);

                                detected_transaction_id_sender
                                    .send((transaction_id, nullifier, height, Some(output_num as u32)))
                                    .unwrap();

                                Ok::<_, String>(())
                            }));

                            // No need to try the other ivks if we found one
                            break;
                        }
                    }
                }

                // Check option to see if we are fetching all transactions.
                if !wallet_transaction && download_memos == MemoDownloadOption::AllMemos {
                    let transaction_id = WalletTx::new_txid(&compact_transaction.hash);
                    let (transmitter, receiver) = oneshot::channel();
                    full_transaction_fetcher.send((transaction_id, transmitter)).unwrap();

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
        bsync_data.read().await.sync_status.write().await.trial_dec_done += blk_count as u64;

        // Return a nothing-value
        Ok::<(), String>(())
    }
}
