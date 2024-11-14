//! AWISOTT
//! LightClient sync stuff.
//! the difference between this and wallet/sync.rs is that these can interact with the network layer.

use futures::future::join_all;

use log::{debug, error, warn};

use std::{
    cmp::{self},
    io::{self},
    sync::Arc,
    time::Duration,
};
use tokio::{
    join,
    runtime::Runtime,
    sync::{mpsc::unbounded_channel, oneshot},
    task::yield_now,
    time::sleep,
};

use zingo_status::confirmation_status::ConfirmationStatus;

use zcash_client_backend::proto::service::RawTransaction;
use zcash_primitives::{
    consensus::{BlockHeight, BranchId},
    transaction::Transaction,
};

use crate::config::MAX_REORG;

static LOG_INIT: std::sync::Once = std::sync::Once::new();

use super::LightClient;
use super::SyncResult;
use crate::{
    blaze::{
        block_management_reorg_detection::BlockManagementData,
        fetch_compact_blocks::FetchCompactBlocks, fetch_taddr_transactions::FetchTaddrTransactions,
        sync_status::BatchSyncStatus, trial_decryptions::TrialDecryptions,
        update_notes::UpdateNotes,
    },
    grpc_connector::GrpcConnector,
    wallet::{now, transaction_context::TransactionContext, utils::get_price},
};

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
/// likely no errors here. but this makes clippy (and fv) happier
pub enum StartMempoolMonitorError {
    #[error("Mempool Monitor is disabled.")]
    Disabled,
    #[error("could not read mempool monitor: {0}")]
    CouldNotRead(String),
    #[error("could not write mempool monitor: {0}")]
    CouldNotWrite(String),
    #[error("Mempool Monitor does not exist.")]
    DoesNotExist,
}

impl LightClient {
    /// TODO: Add Doc Comment Here!
    pub async fn do_sync(&self, print_updates: bool) -> Result<SyncResult, String> {
        // Remember the previous sync id first
        let prev_sync_id = self
            .bsync_data
            .read()
            .await
            .block_data
            .sync_status
            .read()
            .await
            .sync_id;

        // Start the sync
        let r_fut = self.start_sync();

        // If printing updates, start a new task to print updates every 2 seconds.
        let sync_result = if print_updates {
            let sync_status_clone = self.bsync_data.read().await.block_data.sync_status.clone();
            let (transmitter, mut receiver) = oneshot::channel::<i32>();

            tokio::spawn(async move {
                while sync_status_clone.read().await.sync_id == prev_sync_id {
                    yield_now().await;
                    sleep(Duration::from_secs(3)).await;
                }

                loop {
                    if let Ok(_t) = receiver.try_recv() {
                        break;
                    }

                    println!("{}", sync_status_clone.read().await);

                    yield_now().await;
                    sleep(Duration::from_secs(3)).await;
                }
            });

            let r = r_fut.await;
            transmitter.send(1).unwrap();
            r
        } else {
            r_fut.await
        };

        // Mark the sync data as finished, which should clear everything
        self.bsync_data.read().await.finish().await;
        sync_result
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_sync_status(&self) -> BatchSyncStatus {
        self.bsync_data
            .read()
            .await
            .block_data
            .sync_status
            .read()
            .await
            .clone()
    }

    /// TODO: Add Doc Comment Here!
    pub async fn download_initial_tree_state_from_lightwalletd(
        &self,
        height: u64,
    ) -> Option<(u64, String, String)> {
        if height <= self.config.sapling_activation_height() {
            return None;
        }

        debug!(
            "Getting sapling tree from LightwalletD at height {}",
            height
        );
        match crate::grpc_connector::get_trees(self.config.get_lightwalletd_uri(), height).await {
            Ok(tree_state) => {
                let hash = tree_state.hash.clone();
                let tree = tree_state.sapling_tree.clone();
                Some((tree_state.height, hash, tree))
            }
            Err(e) => {
                error!("Error getting sapling tree:{e}.");
                None
            }
        }
    }

    pub(crate) async fn get_sync_interrupt(&self) -> bool {
        *self.interrupt_sync.read().await
    }

    /// TODO: Add Doc Comment Here!
    pub fn init_logging() -> io::Result<()> {
        // Configure logging first.
        LOG_INIT.call_once(tracing_subscriber::fmt::init);

        Ok(())
    }

    /// TODO: Add Doc Comment Here!
    pub async fn interrupt_sync_after_batch(&self, set_interrupt: bool) {
        *self.interrupt_sync.write().await = set_interrupt;
    }

    /// a concurrent task
    /// the mempool includes transactions waiting to be accepted to the chain
    /// we query it through lightwalletd
    /// and record any new data, using ConfirmationStatus::Mempool
    pub fn start_mempool_monitor(lc: Arc<LightClient>) -> Result<(), StartMempoolMonitorError> {
        if !lc.config.monitor_mempool {
            return Err(StartMempoolMonitorError::Disabled);
        }

        if lc
            .mempool_monitor
            .read()
            .map_err(|e| StartMempoolMonitorError::CouldNotRead(e.to_string()))?
            .is_some()
        {
            return Err(StartMempoolMonitorError::DoesNotExist);
        }

        let config = lc.config.clone();
        let lci = lc.clone();

        debug!("Mempool monitoring starting");

        let uri = lc.config.get_lightwalletd_uri();
        // Start monitoring the mempool in a new thread
        let h = std::thread::spawn(move || {
            // Start a new async runtime, which is fine because we are in a new thread.
            Runtime::new().unwrap().block_on(async move {
                let (mempool_transmitter, mut mempool_receiver) =
                    unbounded_channel::<RawTransaction>();
                let lc1 = lci.clone();

                let h1 = tokio::spawn(async move {
                    let key = lc1.wallet.wallet_capability();
                    let transaction_metadata_set = lc1
                        .wallet
                        .transaction_context
                        .transaction_metadata_set
                        .clone();
                    let price = lc1.wallet.price.clone();

                    while let Some(rtransaction) = mempool_receiver.recv().await {
                        if let Ok(transaction) = Transaction::read(
                            &rtransaction.data[..],
                            BranchId::for_height(
                                &config.chain,
                                BlockHeight::from_u32(rtransaction.height as u32),
                            ),
                        ) {
                            let status = ConfirmationStatus::Mempool(BlockHeight::from_u32(
                                // The mempool transaction's height field is the height
                                // it entered the mempool. Making it one above that height,
                                // i.e. the target height, keeps this value consistant with
                                // the transmitted height, which we record as the target height.
                                rtransaction.height as u32 + 1,
                            ));
                            let tms_readlock = transaction_metadata_set.read().await;
                            let record = tms_readlock
                                .transaction_records_by_id
                                .get(&transaction.txid());
                            match record {
                                None => {
                                    // We only need this for the record, and we can't hold it
                                    // for the later scan_full_tx call, as it needs write access.
                                    drop(tms_readlock);
                                    let price = price.read().await.clone();
                                    //debug!("Mempool attempting to scan {}", tx.txid());

                                    TransactionContext::new(
                                        &config,
                                        key.clone(),
                                        transaction_metadata_set.clone(),
                                    )
                                    .scan_full_tx(
                                        &transaction,
                                        status,
                                        Some(now() as u32),
                                        get_price(now(), &price),
                                    )
                                    .await;
                                    transaction_metadata_set
                                        .write()
                                        .await
                                        .transaction_records_by_id
                                        .update_note_spend_statuses(
                                            transaction.txid(),
                                            Some((transaction.txid(), status)),
                                        );
                                }
                                Some(r) => {
                                    if matches!(r.status, ConfirmationStatus::Transmitted(_)) {
                                        // In this case, we need write access, to change the status
                                        // from Transmitted to Mempool
                                        drop(tms_readlock);
                                        let mut tms_writelock =
                                            transaction_metadata_set.write().await;
                                        tms_writelock
                                            .transaction_records_by_id
                                            .get_mut(&transaction.txid())
                                            .expect("None case has already been handled")
                                            .status = status;
                                        tms_writelock
                                            .transaction_records_by_id
                                            .update_note_spend_statuses(
                                                transaction.txid(),
                                                Some((transaction.txid(), status)),
                                            );
                                        drop(tms_writelock);
                                    }
                                }
                            }
                        }
                        // at this point, all transactions accepted by the mempool have been so recorded: ConfirmationStatus::Mempool
                        // so we rebroadcast all those which are stuck in ConfirmationStatus::Transmitted
                        let _ = lc1.broadcast_created_transactions().await;
                    }
                });

                let h2 = tokio::spawn(async move {
                    loop {
                        //debug!("Monitoring mempool");
                        let r = crate::grpc_connector::monitor_mempool(
                            uri.clone(),
                            mempool_transmitter.clone(),
                        )
                        .await;

                        if r.is_err() {
                            sleep(Duration::from_secs(3)).await;
                        } else {
                            let _ = lci.do_sync(false).await;
                        }
                    }
                });

                let (_, _) = join!(h1, h2);
            });
        });

        *lc.mempool_monitor
            .write()
            .map_err(|e| StartMempoolMonitorError::CouldNotWrite(e.to_string()))? = Some(h);
        Ok(())
    }

    /// Start syncing in batches with the max size, to manage memory consumption.
    async fn start_sync(&self) -> Result<SyncResult, String> {
        // We can only do one sync at a time because we sync blocks in serial order
        // If we allow multiple syncs, they'll all get jumbled up.
        // TODO:  We run on resource constrained systems, where a single thread of
        // execution often consumes most of the memory available, on other systems
        // we might parallelize sync.
        let lightclient_exclusion_lock = self.sync_lock.lock().await;

        // The top of the wallet
        let last_synced_height = self.wallet.last_synced_height().await;

        // If our internal state gets damaged somehow (for example,
        // a resync that gets interrupted partway through) we need to make sure
        // our witness trees are aligned with our blockchain data
        self.wallet
            .ensure_witness_tree_not_above_wallet_blocks()
            .await;

        // This is a fresh wallet. We need to get the initial trees
        if self.wallet.has_any_empty_commitment_trees().await
            && last_synced_height >= self.config.sapling_activation_height()
        {
            let trees =
                crate::grpc_connector::get_trees(self.get_server_uri(), last_synced_height).await?;
            self.wallet.initiate_witness_trees(trees).await;
        };

        let latest_blockid =
            crate::grpc_connector::get_latest_block(self.config.get_lightwalletd_uri()).await?;
        // Block hashes are reversed when stored in BlockDatas, so we reverse here to match
        let latest_blockid =
            crate::wallet::data::BlockData::new_with(latest_blockid.height, &latest_blockid.hash);
        if latest_blockid.height < last_synced_height {
            let w = format!(
                "Server's latest block({}) is behind ours({})",
                latest_blockid.height, last_synced_height
            );
            warn!("{}", w);
            return Err(w);
        }

        if latest_blockid.height == last_synced_height
            && !latest_blockid.hash().is_empty()
            && latest_blockid.hash() != self.wallet.last_synced_hash().await
        {
            log::warn!("One block reorg at height {}", last_synced_height);
            // This is a one-block reorg, so pop the last block. Even if there are more blocks to reorg, this is enough
            // to trigger a sync, which will then reorg the remaining blocks
            BlockManagementData::invalidate_block(
                last_synced_height,
                self.wallet.last_100_blocks.clone(),
                self.wallet
                    .transaction_context
                    .transaction_metadata_set
                    .clone(),
            )
            .await;
        }

        // Re-read the last scanned height
        let last_scanned_height = self.wallet.last_synced_height().await;

        let mut latest_block_batches = vec![];
        let mut prev = last_scanned_height;
        while latest_block_batches.is_empty() || prev != latest_blockid.height {
            let batch = cmp::min(latest_blockid.height, prev + crate::config::BATCH_SIZE);
            prev = batch;
            latest_block_batches.push(batch);
        }

        // Increment the sync ID so the caller can determine when it is over
        self.bsync_data
            .write()
            .await
            .block_data
            .sync_status
            .write()
            .await
            .start_new(latest_block_batches.len());

        let mut res = Err("No batches were run!".to_string());
        for (batch_num, batch_latest_block) in latest_block_batches.into_iter().enumerate() {
            res = self.sync_nth_batch(batch_latest_block, batch_num).await;
            if let Err(ref err) = res {
                // If something went wrong during a batch, reset the wallet state to
                // how it was before the latest batch
                println!("sync hit error {}. Rolling back", err);
                BlockManagementData::invalidate_block(
                    self.wallet.last_synced_height().await,
                    self.wallet.last_100_blocks.clone(),
                    self.wallet
                        .transaction_context
                        .transaction_metadata_set
                        .clone(),
                )
                .await;
            }
            res.as_ref()?;
            if *self.interrupt_sync.read().await {
                log::debug!("LightClient interrupt_sync is true");
                break;
            }
        }

        drop(lightclient_exclusion_lock);
        res
    }

    /// start_sync will start synchronizing the blockchain from the wallet's last height. This function will
    /// return immediately after starting the sync.  Use the `do_sync_status` LightClient method to
    /// get the status of the sync
    async fn sync_nth_batch(
        &self,
        start_block: u64,
        batch_num: usize,
    ) -> Result<SyncResult, String> {
        // The top of the wallet
        let last_synced_height = self.wallet.last_synced_height().await;

        debug!(
            "Latest block is {}, wallet block is {}",
            start_block, last_synced_height
        );

        if last_synced_height == start_block {
            debug!("Already at latest block, not syncing");
            return Ok(SyncResult {
                success: true,
                latest_block: last_synced_height,
                total_blocks_synced: 0,
            });
        }

        let bsync_data = self.bsync_data.clone();

        let end_block = last_synced_height + 1;

        // Before we start, we need to do a few things
        // 1. Pre-populate the last 100 blocks, in case of reorgs
        bsync_data
            .write()
            .await
            .setup_nth_batch(
                start_block,
                end_block,
                batch_num,
                self.wallet.get_blocks().await,
                self.wallet.verified_tree.read().await.clone(),
                *self.wallet.wallet_options.read().await,
            )
            .await;

        // 2. Update the current price:: Who's concern is price?
        //self.update_current_price().await;

        // Sapling Tree GRPC Fetcher
        let grpc_connector = GrpcConnector::new(self.config.get_lightwalletd_uri());

        // A signal to detect reorgs, and if so, ask the block_fetcher to fetch new blocks.
        let (reorg_transmitter, reorg_receiver) = unbounded_channel();

        // Node and Witness Data Cache
        let (block_and_witness_handle, block_and_witness_data_transmitter) = bsync_data
            .read()
            .await
            .block_data
            .handle_reorgs_and_populate_block_mangement_data(
                start_block,
                end_block,
                self.wallet.transactions(),
                reorg_transmitter,
            )
            .await;

        // Full Tx GRPC fetcher
        let (full_transaction_fetcher_handle, full_transaction_fetcher_transmitter) =
            crate::grpc_connector::start_full_transaction_fetcher(
                &grpc_connector,
                self.config.chain,
            )
            .await;
        // Transparent Transactions Fetcher
        let (taddr_fetcher_handle, taddr_fetcher_transmitter) =
            crate::grpc_connector::start_taddr_transaction_fetcher(&grpc_connector).await;

        // Local state necessary for a transaction fetch
        let transaction_context = TransactionContext::new(
            &self.config,
            self.wallet.wallet_capability(),
            self.wallet.transactions(),
        );

        // Fetches full transactions only in the batch currently being processed
        let (fetch_full_transactions_handle, txid_sender, full_transaction_sender) =
            crate::blaze::full_transactions_processor::start(
                transaction_context.clone(),
                full_transaction_fetcher_transmitter.clone(),
                bsync_data.clone(),
            )
            .await;

        // The processor to process Transactions detected by the trial decryptions processor
        let update_notes_processor = UpdateNotes::new(self.wallet.transactions());
        let (update_notes_handle, blocks_done_transmitter, detected_transactions_transmitter) =
            update_notes_processor
                .start(bsync_data.clone(), txid_sender)
                .await;

        // Do Trial decryptions of all the outputs, and pass on the successful ones to the update_notes processor
        let trial_decryptions_processor = TrialDecryptions::new(
            Arc::new(self.config.clone()),
            self.wallet.wallet_capability(),
            self.wallet.transactions(),
        );
        let (trial_decrypts_handle, trial_decrypts_transmitter) = trial_decryptions_processor
            .start(
                bsync_data.clone(),
                detected_transactions_transmitter,
                self.wallet
                    .wallet_options
                    .read()
                    .await
                    .transaction_size_filter,
                full_transaction_fetcher_transmitter.clone(),
            )
            .await;

        // Fetch Compact blocks and send them to nullifier cache, node-and-witness cache and the trial-decryption processor
        let fetch_compact_blocks = Arc::new(FetchCompactBlocks::new(&self.config));
        let fetch_compact_blocks_handle = tokio::spawn(async move {
            fetch_compact_blocks
                .start(
                    [
                        block_and_witness_data_transmitter,
                        trial_decrypts_transmitter,
                    ],
                    start_block,
                    end_block,
                    reorg_receiver,
                )
                .await
        });

        // We wait first for the nodes to be updated. This is where reorgs will be handled, so all the steps done after this phase will
        // assume that the reorgs are done.
        let Some(earliest_block) = block_and_witness_handle.await.unwrap().unwrap() else {
            return Ok(SyncResult {
                success: false,
                latest_block: self.wallet.last_synced_height().await,
                total_blocks_synced: 0,
            });
        };

        // 1. Fetch the transparent txns only after reorgs are done.
        let taddr_transactions_handle = FetchTaddrTransactions::new(
            self.wallet.wallet_capability(),
            Arc::new(self.config.clone()),
        )
        .start(
            start_block,
            earliest_block,
            taddr_fetcher_transmitter,
            full_transaction_sender,
            self.config.chain,
        )
        .await;

        // 2. Notify the notes updater that the blocks are done updating
        blocks_done_transmitter.send(earliest_block).unwrap();

        // 3. Targetted rescan to update transactions with missing information
        let targetted_rescan_handle = crate::blaze::targetted_rescan::start(
            self.wallet.last_100_blocks.clone(),
            transaction_context,
            full_transaction_fetcher_transmitter,
        )
        .await;

        // 4. Verify all the downloaded data
        let block_data = bsync_data.clone();

        // Wait for everything to finish

        // Await all the futures
        let r1 = tokio::spawn(async move {
            join_all(vec![
                trial_decrypts_handle,
                full_transaction_fetcher_handle,
                taddr_fetcher_handle,
            ])
            .await
            .into_iter()
            .try_for_each(|r| r.map_err(|e| format!("{}", e)))
        });

        join_all(vec![
            update_notes_handle,
            taddr_transactions_handle,
            fetch_compact_blocks_handle,
            fetch_full_transactions_handle,
            targetted_rescan_handle,
            r1,
        ])
        .await
        .into_iter()
        .try_for_each(|r| r.map_err(|e| format!("{}", e))?)?;

        let verify_handle =
            tokio::spawn(async move { block_data.read().await.block_data.verify_trees().await });
        let (verified, highest_tree) = verify_handle.await.map_err(|e| e.to_string())?;
        debug!("tree verification {}", verified);
        debug!("highest tree exists: {}", highest_tree.is_some());

        // the following check does not make sense in the context of
        // darkside_tests feature and should be disabled since
        // darksidewalletd will manipulate the chain and ultimately
        // break the static checkpoints.
        #[cfg(not(feature = "darkside_tests"))]
        if !verified {
            return Err("Tree Verification Failed".to_string());
        }

        debug!("Batch: {batch_num} synced, doing post-processing");

        let blaze_sync_data = bsync_data.read().await;
        // Post sync, we have to do a bunch of stuff
        // 1. Get the last 100 blocks and store it into the wallet, needed for future re-orgs
        let blocks = blaze_sync_data
            .block_data
            .drain_existingblocks_into_blocks_with_truncation(MAX_REORG)
            .await;
        self.wallet.set_blocks(blocks).await;

        // 2. If sync was successful, also try to get historical prices
        // self.update_historical_prices().await;
        // zingolabs considers this to be a serious privacy/security leak

        // 3. Mark the sync finished, which will clear the nullifier cache etc...
        blaze_sync_data.finish().await;

        // 5. Remove expired mempool transactions, if any
        self.wallet
            .transactions()
            .write()
            .await
            .transaction_records_by_id
            .clear_expired_mempool(start_block);

        // 6. Set the highest verified tree
        if highest_tree.is_some() {
            *self.wallet.verified_tree.write().await = highest_tree;
        }

        debug!("About to run save after syncing {}th batch!", batch_num);

        // #[cfg(not(any(target_os = "ios", target_os = "android")))]
        self.save_internal_rust().await.unwrap();

        Ok(SyncResult {
            success: true,
            latest_block: start_block,
            total_blocks_synced: start_block - end_block + 1,
        })
    }

    /// TODO: Add Doc Comment Here!
    pub async fn do_rescan(&self) -> Result<SyncResult, String> {
        debug!("Rescan starting");

        self.clear_state().await;

        // Then, do a sync, which will force a full rescan from the initial state
        let response = self.do_sync(true).await;

        if response.is_ok() {
            // self.save_internal_rust().await?;
        }

        debug!("Rescan finished");

        response
    }
}

#[cfg(all(test, feature = "testvectors"))]
pub mod test {
    use crate::{lightclient::LightClient, wallet::disk::testing::examples};

    /// loads a wallet from example data
    /// turns on the internet tube
    /// and syncs to the present blockchain moment
    pub(crate) async fn sync_example_wallet(
        wallet_case: examples::NetworkSeedVersion,
    ) -> LightClient {
        // install default crypto provider (ring)
        if let Err(e) = rustls::crypto::ring::default_provider().install_default() {
            log::error!("Error installing crypto provider: {:?}", e)
        };

        let wallet = wallet_case.load_example_wallet().await;
        let lc = LightClient::create_from_wallet_async(wallet).await.unwrap();
        lc.do_sync(true).await.unwrap();
        println!("{:?}", lc.do_balance().await);
        lc
    }

    /// this is a live sync test. its execution time scales linearly since last updated
    #[ignore = "testnet and mainnet tests should be ignored due to increasingly large execution times"]
    #[tokio::test]
    async fn testnet_sync_mskmgdbhotbpetcjwcspgopp() {
        sync_example_wallet(examples::NetworkSeedVersion::Testnet(
            examples::TestnetSeedVersion::MobileShuffle(examples::MobileShuffleVersion::Ga74fed621),
        ))
        .await;
    }
    /// this is a live sync test. its execution time scales linearly since last updated
    #[ignore = "testnet and mainnet tests should be ignored due to increasingly large execution times"]
    #[tokio::test]
    async fn testnet_sync_cbbhrwiilgbrababsshsmtpr() {
        sync_example_wallet(examples::NetworkSeedVersion::Testnet(
            examples::TestnetSeedVersion::ChimneyBetter(examples::ChimneyBetterVersion::G2f3830058),
        ))
        .await;
    }
    /// this is a live sync test. its execution time scales linearly since last updated
    #[tokio::test]
    #[ignore = "testnet and mainnet tests should be ignored due to increasingly large execution times"]
    async fn mainnet_sync() {
        sync_example_wallet(examples::NetworkSeedVersion::Mainnet(
            examples::MainnetSeedVersion::HotelHumor(examples::HotelHumorVersion::Gf0aaf9347),
        ))
        .await;
    }
}
