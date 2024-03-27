use crate::{
    blaze::{
        block_management_reorg_detection::BlockManagementData,
        fetch_compact_blocks::FetchCompactBlocks, fetch_taddr_transactions::FetchTaddrTransactions,
        sync_status::BatchSyncStatus, syncdata::BlazeSyncData, trial_decryptions::TrialDecryptions,
        update_notes::UpdateNotes,
    },
    error::{ZingoLibError, ZingoLibResult},
    grpc_connector::GrpcConnector,
    wallet::{
        data::{
            finsight, summaries::ValueTransfer, summaries::ValueTransferKind, OutgoingTxData,
            TransactionRecord,
        },
        keys::{address_from_pubkeyhash, unified::ReceiverSelection},
        message::Message,
        notes::NoteInterface,
        notes::ShieldedNoteInterface,
        now,
        send::build_transaction_request_from_receivers,
        transaction_context::TransactionContext,
        utils::get_price,
        LightWallet, Pool, SendProgress, WalletBase,
    },
};
use futures::future::join_all;
use json::{array, object, JsonValue};
use log::{debug, error, warn};
use serde::Serialize;
use std::{
    cmp::{self},
    collections::HashMap,
    fs::{remove_file, File},
    io::{self, BufReader, Error, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    join,
    runtime::Runtime,
    sync::{mpsc::unbounded_channel, oneshot, Mutex, RwLock},
    task::yield_now,
    time::sleep,
};
use zcash_address::ZcashAddress;
use zingo_status::confirmation_status::ConfirmationStatus;

use zcash_client_backend::{
    encoding::{decode_payment_address, encode_payment_address},
    proto::service::RawTransaction,
};
use zcash_primitives::{
    consensus::{BlockHeight, BranchId, NetworkConstants},
    memo::{Memo, MemoBytes},
    transaction::{
        components::amount::NonNegativeAmount, fees::zip317::MINIMUM_FEE, Transaction, TxId,
    },
};
use zcash_proofs::prover::LocalTxProver;
use zingoconfig::{ZingoConfig, MAX_REORG};

static LOG_INIT: std::sync::Once = std::sync::Once::new();

const MARGINAL_FEE: u64 = 5_000; // From ZIP-317

#[derive(Clone, Debug, Default)]
pub struct SyncResult {
    pub success: bool,
    pub latest_block: u64,
    pub total_blocks_synced: u64,
}

impl SyncResult {
    /// Converts this object to a JSON object that meets the contract expected by Zingo Mobile.
    pub fn to_json(&self) -> JsonValue {
        object! {
            "result" => if self.success { "success" } else { "failure" },
            "latest_block" => self.latest_block,
            "total_blocks_synced" => self.total_blocks_synced,
        }
    }
}

impl std::fmt::Display for SyncResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            format!(
                "{{ success: {}, latest_block: {}, total_blocks_synced: {}}}",
                self.success, self.latest_block, self.total_blocks_synced
            )
            .as_str(),
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct WalletStatus {
    pub is_syncing: bool,
    pub total_blocks: u64,
    pub synced_blocks: u64,
}

impl WalletStatus {
    pub fn new() -> Self {
        WalletStatus {
            is_syncing: false,
            total_blocks: 0,
            synced_blocks: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LightWalletSendProgress {
    pub progress: SendProgress,
    pub interrupt_sync: bool,
}

impl LightWalletSendProgress {
    pub fn to_json(&self) -> JsonValue {
        object! {
            "id" => self.progress.id,
            "sending" => self.progress.is_send_in_progress,
            "progress" => self.progress.progress,
            "total" => self.progress.total,
            "txid" => self.progress.last_transaction_id.clone(),
            "error" => self.progress.last_error.clone(),
            "sync_interrupt" => self.interrupt_sync
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AccountBackupInfo {
    #[serde(rename = "seed")]
    pub seed_phrase: String,
    pub birthday: u64,
    pub account_index: u32,
}

/// The LightClient provides a unified interface to the separate concerns that the zingolib library manages.
///  1. initialization of stored state
///      * from seed
///      * from keys
///      * from wallets
///      * from a fresh start with reasonable defaults
///  2. synchronization of the client with the state of the blockchain via a gRPC server
///      *
pub struct LightClient {
    pub(crate) config: ZingoConfig,
    pub wallet: LightWallet,

    mempool_monitor: std::sync::RwLock<Option<std::thread::JoinHandle<()>>>,

    sync_lock: Mutex<()>,

    bsync_data: Arc<RwLock<BlazeSyncData>>,
    interrupt_sync: Arc<RwLock<bool>>,

    save_buffer: ZingoSaveBuffer,
}

pub mod describe;
pub mod disk;
pub mod send;

use disk::ZingoSaveBuffer;

///  This is the omnibus interface to the library, we are currently in the process of refining this types
///  overly broad and vague definition!
impl LightClient {
    pub async fn clear_state(&self) {
        // First, clear the state from the wallet
        self.wallet.clear_all().await;

        // Then set the initial block
        let birthday = self.wallet.get_birthday().await;
        self.set_wallet_initial_state(birthday).await;
        debug!("Cleared wallet state, with birthday at {}", birthday);
    }
    pub fn config(&self) -> &ZingoConfig {
        &self.config
    }
    pub async fn do_decrypt_message(&self, enc_base64: String) -> JsonValue {
        let data = match base64::decode(enc_base64) {
            Ok(v) => v,
            Err(e) => {
                return object! {"error" => format!("Couldn't decode base64. Error was {}", e)}
            }
        };

        match self.wallet.decrypt_message(data).await {
            Ok(m) => {
                let memo_bytes: MemoBytes = m.memo.clone().into();
                object! {
                    "to" => encode_payment_address(self.config.chain.hrp_sapling_payment_address(), &m.to),
                    "memo" => LightWallet::memo_str(Some(m.memo)),
                    "memohex" => hex::encode(memo_bytes.as_slice())
                }
            }
            Err(_) => object! { "error" => "Couldn't decrypt with any of the wallet's keys"},
        }
    }

    pub fn do_encrypt_message(&self, to_address_str: String, memo: Memo) -> JsonValue {
        let to = match decode_payment_address(
            self.config.chain.hrp_sapling_payment_address(),
            &to_address_str,
        ) {
            Ok(to) => to,
            _ => {
                return object! {"error" => format!("Couldn't parse {} as a z-address", to_address_str) };
            }
        };

        match Message::new(to, memo).encrypt() {
            Ok(v) => {
                object! {"encrypted_base64" => base64::encode(v) }
            }
            Err(e) => {
                object! {"error" => format!("Couldn't encrypt. Error was {}", e)}
            }
        }
    }

    /// Create a new address, deriving it from the seed.
    pub async fn do_new_address(&self, addr_type: &str) -> Result<JsonValue, String> {
        //TODO: Placeholder interface
        let desired_receivers = ReceiverSelection {
            sapling: addr_type.contains('z'),
            orchard: addr_type.contains('o'),
            transparent: addr_type.contains('t'),
        };

        let new_address = self
            .wallet
            .wallet_capability()
            .new_address(desired_receivers)?;

        // self.save_internal_rust().await?;

        Ok(array![new_address.encode(&self.config.chain)])
    }
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

    fn map_tos_to_receivers(
        &self,
        tos: Vec<(&str, u64, Option<MemoBytes>)>,
    ) -> Result<
        Vec<(
            zcash_client_backend::address::Address,
            NonNegativeAmount,
            Option<MemoBytes>,
        )>,
        String,
    > {
        if tos.is_empty() {
            return Err("Need at least one destination address".to_string());
        }
        tos.iter()
            .map(|to| {
                let ra = match zcash_client_backend::address::Address::decode(
                    &self.config.chain,
                    to.0,
                ) {
                    Some(to) => to,
                    None => {
                        let e = format!("Invalid recipient address: '{}'", to.0);
                        error!("{}", e);
                        return Err(e);
                    }
                };

                let value = NonNegativeAmount::from_u64(to.1).unwrap();

                Ok((ra, value, to.2.clone()))
            })
            .collect()
    }

    pub async fn do_propose(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<MemoBytes>)>,
    ) -> Result<String, String> {
        let receivers = self.map_tos_to_receivers(address_amount_memo_tuples)?;
        let request = build_transaction_request_from_receivers(receivers)
            .map_err(|e| ZingoLibError::RequestConstruction(e).to_string())?;

        let result = {
            // review! is this necessary?
            let _lock = self.sync_lock.lock().await;
            let (sapling_output, sapling_spend) = self.read_sapling_params()?;

            let sapling_prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);

            self.wallet
                .propose_to_addresses(sapling_prover, request)
                .await
        };

        // result.map_err(|e| e.to_string())
        Ok("todo".to_string())
    }

    pub async fn do_send(&self) -> Result<String, String> {
        let transaction_submission_height = self.get_submission_height().await?;

        let result = {
            self.wallet
                .send_to_addresses(transaction_submission_height, |transaction_bytes| {
                    crate::grpc_connector::send_transaction(
                        self.get_server_uri(),
                        transaction_bytes,
                    )
                })
                .await
        };

        // result.map_err(|e| e.to_string())
        Ok("todo".to_string())
    }

    pub async fn do_send_progress(&self) -> Result<LightWalletSendProgress, String> {
        let progress = self.wallet.get_send_progress().await;
        Ok(LightWalletSendProgress {
            progress: progress.clone(),
            interrupt_sync: *self.interrupt_sync.read().await,
        })
    }

    pub async fn do_shield(
        &self,
        pools_to_shield: &[Pool],
        address: Option<String>,
    ) -> Result<String, String> {
        //     let transaction_submission_height = self.get_submission_height().await?;
        //     let fee = u64::from(MINIMUM_FEE); // TODO: This can no longer be hard coded, and must be calced
        //                                       // as a fn of the transactions structure.
        //     let tbal = self
        //         .wallet
        //         .tbalance(None)
        //         .await
        //         .expect("to receive a balance");
        //     let sapling_bal = self
        //         .wallet
        //         .spendable_sapling_balance(None)
        //         .await
        //         .unwrap_or(0);

        //     // Make sure there is a balance, and it is greater than the amount
        //     let balance_to_shield = if pools_to_shield.contains(&Pool::Transparent) {
        //         tbal
        //     } else {
        //         0
        //     } + if pools_to_shield.contains(&Pool::Sapling) {
        //         sapling_bal
        //     } else {
        //         0
        //     };
        //     if balance_to_shield <= fee {
        //         return Err(format!(
        //             "Not enough transparent/sapling balance to shield. Have {} zats, need more than {} zats to cover tx fee",
        //             balance_to_shield, fee
        //         ));
        //     }

        //     let addr = address
        //         .unwrap_or(self.wallet.wallet_capability().addresses()[0].encode(&self.config.chain));

        //     let receiver = self.map_tos_to_receivers(vec![(&addr, balance_to_shield - fee, None)])?;
        //     let request = build_transaction_request_from_receivers(receiver)
        //         .map_err(|e| ZingoLibError::RequestConstruction(e).to_string())?;
        //     let result = {
        //         let _lock = self.sync_lock.lock().await;
        //         let (sapling_output, sapling_spend) = self.read_sapling_params()?;

        //         let sapling_prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);

        //         self.wallet
        //             .send_to_addresses(
        //                 sapling_prover,
        //                 pools_to_shield.to_vec(),
        //                 request,
        //                 transaction_submission_height,
        //                 |transaction_bytes| {
        //                     crate::grpc_connector::send_transaction(
        //                         self.get_server_uri(),
        //                         transaction_bytes,
        //                     )
        //                 },
        //             )
        //             .await
        //     };

        //     result.map_err(|e| e.to_string())
        Ok("todo".to_string())
    }

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

    pub async fn do_total_memobytes_to_address(&self) -> finsight::TotalMemoBytesToAddress {
        let summaries = self.do_list_txsummaries().await;
        let mut memobytes_by_address = HashMap::new();
        for summary in summaries {
            use ValueTransferKind::*;
            match summary.kind {
                Sent { to_address, .. } => {
                    let address = to_address.encode();
                    let bytes = summary.memos.iter().fold(0, |sum, m| sum + m.len());
                    memobytes_by_address
                        .entry(address)
                        .and_modify(|e| *e += bytes)
                        .or_insert(bytes);
                }
                SendToSelf { .. } | Received { .. } | Fee { .. } => (),
            }
        }
        finsight::TotalMemoBytesToAddress(memobytes_by_address)
    }

    pub async fn do_total_spends_to_address(&self) -> finsight::TotalSendsToAddress {
        let values_sent_to_addresses = self.value_transfer_by_to_address().await;
        let mut by_address_number_sends = HashMap::new();
        for key in values_sent_to_addresses.0.keys() {
            let number_sends = values_sent_to_addresses.0[key].len() as u64;
            by_address_number_sends.insert(key.clone(), number_sends);
        }
        finsight::TotalSendsToAddress(by_address_number_sends)
    }

    pub async fn do_total_value_to_address(&self) -> finsight::TotalValueToAddress {
        let values_sent_to_addresses = self.value_transfer_by_to_address().await;
        let mut by_address_total = HashMap::new();
        for key in values_sent_to_addresses.0.keys() {
            let sum = values_sent_to_addresses.0[key].iter().sum();
            by_address_total.insert(key.clone(), sum);
        }
        finsight::TotalValueToAddress(by_address_total)
    }

    pub async fn do_wallet_last_scanned_height(&self) -> JsonValue {
        json::JsonValue::from(self.wallet.last_synced_height().await)
    }

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

    pub fn get_server(&self) -> std::sync::RwLockReadGuard<http::Uri> {
        self.config.lightwalletd_uri.read().unwrap()
    }

    pub fn get_server_uri(&self) -> http::Uri {
        self.config.get_lightwalletd_uri()
    }

    async fn get_submission_height(&self) -> Result<BlockHeight, String> {
        Ok(BlockHeight::from_u32(
            crate::grpc_connector::get_latest_block(self.config.get_lightwalletd_uri())
                .await?
                .height as u32,
        ) + 1)
    }

    pub(crate) async fn get_sync_interrupt(&self) -> bool {
        *self.interrupt_sync.read().await
    }

    pub fn init_logging() -> io::Result<()> {
        // Configure logging first.
        LOG_INIT.call_once(tracing_subscriber::fmt::init);

        Ok(())
    }

    pub async fn interrupt_sync_after_batch(&self, set_interrupt: bool) {
        *self.interrupt_sync.write().await = set_interrupt;
    }
    #[cfg(feature = "embed_params")]
    fn read_sapling_params(&self) -> Result<(Vec<u8>, Vec<u8>), String> {
        // Read Sapling Params
        use crate::SaplingParams;
        let mut sapling_output = vec![];
        sapling_output.extend_from_slice(
            SaplingParams::get("sapling-output.params")
                .unwrap()
                .data
                .as_ref(),
        );

        let mut sapling_spend = vec![];
        sapling_spend.extend_from_slice(
            SaplingParams::get("sapling-spend.params")
                .unwrap()
                .data
                .as_ref(),
        );

        Ok((sapling_output, sapling_spend))
    }
    #[cfg(not(feature = "embed_params"))]
    fn read_sapling_params(&self) -> Result<(vec<u8>, vec<u8>), String> {
        let path = self
            .config
            .get_zcash_params_path()
            .map_err(|e| e.to_string())?;

        let mut path_buf = path.to_path_buf();
        path_buf.push("sapling-output.params");
        let mut file = File::open(path_buf).map_err(|e| e.to_string())?;
        let mut sapling_output = vec![];
        file.read_to_end(&mut sapling_output)
            .map_err(|e| e.to_string())?;

        let mut path_buf = path.to_path_buf();
        path_buf.push("sapling-spend.params");
        let mut file = File::open(path_buf).map_err(|e| e.to_string())?;
        let mut sapling_spend = vec![];
        file.read_to_end(&mut sapling_spend)
            .map_err(|e| e.to_string())?;

        Ok((sapling_output, sapling_spend))
    }

    pub fn set_sapling_params(
        &mut self,
        sapling_output: &[u8],
        sapling_spend: &[u8],
    ) -> Result<(), String> {
        use sha2::{Digest, Sha256};

        // The hashes of the params need to match
        const SAPLING_OUTPUT_HASH: &str =
            "2f0ebbcbb9bb0bcffe95a397e7eba89c29eb4dde6191c339db88570e3f3fb0e4";
        const SAPLING_SPEND_HASH: &str =
            "8e48ffd23abb3a5fd9c5589204f32d9c31285a04b78096ba40a79b75677efc13";

        if !sapling_output.is_empty()
            && *SAPLING_OUTPUT_HASH != hex::encode(Sha256::digest(sapling_output))
        {
            return Err(format!(
                "sapling-output hash didn't match. expected {}, found {}",
                SAPLING_OUTPUT_HASH,
                hex::encode(Sha256::digest(sapling_output))
            ));
        }

        if !sapling_spend.is_empty()
            && *SAPLING_SPEND_HASH != hex::encode(Sha256::digest(sapling_spend))
        {
            return Err(format!(
                "sapling-spend hash didn't match. expected {}, found {}",
                SAPLING_SPEND_HASH,
                hex::encode(Sha256::digest(sapling_spend))
            ));
        }

        // Ensure that the sapling params are stored on disk properly as well. Only on desktop
        match self.config.get_zcash_params_path() {
            Ok(zcash_params_dir) => {
                // Create the sapling output and spend params files
                match LightClient::write_file_if_not_exists(
                    &zcash_params_dir,
                    "sapling-output.params",
                    sapling_output,
                ) {
                    Ok(_) => {}
                    Err(e) => {
                        return Err(format!("Warning: Couldn't write the output params!\n{}", e))
                    }
                };

                match LightClient::write_file_if_not_exists(
                    &zcash_params_dir,
                    "sapling-spend.params",
                    sapling_spend,
                ) {
                    Ok(_) => {}
                    Err(e) => {
                        return Err(format!("Warning: Couldn't write the spend params!\n{}", e))
                    }
                }
            }
            Err(e) => {
                return Err(format!("{}", e));
            }
        };

        Ok(())
    }

    pub fn set_server(&self, server: http::Uri) {
        *self.config.lightwalletd_uri.write().unwrap() = server
    }

    pub async fn set_wallet_initial_state(&self, height: u64) {
        let state = self
            .download_initial_tree_state_from_lightwalletd(height)
            .await;

        if let Some((height, hash, tree)) = state {
            debug!("Setting initial state to height {}, tree {}", height, tree);
            self.wallet
                .set_initial_block(height, hash.as_str(), tree.as_str())
                .await;
        }
    }

    pub fn start_mempool_monitor(lc: Arc<LightClient>) {
        if !lc.config.monitor_mempool {
            return;
        }

        if lc.mempool_monitor.read().unwrap().is_some() {
            return;
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
                            let price = price.read().await.clone();
                            //debug!("Mempool attempting to scan {}", tx.txid());
                            let status = ConfirmationStatus::Broadcast(BlockHeight::from_u32(
                                rtransaction.height as u32,
                            ));

                            TransactionContext::new(
                                &config,
                                key.clone(),
                                transaction_metadata_set.clone(),
                            )
                            .scan_full_tx(
                                &transaction,
                                status,
                                now() as u32,
                                get_price(now(), &price),
                            )
                            .await;
                        }
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

        *lc.mempool_monitor.write().unwrap() = Some(h);
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
                self.wallet.blocks.clone(),
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
            let batch = cmp::min(latest_blockid.height, prev + zingoconfig::BATCH_SIZE);
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
                    self.wallet.blocks.clone(),
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
        let (
            fetch_full_transactions_handle,
            fetch_full_transaction_transmitter,
            fetch_taddr_transactions_transmitter,
        ) = crate::blaze::fetch_full_transaction::start(
            transaction_context,
            full_transaction_fetcher_transmitter.clone(),
            bsync_data.clone(),
        )
        .await;

        // The processor to process Transactions detected by the trial decryptions processor
        let update_notes_processor = UpdateNotes::new(self.wallet.transactions());
        let (update_notes_handle, blocks_done_transmitter, detected_transactions_transmitter) =
            update_notes_processor
                .start(bsync_data.clone(), fetch_full_transaction_transmitter)
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
                full_transaction_fetcher_transmitter,
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
            fetch_taddr_transactions_transmitter,
            self.config.chain,
        )
        .await;

        // 2. Notify the notes updater that the blocks are done updating
        blocks_done_transmitter.send(earliest_block).unwrap();

        // 3. Verify all the downloaded data
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

    fn tx_summary_matcher(
        summaries: &mut Vec<ValueTransfer>,
        txid: TxId,
        transaction_md: &TransactionRecord,
    ) {
        let (block_height, datetime, price, unconfirmed) = (
            transaction_md.status.get_height(),
            transaction_md.datetime,
            transaction_md.price,
            !transaction_md.status.is_confirmed(),
        );
        match (
            transaction_md.is_outgoing_transaction(),
            transaction_md.is_incoming_transaction(),
        ) {
            // This transaction is entirely composed of what we consider
            // to be 'change'. We just make a Fee transfer and move on
            (false, false) => (),
            // All received funds were change, this is a normal send
            (true, false) => {
                for OutgoingTxData {
                    to_address,
                    value,
                    memo,
                    recipient_ua,
                } in &transaction_md.outgoing_tx_data
                {
                    if let Ok(to_address) =
                        ZcashAddress::try_from_encoded(recipient_ua.as_ref().unwrap_or(to_address))
                    {
                        let memos = if let Memo::Text(textmemo) = memo {
                            vec![textmemo.clone()]
                        } else {
                            vec![]
                        };
                        summaries.push(ValueTransfer {
                            block_height,
                            datetime,
                            kind: ValueTransferKind::Sent {
                                to_address,
                                amount: *value,
                            },
                            memos,
                            price,
                            txid,
                            unconfirmed,
                        });
                    }
                }
            }
            // No funds spent, this is a normal receipt
            (false, true) => {
                for received_transparent in transaction_md.transparent_notes.iter() {
                    summaries.push(ValueTransfer {
                        block_height,
                        datetime,
                        kind: ValueTransferKind::Received {
                            pool: Pool::Transparent,
                            amount: received_transparent.value,
                        },
                        memos: vec![],
                        price,
                        txid,
                        unconfirmed,
                    });
                }
                for received_sapling in transaction_md.sapling_notes.iter() {
                    let memos = if let Some(Memo::Text(textmemo)) = &received_sapling.memo {
                        vec![textmemo.clone()]
                    } else {
                        vec![]
                    };
                    summaries.push(ValueTransfer {
                        block_height,
                        datetime,
                        kind: ValueTransferKind::Received {
                            pool: Pool::Sapling,
                            amount: received_sapling.value(),
                        },
                        memos,
                        price,
                        txid,
                        unconfirmed,
                    });
                }
                for received_orchard in transaction_md.orchard_notes.iter() {
                    let memos = if let Some(Memo::Text(textmemo)) = &received_orchard.memo {
                        vec![textmemo.clone()]
                    } else {
                        vec![]
                    };
                    summaries.push(ValueTransfer {
                        block_height,
                        datetime,
                        kind: ValueTransferKind::Received {
                            pool: Pool::Orchard,
                            amount: received_orchard.value(),
                        },
                        memos,
                        price,
                        txid,
                        unconfirmed,
                    });
                }
            }
            // We spent funds, and received funds as non-change. This is most likely a send-to-self,
            // TODO: Figure out what kind of special-case handling we want for these
            (true, true) => {
                summaries.push(ValueTransfer {
                    block_height,
                    datetime,
                    kind: ValueTransferKind::SendToSelf,
                    memos: transaction_md
                        .sapling_notes
                        .iter()
                        .filter_map(|sapling_note| sapling_note.memo.clone())
                        .chain(
                            transaction_md
                                .orchard_notes
                                .iter()
                                .filter_map(|orchard_note| orchard_note.memo.clone()),
                        )
                        .filter_map(|memo| {
                            if let Memo::Text(text_memo) = memo {
                                Some(text_memo)
                            } else {
                                None
                            }
                        })
                        .collect(),
                    price,
                    txid,
                    unconfirmed,
                });
            }
        };
    }

    pub(crate) async fn update_current_price(&self) -> String {
        // Get the zec price from the server
        match get_recent_median_price_from_gemini().await {
            Ok(price) => {
                self.wallet.set_latest_zec_price(price).await;
                price.to_string()
            }
            Err(s) => {
                error!("Error fetching latest price: {}", s);
                s.to_string()
            }
        }
    }
    async fn value_transfer_by_to_address(&self) -> finsight::ValuesSentToAddress {
        let summaries = self.do_list_txsummaries().await;
        let mut amount_by_address = HashMap::new();
        for summary in summaries {
            use ValueTransferKind::*;
            match summary.kind {
                Sent { amount, to_address } => {
                    let address = to_address.encode();
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        amount_by_address.entry(address.clone())
                    {
                        e.insert(vec![amount]);
                    } else {
                        amount_by_address
                            .get_mut(&address)
                            .expect("a vec of u64")
                            .push(amount);
                    };
                }
                Fee { amount } => {
                    let fee_key = "fee".to_string();
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        amount_by_address.entry(fee_key.clone())
                    {
                        e.insert(vec![amount]);
                    } else {
                        amount_by_address
                            .get_mut(&fee_key)
                            .expect("a vec of u64.")
                            .push(amount);
                    };
                }
                SendToSelf { .. } | Received { .. } => (),
            }
        }
        finsight::ValuesSentToAddress(amount_by_address)
    }

    fn write_file_if_not_exists(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
        let mut file_path = dir.to_path_buf();
        file_path.push(name);
        if !file_path.exists() {
            let mut file = File::create(&file_path)?;
            file.write_all(bytes)?;
        }

        Ok(())
    }

    /// Some LightClients have a data dir in state. Mobile versions instead rely on a buffer and will return an error if this function is called.
    /// ZingoConfig specifies both a wallet file and a directory containing it.
    /// This function returns a PathBuf, the absolute path of the wallet file typically named zingo-wallet.dat
    pub fn get_wallet_file_location(&self) -> Result<PathBuf, ZingoLibError> {
        if let Some(mut loc) = self.config.wallet_dir.clone() {
            loc.push(self.config.wallet_name.clone());
            Ok(loc)
        } else {
            Err(ZingoLibError::NoWalletLocation)
        }
    }

    /// Some LightClients have a data dir in state. Mobile versions instead rely on a buffer and will return an error if this function is called.
    /// ZingoConfig specifies both a wallet file and a directory containing it.
    /// This function returns a PathBuf, the absolute path of a directory which typically contains a wallet.dat file
    pub fn get_wallet_dir_location(&self) -> Result<PathBuf, ZingoLibError> {
        if let Some(loc) = self.config.wallet_dir.clone() {
            Ok(loc)
        } else {
            Err(ZingoLibError::NoWalletLocation)
        }
    }

    fn unspent_pending_spent(
        &self,
        note: JsonValue,
        unspent: &mut Vec<JsonValue>,
        pending: &mut Vec<JsonValue>,
        spent: &mut Vec<JsonValue>,
    ) {
        if note["spent"].is_null() && note["unconfirmed_spent"].is_null() {
            unspent.push(note);
        } else if !note["spent"].is_null() {
            spent.push(note);
        } else {
            pending.push(note);
        }
    }
    async fn list_sapling_notes(
        &self,
        all_notes: bool,
        anchor_height: BlockHeight,
    ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
        let mut unspent_sapling_notes: Vec<JsonValue> = vec![];
        let mut pending_sapling_notes: Vec<JsonValue> = vec![];
        let mut spent_sapling_notes: Vec<JsonValue> = vec![];
        // Collect Sapling notes
        self.wallet.transaction_context.transaction_metadata_set.read().await.current.iter()
                .flat_map( |(transaction_id, transaction_metadata)| {
                    transaction_metadata.sapling_notes.iter().filter_map(move |note_metadata|
                        if !all_notes && note_metadata.spent.is_some() {
                            None
                        } else {
                            let address = LightWallet::note_address::<sapling_crypto::note_encryption::SaplingDomain>(&self.config.chain, note_metadata, &self.wallet.wallet_capability());
                            let spendable = transaction_metadata.status.is_confirmed_after_or_at(&anchor_height) && note_metadata.spent.is_none() && note_metadata.unconfirmed_spent.is_none();

                            let created_block:u32 = transaction_metadata.status.get_height().into();
                            Some(object!{
                                "created_in_block"   => created_block,
                                "datetime"           => transaction_metadata.datetime,
                                "created_in_txid"    => format!("{}", transaction_id.clone()),
                                "value"              => note_metadata.note.value().inner(),
                                "unconfirmed"        => !transaction_metadata.status.is_confirmed(),
                                "is_change"          => note_metadata.is_change,
                                "address"            => address,
                                "spendable"          => spendable,
                                "spent"              => note_metadata.spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                                "spent_at_height"    => note_metadata.spent.map(|(_, h)| h),
                                "unconfirmed_spent"  => note_metadata.unconfirmed_spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                            })
                        }
                    )
                })
                .for_each( |note| {
                    self.unspent_pending_spent(note, &mut unspent_sapling_notes, &mut pending_sapling_notes, &mut spent_sapling_notes)
                });
        (
            unspent_sapling_notes,
            spent_sapling_notes,
            pending_sapling_notes,
        )
    }
    async fn list_orchard_notes(
        &self,
        all_notes: bool,
        anchor_height: BlockHeight,
    ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
        let mut unspent_orchard_notes: Vec<JsonValue> = vec![];
        let mut pending_orchard_notes: Vec<JsonValue> = vec![];
        let mut spent_orchard_notes: Vec<JsonValue> = vec![];
        self.wallet.transaction_context.transaction_metadata_set.read().await.current.iter()
                .flat_map( |(transaction_id, transaction_metadata)| {
                    transaction_metadata.orchard_notes.iter().filter_map(move |orch_note_metadata|
                        if !all_notes && orch_note_metadata.is_spent() {
                            None
                        } else {
                            let address = LightWallet::note_address::<orchard::note_encryption::OrchardDomain>(&self.config.chain, orch_note_metadata, &self.wallet.wallet_capability());
                            let spendable = transaction_metadata.status.is_confirmed_after_or_at(&anchor_height) && orch_note_metadata.spent.is_none() && orch_note_metadata.unconfirmed_spent.is_none();

                            let created_block:u32 = transaction_metadata.status.get_height().into();
                            Some(object!{
                                "created_in_block"   => created_block,
                                "datetime"           => transaction_metadata.datetime,
                                "created_in_txid"    => format!("{}", transaction_id),
                                "value"              => orch_note_metadata.note.value().inner(),
                                "unconfirmed"        => !transaction_metadata.status.is_confirmed(),
                                "is_change"          => orch_note_metadata.is_change,
                                "address"            => address,
                                "spendable"          => spendable,
                                "spent"              => orch_note_metadata.spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                                "spent_at_height"    => orch_note_metadata.spent.map(|(_, h)| h),
                                "unconfirmed_spent"  => orch_note_metadata.unconfirmed_spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                            })
                        }
                    )
                })
                .for_each( |note| {
                    self.unspent_pending_spent(note, &mut unspent_orchard_notes, &mut pending_orchard_notes, &mut spent_orchard_notes)
                });
        (
            unspent_orchard_notes,
            spent_orchard_notes,
            pending_orchard_notes,
        )
    }
    async fn list_transparent_notes(
        &self,
        all_notes: bool,
    ) -> (Vec<JsonValue>, Vec<JsonValue>, Vec<JsonValue>) {
        let mut unspent_transparent_notes: Vec<JsonValue> = vec![];
        let mut pending_transparent_notes: Vec<JsonValue> = vec![];
        let mut spent_transparent_notes: Vec<JsonValue> = vec![];

        self.wallet.transaction_context.transaction_metadata_set.read().await.current.iter()
                .flat_map( |(transaction_id, wtx)| {
                    wtx.transparent_notes.iter().filter_map(move |utxo|
                        if !all_notes && utxo.is_spent() {
                            None
                        } else {
                            let created_block:u32 = wtx.status.get_height().into();
                            let recipient = zcash_client_backend::address::Address::decode(&self.config.chain, &utxo.address);
                            let taddr = match recipient {
                            Some(zcash_client_backend::address::Address::Transparent(taddr)) => taddr,
                                _otherwise => panic!("Read invalid taddr from wallet-local Utxo, this should be impossible"),
                            };

                            Some(object!{
                                "created_in_block"   => created_block,
                                "datetime"           => wtx.datetime,
                                "created_in_txid"    => format!("{}", transaction_id),
                                "value"              => utxo.value,
                                "scriptkey"          => hex::encode(utxo.script.clone()),
                                "is_change"          => false, // TODO: Identify notes as change if we send change to our own taddrs
                                "address"            => self.wallet.wallet_capability().get_ua_from_contained_transparent_receiver(&taddr).map(|ua| ua.encode(&self.config.chain)),
                                "spent"              => utxo.spent().map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                                "spent_at_height"    => utxo.spent().map(|(_, h)| h),
                                "unconfirmed_spent"  => utxo.unconfirmed_spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                            })
                        }
                    )
                })
                .for_each( |note| {
                    self.unspent_pending_spent(note, &mut unspent_transparent_notes, &mut pending_transparent_notes, &mut spent_transparent_notes)
                });

        (
            unspent_transparent_notes,
            spent_transparent_notes,
            pending_transparent_notes,
        )
    }

    /// Return a list of notes, if `all_notes` is false, then only return unspent notes
    ///  * TODO:  This fn does not handle failure it must be promoted to return a Result
    ///  * TODO:  The Err variant of the result must be a proper type
    ///  * TODO:  remove all_notes bool
    ///  * TODO:   This fn must (on success) return an Ok(Vec\<Notes\>) where Notes is a 3 variant enum....
    ///  * TODO:   type-associated to the variants of the enum must impl From\<Type\> for JsonValue
    pub async fn do_list_notes(&self, all_notes: bool) -> JsonValue {
        let anchor_height = BlockHeight::from_u32(self.wallet.get_anchor_height().await);

        let (mut unspent_sapling_notes, mut spent_sapling_notes, mut pending_sapling_notes) =
            self.list_sapling_notes(all_notes, anchor_height).await;
        let (mut unspent_orchard_notes, mut spent_orchard_notes, mut pending_orchard_notes) =
            self.list_orchard_notes(all_notes, anchor_height).await;
        let (
            mut unspent_transparent_notes,
            mut spent_transparent_notes,
            mut pending_transparent_notes,
        ) = self.list_transparent_notes(all_notes).await;

        unspent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        unspent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        unspent_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_transparent_notes.sort_by_key(|note| note["created_in_block"].as_u64());

        let mut res = object! {
            "unspent_sapling_notes" => unspent_sapling_notes,
            "pending_sapling_notes" => pending_sapling_notes,
            "unspent_orchard_notes" => unspent_orchard_notes,
            "pending_orchard_notes" => pending_orchard_notes,
            "utxos"         => unspent_transparent_notes,
            "pending_utxos" => pending_transparent_notes,
        };

        if all_notes {
            res["spent_sapling_notes"] = JsonValue::Array(spent_sapling_notes);
            res["spent_orchard_notes"] = JsonValue::Array(spent_orchard_notes);
            res["spent_utxos"] = JsonValue::Array(spent_transparent_notes);
        }

        res
    }
}

use serde_json::Value;

enum PriceFetchError {
    ReqwestError(String),
    NotJson,
    NoElements,
    PriceReprError(PriceReprError),
    NanValue,
}

impl std::fmt::Display for PriceFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use PriceFetchError::*;
        f.write_str(&match self {
            ReqwestError(e) => format!("ReqwestError: {}", e),
            NotJson => "NotJson".to_string(),
            NoElements => "NoElements".to_string(),
            PriceReprError(e) => format!("PriceReprError: {}", e),
            NanValue => "NanValue".to_string(),
        })
    }
}

enum PriceReprError {
    NoValue,
    NoAsStrValue,
    NotParseable,
}

impl std::fmt::Display for PriceReprError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use PriceReprError::*;
        fmt.write_str(match self {
            NoValue => "NoValue",
            NoAsStrValue => "NoAsStrValue",
            NotParseable => "NotParseable",
        })
    }
}
fn repr_price_as_f64(from_gemini: &Value) -> Result<f64, PriceReprError> {
    if let Some(value) = from_gemini.get("price") {
        if let Some(stringable) = value.as_str() {
            if let Ok(parsed) = stringable.parse::<f64>() {
                Ok(parsed)
            } else {
                Err(PriceReprError::NotParseable)
            }
        } else {
            Err(PriceReprError::NoAsStrValue)
        }
    } else {
        Err(PriceReprError::NoValue)
    }
}

async fn get_recent_median_price_from_gemini() -> Result<f64, PriceFetchError> {
    let httpget =
        match reqwest::get("https://api.gemini.com/v1/trades/zecusd?limit_trades=11").await {
            Ok(httpresponse) => httpresponse,
            Err(e) => {
                return Err(PriceFetchError::ReqwestError(e.to_string()));
            }
        };
    let serialized = match httpget.json::<Value>().await {
        Ok(asjson) => asjson,
        Err(_) => {
            return Err(PriceFetchError::NotJson);
        }
    };
    let elements = match serialized.as_array() {
        Some(elements) => elements,
        None => {
            return Err(PriceFetchError::NoElements);
        }
    };
    let mut trades: Vec<f64> = match elements.iter().map(repr_price_as_f64).collect() {
        Ok(trades) => trades,
        Err(e) => {
            return Err(PriceFetchError::PriceReprError(e));
        }
    };
    if trades.iter().any(|x| x.is_nan()) {
        return Err(PriceFetchError::NanValue);
    }
    // NOTE:  This code will panic if a value is received that:
    // 1. was parsed from a string to an f64
    // 2. is not a NaN
    // 3. cannot be compared to an f64
    // TODO:  Show that this is impossible, or write code to handle
    // that case.
    trades.sort_by(|a, b| {
        a.partial_cmp(b)
            .expect("a and b are non-nan f64, I think that makes them comparable")
    });
    Ok(trades[5])
}

#[cfg(test)]
#[cfg(feature = "test-features")]
mod tests {
    use tokio::runtime::Runtime;
    use zingo_testvectors::seeds::CHIMNEY_BETTER_SEED;
    use zingoconfig::{ChainType, ZingoConfig};

    use crate::{lightclient::LightClient, wallet::WalletBase};

    #[test]
    fn new_wallet_from_phrase() {
        let temp_dir = tempfile::Builder::new().prefix("test").tempdir().unwrap();
        let data_dir = temp_dir
            .into_path()
            .canonicalize()
            .expect("This path is available.");

        let wallet_name = data_dir.join("zingo-wallet.dat");
        let config = ZingoConfig::build(ChainType::FakeMainnet)
            .set_wallet_dir(data_dir)
            .create();
        let lc = LightClient::create_from_wallet_base(
            WalletBase::MnemonicPhrase(CHIMNEY_BETTER_SEED.to_string()),
            &config,
            0,
            false,
        )
        .unwrap();
        assert_eq!(
        format!(
            "{:?}",
            LightClient::create_from_wallet_base(
                WalletBase::MnemonicPhrase(CHIMNEY_BETTER_SEED.to_string()),
                &config,
                0,
                false
            )
            .err()
            .unwrap()
        ),
        format!(
            "{:?}",
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("Cannot create a new wallet from seed, because a wallet already exists at:\n{:?}", wallet_name),
            )
        )
    );

        // The first t address and z address should be derived
        Runtime::new().unwrap().block_on(async move {
            let addresses = lc.do_addresses().await;
            assert_eq!(
                "zs1q6xk3q783t5k92kjqt2rkuuww8pdw2euzy5rk6jytw97enx8fhpazdv3th4xe7vsk6e9sfpawfg"
                    .to_string(),
                addresses[0]["receivers"]["sapling"]
            );
            assert_eq!(
                "t1eQ63fwkQ4n4Eo5uCrPGaAV8FWB2tmx7ui",
                addresses[0]["receivers"]["transparent"]
            );
        });
    }
}

#[cfg(feature = "lightclient-deprecated")]
mod deprecated;
#[cfg(feature = "test-features")]
pub mod test_features;
