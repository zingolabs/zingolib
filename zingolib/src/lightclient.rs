//! TODO: Add Mod Description Here!

use json::{array, object, JsonValue};
use log::{debug, error};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use zcash_client_backend::encoding::{decode_payment_address, encode_payment_address};
use zcash_primitives::{
    consensus::NetworkConstants,
    memo::{Memo, MemoBytes},
};

use crate::config::ZingoConfig;

use crate::{
    blaze::syncdata::BlazeSyncData,
    wallet::{keys::unified::ReceiverSelection, message::Message, LightWallet, SendProgress},
};

use crate::data::proposal::ZingoProposal;

/// TODO: Add Doc Comment Here!
#[derive(Clone, Debug, Default)]
pub struct SyncResult {
    /// TODO: Add Doc Comment Here!
    pub success: bool,
    /// TODO: Add Doc Comment Here!
    pub latest_block: u64,
    /// TODO: Add Doc Comment Here!
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

/// TODO: Add Doc Comment Here!
#[derive(Clone, Debug, Default)]
pub struct WalletStatus {
    /// TODO: Add Doc Comment Here!
    pub is_syncing: bool,
    /// TODO: Add Doc Comment Here!
    pub total_blocks: u64,
    /// TODO: Add Doc Comment Here!
    pub synced_blocks: u64,
}

impl WalletStatus {
    /// TODO: Add Doc Comment Here!
    pub fn new() -> Self {
        WalletStatus {
            is_syncing: false,
            total_blocks: 0,
            synced_blocks: 0,
        }
    }
}

/// TODO: Add Doc Comment Here!
#[derive(Debug, Clone)]
pub struct LightWalletSendProgress {
    /// TODO: Add Doc Comment Here!
    pub progress: SendProgress,
    /// TODO: Add Doc Comment Here!
    pub interrupt_sync: bool,
}

impl LightWalletSendProgress {
    /// TODO: Add Doc Comment Here!
    pub fn to_json(&self) -> JsonValue {
        let last_result = self.progress.last_result.clone();
        let txids: Option<String> = last_result
            .clone()
            .and_then(|result| result.ok().map(|json_result| json_result.to_string()));
        let error: Option<String> = last_result.and_then(|result| result.err());
        object! {
            "id" => self.progress.id,
            "sending" => self.progress.is_send_in_progress,
            "progress" => self.progress.progress,
            "total" => self.progress.total,
            "txids" => txids,
            "error" => error,
            "sync_interrupt" => self.interrupt_sync
        }
    }
}

/// TODO: Add Doc Comment Here!
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PoolBalances {
    /// TODO: Add Doc Comment Here!
    pub sapling_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub verified_sapling_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub spendable_sapling_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub unverified_sapling_balance: Option<u64>,

    /// TODO: Add Doc Comment Here!
    pub orchard_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub verified_orchard_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub unverified_orchard_balance: Option<u64>,
    /// TODO: Add Doc Comment Here!
    pub spendable_orchard_balance: Option<u64>,

    /// TODO: Add Doc Comment Here!
    pub transparent_balance: Option<u64>,
}
fn format_option_zatoshis(ioz: &Option<u64>) -> String {
    ioz.map(|ioz_num| {
        if ioz_num == 0 {
            "0".to_string()
        } else {
            let mut digits = vec![];
            let mut remainder = ioz_num;
            while remainder != 0 {
                digits.push(remainder % 10);
                remainder /= 10;
            }
            let mut backwards = "".to_string();
            for (i, digit) in digits.iter().enumerate() {
                if i % 8 == 4 {
                    backwards.push('_');
                }
                if let Some(ch) = char::from_digit(*digit as u32, 10) {
                    backwards.push(ch);
                }
                if i == 7 {
                    backwards.push('.');
                }
            }
            backwards.chars().rev().collect::<String>()
        }
    })
    .unwrap_or("null".to_string())
}
impl std::fmt::Display for PoolBalances {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[
    sapling_balance: {}
    verified_sapling_balance: {}
    spendable_sapling_balance: {}
    unverified_sapling_balance: {}

    orchard_balance: {}
    verified_orchard_balance: {}
    spendable_orchard_balance: {}
    unverified_orchard_balance: {}

    transparent_balance: {}
]",
            format_option_zatoshis(&self.sapling_balance),
            format_option_zatoshis(&self.verified_sapling_balance),
            format_option_zatoshis(&self.spendable_sapling_balance),
            format_option_zatoshis(&self.unverified_sapling_balance),
            format_option_zatoshis(&self.orchard_balance),
            format_option_zatoshis(&self.verified_orchard_balance),
            format_option_zatoshis(&self.spendable_orchard_balance),
            format_option_zatoshis(&self.unverified_orchard_balance),
            format_option_zatoshis(&self.transparent_balance),
        )
    }
}

/// TODO: Add Doc Comment Here!
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AccountBackupInfo {
    /// TODO: Add Doc Comment Here!
    #[serde(rename = "seed")]
    pub seed_phrase: String,
    /// TODO: Add Doc Comment Here!
    pub birthday: u64,
    /// TODO: Add Doc Comment Here!
    pub account_index: u32,
}

#[derive(Default)]
struct ZingoSaveBuffer {
    pub buffer: Arc<RwLock<Vec<u8>>>,
}

impl ZingoSaveBuffer {
    fn new(buffer: Vec<u8>) -> Self {
        ZingoSaveBuffer {
            buffer: Arc::new(RwLock::new(buffer)),
        }
    }
}

/// Balances that may be presented to a user in a wallet app.
/// The goal is to present a user-friendly and useful view of what the user has or can soon expect
/// *without* requiring the user to understand the details of the Zcash protocol.
///
/// Showing all these balances all the time may overwhelm the user with information.
/// A simpler view may present an overall balance as:
///
/// Name | Value
/// --- | ---
/// "Balance" | `spendable` - `minimum_fees` + `immature_change` + `immature_income`
/// "Incoming" | `incoming`
///
/// If dust is sent to the wallet, the simpler view's Incoming balance would include it,
/// only for it to evaporate when confirmed.
/// But incoming can always evaporate (e.g. a transaction expires before confirmation),
/// and the alternatives being to either hide that a transmission was made at all, or to include
/// the dust in other balances could be more misleading.
///
/// An app *could* choose to prominently warn the user if a significant proportion of the incoming balance is dust,
/// although this event seems very unlikely since it will cost the sender *more* than the amount the recipient is expecting
/// to 'fool' them into thinking they are receiving value.
/// The more likely scenario is that the sender is trying to send a small amount of value as a new user and doesn't realize
/// the value is too small to be useful.
/// A good Zcash wallet should prevent sending dust in the first place.
pub struct UserBalances {
    /// Available for immediate spending.
    /// Expected fees are *not* deducted from this value, but the app may do so by subtracting `minimum_fees`.
    /// `dust` is excluded from this value.
    ///
    /// For enhanced privacy, the minimum number of required confirmations to spend a note is usually greater than one.
    pub spendable: u64,

    /// The sum of the change notes that have insufficient confirmations to be spent.
    pub immature_change: u64,

    /// The minimum fees that can be expected to spend all `spendable + immature_change` funds in the wallet.
    /// This fee assumes all funds will be sent to a single note.
    ///
    /// Balances described by other fields in this struct are not included because they are not confirmed,
    /// they may amount to dust, or because as `immature_income` funds they may require shielding which has a cost
    /// and can change the amount of fees required to spend them (e.g. 3 UTXOs shielded together become only 1 note).
    pub minimum_fees: u64,

    /// The sum of non-change notes with a non-zero confirmation count that is less than the minimum required for spending.
    /// `dust` is excluded from this value.
    /// All UTXOs are considered immature if the policy applies that requires all funds to be shielded before spending.
    ///
    /// As funds mature, this may not be the exact amount added to `spendable`, since the process of maturing
    /// may require shielding, which has a cost.
    pub immature_income: u64,

    /// The sum of all *confirmed* UTXOs and notes that are worth less than the fee to spend them,
    /// making them essentially inaccessible.
    pub dust: u64,

    /// The sum of all *pending* UTXOs and notes that are not change.
    /// This value includes any applicable `incoming_dust`.
    pub incoming: u64,

    /// The sum of all *pending* UTXOs and notes that are not change and are each counted as dust.
    pub incoming_dust: u64,
}

/// The LightClient connects one LightWallet to one lightwalletd server via gRPC.
///  1. initialization of stored state
///      * from seed
///      * from keys
///      * from wallets
///      * from a fresh start with reasonable defaults
///  2. synchronization of the client with the state of the blockchain via a gRPC server
///      *
pub struct LightClient {
    // / the LightClient connects to one server.
    // pub(crate) server_uri: Arc<RwLock<Uri>>,
    pub(crate) config: ZingoConfig,
    /// TODO: Add Doc Comment Here!
    pub wallet: LightWallet,

    mempool_monitor: std::sync::RwLock<Option<std::thread::JoinHandle<()>>>,

    sync_lock: Mutex<()>,

    bsync_data: Arc<RwLock<BlazeSyncData>>,
    interrupt_sync: Arc<RwLock<bool>>,

    latest_proposal: Arc<RwLock<Option<ZingoProposal>>>,

    save_buffer: ZingoSaveBuffer,
}

/// all the wonderfully intertwined ways to conjure a LightClient
pub mod instantiation {
    use log::debug;
    use std::{
        io::{self, Error, ErrorKind},
        sync::Arc,
    };
    use tokio::{
        runtime::Runtime,
        sync::{Mutex, RwLock},
    };

    use crate::config::ZingoConfig;

    use super::{LightClient, ZingoSaveBuffer};
    use crate::{
        blaze::syncdata::BlazeSyncData,
        wallet::{LightWallet, WalletBase},
    };

    impl LightClient {
        // toDo rework ZingoConfig.

        /// This is the fundamental invocation of a LightClient. It lives in an asyncronous runtime.
        pub async fn create_from_wallet_async(wallet: LightWallet) -> io::Result<Self> {
            let mut buffer: Vec<u8> = vec![];
            wallet.write(&mut buffer).await?;
            let config = wallet.transaction_context.config.clone();
            Ok(LightClient {
                wallet,
                config: config.clone(),
                mempool_monitor: std::sync::RwLock::new(None),
                sync_lock: Mutex::new(()),
                bsync_data: Arc::new(RwLock::new(BlazeSyncData::new())),
                interrupt_sync: Arc::new(RwLock::new(false)),
                latest_proposal: Arc::new(RwLock::new(None)),
                save_buffer: ZingoSaveBuffer::new(buffer),
            })
        }

        /// The wallet this fn associates with the lightclient is specifically derived from
        /// a spend authority.
        /// this pubfn is consumed in zingocli, zingo-mobile, and ZingoPC
        pub fn create_from_wallet_base(
            wallet_base: WalletBase,
            config: &ZingoConfig,
            birthday: u64,
            overwrite: bool,
        ) -> io::Result<Self> {
            Runtime::new().unwrap().block_on(async move {
                LightClient::create_from_wallet_base_async(wallet_base, config, birthday, overwrite)
                    .await
            })
        }

        /// The wallet this fn associates with the lightclient is specifically derived from
        /// a spend authority.
        pub async fn create_from_wallet_base_async(
            wallet_base: WalletBase,
            config: &ZingoConfig,
            birthday: u64,
            overwrite: bool,
        ) -> io::Result<Self> {
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            {
                if !overwrite && config.wallet_path_exists() {
                    return Err(Error::new(
                    ErrorKind::AlreadyExists,
                    format!(
                        "Cannot create a new wallet from seed, because a wallet already exists at:\n{:?}",
                        config.get_wallet_path().as_os_str()
                    ),
                ));
                }
            }
            let lightclient = LightClient::create_from_wallet_async(LightWallet::new(
                config.clone(),
                wallet_base,
                birthday,
            )?)
            .await?;

            lightclient.set_wallet_initial_state(birthday).await;
            lightclient
                .save_internal_rust()
                .await
                .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;

            debug!("Created new wallet!");

            Ok(lightclient)
        }

        /// TODO: Add Doc Comment Here!
        pub async fn create_unconnected(
            config: &ZingoConfig,
            wallet_base: WalletBase,
            height: u64,
        ) -> io::Result<Self> {
            let lightclient = LightClient::create_from_wallet_async(LightWallet::new(
                config.clone(),
                wallet_base,
                height,
            )?)
            .await?;
            Ok(lightclient)
        }

        fn create_with_new_wallet(config: &ZingoConfig, height: u64) -> io::Result<Self> {
            Runtime::new().unwrap().block_on(async move {
                let l = LightClient::create_unconnected(config, WalletBase::FreshEntropy, height)
                    .await?;
                l.set_wallet_initial_state(height).await;

                debug!("Created new wallet with a new seed!");
                debug!("Created LightClient to {}", &config.get_lightwalletd_uri());

                // Save
                l.save_internal_rust()
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
                if config.wallet_path_exists() {
                    return Err(Error::new(
                        ErrorKind::AlreadyExists,
                        "Cannot create a new wallet from seed, because a wallet already exists",
                    ));
                }
            }

            Self::create_with_new_wallet(config, latest_block)
        }
    }
}

pub mod save;

pub mod read;

pub mod describe;

pub mod sync;

pub mod send;

pub mod propose;

// other functions
impl LightClient {
    /// TODO: Add Doc Comment Here!
    pub async fn clear_state(&self) {
        // First, clear the state from the wallet
        self.wallet.clear_all().await;

        // Then set the initial block
        let birthday = self.wallet.get_birthday().await;
        self.set_wallet_initial_state(birthday).await;
        debug!("Cleared wallet state, with birthday at {}", birthday);
    }

    /// TODO: Add Doc Comment Here!
    pub fn config(&self) -> &ZingoConfig {
        &self.config
    }

    /// TODO: Add Doc Comment Here!
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

    /// TODO: Add Doc Comment Here!
    pub fn do_encrypt_message(&self, recipient_address_str: String, memo: Memo) -> JsonValue {
        let to = match decode_payment_address(
            self.config.chain.hrp_sapling_payment_address(),
            &recipient_address_str,
        ) {
            Ok(to) => to,
            _ => {
                return object! {"error" => format!("Couldn't parse {} as a z-address", recipient_address_str) };
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
            .new_address(desired_receivers, false)?;

        // self.save_internal_rust().await?;

        Ok(array![new_address.encode(&self.config.chain)])
    }

    /// TODO: Add Doc Comment Here!
    pub fn set_server(&self, server: http::Uri) {
        *self.config.lightwalletd_uri.write().unwrap() = server
    }

    /// TODO: Add Doc Comment Here!
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

    /// This function sorts notes into
    /// unspent
    /// spend_is_pending
    /// spend_is_confirmed
    fn unspent_pending_spent(
        &self,
        note: JsonValue,
        unspent: &mut Vec<JsonValue>,
        spend_is_pending: &mut Vec<JsonValue>,
        spend_is_confirmed: &mut Vec<JsonValue>,
    ) {
        if note["spent"].is_null() && note["pending_spent"].is_null() {
            unspent.push(note);
        } else if !note["spent"].is_null() {
            spend_is_confirmed.push(note);
        } else {
            spend_is_pending.push(note);
        }
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
mod tests {
    use crate::config::{ChainType, RegtestNetwork, ZingoConfig};
    use crate::testvectors::seeds::CHIMNEY_BETTER_SEED;
    use tokio::runtime::Runtime;

    use crate::{lightclient::LightClient, wallet::WalletBase};

    #[test]
    fn new_wallet_from_phrase() {
        let temp_dir = tempfile::Builder::new().prefix("test").tempdir().unwrap();
        let data_dir = temp_dir
            .into_path()
            .canonicalize()
            .expect("This path is available.");

        let wallet_name = data_dir.join("zingo-wallet.dat");
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let config = ZingoConfig::build(ChainType::Regtest(regtest_network))
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
                "zregtestsapling1etnl5s47cqves0g5hk2dx5824rme4xv4aeauwzp4d6ys3qxykt5sw5rnaqh9syxry8vgxr7x3x4"
                    .to_string(),
                addresses[0]["receivers"]["sapling"]
            );
            assert_eq!(
                "tmYd5GP6JxUxTUcz98NLPumEotvaMPaXytz".to_string(),
                addresses[0]["receivers"]["transparent"]
            );
        });
    }
}

#[cfg(feature = "lightclient-deprecated")]
mod deprecated;
