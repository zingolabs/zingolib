use crate::{
    blaze::{
        block_witness_data::BlockAndWitnessData, fetch_compact_blocks::FetchCompactBlocks,
        fetch_full_transaction::TransactionContext,
        fetch_taddr_transactions::FetchTaddrTransactions, sync_status::SyncStatus,
        syncdata::BlazeSyncData, trial_decryptions::TrialDecryptions, update_notes::UpdateNotes,
    },
    compact_formats::RawTransaction,
    grpc_connector::GrpcConnector,
    wallet::{
        data::{OrchardNoteAndMetadata, SaplingNoteAndMetadata, TransactionMetadata},
        keys::Keys,
        message::Message,
        now,
        traits::NoteAndMetadata,
        LightWallet,
    },
};
use futures::future::join_all;
use json::{array, object, JsonValue};
use log::{error, info, warn};
use std::{
    cmp,
    collections::HashSet,
    fs::File,
    io::{self, BufReader, Error, ErrorKind, Read, Write},
    path::Path,
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

use orchard::keys::{FullViewingKey as OrchardFullViewingKey, SpendingKey as OrchardSpendingKey};
use zcash_client_backend::{
    address::UnifiedAddress,
    encoding::{decode_payment_address, encode_payment_address},
};
use zcash_primitives::{
    block::BlockHash,
    consensus::{BlockHeight, BranchId},
    memo::{Memo, MemoBytes},
    transaction::{components::amount::DEFAULT_FEE, Transaction},
};
use zcash_proofs::prover::LocalTxProver;
use zingoconfig::{ZingoConfig, MAX_REORG};

pub(crate) mod checkpoints;

#[derive(Clone, Debug)]
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

pub struct LightClient {
    pub(crate) config: ZingoConfig,
    pub(crate) wallet: LightWallet,

    mempool_monitor: std::sync::RwLock<Option<std::thread::JoinHandle<()>>>,

    sync_lock: Mutex<()>,

    bsync_data: Arc<RwLock<BlazeSyncData>>,
}

use serde_json::Value;

fn repr_price_as_f64(from_gemini: &Value) -> f64 {
    from_gemini
        .get("price")
        .unwrap()
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap()
}

async fn get_recent_median_price_from_gemini() -> Result<f64, reqwest::Error> {
    let mut trades: Vec<f64> =
        reqwest::get("https://api.gemini.com/v1/trades/zecusd?limit_trades=11")
            .await?
            .json::<Value>()
            .await?
            .as_array()
            .unwrap()
            .into_iter()
            .map(repr_price_as_f64)
            .collect();
    trades.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Ok(trades[5])
}

#[cfg(test)]
impl LightClient {
    /// Method to create a test-only version of the LightClient
    pub async fn test_new(
        config: &ZingoConfig,
        seed_phrase: Option<String>,
        height: u64,
    ) -> io::Result<Self> {
        if seed_phrase.is_some() && config.wallet_exists() {
            return Err(Error::new(
                ErrorKind::AlreadyExists,
                "Cannot create a new wallet from seed, because a wallet already exists",
            ));
        }

        let l = LightClient::create_unconnected(config, seed_phrase, height, 1)
            .expect("Unconnected client creation failed!");
        l.set_wallet_initial_state(height).await;

        info!("Created new wallet!");
        info!("Created LightClient to {}", &config.get_server_uri());
        Ok(l)
    }

    //TODO: Add migrate_sapling_to_orchard argument
    pub async fn test_do_send(
        &self,
        addrs: Vec<(&str, u64, Option<String>)>,
    ) -> Result<String, String> {
        // First, get the concensus branch ID
        info!("Creating transaction");

        let result = {
            let _lock = self.sync_lock.lock().await;
            let prover = crate::blaze::test_utils::FakeTransactionProver {};

            self.wallet
                .send_to_address(prover, false, false, addrs, |transaction_bytes| {
                    GrpcConnector::send_transaction(self.get_server_uri(), transaction_bytes)
                })
                .await
        };

        result.map(|(transaction_id, _)| transaction_id)
    }
}
impl LightClient {
    pub fn create_unconnected(
        config: &ZingoConfig,
        seed_phrase: Option<String>,
        height: u64,
        num_zaddrs: u32,
    ) -> io::Result<Self> {
        Ok(LightClient {
            wallet: LightWallet::new(config.clone(), seed_phrase, height, num_zaddrs)?,
            config: config.clone(),
            mempool_monitor: std::sync::RwLock::new(None),
            bsync_data: Arc::new(RwLock::new(BlazeSyncData::new(&config))),
            sync_lock: Mutex::new(()),
        })
    }
    pub fn set_server(&self, server: http::Uri) {
        *self.config.server_uri.write().unwrap() = server
    }

    pub fn get_server(&self) -> std::sync::RwLockReadGuard<http::Uri> {
        self.config.server_uri.read().unwrap()
    }

    fn write_file_if_not_exists(dir: &Box<Path>, name: &str, bytes: &[u8]) -> io::Result<()> {
        let mut file_path = dir.to_path_buf();
        file_path.push(name);
        if !file_path.exists() {
            let mut file = File::create(&file_path)?;
            file.write_all(bytes)?;
        }

        Ok(())
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
    fn read_sapling_params(&self) -> Result<(Vec<u8>, Vec<u8>), String> {
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

        if sapling_output.len() > 0 {
            if SAPLING_OUTPUT_HASH.to_string() != hex::encode(Sha256::digest(&sapling_output)) {
                return Err(format!(
                    "sapling-output hash didn't match. expected {}, found {}",
                    SAPLING_OUTPUT_HASH,
                    hex::encode(Sha256::digest(&sapling_output))
                ));
            }
        }

        if sapling_spend.len() > 0 {
            if SAPLING_SPEND_HASH.to_string() != hex::encode(Sha256::digest(&sapling_spend)) {
                return Err(format!(
                    "sapling-spend hash didn't match. expected {}, found {}",
                    SAPLING_SPEND_HASH,
                    hex::encode(Sha256::digest(&sapling_spend))
                ));
            }
        }

        // Ensure that the sapling params are stored on disk properly as well. Only on desktop
        match self.config.get_zcash_params_path() {
            Ok(zcash_params_dir) => {
                // Create the sapling output and spend params files
                match LightClient::write_file_if_not_exists(
                    &zcash_params_dir,
                    "sapling-output.params",
                    &sapling_output,
                ) {
                    Ok(_) => {}
                    Err(e) => {
                        return Err(format!("Warning: Couldn't write the output params!\n{}", e))
                    }
                };

                match LightClient::write_file_if_not_exists(
                    &zcash_params_dir,
                    "sapling-spend.params",
                    &sapling_spend,
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

    pub async fn get_initial_state(&self, height: u64) -> Option<(u64, String, String)> {
        if height <= self.config.sapling_activation_height() {
            return None;
        }

        info!(
            "Getting sapling tree from LightwalletD at height {}",
            height
        );
        match GrpcConnector::get_trees(self.config.get_server_uri(), height).await {
            Ok(tree_state) => {
                let hash = tree_state.hash.clone();
                let tree = tree_state.sapling_tree.clone();
                Some((tree_state.height, hash, tree))
            }
            Err(e) => {
                error!(
                    "Error getting sapling tree:{}\nWill return checkpoint instead.",
                    e
                );
                match checkpoints::get_closest_checkpoint(&self.config.chain, height) {
                    Some((height, hash, tree)) => {
                        Some((height, hash.to_string(), tree.to_string()))
                    }
                    None => None,
                }
            }
        }
    }

    pub async fn set_wallet_initial_state(&self, height: u64) {
        let state = self.get_initial_state(height).await;

        match state {
            Some((height, hash, tree)) => {
                info!("Setting initial state to height {}, tree {}", height, tree);
                self.wallet
                    .set_initial_block(height, &hash.as_str(), &tree.as_str())
                    .await;
            }
            _ => {}
        };
    }

    fn new_wallet(config: &ZingoConfig, latest_block: u64, num_zaddrs: u32) -> io::Result<Self> {
        Runtime::new().unwrap().block_on(async move {
            let l = LightClient {
                wallet: LightWallet::new(config.clone(), None, latest_block, num_zaddrs)?,
                config: config.clone(),
                mempool_monitor: std::sync::RwLock::new(None),
                sync_lock: Mutex::new(()),
                bsync_data: Arc::new(RwLock::new(BlazeSyncData::new(&config))),
            };

            l.set_wallet_initial_state(latest_block).await;

            info!("Created new wallet with a new seed!");
            info!("Created LightClient to {}", &config.get_server_uri());

            // Save
            l.do_save()
                .await
                .map_err(|s| io::Error::new(ErrorKind::PermissionDenied, s))?;

            Ok(l)
        })
    }

    /// Create a brand new wallet with a new seed phrase. Will fail if a wallet file
    /// already exists on disk
    pub fn new(config: &ZingoConfig, latest_block: u64) -> io::Result<Self> {
        #[cfg(all(not(target_os = "ios"), not(target_os = "android")))]
        {
            if config.wallet_exists() {
                return Err(Error::new(
                    ErrorKind::AlreadyExists,
                    "Cannot create a new wallet from seed, because a wallet already exists",
                ));
            }
        }

        Self::new_wallet(config, latest_block, 1)
    }

    pub fn create_with_capable_wallet(
        key_or_seedphrase: String,
        config: &ZingoConfig,
        birthday: u64,
        overwrite: bool,
    ) -> io::Result<Self> {
        #[cfg(all(not(target_os = "ios"), not(target_os = "android")))]
        {
            if !overwrite && config.wallet_exists() {
                return Err(Error::new(
                    ErrorKind::AlreadyExists,
                    format!(
                        "Cannot create a new wallet from seed, because a wallet already exists"
                    ),
                ));
            }
        }
        println!(
            "Seed: {key_or_seedphrase}\nhrp_view: {}",
            config.hrp_sapling_viewing_key()
        );

        let lr = if key_or_seedphrase.starts_with(config.hrp_sapling_private_key())
            || key_or_seedphrase.starts_with(config.hrp_sapling_viewing_key())
        {
            let lc = Self::new_wallet(config, birthday, 0)?;
            Runtime::new().unwrap().block_on(async move {
                lc.do_import_key(key_or_seedphrase, birthday)
                    .await
                    .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;

                info!("Created wallet with 0 keys, imported private key");

                Ok(lc)
            })
        } else if key_or_seedphrase.starts_with(config.chain.hrp_orchard_spending_key()) {
            todo!()
        } else {
            Runtime::new().unwrap().block_on(async move {
                let l = LightClient {
                    wallet: LightWallet::new(config.clone(), Some(key_or_seedphrase), birthday, 1)?,
                    config: config.clone(),
                    mempool_monitor: std::sync::RwLock::new(None),
                    sync_lock: Mutex::new(()),
                    bsync_data: Arc::new(RwLock::new(BlazeSyncData::new(&config))),
                };

                l.set_wallet_initial_state(birthday).await;
                l.do_save()
                    .await
                    .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;

                info!("Created new wallet!");

                Ok(l)
            })
        };

        info!("Created LightClient to {}", &config.get_server_uri());

        lr
    }

    pub fn read_from_buffer<R: Read>(config: &ZingoConfig, mut reader: R) -> io::Result<Self> {
        let l = Runtime::new().unwrap().block_on(async move {
            let wallet = LightWallet::read(&mut reader, config).await?;

            let lc = LightClient {
                wallet,
                config: config.clone(),
                mempool_monitor: std::sync::RwLock::new(None),
                sync_lock: Mutex::new(()),
                bsync_data: Arc::new(RwLock::new(BlazeSyncData::new(&config))),
            };

            info!(
                "Read wallet with birthday {}",
                lc.wallet.get_birthday().await
            );
            info!("Created LightClient to {}", &config.get_server_uri());

            Ok(lc)
        });

        l
    }

    pub fn read_from_disk(config: &ZingoConfig) -> io::Result<Self> {
        let wallet_path = if config.wallet_exists() {
            config.get_wallet_path()
        } else {
            return Err(Error::new(
                ErrorKind::AlreadyExists,
                format!(
                    "Cannot read wallet. No file at {}",
                    config.get_wallet_path().display()
                ),
            ));
        };

        let l = Runtime::new().unwrap().block_on(async move {
            let mut file_buffer = BufReader::new(File::open(wallet_path)?);

            let wallet = LightWallet::read(&mut file_buffer, config).await?;

            let lc = LightClient {
                wallet,
                config: config.clone(),
                mempool_monitor: std::sync::RwLock::new(None),
                sync_lock: Mutex::new(()),
                bsync_data: Arc::new(RwLock::new(BlazeSyncData::new(&config))),
            };

            info!(
                "Read wallet with birthday {}",
                lc.wallet.get_birthday().await
            );
            info!("Created LightClient to {}", &config.get_server_uri());

            Ok(lc)
        });

        l
    }

    pub fn init_logging(&self) -> io::Result<()> {
        // Configure logging first.
        let log_config = self.config.get_log_config()?;
        log4rs::init_config(log_config).map_err(|e| std::io::Error::new(ErrorKind::Other, e))?;

        Ok(())
    }

    // Export private keys
    pub async fn do_export(&self, addr: Option<String>) -> Result<JsonValue, &str> {
        if !self.wallet.is_unlocked_for_spending().await {
            error!("Wallet is locked");
            return Err("Wallet is locked");
        }

        // Clone address so it can be moved into the closure
        let address = addr.clone();
        // Go over all z addresses
        let z_keys = self
            .export_x_keys(Keys::get_z_private_keys, |(addr, pk, vk)| match &address {
                Some(target_addr) if target_addr != addr => None,
                _ => Some(object! {
                    "address"     => addr.clone(),
                    "private_key" => pk.clone(),
                    "viewing_key" => vk.clone(),
                }),
            })
            .await;

        // Clone address so it can be moved into the closure
        let address = addr.clone();

        // Go over all t addresses
        let t_keys = self
            .export_x_keys(Keys::get_t_secret_keys, |(addr, sk)| match &address {
                Some(target_addr) if target_addr != addr => None,
                _ => Some(object! {
                    "address"     => addr.clone(),
                    "private_key" => sk.clone(),
                }),
            })
            .await;

        // Clone address so it can be moved into the closure
        let address = addr.clone();

        // Go over all orchard addresses
        let o_keys = self
            .export_x_keys(
                Keys::get_orchard_spending_keys,
                |(addr, sk, vk)| match &address {
                    Some(target_addr) if target_addr != addr => None,
                    _ => Some(object! {
                        "address"     => addr.clone(),
                        "spending_key" => sk.clone(),
                        "full_viewing_key" => vk.clone(),
                    }),
                },
            )
            .await;

        let mut all_keys = vec![];
        all_keys.extend_from_slice(&o_keys);
        all_keys.extend_from_slice(&z_keys);
        all_keys.extend_from_slice(&t_keys);

        Ok(all_keys.into())
    }

    //helper function to export all keys of a type
    async fn export_x_keys<T, F: Fn(&Keys) -> Vec<T>, G: Fn(&T) -> Option<JsonValue>>(
        &self,
        key_getter: F,
        key_filter_map: G,
    ) -> Vec<JsonValue> {
        key_getter(&*self.wallet.keys().read().await)
            .iter()
            .filter_map(key_filter_map)
            .collect()
    }

    pub async fn do_address(&self) -> JsonValue {
        // Collect z addresses
        let z_addresses = self.wallet.keys().read().await.get_all_sapling_addresses();

        // Collect t addresses
        let t_addresses = self.wallet.keys().read().await.get_all_taddrs();

        // Collect o addresses
        let o_addresses = self.wallet.keys().read().await.get_all_orchard_addresses();

        object! {
            "sapling_addresses" => z_addresses,
            "transparent_addresses" => t_addresses,
            "orchard_addresses" => o_addresses,
        }
    }

    pub async fn do_last_transaction_id(&self) -> JsonValue {
        object! {
            "last_txid" => self.wallet.transactions().read().await.get_last_txid().map(|t| t.to_string())
        }
    }

    pub async fn do_balance(&self) -> JsonValue {
        // Collect z addresses
        let mut sapling_addresses = vec![];
        for sapling_address in self.wallet.keys().read().await.get_all_sapling_addresses() {
            sapling_addresses.push(object! {
                "address"                     => sapling_address.clone(),
                "sapling_balance"             => self.wallet.maybe_verified_sapling_balance(Some(sapling_address.clone())).await,
                "verified_sapling_balance"    => self.wallet.verified_sapling_balance(Some(sapling_address.clone())).await,
                "spendable_sapling_balance"   => self.wallet.spendable_sapling_balance(Some(sapling_address.clone())).await,
                "unverified_sapling_balance"  => self.wallet.unverified_sapling_balance(Some(sapling_address.clone())).await
            });
        }

        let mut orchard_addresses = vec![];
        for orchard_address in self.wallet.keys().read().await.get_all_orchard_addresses() {
            orchard_addresses.push(object! {
                "address"                     => orchard_address.clone(),
                "orchard_balance"             => self.wallet.maybe_verified_orchard_balance(Some(orchard_address.clone())).await,
                "verified_orchard_balance"    => self.wallet.verified_orchard_balance(Some(orchard_address.clone())).await,
                "spendable_orchard_balance"   => self.wallet.spendable_orchard_balance(Some(orchard_address.clone())).await,
                "unverified_orchard_balance"  => self.wallet.unverified_orchard_balance(Some(orchard_address.clone())).await
            });
        }

        // Collect t addresses
        let mut t_addresses = vec![];
        for taddress in self.wallet.keys().read().await.get_all_taddrs() {
            // Get the balance for this address
            let balance = self.wallet.tbalance(Some(taddress.clone())).await;

            t_addresses.push(object! {
                "address"                     => taddress,
                "balance"                     => balance,
            });
        }

        object! {
            "sapling_balance"                 => self.wallet.maybe_verified_sapling_balance(None).await,
            "verified_sapling_balance"        => self.wallet.verified_sapling_balance(None).await,
            "spendable_sapling_balance"       => self.wallet.spendable_sapling_balance(None).await,
            "unverified_sapling_balance"      => self.wallet.unverified_sapling_balance(None).await,
            "orchard_balance"                 => self.wallet.maybe_verified_orchard_balance(None).await,
            "verified_orchard_balance"        => self.wallet.verified_orchard_balance(None).await,
            "spendable_orchard_balance"       => self.wallet.spendable_orchard_balance(None).await,
            "unverified_orchard_balance"      => self.wallet.unverified_orchard_balance(None).await,
            "transparent_balance"             => self.wallet.tbalance(None).await,
            "sapling_addresses"               => sapling_addresses,
            "orchard_addresses"               => orchard_addresses,
            "transparent_addresses"           => t_addresses,
        }
    }

    pub async fn do_save(&self) -> Result<(), String> {
        // On mobile platforms, disable the save, because the saves will be handled by the native layer, and not in rust
        if cfg!(all(not(target_os = "ios"), not(target_os = "android"))) {
            // If the wallet is encrypted but unlocked, lock it again.
            {
                if self.wallet.is_encrypted().await && self.wallet.is_unlocked_for_spending().await
                {
                    match self.wallet.lock().await {
                        Ok(_) => {}
                        Err(e) => {
                            let err = format!("ERR: {}", e);
                            error!("{}", err);
                            return Err(e.to_string());
                        }
                    }
                }
            }

            {
                // Prevent any overlapping syncs during save, and don't save in the middle of a sync
                let _lock = self.sync_lock.lock().await;

                let mut wallet_bytes = vec![];
                match self.wallet.write(&mut wallet_bytes).await {
                    Ok(_) => {
                        let mut file = File::create(self.config.get_wallet_path()).unwrap();
                        file.write_all(&wallet_bytes)
                            .map_err(|e| format!("{}", e))?;
                        Ok(())
                    }
                    Err(e) => {
                        let err = format!("ERR: {}", e);
                        error!("{}", err);
                        Err(e.to_string())
                    }
                }
            }
        } else {
            // On ios and android just return OK
            Ok(())
        }
    }

    pub fn do_save_to_buffer_sync(&self) -> Result<Vec<u8>, String> {
        Runtime::new()
            .unwrap()
            .block_on(async move { self.do_save_to_buffer().await })
    }

    pub async fn do_save_to_buffer(&self) -> Result<Vec<u8>, String> {
        // If the wallet is encrypted but unlocked, lock it again.
        {
            if self.wallet.is_encrypted().await && self.wallet.is_unlocked_for_spending().await {
                match self.wallet.lock().await {
                    Ok(_) => {}
                    Err(e) => {
                        let err = format!("ERR: {}", e);
                        error!("{}", err);
                        return Err(e.to_string());
                    }
                }
            }
        }

        let mut buffer: Vec<u8> = vec![];
        match self.wallet.write(&mut buffer).await {
            Ok(_) => Ok(buffer),
            Err(e) => {
                let err = format!("ERR: {}", e);
                error!("{}", err);
                Err(e.to_string())
            }
        }
    }

    pub fn get_server_uri(&self) -> http::Uri {
        self.config.get_server_uri()
    }

    pub async fn do_zec_price(&self) -> String {
        let mut price = self.wallet.price.read().await.clone();

        // If there is no price, try to fetch it first.
        if price.zec_price.is_none() {
            self.update_current_price().await;
            price = self.wallet.price.read().await.clone();
        }

        match price.zec_price {
            None => return "Error: No price".to_string(),
            Some((timestamp, p)) => {
                let o = object! {
                    "zec_price" => p,
                    "fetched_at" =>  timestamp,
                    "currency" => price.currency
                };

                o.pretty(2)
            }
        }
    }

    pub async fn do_info(&self) -> String {
        match GrpcConnector::get_info(self.get_server_uri()).await {
            Ok(i) => {
                let o = object! {
                    "version" => i.version,
                    "git_commit" => i.git_commit,
                    "server_uri" => self.get_server_uri().to_string(),
                    "vendor" => i.vendor,
                    "taddr_support" => i.taddr_support,
                    "chain_name" => i.chain_name,
                    "sapling_activation_height" => i.sapling_activation_height,
                    "consensus_branch_id" => i.consensus_branch_id,
                    "latest_block_height" => i.block_height
                };
                o.pretty(2)
            }
            Err(e) => e,
        }
    }

    pub async fn do_send_progress(&self) -> Result<JsonValue, String> {
        let progress = self.wallet.get_send_progress().await;

        Ok(object! {
            "id" => progress.id,
            "sending" => progress.is_send_in_progress,
            "progress" => progress.progress,
            "total" => progress.total,
            "txid" => progress.last_transaction_id,
            "error" => progress.last_error,
        })
    }

    pub fn do_seed_phrase_sync(&self) -> Result<JsonValue, &str> {
        Runtime::new()
            .unwrap()
            .block_on(async move { self.do_seed_phrase().await })
    }

    pub async fn do_seed_phrase(&self) -> Result<JsonValue, &str> {
        if !self.wallet.is_unlocked_for_spending().await {
            error!("Wallet is locked");
            return Err("Wallet is locked");
        }

        Ok(object! {
            "seed"     => self.wallet.keys().read().await.get_seed_phrase(),
            "birthday" => self.wallet.get_birthday().await
        })
    }

    // Return a list of all notes, spent and unspent
    pub async fn do_list_notes(&self, all_notes: bool) -> JsonValue {
        let mut unspent_notes: Vec<JsonValue> = vec![];
        let mut spent_notes: Vec<JsonValue> = vec![];
        let mut pending_notes: Vec<JsonValue> = vec![];

        let anchor_height = BlockHeight::from_u32(self.wallet.get_anchor_height().await);

        {
            // First, collect all extfvk's that are spendable (i.e., we have the private key)
            let spendable_address: HashSet<String> = self
                .wallet
                .keys()
                .read()
                .await
                .get_all_spendable_zaddresses()
                .into_iter()
                .collect();

            // Collect Sapling notes
            self.wallet.transaction_context.transaction_metadata_set.read().await.current.iter()
                .flat_map( |(transaction_id, wtx)| {
                    let spendable_address = spendable_address.clone();
                    wtx.sapling_notes.iter().filter_map(move |nd|
                        if !all_notes && nd.spent.is_some() {
                            None
                        } else {
                            let address = LightWallet::note_address(&self.config.chain, nd);
                            let spendable = address.is_some() &&
                                                    spendable_address.contains(&address.clone().unwrap()) &&
                                                    wtx.block <= anchor_height && nd.spent.is_none() && nd.unconfirmed_spent.is_none();

                            let created_block:u32 = wtx.block.into();
                            Some(object!{
                                "created_in_block"   => created_block,
                                "datetime"           => wtx.datetime,
                                "created_in_txid"    => format!("{}", transaction_id),
                                "value"              => nd.note.value,
                                "unconfirmed"        => wtx.unconfirmed,
                                "is_change"          => nd.is_change,
                                "address"            => address,
                                "spendable"          => spendable,
                                "spent"              => nd.spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                                "spent_at_height"    => nd.spent.map(|(_, h)| h),
                                "unconfirmed_spent"  => nd.unconfirmed_spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                            })
                        }
                    )
                })
                .for_each( |note| {
                    if note["spent"].is_null() && note["unconfirmed_spent"].is_null() {
                        unspent_notes.push(note);
                    } else if !note["spent"].is_null() {
                        spent_notes.push(note);
                    } else {
                        pending_notes.push(note);
                    }
                });
        }

        let mut unspent_utxos: Vec<JsonValue> = vec![];
        let mut spent_utxos: Vec<JsonValue> = vec![];
        let mut pending_utxos: Vec<JsonValue> = vec![];

        {
            self.wallet.transaction_context.transaction_metadata_set.read().await.current.iter()
                .flat_map( |(transaction_id, wtx)| {
                    wtx.utxos.iter().filter_map(move |utxo|
                        if !all_notes && utxo.spent.is_some() {
                            None
                        } else {
                            let created_block:u32 = wtx.block.into();

                            Some(object!{
                                "created_in_block"   => created_block,
                                "datetime"           => wtx.datetime,
                                "created_in_txid"    => format!("{}", transaction_id),
                                "value"              => utxo.value,
                                "scriptkey"          => hex::encode(utxo.script.clone()),
                                "is_change"          => false, // TODO: Identify notes as change if we send change to our own taddrs
                                "address"            => utxo.address.clone(),
                                "spent_at_height"    => utxo.spent_at_height,
                                "spent"              => utxo.spent.map(|spent_transaction_id| format!("{}", spent_transaction_id)),
                                "unconfirmed_spent"  => utxo.unconfirmed_spent.map(|(spent_transaction_id, _)| format!("{}", spent_transaction_id)),
                            })
                        }
                    )
                })
                .for_each( |utxo| {
                    if utxo["spent"].is_null() && utxo["unconfirmed_spent"].is_null() {
                        unspent_utxos.push(utxo);
                    } else if !utxo["spent"].is_null() {
                        spent_utxos.push(utxo);
                    } else {
                        pending_utxos.push(utxo);
                    }
                });
        }

        let mut res = object! {
            "unspent_notes" => unspent_notes,
            "pending_notes" => pending_notes,
            "utxos"         => unspent_utxos,
            "pending_utxos" => pending_utxos,
        };

        if all_notes {
            res["spent_notes"] = JsonValue::Array(spent_notes);
            res["spent_utxos"] = JsonValue::Array(spent_utxos);
        }

        res
    }

    pub fn do_encrypt_message(&self, to_address_str: String, memo: Memo) -> JsonValue {
        let to = match decode_payment_address(self.config.hrp_sapling_address(), &to_address_str) {
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

    pub async fn do_decrypt_message(&self, enc_base64: String) -> JsonValue {
        let data = match base64::decode(enc_base64) {
            Ok(v) => v,
            Err(e) => {
                return object! {"error" => format!("Couldn't decode base64. Error was {}", e)}
            }
        };

        match self.wallet.decrypt_message(data).await {
            Some(m) => {
                let memo_bytes: MemoBytes = m.memo.clone().into();
                object! {
                    "to" => encode_payment_address(self.config.hrp_sapling_address(), &m.to),
                    "memo" => LightWallet::memo_str(Some(m.memo)),
                    "memohex" => hex::encode(memo_bytes.as_slice())
                }
            }
            None => object! { "error" => "Couldn't decrypt with any of the wallet's keys"},
        }
    }

    pub async fn do_encryption_status(&self) -> JsonValue {
        object! {
            "encrypted" => self.wallet.is_encrypted().await,
            "locked"    => !self.wallet.is_unlocked_for_spending().await
        }
    }

    pub async fn do_list_transactions(&self, include_memo_hex: bool) -> JsonValue {
        // Create a list of TransactionItems from wallet transactions
        let mut transaction_list = self
            .wallet
            .transaction_context.transaction_metadata_set
            .read()
            .await
            .current
            .iter()
            .flat_map(|(_k, wallet_transaction)| {
                let mut transactions: Vec<JsonValue> = vec![];

                if wallet_transaction.total_value_spent() > 0 {
                    // If money was spent, create a transaction. For this, we'll subtract
                    // all the change notes + Utxos
                    let total_change = wallet_transaction
                        .sapling_notes
                        .iter()
                        .filter(|nd| nd.is_change)
                        .map(|nd| nd.note.value)
                        .sum::<u64>()
                        + wallet_transaction.utxos.iter().map(|ut| ut.value).sum::<u64>();

                    // Collect outgoing metadata
                    let outgoing_json = wallet_transaction
                        .outgoing_metadata
                        .iter()
                        .map(|om| {
                            let mut o = object! {
                                "address" => om.address.clone(),
                                "value"   => om.value,
                                "memo"    => LightWallet::memo_str(Some(om.memo.clone()))
                            };

                            if include_memo_hex {
                                let memo_bytes: MemoBytes = om.memo.clone().into();
                                o.insert("memohex", hex::encode(memo_bytes.as_slice())).unwrap();
                            }

                            return o;
                        })
                        .collect::<Vec<JsonValue>>();

                    let block_height: u32 = wallet_transaction.block.into();
                    transactions.push(object! {
                        "block_height" => block_height,
                        "unconfirmed" => wallet_transaction.unconfirmed,
                        "datetime"     => wallet_transaction.datetime,
                        "txid"         => format!("{}", wallet_transaction.txid),
                        "zec_price"    => wallet_transaction.zec_price.map(|p| (p * 100.0).round() / 100.0),
                        "amount"       => total_change as i64 - wallet_transaction.total_value_spent() as i64,
                        "outgoing_metadata" => outgoing_json,
                    });
                }

                // For each sapling note that is not a change, add a transaction.
                transactions.extend(self.add_wallet_notes_in_transaction_to_list(&wallet_transaction, &include_memo_hex));

                // Get the total transparent received
                let total_transparent_received = wallet_transaction.utxos.iter().map(|u| u.value).sum::<u64>();
                if total_transparent_received > wallet_transaction.total_transparent_value_spent {
                    // Create an input transaction for the transparent value as well.
                    let block_height: u32 = wallet_transaction.block.into();
                    transactions.push(object! {
                        "block_height" => block_height,
                        "unconfirmed" => wallet_transaction.unconfirmed,
                        "datetime"     => wallet_transaction.datetime,
                        "txid"         => format!("{}", wallet_transaction.txid),
                        "amount"       => total_transparent_received as i64 - wallet_transaction.total_transparent_value_spent as i64,
                        "zec_price"    => wallet_transaction.zec_price.map(|p| (p * 100.0).round() / 100.0),
                        "address"      => wallet_transaction.utxos.iter().map(|u| u.address.clone()).collect::<Vec<String>>().join(","),
                        "memo"         => None::<String>
                    })
                }

                transactions
            })
            .collect::<Vec<JsonValue>>();

        transaction_list.sort_by(|a, b| {
            if a["block_height"] == b["block_height"] {
                a["txid"].as_str().cmp(&b["txid"].as_str())
            } else {
                a["block_height"].as_i32().cmp(&b["block_height"].as_i32())
            }
        });

        JsonValue::Array(transaction_list)
    }

    fn add_wallet_notes_in_transaction_to_list<'a, 'b>(
        &'a self,
        transaction_metadata: &'b TransactionMetadata,
        include_memo_hex: &'b bool,
    ) -> impl Iterator<Item = JsonValue> + 'b
    where
        'a: 'b,
    {
        self.add_wallet_notes_in_transaction_to_list_inner::<'a, 'b, SaplingNoteAndMetadata>(
            transaction_metadata,
            include_memo_hex,
        )
        .chain(
            self.add_wallet_notes_in_transaction_to_list_inner::<'a, 'b, OrchardNoteAndMetadata>(
                transaction_metadata,
                include_memo_hex,
            ),
        )
    }

    fn add_wallet_notes_in_transaction_to_list_inner<'a, 'b, NnMd>(
        &'a self,
        transaction_metadata: &'b TransactionMetadata,
        include_memo_hex: &'b bool,
    ) -> impl Iterator<Item = JsonValue> + 'b
    where
        'a: 'b,
        NnMd: NoteAndMetadata + 'b,
    {
        NnMd::transaction_metadata_notes(&transaction_metadata).iter().filter(|nd| !nd.is_change()).enumerate().map(|(i, nd)| {
                    let block_height: u32 = transaction_metadata.block.into();
                    let mut o = object! {
                        "block_height" => block_height,
                        "unconfirmed" => transaction_metadata.unconfirmed,
                        "datetime"     => transaction_metadata.datetime,
                        "position"     => i,
                        "txid"         => format!("{}", transaction_metadata.txid),
                        "amount"       => nd.value() as i64,
                        "zec_price"    => transaction_metadata.zec_price.map(|p| (p * 100.0).round() / 100.0),
                        "address"      => LightWallet::note_address(&self.config.chain, nd),
                        "memo"         => LightWallet::memo_str(nd.memo().clone())
                    };

                    if *include_memo_hex {
                        o.insert(
                            "memohex",
                            match &nd.memo() {
                                Some(m) => {
                                    let memo_bytes: MemoBytes = m.into();
                                    hex::encode(memo_bytes.as_slice())
                                }
                                _ => "".to_string(),
                            },
                        )
                        .unwrap();
                    }

                    return o;
                })
    }

    /// Create a new address, deriving it from the seed.
    pub async fn do_new_address(&self, addr_type: &str) -> Result<JsonValue, String> {
        if !self.wallet.is_unlocked_for_spending().await {
            error!("Wallet is locked");
            return Err("Wallet is locked".to_string());
        }

        let new_address = {
            let addr = match addr_type {
                "z" => self.wallet.keys().write().await.add_zaddr(),
                "t" => self.wallet.keys().write().await.add_taddr(),
                "o" => self.wallet.keys().write().await.add_orchard_addr(),
                _ => {
                    let e = format!("Unrecognized address type: {}", addr_type);
                    error!("{}", e);
                    return Err(e);
                }
            };

            if addr.starts_with("Error") {
                let e = format!("Error creating new address: {}", addr);
                error!("{}", e);
                return Err(e);
            }

            addr
        };

        self.do_save().await?;

        Ok(array![new_address])
    }

    /// Convinence function to determine what type of key this is and import it
    pub async fn do_import_key(&self, key: String, birthday: u64) -> Result<JsonValue, String> {
        macro_rules! match_key_type {
            ($key:ident: $($start:expr => $do:expr,)+) => {
                match $key {
                    $(_ if $key.starts_with($start) => $do,)+
                    _ => Err(format!(
                        "'{}' was not recognized as either a spending key or a viewing key",
                        key,
                    )),
                }
            }
        }

        match_key_type!(key:
            self.config.hrp_sapling_private_key() => self.do_import_sapling_spend_key(key, birthday).await,
            self.config.hrp_sapling_viewing_key() => self.do_import_sapling_full_view_key(key, birthday).await,
            "K" => self.do_import_tk(key).await,
            "L" => self.do_import_tk(key).await,
            self.config.chain.hrp_orchard_spending_key() => self.do_import_orchard_secret_key(key, birthday).await,
            self.config.chain.hrp_unified_full_viewing_key() => todo!(),
        )
    }

    /// Import a new transparent private key
    pub async fn do_import_tk(&self, sk: String) -> Result<JsonValue, String> {
        if !self.wallet.is_unlocked_for_spending().await {
            error!("Wallet is locked");
            return Err("Wallet is locked".to_string());
        }

        let address = self.wallet.add_imported_tk(sk).await;
        if address.starts_with("Error") {
            let e = address;
            error!("{}", e);
            return Err(e);
        }

        self.do_save().await?;
        Ok(array![address])
    }

    /// Import a new orchard private key
    pub async fn do_import_orchard_secret_key(
        &self,
        key: String,
        birthday: u64,
    ) -> Result<JsonValue, String> {
        if !self.wallet.is_unlocked_for_spending().await {
            error!("Wallet is locked");
            return Err("Wallet is locked".to_string());
        }

        let new_address = {
            let addr = self
                .wallet
                .add_imported_orchard_spending_key(key, birthday)
                .await;
            if addr.starts_with("Error") {
                let e = addr;
                error!("{}", e);
                return Err(e);
            }

            addr
        };

        self.do_save().await?;

        Ok(array![new_address])
    }

    /// Import a new z-address private key
    pub async fn do_import_sapling_spend_key(
        &self,
        sk: String,
        birthday: u64,
    ) -> Result<JsonValue, String> {
        if !self.wallet.is_unlocked_for_spending().await {
            error!("Wallet is locked");
            return Err("Wallet is locked".to_string());
        }

        let new_address = {
            let addr = self.wallet.add_imported_sapling_extsk(sk, birthday).await;
            if addr.starts_with("Error") {
                let e = addr;
                error!("{}", e);
                return Err(e);
            }

            addr
        };

        self.do_save().await?;

        Ok(array![new_address])
    }

    /// Import a new viewing key
    pub async fn do_import_sapling_full_view_key(
        &self,
        vk: String,
        birthday: u64,
    ) -> Result<JsonValue, String> {
        if !self.wallet.is_unlocked_for_spending().await {
            error!("Wallet is locked");
            return Err("Wallet is locked".to_string());
        }

        let new_address = {
            let addr = self.wallet.add_imported_sapling_extfvk(vk, birthday).await;
            if addr.starts_with("Error") {
                let e = addr;
                error!("{}", e);
                return Err(e);
            }

            addr
        };

        self.do_save().await?;

        Ok(array![new_address])
    }

    pub async fn clear_state(&self) {
        // First, clear the state from the wallet
        self.wallet.clear_all().await;

        // Then set the initial block
        let birthday = self.wallet.get_birthday().await;
        self.set_wallet_initial_state(birthday).await;
        info!("Cleared wallet state, with birthday at {}", birthday);
    }

    pub async fn do_rescan(&self) -> Result<JsonValue, String> {
        if !self.wallet.is_unlocked_for_spending().await {
            warn!("Wallet is locked, new HD addresses won't be added!");
        }

        info!("Rescan starting");

        self.clear_state().await;

        // Then, do a sync, which will force a full rescan from the initial state
        let response = self.do_sync(true).await;

        if response.is_ok() {
            self.do_save().await?;
        }

        info!("Rescan finished");

        response
    }

    pub(crate) async fn update_current_price(&self) -> String {
        // Get the zec price from the server
        match get_recent_median_price_from_gemini().await {
            Ok(price) => {
                self.wallet.set_latest_zec_price(price).await;
                return price.to_string();
            }
            Err(s) => {
                error!("Error fetching latest price: {}", s);
                return s.to_string();
            }
        }
    }

    pub async fn do_sync_status(&self) -> SyncStatus {
        self.bsync_data
            .read()
            .await
            .sync_status
            .read()
            .await
            .clone()
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

        info!("Mempool monitoring starting");

        let uri = lc.config.get_server_uri();
        // Start monitoring the mempool in a new thread
        let h = std::thread::spawn(move || {
            // Start a new async runtime, which is fine because we are in a new thread.
            Runtime::new().unwrap().block_on(async move {
                let (mempool_transmitter, mut mempool_receiver) =
                    unbounded_channel::<RawTransaction>();
                let lc1 = lci.clone();

                let h1 = tokio::spawn(async move {
                    let keys = lc1.wallet.keys();
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
                            //info!("Mempool attempting to scan {}", tx.txid());

                            TransactionContext::new(
                                &config,
                                keys.clone(),
                                transaction_metadata_set.clone(),
                            )
                            .scan_full_tx(
                                transaction,
                                BlockHeight::from_u32(rtransaction.height as u32),
                                true,
                                now() as u32,
                                TransactionMetadata::get_price(now(), &price),
                            )
                            .await;
                        }
                    }
                });

                let h2 = tokio::spawn(async move {
                    loop {
                        //info!("Monitoring mempool");
                        let r = GrpcConnector::monitor_mempool(
                            uri.clone(),
                            mempool_transmitter.clone(),
                        )
                        .await;

                        if r.is_err() {
                            warn!("Mempool monitor returned {:?}, will restart listening", r);
                            sleep(Duration::from_secs(10)).await;
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

    pub async fn do_sync(&self, print_updates: bool) -> Result<JsonValue, String> {
        // Remember the previous sync id first
        let prev_sync_id = self
            .bsync_data
            .read()
            .await
            .sync_status
            .read()
            .await
            .sync_id;

        // Start the sync
        let r_fut = self.start_sync();

        // If printing updates, start a new task to print updates every 2 seconds.
        let sync_result = if print_updates {
            let sync_status = self.bsync_data.read().await.sync_status.clone();
            let (transmitter, mut receiver) = oneshot::channel::<i32>();

            tokio::spawn(async move {
                while sync_status.read().await.sync_id == prev_sync_id {
                    yield_now().await;
                    sleep(Duration::from_secs(3)).await;
                }

                loop {
                    if let Ok(_t) = receiver.try_recv() {
                        break;
                    }

                    let progress = format!("{}", sync_status.read().await);
                    if print_updates {
                        println!("{}", progress);
                    }

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

    /// Start syncing in batches with the max size, to manage memory consumption.
    async fn start_sync(&self) -> Result<JsonValue, String> {
        // We can only do one sync at a time because we sync blocks in serial order
        // If we allow multiple syncs, they'll all get jumbled up.
        let _lock = self.sync_lock.lock().await;

        // The top of the wallet
        let last_scanned_height = self.wallet.last_scanned_height().await;

        let latest_blockid = GrpcConnector::get_latest_block(self.config.get_server_uri()).await?;
        if latest_blockid.height < last_scanned_height {
            let w = format!(
                "Server's latest block({}) is behind ours({})",
                latest_blockid.height, last_scanned_height
            );
            warn!("{}", w);
            return Err(w);
        }

        if latest_blockid.height == last_scanned_height {
            if !latest_blockid.hash.is_empty()
                && BlockHash::from_slice(&latest_blockid.hash).to_string()
                    != self.wallet.last_scanned_hash().await
            {
                warn!("One block reorg at height {}", last_scanned_height);
                // This is a one-block reorg, so pop the last block. Even if there are more blocks to reorg, this is enough
                // to trigger a sync, which will then reorg the remaining blocks
                BlockAndWitnessData::invalidate_block(
                    last_scanned_height,
                    self.wallet.blocks.clone(),
                    self.wallet
                        .transaction_context
                        .transaction_metadata_set
                        .clone(),
                )
                .await;
            }
        }

        // Re-read the last scanned height
        let last_scanned_height = self.wallet.last_scanned_height().await;
        let batch_size = 1_000;

        let mut latest_block_batches = vec![];
        let mut prev = last_scanned_height;
        while latest_block_batches.is_empty() || prev != latest_blockid.height {
            let batch = cmp::min(latest_blockid.height, prev + batch_size);
            prev = batch;
            latest_block_batches.push(batch);
        }

        //println!("Batches are {:?}", latest_block_batches);

        // Increment the sync ID so the caller can determine when it is over
        self.bsync_data
            .write()
            .await
            .sync_status
            .write()
            .await
            .start_new(latest_block_batches.len());

        let mut res = Err("No batches were run!".to_string());
        for (batch_num, batch_latest_block) in latest_block_batches.into_iter().enumerate() {
            res = self.start_sync_batch(batch_latest_block, batch_num).await;
            if res.is_err() {
                return res;
            }
        }

        res
    }

    /// start_sync will start synchronizing the blockchain from the wallet's last height. This function will
    /// return immediately after starting the sync.  Use the `do_sync_status` LightClient method to
    /// get the status of the sync
    async fn start_sync_batch(
        &self,
        latest_block: u64,
        batch_num: usize,
    ) -> Result<JsonValue, String> {
        // The top of the wallet
        let last_scanned_height = self.wallet.last_scanned_height().await;

        info!(
            "Latest block is {}, wallet block is {}",
            latest_block, last_scanned_height
        );

        if last_scanned_height == latest_block {
            info!("Already at latest block, not syncing");
            return Ok(object! { "result" => "success" });
        }

        let bsync_data = self.bsync_data.clone();

        let start_block = latest_block;
        let end_block = last_scanned_height + 1;

        // Before we start, we need to do a few things
        // 1. Pre-populate the last 100 blocks, in case of reorgs
        bsync_data
            .write()
            .await
            .setup_for_sync(
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
        let grpc_connector = GrpcConnector::new(self.config.get_server_uri());

        // A signal to detect reorgs, and if so, ask the block_fetcher to fetch new blocks.
        let (reorg_transmitter, reorg_receiver) = unbounded_channel();

        // Node and Witness Data Cache
        let (block_and_witness_handle, block_and_witness_data_transmitter) = bsync_data
            .read()
            .await
            .block_data
            .start(
                start_block,
                end_block,
                self.wallet.transactions(),
                reorg_transmitter,
            )
            .await;

        // Full Tx GRPC fetcher
        let (full_transaction_fetcher_handle, full_transaction_fetcher_transmitter) =
            grpc_connector
                .start_full_transaction_fetcher(self.config.chain)
                .await;
        // Transparent Transactions Fetcher
        let (taddr_fetcher_handle, taddr_fetcher_transmitter) =
            grpc_connector.start_taddr_transaction_fetcher().await;

        // Local state necessary for a transaction fetch
        let transaction_context =
            TransactionContext::new(&self.config, self.wallet.keys(), self.wallet.transactions());
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

        // Do Trial decryptions of all the sapling outputs, and pass on the successful ones to the update_notes processor
        let trial_decryptions_processor =
            TrialDecryptions::new(self.wallet.keys(), self.wallet.transactions());
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
        let earliest_block = block_and_witness_handle.await.unwrap().unwrap();

        // 1. Fetch the transparent txns only after reorgs are done.
        let taddr_transactions_handle = FetchTaddrTransactions::new(self.wallet.keys())
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
            .map(|r| r.map_err(|e| format!("{}", e)))
            .collect::<Result<(), _>>()
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
        .map(|r| r.map_err(|e| format!("{}", e))?)
        .collect::<Result<(), String>>()?;

        let verify_handle =
            tokio::spawn(async move { block_data.read().await.block_data.verify_trees().await });
        let (verified, highest_tree) = verify_handle.await.map_err(|e| e.to_string())?;
        info!("tree verification {}", verified);
        info!("highest tree exists: {}", highest_tree.is_some());
        if !verified {
            return Err("Tree Verification Failed".to_string());
        }

        info!("Sync finished, doing post-processing");

        let blaze_sync_data = bsync_data.read().await;
        // Post sync, we have to do a bunch of stuff
        // 1. Get the last 100 blocks and store it into the wallet, needed for future re-orgs
        let blocks = blaze_sync_data
            .block_data
            .drain_existingblocks_into_blocks_with_truncation(MAX_REORG)
            .await;
        self.wallet.set_blocks(blocks).await;

        // Store the ten highest orchard anchors
        {
            let mut anchors = self.wallet.orchard_anchors.write().await;
            *anchors = blaze_sync_data
                .block_data
                .orchard_anchors
                .read()
                .await
                .clone()
                .into_iter()
                .chain(anchors.drain(..))
                .collect();
            anchors.sort_unstable_by(|(_, height_a), (_, height_b)| height_b.cmp(height_a));
            anchors.truncate(10);
        }
        // 2. If sync was successfull, also try to get historical prices
        // self.update_historical_prices().await;
        // zingolabs considers this to be a serious privacy/secuity leak

        // 3. Mark the sync finished, which will clear the nullifier cache etc...
        blaze_sync_data.finish().await;

        // 4. Remove the witnesses for spent notes more than 100 blocks old, since now there
        // is no risk of reorg
        self.wallet
            .transactions()
            .write()
            .await
            .clear_old_witnesses(latest_block);

        // 5. Remove expired mempool transactions, if any
        self.wallet
            .transactions()
            .write()
            .await
            .clear_expired_mempool(latest_block);

        // 6. Set the heighest verified tree
        if highest_tree.is_some() {
            *self.wallet.verified_tree.write().await = highest_tree;
        }

        Ok(object! {
            "result" => "success",
            "latest_block" => latest_block,
            "total_blocks_synced" => start_block - end_block + 1,
        })
    }

    pub async fn do_shield(&self, address: Option<String>) -> Result<String, String> {
        let fee = u64::from(DEFAULT_FEE);
        let tbal = self.wallet.tbalance(None).await;

        // Make sure there is a balance, and it is greated than the amount
        if tbal <= fee {
            return Err(format!(
                "Not enough transparent balance to shield. Have {} zats, need more than {} zats to cover tx fee",
                tbal, fee
            ));
        }

        let addr = address
            .or({
                let keys = self.wallet.keys();
                let readlocked_keys = keys.read().await;
                UnifiedAddress::from_receivers(
                    readlocked_keys
                        .get_all_orchard_keys_of_type::<OrchardSpendingKey>()
                        .iter()
                        .map(|orchard_spend_key| {
                            OrchardFullViewingKey::from(orchard_spend_key)
                                .address_at(0u32, orchard::keys::Scope::External)
                        })
                        .next(),
                    readlocked_keys
                        .zkeys
                        .iter()
                        .filter(|sapling_key| sapling_key.extsk.is_some())
                        .map(|sapling_key| sapling_key.zaddress.clone())
                        .next(),
                    None,
                )
                .map(|ua| ua.encode(&self.config.chain))
            })
            .ok_or(String::from(
                "No sapling or orchard spend authority in wallet. \n
                    please generate a shielded address and try again, or supply a destination address",
            ))?;

        let result = {
            let _lock = self.sync_lock.lock().await;
            let (sapling_output, sapling_spend) = self.read_sapling_params()?;

            let prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);

            self.wallet
                .send_to_address(
                    prover,
                    true,
                    true,
                    vec![(&addr, tbal - fee, None)],
                    |transaction_bytes| {
                        GrpcConnector::send_transaction(self.get_server_uri(), transaction_bytes)
                    },
                )
                .await
        };

        result.map(|(transaction_id, _)| transaction_id)
    }

    //TODO: Add migrate_sapling_to_orchard argument
    pub async fn do_send(&self, addrs: Vec<(&str, u64, Option<String>)>) -> Result<String, String> {
        // First, get the concensus branch ID
        info!("Creating transaction");

        let result = {
            let _lock = self.sync_lock.lock().await;
            let (sapling_output, sapling_spend) = self.read_sapling_params()?;

            let prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);

            self.wallet
                .send_to_address(prover, false, false, addrs, |transaction_bytes| {
                    GrpcConnector::send_transaction(self.get_server_uri(), transaction_bytes)
                })
                .await
        };

        result.map(|(transaction_id, _)| transaction_id)
    }
}

#[cfg(test)]
pub mod tests;

#[cfg(test)]
pub(crate) mod test_server;

#[cfg(test)]
pub(crate) mod testmocks;
