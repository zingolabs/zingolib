use crate::wallet::keys::address_from_pubkeyhash;
use crate::wallet::keys::unified::WalletCapability;
use zcash_client_backend::proto::service::RawTransaction;

use std::sync::Arc;
use tokio::join;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;
use tokio::{sync::mpsc::UnboundedSender, task::JoinHandle};

use crate::config::ZingoConfig;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::consensus::BranchId;
use zcash_primitives::consensus::Parameters;
use zcash_primitives::transaction::Transaction;

pub struct FetchTaddrTransactions {
    wc: Arc<WalletCapability>,
    config: Arc<ZingoConfig>,
}

/// A request to fetch taddrs between a start/end height: `(taddrs, start, end)`
pub type FetchTaddrRequest = (Vec<String>, u64, u64);

/// A response for a particular taddr
pub type FetchTaddrResponse = Result<RawTransaction, String>;

impl FetchTaddrTransactions {
    pub fn new(wc: Arc<WalletCapability>, config: Arc<ZingoConfig>) -> Self {
        Self { wc, config }
    }

    pub async fn start(
        &self,
        start_height: u64,
        end_height: u64,
        taddr_fetcher: oneshot::Sender<(
            FetchTaddrRequest,
            oneshot::Sender<Vec<UnboundedReceiver<FetchTaddrResponse>>>,
        )>,
        full_transaction_scanner: UnboundedSender<(Transaction, BlockHeight)>,
        network: impl Parameters + Send + Copy + 'static,
    ) -> JoinHandle<Result<(), String>> {
        let wc = self.wc.clone();
        let config = self.config.clone();
        tokio::spawn(async move {
            let taddrs = wc
                .addresses()
                .iter()
                .filter_map(|ua| ua.transparent())
                .chain(
                    wc.get_rejection_addresses()
                        .iter()
                        .map(|(taddr, _metadata)| taddr),
                )
                .map(|taddr| address_from_pubkeyhash(&config, *taddr))
                .collect::<Vec<_>>();

            // Fetch all transactions for all t-addresses in parallel, and process them in height order
            let req = (taddrs, start_height, end_height);
            let (res_transmitter, res_receiver) =
                oneshot::channel::<Vec<UnboundedReceiver<Result<RawTransaction, String>>>>();
            taddr_fetcher.send((req, res_transmitter)).unwrap();

            let (ordered_raw_transaction_transmitter, mut ordered_raw_transaction_receiver) =
                unbounded_channel();

            // Process every transparent address transaction, in order of height
            let h1: JoinHandle<Result<(), String>> = tokio::spawn(async move {
                // Now, read the transactions one-at-a-time, and then dispatch them in height order
                let mut transactions_top = vec![];

                // Fill the array with the first transaction for every taddress
                let mut transaction_responses = res_receiver.await.unwrap();
                for transaction_response in transaction_responses.iter_mut() {
                    if let Some(Ok(transaction)) = transaction_response.recv().await {
                        transactions_top.push(Some(transaction));
                    } else {
                        transactions_top.push(None);
                    }
                }

                // While at least one of them is still returning transactions
                while transactions_top.iter().any(|t| t.is_some()) {
                    // Find the transaction with the lowest height
                    let (_height, idx) = transactions_top.iter().enumerate().fold(
                        (u64::MAX, 0),
                        |(prev_height, prev_idx), (idx, t)| {
                            if let Some(transaction) = t {
                                if transaction.height < prev_height {
                                    (transaction.height, idx)
                                } else {
                                    (prev_height, prev_idx)
                                }
                            } else {
                                (prev_height, prev_idx)
                            }
                        },
                    );

                    // Grab the transaction at the index
                    let transaction = transactions_top[idx].as_ref().unwrap().clone();

                    // Replace the transaction at the index that was just grabbed
                    if let Some(Ok(transaction)) = transaction_responses[idx].recv().await {
                        transactions_top[idx] = Some(transaction);
                    } else {
                        transactions_top[idx] = None;
                    }

                    // Dispatch the result only if it is in out scan range
                    if transaction.height <= start_height && transaction.height >= end_height {
                        ordered_raw_transaction_transmitter
                            .send(transaction)
                            .unwrap();
                    }
                }

                //info!("Finished fetching all t-addr transactions");

                Ok(())
            });

            let h2: JoinHandle<Result<(), String>> = tokio::spawn(async move {
                let mut prev_height = 0;

                while let Some(raw_transaction) = ordered_raw_transaction_receiver.recv().await {
                    // We should be receiving transactions strictly in height order, so make sure
                    if raw_transaction.height < prev_height {
                        return Err(format!(
                            "Wrong height order while processing transparent transactions!. Was {}, prev={}",
                            raw_transaction.height, prev_height
                        ));
                    }
                    prev_height = raw_transaction.height;

                    let transaction = Transaction::read(
                        &raw_transaction.data[..],
                        BranchId::for_height(
                            &network,
                            BlockHeight::from_u32(raw_transaction.height as u32),
                        ),
                    )
                    .map_err(|e| format!("Error reading transaction: {}", e))?;
                    full_transaction_scanner
                        .send((
                            transaction,
                            BlockHeight::from_u32(raw_transaction.height as u32),
                        ))
                        .unwrap();
                }

                //info!("Finished scanning all t-addr transactions");
                Ok(())
            });

            let (r1, r2) = join!(h1, h2);
            r1.map_err(|e| format!("{}", e))??;
            r2.map_err(|e| format!("{}", e))??;

            Ok(())
        })
    }
}

// TODO: Reexamine this test, which relies on explicit creation of transparent spend auths
/*
#[cfg(test)]
mod tests {
    use futures::future::join_all;
    use rand::Rng;
    use std::sync::Arc;
    use tokio::join;
    use tokio::sync::mpsc::UnboundedReceiver;
    use tokio::task::JoinError;

    use tokio::sync::oneshot;
    use tokio::sync::RwLock;
    use tokio::{sync::mpsc::unbounded_channel, task::JoinHandle};
    use zcash_primitives::consensus::BlockHeight;

    use crate::compact_formats::RawTransaction;
    use zcash_primitives::transaction::Transaction;

    use crate::wallet::keys::transparent::TransparentKey;
    use crate::wallet::keys::UnifiedSpendAuthority;

    use super::FetchTaddrTransactions;

    #[tokio::test]
    async fn out_of_order_transactions() {
        // 5 t addresses
        let mut keys = UnifiedSpendAuthority::new_empty();
        let gened_taddrs: Vec<_> = (0..5).into_iter().map(|n| format!("taddr{}", n)).collect();
        keys.tkeys = gened_taddrs
            .iter()
            .map(|ta| TransparentKey::empty(ta))
            .collect::<Vec<_>>();

        let ftt = FetchTaddrTransactions::new(Arc::new(RwLock::new(keys)));

        let (taddr_fetcher_transmitter, taddr_fetcher_receiver) = oneshot::channel::<(
            (Vec<String>, u64, u64),
            oneshot::Sender<Vec<UnboundedReceiver<Result<RawTransaction, String>>>>,
        )>();

        let h1: JoinHandle<Result<i32, String>> = tokio::spawn(async move {
            let mut transaction_receivers = vec![];
            let mut transaction_receivers_workers: Vec<JoinHandle<i32>> = vec![];

            let ((taddrs, _, _), raw_transaction) = taddr_fetcher_receiver.await.unwrap();
            assert_eq!(taddrs, gened_taddrs);

            // Create a stream for every t-addr
            for _taddr in taddrs {
                let (transaction_transmitter, transaction_receiver) = unbounded_channel();
                transaction_receivers.push(transaction_receiver);
                transaction_receivers_workers.push(tokio::spawn(async move {
                    // Send 100 RawTransactions at a random (but sorted) heights
                    let mut rng = rand::thread_rng();

                    // Generate between 50 and 200 transactions per taddr
                    let num_transactions = rng.gen_range(50..200);

                    let mut raw_transactions = (0..num_transactions)
                        .into_iter()
                        .map(|_| rng.gen_range(1..100))
                        .map(|h| {
                            let mut raw_transaction = RawTransaction::default();
                            raw_transaction.height = h;

                            let mut b = vec![];
                            use crate::lightclient::testmocks;
                            testmocks::new_transactiondata()
                                .freeze()
                                .unwrap()
                                .write(&mut b)
                                .unwrap();
                            raw_transaction.data = b;

                            raw_transaction
                        })
                        .collect::<Vec<_>>();
                    raw_transactions.sort_by_key(|r| r.height);

                    for raw_transaction in raw_transactions {
                        transaction_transmitter.send(Ok(raw_transaction)).unwrap();
                    }

                    num_transactions
                }));
            }

            // Dispatch a set of receivers
            raw_transaction.send(transaction_receivers).unwrap();

            let total = join_all(transaction_receivers_workers)
                .await
                .into_iter()
                .collect::<Result<Vec<i32>, JoinError>>()
                .map_err(|e| format!("{}", e))?
                .iter()
                .sum();

            Ok(total)
        });

        let (full_transaction_scanner_transmitter, mut full_transaction_scanner_receiver) =
            unbounded_channel::<(Transaction, BlockHeight)>();
        let h2: JoinHandle<Result<i32, String>> = tokio::spawn(async move {
            let mut prev_height = BlockHeight::from_u32(0);
            let mut total = 0;
            while let Some((_transaction, h)) = full_transaction_scanner_receiver.recv().await {
                if h < prev_height {
                    return Err(format!(
                        "Wrong height. prev = {}, current = {}",
                        prev_height, h
                    ));
                }
                prev_height = h;
                total += 1;
            }
            Ok(total)
        });

        let h3 = ftt
            .start(
                100,
                1,
                taddr_fetcher_transmitter,
                full_transaction_scanner_transmitter,
                crate::config::Network::FakeMainnet,
            )
            .await;
        //Todo: Add regtest support
        //this test will probably fail until we do

        let (total_sent, total_recieved) = join!(h1, h2);
        assert_eq!(
            total_sent.unwrap().unwrap(),
            total_recieved.unwrap().unwrap()
        );

        h3.await.unwrap().unwrap();
    }
}
*/
