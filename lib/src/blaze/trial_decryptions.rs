use crate::{
    compact_formats::CompactBlock,
    lightwallet::{data::WalletTx, keys::Keys, wallet_txns::WalletTxns},
};
use futures::future;
use log::info;
use std::sync::Arc;
use tokio::{
    sync::{
        mpsc::{unbounded_channel, UnboundedSender},
        RwLock,
    },
    task::JoinHandle,
};

use zcash_primitives::{
    consensus::{BlockHeight, MAIN_NETWORK},
    note_encryption::try_sapling_compact_note_decryption,
    primitives::{Nullifier, SaplingIvk},
    transaction::TxId,
};

use super::syncdata::BlazeSyncData;

pub struct TrialDecryptions {
    keys: Arc<RwLock<Keys>>,
    wallet_txns: Arc<RwLock<WalletTxns>>,
}

impl TrialDecryptions {
    pub fn new(keys: Arc<RwLock<Keys>>, wallet_txns: Arc<RwLock<WalletTxns>>) -> Self {
        Self { keys, wallet_txns }
    }

    pub async fn start(
        &self,
        bsync_data: Arc<RwLock<BlazeSyncData>>,
        detected_txid_sender: UnboundedSender<(TxId, Nullifier, BlockHeight, Option<u32>)>,
    ) -> (JoinHandle<()>, UnboundedSender<CompactBlock>) {
        //info!("Starting trial decrptions processor");

        // Create a new channel where we'll receive the blocks
        let (tx, mut rx) = unbounded_channel::<CompactBlock>();

        let keys = self.keys.clone();
        let wallet_txns = self.wallet_txns.clone();

        let h = tokio::spawn(async move {
            let mut workers = vec![];
            let mut cbs = vec![];

            let ivks = Arc::new(
                keys.read()
                    .await
                    .zkeys
                    .iter()
                    .map(|zk| zk.extfvk().fvk.vk.ivk())
                    .collect::<Vec<_>>(),
            );

            let sync_status = bsync_data.read().await.sync_status.clone();

            while let Some(cb) = rx.recv().await {
                cbs.push(cb);

                if cbs.len() >= 1_000 {
                    let keys = keys.clone();
                    let ivks = ivks.clone();
                    let wallet_txns = wallet_txns.clone();
                    let bsync_data = bsync_data.clone();
                    let detected_txid_sender = detected_txid_sender.clone();

                    sync_status.write().await.trial_dec_done += cbs.len() as u64;
                    workers.push(tokio::spawn(Self::trial_decrypt_batch(
                        cbs.split_off(0),
                        keys,
                        bsync_data,
                        ivks,
                        wallet_txns,
                        detected_txid_sender,
                    )));
                }
            }

            sync_status.write().await.trial_dec_done += cbs.len() as u64;
            workers.push(tokio::spawn(Self::trial_decrypt_batch(
                cbs,
                keys,
                bsync_data,
                ivks,
                wallet_txns,
                detected_txid_sender,
            )));

            for r in future::join_all(workers).await {
                r.unwrap().unwrap();
            }

            //info!("Finished final trial decryptions");
        });

        return (h, tx);
    }

    async fn trial_decrypt_batch(
        cbs: Vec<CompactBlock>,
        keys: Arc<RwLock<Keys>>,
        bsync_data: Arc<RwLock<BlazeSyncData>>,
        ivks: Arc<Vec<SaplingIvk>>,
        wallet_txns: Arc<RwLock<WalletTxns>>,
        detected_txid_sender: UnboundedSender<(TxId, Nullifier, BlockHeight, Option<u32>)>,
    ) -> Result<(), String> {
        for cb in cbs {
            let height = BlockHeight::from_u32(cb.height as u32);

            for (tx_num, ctx) in cb.vtx.iter().enumerate() {
                for (output_num, co) in ctx.outputs.iter().enumerate() {
                    let cmu = co.cmu().map_err(|_| "No CMU".to_string())?;
                    let epk = match co.epk() {
                        Err(_) => continue,
                        Ok(epk) => epk,
                    };

                    for (i, ivk) in ivks.iter().enumerate() {
                        if let Some((note, to)) =
                            try_sapling_compact_note_decryption(&MAIN_NETWORK, height, &ivk, &epk, &cmu, &co.ciphertext)
                        {
                            let keys = keys.read().await;
                            let extfvk = keys.zkeys[i].extfvk();
                            let have_spending_key = keys.have_spending_key(extfvk);
                            let uri = bsync_data.read().await.uri().clone();

                            // Get the witness for the note
                            let witness = bsync_data
                                .read()
                                .await
                                .block_data
                                .get_note_witness(uri, height, tx_num, output_num)
                                .await?;

                            let txid = WalletTx::new_txid(&ctx.hash);
                            let nullifier = note.nf(&extfvk.fvk.vk, witness.position() as u64);

                            wallet_txns.write().await.add_new_note(
                                txid.clone(),
                                height,
                                false,
                                cb.time as u64,
                                note,
                                to,
                                &extfvk,
                                have_spending_key,
                                witness,
                            );

                            info!("Trial decrypt Detected txid {}", &txid);

                            detected_txid_sender
                                .send((txid, nullifier, height, Some(output_num as u32)))
                                .unwrap();

                            // No need to try the other ivks if we found one
                            break;
                        }
                    }
                }
            }
        }

        // Return a nothing-value
        Ok::<(), String>(())
    }
}
