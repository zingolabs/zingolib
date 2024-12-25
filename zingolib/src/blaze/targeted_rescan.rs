use crate::wallet::{data::BlockData, transaction_context::TransactionContext};
use futures::{stream::FuturesUnordered, StreamExt};

use std::sync::Arc;
use tokio::{
    sync::{mpsc::UnboundedSender, oneshot, RwLock},
    task::JoinHandle,
};

use zcash_primitives::{
    consensus::BlockHeight,
    transaction::{Transaction, TxId},
};

use zingo_status::confirmation_status::ConfirmationStatus;

pub async fn start(
    last_100_blocks: Arc<RwLock<Vec<BlockData>>>,
    transaction_context: TransactionContext,
    fulltx_fetcher: UnboundedSender<(TxId, oneshot::Sender<Result<Transaction, String>>)>,
) -> JoinHandle<Result<(), String>> {
    tokio::spawn(async move {
        let mut workers = FuturesUnordered::new();

        // collect any outdated transaction records that are incomplete and missing output indexes
        let unindexed_records_result =
            if let Some(highest_wallet_block) = last_100_blocks.read().await.first() {
                transaction_context
                    .unindexed_records(BlockHeight::from_u32(highest_wallet_block.height as u32))
                    .await
            } else {
                Ok(())
            };

        // fetch and re-scan incomplete transactions
        if let Err(incomplete_transaction_txids) = unindexed_records_result {
            for (txid, height) in incomplete_transaction_txids {
                let fulltx_fetcher_sub = fulltx_fetcher.clone();
                let transaction_context_sub = transaction_context.clone();
                workers.push(tokio::spawn(async move {
                    let transaction = {
                        // Fetch the TxId from LightwalletD and process all the parts of it.
                        let (transmitter, receiver) = oneshot::channel();
                        fulltx_fetcher_sub.send((txid, transmitter)).unwrap();
                        receiver.await.unwrap()?
                    };

                    let status = ConfirmationStatus::Confirmed(height);
                    transaction_context_sub
                        .scan_full_tx(&transaction, status, None, None)
                        .await;

                    Ok::<_, String>(())
                }));
            }
        }

        while let Some(r) = workers.next().await {
            r.map_err(|r| r.to_string())??;
        }

        Ok(())
    })
}
