use crate::wallet::traits::{DomainWalletExt, ReceivedNoteAndMetadata};
use crate::wallet::MemoDownloadOption;
use crate::wallet::{
    data::{PoolNullifier, TransactionMetadata},
    transactions::TransactionMetadataSet,
};
use std::sync::Arc;

use futures::stream::FuturesUnordered;
use futures::StreamExt;
use orchard::note_encryption::OrchardDomain;
use tokio::join;
use tokio::sync::oneshot;
use tokio::sync::{mpsc::unbounded_channel, RwLock};
use tokio::{sync::mpsc::UnboundedSender, task::JoinHandle};

use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::sapling::note_encryption::SaplingDomain;
use zcash_primitives::transaction::TxId;

use super::syncdata::BlazeSyncData;

/// A processor to update notes that we have received in the wallet.
/// We need to identify if this note has been spent in future blocks.
/// If YES, then:
///    - Mark this note as spent
///    - In the future Tx, add the nullifiers as spent and do the accounting
///    - In the future Tx, mark incoming notes as change
///
/// If No, then:
///    - Update the witness for this note
pub struct UpdateNotes {
    transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
}

impl UpdateNotes {
    pub fn new(wallet_txns: Arc<RwLock<TransactionMetadataSet>>) -> Self {
        Self {
            transaction_metadata_set: wallet_txns,
        }
    }

    pub async fn start(
        &self,
        bsync_data: Arc<RwLock<BlazeSyncData>>,
        fetch_full_sender: UnboundedSender<(TxId, BlockHeight)>,
        use_witnesses: bool,
    ) -> (
        JoinHandle<Result<(), String>>,
        oneshot::Sender<u64>,
        UnboundedSender<(TxId, PoolNullifier, BlockHeight, Option<u32>)>,
    ) {
        //info!("Starting Note Update processing");
        let download_memos = bsync_data.read().await.wallet_options.download_memos;

        // Create a new channel where we'll be notified of TxIds that are to be processed
        let (transmitter, mut receiver) =
            unbounded_channel::<(TxId, PoolNullifier, BlockHeight, Option<u32>)>();

        // Aside from the incoming Txns, we also need to update the notes that are currently in the wallet
        let wallet_transactions = self.transaction_metadata_set.clone();
        let transmitter_existing = transmitter.clone();

        let (blocks_done_transmitter, blocks_done_receiver) = oneshot::channel::<u64>();

        let h0: JoinHandle<Result<(), String>> = tokio::spawn(async move {
            // First, wait for notification that the blocks are done loading, and get the earliest block from there.
            let earliest_block = blocks_done_receiver
                .await
                .map_err(|e| format!("Error getting notification that blocks are done. {}", e))?;

            // Get all notes from the wallet that are already existing, i.e., the ones that are before the earliest block that the block loader loaded
            let notes = wallet_transactions
                .read()
                .await
                .get_notes_for_updating(earliest_block - 1);
            for (transaction_id, nf) in notes {
                transmitter_existing
                    .send((
                        transaction_id,
                        nf,
                        BlockHeight::from(earliest_block as u32),
                        None,
                    ))
                    .map_err(|e| format!("Error sending note for updating: {}", e))?;
            }

            //info!("Finished processing all existing notes in wallet");
            Ok(())
        });

        let wallet_transactions = self.transaction_metadata_set.clone();
        let h1 = tokio::spawn(async move {
            let mut workers = FuturesUnordered::new();

            // Receive Txns that are sent to the wallet. We need to update the notes for this.
            while let Some((transaction_id, nf, at_height, output_num)) = receiver.recv().await {
                let bsync_data = bsync_data.clone();
                let wallet_transactions = wallet_transactions.clone();
                let fetch_full_sender = fetch_full_sender.clone();

                workers.push(tokio::spawn(async move {
                    // If this nullifier was spent at a future height, fetch the TxId at the height and process it
                    if let Some(spent_height) = bsync_data
                        .read()
                        .await
                        .block_data
                        .is_nf_spent(nf, at_height.into())
                        .await
                    {
                        //info!("Note was spent, just add it as spent for TxId {}", txid);
                        let (compact_transaction, ts) = bsync_data
                            .read()
                            .await
                            .block_data
                            .get_compact_transaction_for_nullifier_at_height(&nf, spent_height)
                            .await;

                        let spent_transaction_id =
                            TransactionMetadata::new_txid(&compact_transaction.hash);
                        let spent_at_height = BlockHeight::from_u32(spent_height as u32);

                        // Mark this note as being spent
                        let value = wallet_transactions.write().await.mark_txid_nf_spent(
                            transaction_id,
                            &nf,
                            &spent_transaction_id,
                            spent_at_height,
                        );

                        // Record the future transaction, the one that has spent the nullifiers received in this transaction in the wallet
                        wallet_transactions
                            .write()
                            .await
                            .add_new_spent(
                                spent_transaction_id,
                                spent_at_height,
                                false,
                                ts,
                                nf,
                                value,
                                transaction_id,
                            )
                            .await;

                        // Send the future transaction to be fetched too, in case it has only spent nullifiers and not received any change
                        if download_memos != MemoDownloadOption::NoMemos {
                            fetch_full_sender
                                .send((spent_transaction_id, spent_at_height))
                                .unwrap();
                        }
                    } else {
                        //info!("Note was NOT spent, update its witnesses for TxId {}", txid);
                        // not for viewkey

                        // If this note's nullifier was not spent, then we need to update the witnesses for this.
                        //TODO: We no longer update witnesses individually
                    }

                    // Send it off to get the full transaction if this is a new transaction, that is, it has an output_num
                    if output_num.is_some() && download_memos != MemoDownloadOption::NoMemos {
                        fetch_full_sender.send((transaction_id, at_height)).unwrap();
                    }
                }));
            }

            // Wait for all the workers
            while let Some(r) = workers.next().await {
                r.unwrap();
            }

            //info!("Finished Note Update processing");
        });

        let h = tokio::spawn(async move {
            let (r0, r1) = join!(h0, h1);
            r0.map_err(|e| format!("{}", e))??;
            r1.map_err(|e| format!("{}", e))
        });

        (h, blocks_done_transmitter, transmitter)
    }
}
