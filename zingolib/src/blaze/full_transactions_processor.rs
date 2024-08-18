use super::syncdata::BlazeSyncData;
use crate::wallet::transaction_context::TransactionContext;
use futures::{future::join_all, stream::FuturesUnordered, StreamExt};

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::{
    sync::{
        mpsc::{unbounded_channel, UnboundedSender},
        oneshot, RwLock,
    },
    task::JoinHandle,
};

use zcash_primitives::{
    consensus::BlockHeight,
    transaction::{Transaction, TxId},
};

use zingo_status::confirmation_status::ConfirmationStatus;

// despite being called fetch_full_transaction, this function sends a txid somewhere else to be fetched as a Transaction. then processes it.
pub async fn start(
    transaction_context: TransactionContext,
    fulltx_fetcher: UnboundedSender<(TxId, oneshot::Sender<Result<Transaction, String>>)>,
    bsync_data: Arc<RwLock<BlazeSyncData>>,
) -> (
    JoinHandle<Result<(), String>>,
    UnboundedSender<(TxId, BlockHeight)>,
    UnboundedSender<(Transaction, BlockHeight)>,
) {
    let local_transaction_context = transaction_context.clone();

    let start_height = bsync_data
        .read()
        .await
        .block_data
        .sync_status
        .read()
        .await
        .start_block;
    let end_height = bsync_data
        .read()
        .await
        .block_data
        .sync_status
        .read()
        .await
        .end_block;

    let bsync_data_i = bsync_data.clone();

    let (txid_sender, mut transaction_id_receiver) = unbounded_channel::<(TxId, BlockHeight)>();
    let h1: JoinHandle<Result<(), String>> = tokio::spawn(async move {
        let last_progress = Arc::new(AtomicU64::new(0));
        let mut workers = FuturesUnordered::new();

        while let Some((transaction_id, height)) = transaction_id_receiver.recv().await {
            let per_txid_iter_context = local_transaction_context.clone();
            let block_time = bsync_data_i
                .read()
                .await
                .block_data
                .get_block_timestamp(&height)
                .await;
            let fulltx_fetcher_sub = fulltx_fetcher.clone();
            let bsync_data = bsync_data_i.clone();
            let last_progress = last_progress.clone();

            workers.push(tokio::spawn(async move {
                // It is possible that we receive the same txid multiple times, so we keep track of all the txids that were fetched
                let transaction = {
                    // Fetch the TxId from LightwalletD and process all the parts of it.
                    let (transmitter, receiver) = oneshot::channel();
                    fulltx_fetcher_sub
                        .send((transaction_id, transmitter))
                        .unwrap();
                    receiver.await.unwrap()?
                };

                let progress = start_height - u64::from(height);
                if progress > last_progress.load(Ordering::SeqCst) {
                    bsync_data
                        .read()
                        .await
                        .block_data
                        .sync_status
                        .write()
                        .await
                        .txn_scan_done = progress;
                    last_progress.store(progress, Ordering::SeqCst);
                }

                let status = ConfirmationStatus::Confirmed(height);
                per_txid_iter_context
                    .scan_full_tx(&transaction, status, Some(block_time), None)
                    .await;

                Ok::<_, String>(())
            }));
        }

        while let Some(r) = workers.next().await {
            r.map_err(|r| r.to_string())??;
        }

        bsync_data_i
            .read()
            .await
            .block_data
            .sync_status
            .write()
            .await
            .txn_scan_done = start_height - end_height + 1;
        //info!("Finished fetching all full transactions");

        Ok(())
    });

    let transaction_context = transaction_context.clone(); // TODO: Delete and study error.
    let (full_transaction_sender, mut full_transaction_receiver) =
        unbounded_channel::<(Transaction, BlockHeight)>();

    let h2: JoinHandle<Result<(), String>> = tokio::spawn(async move {
        let bsync_data = bsync_data.clone();

        while let Some((transaction, height)) = full_transaction_receiver.recv().await {
            let block_time = bsync_data
                .read()
                .await
                .block_data
                .get_block_timestamp(&height)
                .await;
            let status = ConfirmationStatus::Confirmed(height);
            transaction_context
                .scan_full_tx(&transaction, status, Some(block_time), None)
                .await;
        }

        //info!("Finished full_tx scanning all txns");
        Ok(())
    });

    let h = tokio::spawn(async move {
        join_all(vec![h1, h2])
            .await
            .into_iter()
            .try_for_each(|r| r.map_err(|e| format!("{}", e))?)
    });

    (h, txid_sender, full_transaction_sender)
}
