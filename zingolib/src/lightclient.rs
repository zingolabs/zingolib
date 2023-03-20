use crate::{
    blaze::{
        block_witness_data::BlockAndWitnessData, fetch_compact_blocks::FetchCompactBlocks,
        fetch_full_transaction::TransactionContext,
        fetch_taddr_transactions::FetchTaddrTransactions, sync_status::BatchSyncStatus,
        syncdata::BlazeSyncData, trial_decryptions::TrialDecryptions, update_notes::UpdateNotes,
    },
    compact_formats::RawTransaction,
    grpc_connector::GrpcConnector,
    wallet::{
        data::TransactionMetadata,
        keys::{
            address_from_pubkeyhash,
            unified::{ReceiverSelection, WalletCapability},
        },
        message::Message,
        now,
        traits::{DomainWalletExt, ReceivedNoteAndMetadata, Recipient},
        LightWallet, WalletBase,
    },
};
use futures::future::join_all;
use json::{array, object, JsonValue};
use log::{debug, error, warn};
use orchard::note_encryption::OrchardDomain;
use std::{
    cmp,
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
use zcash_note_encryption::Domain;

use zcash_client_backend::{
    address::RecipientAddress,
    encoding::{decode_payment_address, encode_payment_address},
};
use zcash_primitives::{
    block::BlockHash,
    consensus::{BlockHeight, BranchId, Parameters},
    memo::{Memo, MemoBytes},
    sapling::note_encryption::SaplingDomain,
    transaction::{components::amount::DEFAULT_FEE, Transaction},
};
use zcash_proofs::prover::LocalTxProver;
use zingoconfig::{ChainType, ZingoConfig, MAX_REORG};

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
    interrupt_sync: Arc<RwLock<bool>>,
}

use serde_json::Value;

enum PriceFetchError {
    ReqwestError(String),
    NotJson,
    NoElements,
    PriceReprError(PriceReprError),
    NanValue,
}
impl PriceFetchError {
    pub(crate) fn to_string(&self) -> String {
        use PriceFetchError::*;
        match &*self {
            ReqwestError(e) => format!("ReqwestError: {}", e),
            NotJson => "NotJson".to_string(),
            NoElements => "NoElements".to_string(),
            PriceReprError(e) => format!("PriceReprError: {}", e),
            NanValue => "NanValue".to_string(),
        }
    }
}
impl std::fmt::Display for PriceFetchError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.write_str(self.to_string().as_str())
    }
}
enum PriceReprError {
    NoValue,
    NoAsStrValue,
    NotParseable,
}
impl PriceReprError {
    fn to_string(&self) -> String {
        use PriceReprError::*;
        match &*self {
            NoValue => "NoValue".to_string(),
            NoAsStrValue => "NoAsStrValue".to_string(),
            NotParseable => "NotParseable".to_string(),
        }
    }
}
impl std::fmt::Display for PriceReprError {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.write_str(self.to_string().as_str())
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
    let mut trades: Vec<f64> = match elements.into_iter().map(repr_price_as_f64).collect() {
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
impl LightClient {
    /// Method to create a test-only version of the LightClient
    pub async fn test_new(
        config: &ZingoConfig,
        wallet_base: WalletBase,
        height: u64,
    ) -> io::Result<Self> {
        let l = LightClient::create_unconnected(config, wallet_base, height)
            .expect("Unconnected client creation failed!");
        l.set_wallet_initial_state(height).await;

        debug!("Created new wallet!");
        debug!("Created LightClient to {}", &config.get_server_uri());
        Ok(l)
    }

    //TODO: Add migrate_sapling_to_orchard argument
    pub async fn test_do_send(
        &self,
        addrs: Vec<(&str, u64, Option<String>)>,
    ) -> Result<String, String> {
        let transaction_submission_height = self.get_submission_height().await;
        // First, get the concensus branch ID
        debug!("Creating transaction");

        let result = {
            let _lock = self.sync_lock.lock().await;
            let prover = crate::blaze::test_utils::FakeTransactionProver {};

            self.wallet
                .send_to_address(
                    prover,
                    false,
                    false,
                    addrs,
                    transaction_submission_height,
                    |transaction_bytes| {
                        GrpcConnector::send_transaction(self.get_server_uri(), transaction_bytes)
                    },
                )
                .await
        };

        result.map(|(transaction_id, _)| transaction_id)
    }

    pub async fn do_maybe_recent_txid(&self) -> JsonValue {
        object! {
            "last_txid" => self.wallet.transactions().read().await.get_some_txid_from_highest_wallet_block().map(|t| t.to_string())
        }
    }
}

#[cfg(feature = "integration_test")]
impl LightClient {
    pub fn create_with_wallet(wallet: LightWallet, config: ZingoConfig) -> Self {
        LightClient {
            wallet,
            config: config.clone(),
            mempool_monitor: std::sync::RwLock::new(None),
            sync_lock: Mutex::new(()),
            bsync_data: Arc::new(RwLock::new(BlazeSyncData::new(&config))),
            interrupt_sync: Arc::new(RwLock::new(false)),
        }
    }
    pub fn extract_unified_capability(&self) -> Arc<RwLock<WalletCapability>> {
        self.wallet.wallet_capability()
    }
}
impl LightClient {
    /// The wallet this fn associates with the lightclient is specifically derived from
    /// a spend authority.
    pub async fn new_from_wallet_base_async(
        wallet_base: WalletBase,
        config: &ZingoConfig,
        birthday: u64,
        overwrite: bool,
    ) -> io::Result<Self> {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            if !overwrite && config.wallet_exists() {
                return Err(Error::new(
                    ErrorKind::AlreadyExists,
                    format!(
                        "Cannot create a new wallet from seed, because a wallet already exists at:\n{:?}",
                        config.get_wallet_path().as_os_str()
                    ),
                ));
            }
        }
        let lightclient = LightClient {
            wallet: LightWallet::new(config.clone(), wallet_base, birthday)?,
            config: config.clone(),
            mempool_monitor: std::sync::RwLock::new(None),
            sync_lock: Mutex::new(()),
            bsync_data: Arc::new(RwLock::new(BlazeSyncData::new(&config))),
            interrupt_sync: Arc::new(RwLock::new(false)),
        };

        lightclient.set_wallet_initial_state(birthday).await;
        lightclient
            .do_save()
            .await
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;

        debug!("Created new wallet!");

        Ok(lightclient)
    }

    /// The wallet this fn associates with the lightclient is specifically derived from
    /// a spend authority.
    pub fn new_from_wallet_base(
        wallet_base: WalletBase,
        config: &ZingoConfig,
        birthday: u64,
        overwrite: bool,
    ) -> io::Result<Self> {
        Runtime::new().unwrap().block_on(async move {
            LightClient::new_from_wallet_base_async(wallet_base, config, birthday, overwrite).await
        })
    }

    pub fn create_unconnected(
        config: &ZingoConfig,
        wallet_base: WalletBase,
        height: u64,
    ) -> io::Result<Self> {
        Ok(LightClient {
            wallet: LightWallet::new(config.clone(), wallet_base, height)?,
            config: config.clone(),
            mempool_monitor: std::sync::RwLock::new(None),
            bsync_data: Arc::new(RwLock::new(BlazeSyncData::new(&config))),
            sync_lock: Mutex::new(()),
            interrupt_sync: Arc::new(RwLock::new(false)),
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

    pub async fn interrupt_sync_after_batch(&self, set_interrupt: bool) {
        *self.interrupt_sync.write().await = set_interrupt;
    }

    pub async fn get_initial_state(&self, height: u64) -> Option<(u64, String, String)> {
        if height <= self.config.sapling_activation_height() {
            return None;
        }

        debug!(
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
                debug!("Setting initial state to height {}, tree {}", height, tree);
                self.wallet
                    .set_initial_block(height, &hash.as_str(), &tree.as_str())
                    .await;
            }
            _ => {}
        };
    }

    fn new_wallet(config: &ZingoConfig, height: u64) -> io::Result<Self> {
        Runtime::new().unwrap().block_on(async move {
            let l = LightClient::create_unconnected(&config, WalletBase::FreshEntropy, height)?;
            l.set_wallet_initial_state(height).await;

            debug!("Created new wallet with a new seed!");
            debug!("Created LightClient to {}", &config.get_server_uri());

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
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            if config.wallet_exists() {
                return Err(Error::new(
                    ErrorKind::AlreadyExists,
                    "Cannot create a new wallet from seed, because a wallet already exists",
                ));
            }
        }

        Self::new_wallet(config, latest_block)
    }

    pub fn read_wallet_from_disk(config: &ZingoConfig) -> io::Result<Self> {
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
        LightClient::read_wallet_from_buffer(&config, BufReader::new(File::open(wallet_path)?))
    }

    /// This constructor depends on a wallet that's read from a buffer.
    /// It is used internally by read_from_disk, and directly called by
    /// zingo-mobile.
    pub fn read_wallet_from_buffer<R: Read>(
        config: &ZingoConfig,
        mut reader: R,
    ) -> io::Result<Self> {
        Runtime::new().unwrap().block_on(async move {
            let wallet = LightWallet::read_internal(&mut reader, config).await?;

            let lc = LightClient {
                wallet,
                config: config.clone(),
                mempool_monitor: std::sync::RwLock::new(None),
                sync_lock: Mutex::new(()),
                bsync_data: Arc::new(RwLock::new(BlazeSyncData::new(&config))),
                interrupt_sync: Arc::new(RwLock::new(false)),
            };

            debug!(
                "Read wallet with birthday {}",
                lc.wallet.get_birthday().await
            );
            debug!("Created LightClient to {}", &config.get_server_uri());

            Ok(lc)
        })
    }
    pub fn init_logging() -> io::Result<()> {
        // Configure logging first.
        tracing_subscriber::fmt::init();

        Ok(())
    }

    pub async fn do_addresses(&self) -> JsonValue {
        let mut objectified_addresses = Vec::new();
        for address in self.wallet.wallet_capability().read().await.addresses() {
            let encoded_ua = address.encode(&self.config.chain);
            objectified_addresses.push(object! {
            "address" => encoded_ua,
            "receivers" => object!(
                "transparent" => address_from_pubkeyhash(&self.config, address.transparent().cloned()),
                "sapling" => address.sapling().map(|z_addr| encode_payment_address(self.config.chain.hrp_sapling_payment_address(), z_addr)),
                "orchard_exists" => address.orchard().is_some(),
                )
            })
        }
        JsonValue::Array(objectified_addresses)
    }

    pub async fn do_balance(&self) -> JsonValue {
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
        }
    }

    pub async fn do_save(&self) -> Result<(), String> {
        #[cfg(any(target_os = "ios", target_os = "android"))]
        // On mobile platforms, disable the save, because the saves will be handled by the native layer, and not in rust
        {
            log::debug!("do_save entered");

            // On ios and android just return OK
            Ok(())
        }

        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            log::debug!("do_save entered");
            log::debug!("target_os is not ios or android");
            // Prevent any overlapping syncs during save, and don't save in the middle of a sync
            //let _lock = self.sync_lock.lock().await;

            let mut wallet_bytes = vec![];
            match self.wallet.write(&mut wallet_bytes).await {
                Ok(_) => {
                    let mut file = File::create(self.config.get_wallet_path()).unwrap();
                    file.write_all(&wallet_bytes)
                        .map_err(|e| format!("{}", e))?;
                    log::debug!("In the guts of a successful save!");
                    Ok(())
                }
                Err(e) => {
                    let err = format!("ERR: {}", e);
                    error!("{}", err);
                    log::debug!("SAVE FAIL ON WALLET WRITE LOCK!");
                    Err(e.to_string())
                }
            }
        }
    }

    pub fn do_save_to_buffer_sync(&self) -> Result<Vec<u8>, String> {
        Runtime::new()
            .unwrap()
            .block_on(async move { self.do_save_to_buffer().await })
    }

    pub async fn do_save_to_buffer(&self) -> Result<Vec<u8>, String> {
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

    pub(crate) async fn get_sync_interrupt(&self) -> bool {
        *self.interrupt_sync.read().await
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
            "sync_interrupt" => *self.interrupt_sync.read().await
        })
    }

    pub fn do_seed_phrase_sync(&self) -> Result<JsonValue, &str> {
        Runtime::new()
            .unwrap()
            .block_on(async move { self.do_seed_phrase().await })
    }

    pub async fn do_seed_phrase(&self) -> Result<JsonValue, &str> {
        match self.wallet.mnemonic() {
            Some(m) => Ok(object! {
                "seed"     => m.to_string(),
                "birthday" => self.wallet.get_birthday().await
            }),
            None => Err("This wallet is watch-only."),
        }
    }

    /// Return a list of all notes, spent and unspent
    pub async fn do_list_notes(&self, all_notes: bool) -> JsonValue {
        let mut unspent_sapling_notes: Vec<JsonValue> = vec![];
        let mut spent_sapling_notes: Vec<JsonValue> = vec![];
        let mut pending_sapling_notes: Vec<JsonValue> = vec![];

        let anchor_height = BlockHeight::from_u32(self.wallet.get_anchor_height().await);
        let unified_spend_capability_arc = self.wallet.wallet_capability();
        let unified_spend_capability = &unified_spend_capability_arc.read().await;

        {
            // Collect Sapling notes
            self.wallet.transaction_context.transaction_metadata_set.read().await.current.iter()
                .flat_map( |(transaction_id, transaction_metadata)| {
                    transaction_metadata.sapling_notes.iter().filter_map(move |note_metadata|
                        if !all_notes && note_metadata.spent.is_some() {
                            None
                        } else {
                            let address = LightWallet::note_address::<SaplingDomain<ChainType>>(&self.config.chain, note_metadata, &unified_spend_capability);
                            let spendable = transaction_metadata.block_height <= anchor_height && note_metadata.spent.is_none() && note_metadata.unconfirmed_spent.is_none();

                            let created_block:u32 = transaction_metadata.block_height.into();
                            Some(object!{
                                "created_in_block"   => created_block,
                                "datetime"           => transaction_metadata.datetime,
                                "created_in_txid"    => format!("{}", transaction_id.clone()),
                                "value"              => note_metadata.note.value,
                                "unconfirmed"        => transaction_metadata.unconfirmed,
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
                    if note["spent"].is_null() && note["unconfirmed_spent"].is_null() {
                        unspent_sapling_notes.push(note);
                    } else if !note["spent"].is_null() {
                        spent_sapling_notes.push(note);
                    } else {
                        pending_sapling_notes.push(note);
                    }
                });
        }

        let mut unspent_orchard_notes: Vec<JsonValue> = vec![];
        let mut spent_orchard_notes: Vec<JsonValue> = vec![];
        let mut pending_orchard_notes: Vec<JsonValue> = vec![];

        let unified_spend_auth_arc = self.wallet.wallet_capability();
        let unified_spend_auth = &unified_spend_auth_arc.read().await;
        {
            self.wallet.transaction_context.transaction_metadata_set.read().await.current.iter()
                .flat_map( |(transaction_id, transaction_metadata)| {
                    transaction_metadata.orchard_notes.iter().filter_map(move |orch_note_metadata|
                        if !all_notes && orch_note_metadata.is_spent() {
                            None
                        } else {
                            let address = LightWallet::note_address::<OrchardDomain>(&self.config.chain, orch_note_metadata, &unified_spend_auth);
                            let spendable = transaction_metadata.block_height <= anchor_height && orch_note_metadata.spent.is_none() && orch_note_metadata.unconfirmed_spent.is_none();

                            let created_block:u32 = transaction_metadata.block_height.into();
                            Some(object!{
                                "created_in_block"   => created_block,
                                "datetime"           => transaction_metadata.datetime,
                                "created_in_txid"    => format!("{}", transaction_id),
                                "value"              => orch_note_metadata.note.value().inner(),
                                "unconfirmed"        => transaction_metadata.unconfirmed,
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
                    if note["spent"].is_null() && note["unconfirmed_spent"].is_null() {
                        unspent_orchard_notes.push(note);
                    } else if !note["spent"].is_null() {
                        spent_orchard_notes.push(note);
                    } else {
                        pending_orchard_notes.push(note);
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
                            let created_block:u32 = wtx.block_height.into();
                            let recipient = RecipientAddress::decode(&self.config.chain, &utxo.address);
                            let taddr = match recipient {
                            Some(RecipientAddress::Transparent(taddr)) => taddr,
                                _otherwise => panic!("Read invalid taddr from wallet-local Utxo, this should be impossible"),
                            };

                            Some(object!{
                                "created_in_block"   => created_block,
                                "datetime"           => wtx.datetime,
                                "created_in_txid"    => format!("{}", transaction_id),
                                "value"              => utxo.value,
                                "scriptkey"          => hex::encode(utxo.script.clone()),
                                "is_change"          => false, // TODO: Identify notes as change if we send change to our own taddrs
                                "address"            => unified_spend_auth.get_ua_from_contained_transparent_receiver(&taddr).map(|ua| ua.encode(&self.config.chain)),
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

        unspent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_sapling_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        unspent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_orchard_notes.sort_by_key(|note| note["created_in_block"].as_u64());
        unspent_utxos.sort_by_key(|note| note["created_in_block"].as_u64());
        pending_utxos.sort_by_key(|note| note["created_in_block"].as_u64());
        spent_utxos.sort_by_key(|note| note["created_in_block"].as_u64());

        let mut res = object! {
            "unspent_sapling_notes" => unspent_sapling_notes,
            "pending_sapling_notes" => pending_sapling_notes,
            "unspent_orchard_notes" => unspent_orchard_notes,
            "pending_orchard_notes" => pending_orchard_notes,
            "utxos"         => unspent_utxos,
            "pending_utxos" => pending_utxos,
        };

        if all_notes {
            res["spent_sapling_notes"] = JsonValue::Array(spent_sapling_notes);
            res["spent_orchard_notes"] = JsonValue::Array(spent_orchard_notes);
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
            Ok(m) => {
                let memo_bytes: MemoBytes = m.memo.clone().into();
                object! {
                    "to" => encode_payment_address(self.config.hrp_sapling_address(), &m.to),
                    "memo" => LightWallet::memo_str(Some(m.memo)),
                    "memohex" => hex::encode(memo_bytes.as_slice())
                }
            }
            Err(_) => object! { "error" => "Couldn't decrypt with any of the wallet's keys"},
        }
    }

    pub async fn do_list_transactions(&self, include_memo_hex: bool) -> JsonValue {
        // Create a list of TransactionItems from wallet transactions
        let unified_spend_capability_arc = self.wallet.wallet_capability();
        let unified_spend_capability = &unified_spend_capability_arc.read().await;
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
                        + wallet_transaction
                        .orchard_notes
                        .iter()
                        .filter(|nd| nd.is_change)
                        .map(|nd| nd.note.value().inner())
                        .sum::<u64>()
                        + wallet_transaction.utxos.iter().map(|ut| ut.value).sum::<u64>();

                    // Collect outgoing metadata
                    let outgoing_json = wallet_transaction
                        .outgoing_metadata
                        .iter()
                        .map(|om| {
                            let mut o = object! {
                                "address" => om.ua.clone().unwrap_or(om.to_address.clone()),
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

                    let block_height: u32 = wallet_transaction.block_height.into();
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

                // For each note that is not a change, add a transaction.
                transactions.extend(self.add_wallet_notes_in_transaction_to_list(&wallet_transaction, &include_memo_hex, &**unified_spend_capability));

                // Get the total transparent received
                let total_transparent_received = wallet_transaction.utxos.iter().map(|u| u.value).sum::<u64>();
                let amount = total_transparent_received as i64 - wallet_transaction.total_transparent_value_spent as i64;
                let address = wallet_transaction.utxos.iter().map(|u| u.address.clone()).collect::<Vec<String>>().join(",");
                if total_transparent_received > wallet_transaction.total_transparent_value_spent {
                    if let Some(transaction) = transactions.iter_mut().find(|transaction| transaction["txid"] == wallet_transaction.txid.to_string()) {
                        let old_address = transaction.remove("address");
                        transaction.insert("address", match old_address {
                            JsonValue::String(addr) => [addr, address].join(","),
                            _ => address
                        }).unwrap();
                    } else {
                    // Create an input transaction for the transparent value as well.
                    let block_height: u32 = wallet_transaction.block_height.into();
                    transactions.push(object! {
                        "block_height" => block_height,
                        "unconfirmed" => wallet_transaction.unconfirmed,
                        "datetime"     => wallet_transaction.datetime,
                        "txid"         => format!("{}", wallet_transaction.txid),
                        "amount"       => amount,
                        "zec_price"    => wallet_transaction.zec_price.map(|p| (p * 100.0).round() / 100.0),
                        "address"      => address,
                        "memo"         => None::<String>
                    })}
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

    fn add_wallet_notes_in_transaction_to_list<'a, 'b, 'c>(
        &'a self,
        transaction_metadata: &'b TransactionMetadata,
        include_memo_hex: &'b bool,
        unified_spend_auth: &'c WalletCapability,
    ) -> impl Iterator<Item = JsonValue> + 'b
    where
        'a: 'b,
        'c: 'b,
    {
        self.add_wallet_notes_in_transaction_to_list_inner::<'a, 'b, 'c, SaplingDomain<ChainType>>(
            transaction_metadata,
            include_memo_hex,
            unified_spend_auth,
        )
        .chain(
            self.add_wallet_notes_in_transaction_to_list_inner::<'a, 'b, 'c, OrchardDomain>(
                transaction_metadata,
                include_memo_hex,
                unified_spend_auth,
            ),
        )
    }

    fn add_wallet_notes_in_transaction_to_list_inner<'a, 'b, 'c, D>(
        &'a self,
        transaction_metadata: &'b TransactionMetadata,
        include_memo_hex: &'b bool,
        unified_spend_auth: &'c WalletCapability,
    ) -> impl Iterator<Item = JsonValue> + 'b
    where
        'a: 'b,
        'c: 'b,
        D: DomainWalletExt,
        D::WalletNote: 'b,
        <D as Domain>::Recipient: Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        D::WalletNote::transaction_metadata_notes(&transaction_metadata).iter().filter(|nd| !nd.is_change()).enumerate().map(|(i, nd)| {
                    let block_height: u32 = transaction_metadata.block_height.into();
                    let mut o = object! {
                        "block_height" => block_height,
                        "unconfirmed" => transaction_metadata.unconfirmed,
                        "datetime"     => transaction_metadata.datetime,
                        "position"     => i,
                        "txid"         => format!("{}", transaction_metadata.txid),
                        "amount"       => nd.value() as i64,
                        "zec_price"    => transaction_metadata.zec_price.map(|p| (p * 100.0).round() / 100.0),
                        "address"      => LightWallet::note_address::<D>(&self.config.chain, nd, unified_spend_auth),
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
        //TODO: Placeholder interface
        let desired_receivers = ReceiverSelection {
            sapling: addr_type.contains('z'),
            orchard: addr_type.contains('o'),
            transparent: addr_type.contains('t'),
        };

        let new_address = self
            .wallet
            .wallet_capability()
            .write()
            .await
            .new_address(desired_receivers)?;

        self.do_save().await?;

        Ok(array![new_address.encode(&self.config.chain)])
    }

    pub async fn clear_state(&self) {
        // First, clear the state from the wallet
        self.wallet.clear_all().await;

        // Then set the initial block
        let birthday = self.wallet.get_birthday().await;
        self.set_wallet_initial_state(birthday).await;
        debug!("Cleared wallet state, with birthday at {}", birthday);
    }

    pub async fn do_rescan(&self) -> Result<JsonValue, String> {
        debug!("Rescan starting");

        self.clear_state().await;

        // Then, do a sync, which will force a full rescan from the initial state
        let response = self.do_sync(true).await;

        if response.is_ok() {
            self.do_save().await?;
        }

        debug!("Rescan finished");

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

    pub async fn do_sync_status(&self) -> BatchSyncStatus {
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

        debug!("Mempool monitoring starting");

        let uri = lc.config.get_server_uri();
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

                            TransactionContext::new(
                                &config,
                                key.clone(),
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
                        //debug!("Monitoring mempool");
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
        // TODO:  We run on resource constrained systems, where a single thread of
        // execution often consumes most of the memory available, on other systems
        // we might parallelize sync.
        let lightclient_exclusion_lock = self.sync_lock.lock().await;

        // The top of the wallet
        let last_synced_height = self.wallet.last_synced_height().await;

        let latest_blockid = GrpcConnector::get_latest_block(self.config.get_server_uri()).await?;
        if latest_blockid.height < last_synced_height {
            let w = format!(
                "Server's latest block({}) is behind ours({})",
                latest_blockid.height, last_synced_height
            );
            warn!("{}", w);
            return Err(w);
        }

        if latest_blockid.height == last_synced_height {
            if !latest_blockid.hash.is_empty()
                && BlockHash::from_slice(&latest_blockid.hash).to_string()
                    != self.wallet.last_synced_hash().await
            {
                warn!("One block reorg at height {}", last_synced_height);
                // This is a one-block reorg, so pop the last block. Even if there are more blocks to reorg, this is enough
                // to trigger a sync, which will then reorg the remaining blocks
                BlockAndWitnessData::invalidate_block(
                    last_synced_height,
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
        let last_scanned_height = self.wallet.last_synced_height().await;
        let batch_size = 100;

        let mut latest_block_batches = vec![];
        let mut prev = last_scanned_height;
        while latest_block_batches.is_empty() || prev != latest_blockid.height {
            let batch = cmp::min(latest_blockid.height, prev + batch_size);
            prev = batch;
            latest_block_batches.push(batch);
        }

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
            res = self.sync_nth_batch(batch_latest_block, batch_num).await;
            if res.is_err() {
                return res;
            }
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
    ) -> Result<JsonValue, String> {
        // The top of the wallet
        let last_synced_height = self.wallet.last_synced_height().await;

        debug!(
            "Latest block is {}, wallet block is {}",
            start_block, last_synced_height
        );

        if last_synced_height == start_block {
            debug!("Already at latest block, not syncing");
            return Ok(object! { "result" => "success" });
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

        // Do Trial decryptions of all the sapling outputs, and pass on the successful ones to the update_notes processor
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
        let earliest_block = block_and_witness_handle.await.unwrap().unwrap();

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
        debug!("tree verification {}", verified);
        debug!("highest tree exists: {}", highest_tree.is_some());
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
            .clear_old_witnesses(start_block);

        // 5. Remove expired mempool transactions, if any
        self.wallet
            .transactions()
            .write()
            .await
            .clear_expired_mempool(start_block);

        // 6. Set the heighest verified tree
        if highest_tree.is_some() {
            *self.wallet.verified_tree.write().await = highest_tree;
        }

        debug!("About to run save after syncing {}th batch!", batch_num);

        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        self.do_save().await.unwrap();

        Ok(object! {
            "result" => "success",
            "latest_block" => start_block,
            "total_blocks_synced" => start_block - end_block + 1,
        })
    }

    pub async fn do_shield(&self, address: Option<String>) -> Result<String, String> {
        let transaction_submission_height = self.get_submission_height().await;
        let fee = u64::from(DEFAULT_FEE);
        let tbal = self.wallet.tbalance(None).await;

        // Make sure there is a balance, and it is greated than the amount
        if tbal <= fee {
            return Err(format!(
                "Not enough transparent balance to shield. Have {} zats, need more than {} zats to cover tx fee",
                tbal, fee
            ));
        }

        let addr = address.unwrap_or(
            self.wallet.wallet_capability().read().await.addresses()[0].encode(&self.config.chain),
        );

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
                    transaction_submission_height,
                    |transaction_bytes| {
                        GrpcConnector::send_transaction(self.get_server_uri(), transaction_bytes)
                    },
                )
                .await
        };

        result.map(|(transaction_id, _)| transaction_id)
    }

    async fn get_submission_height(&self) -> BlockHeight {
        BlockHeight::from_u32(
            GrpcConnector::get_latest_block(self.config.get_server_uri())
                .await
                .unwrap()
                .height as u32,
        ) + 1
    }
    //TODO: Add migrate_sapling_to_orchard argument
    pub async fn do_send(
        &self,
        address_amount_memo_tuples: Vec<(&str, u64, Option<String>)>,
    ) -> Result<String, String> {
        let transaction_submission_height = self.get_submission_height().await;
        // First, get the concensus branch ID
        debug!("Creating transaction");

        let result = {
            let _lock = self.sync_lock.lock().await;
            let (sapling_output, sapling_spend) = self.read_sapling_params()?;

            let prover = LocalTxProver::from_bytes(&sapling_spend, &sapling_output);

            self.wallet
                .send_to_address(
                    prover,
                    false,
                    false,
                    address_amount_memo_tuples,
                    transaction_submission_height,
                    |transaction_bytes| {
                        GrpcConnector::send_transaction(self.get_server_uri(), transaction_bytes)
                    },
                )
                .await
        };

        result.map(|(transaction_id, _)| transaction_id)
    }

    pub async fn do_wallet_last_scanned_height(&self) -> JsonValue {
        json::JsonValue::from(self.wallet.last_synced_height().await)
    }
}

#[cfg(test)]
pub mod tests;

#[cfg(test)]
pub(crate) mod test_server;

#[cfg(test)]
pub(crate) mod testmocks;
