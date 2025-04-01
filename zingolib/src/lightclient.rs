//! TODO: Add Mod Description Here!

use std::{
    fs::File,
    io::BufReader,
    sync::{
        atomic::{AtomicBool, AtomicU8},
        Arc,
    },
};

use json::{array, JsonValue};
use log::error;
use serde::Serialize;
use serde_json::Value;
use tokio::{sync::Mutex, task::JoinHandle};

use zcash_primitives::consensus::BlockHeight;

use pepper_sync::{error::SyncError, sync::SyncResult, wallet::SyncMode};

use crate::{
    config::ZingoConfig,
    data::proposal::ZingoProposal,
    wallet::{error::WalletError, keys::unified::ReceiverSelection, LightWallet, WalletBase},
};
use error::LightClientError;

pub mod describe;
pub mod error;
pub mod propose;
pub mod save;
pub mod send;
pub mod sync;

/// TODO: Add Doc Comment Here!
// TODO: move balance fns to wallet balance sub-module and also move this struct there
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

// TODO: underscore every 3 digits instead of 4
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
// TODO: move seed fns to wallet and move this struct also
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

/// Struct which owns and manages the [`crate::wallet::LightWallet`]. Responsible for network operations such as
/// storing the indexer URI, creating gRPC clients and syncing the wallet to the blockchain.
///
/// `sync_mode` is an atomic representation of [`pepper_sync::wallet::SyncMode`].
#[derive(Debug)]
pub struct LightClient {
    // TODO: split zingoconfig so data is not duplicated
    pub(crate) config: ZingoConfig,
    /// Wallet data
    pub wallet: Arc<Mutex<LightWallet>>,
    sync_mode: Arc<AtomicU8>,
    sync_handle: Option<JoinHandle<Result<SyncResult, SyncError<WalletError>>>>,
    save_active: Arc<AtomicBool>,
    save_handle: Option<JoinHandle<std::io::Result<()>>>,
    latest_proposal: Option<ZingoProposal>, // TODO: move to wallet
}

impl LightClient {
    /// Creates a LightClient with a new wallet from fresh entropy and a birthday of `chain_height`.
    /// Will fail if a wallet file already exists in the given data directory unless `overwrite` is `true`.
    ///
    /// It is worth considering setting `chain_height` to 100 blocks below current height of block chain to protect
    /// from re-orgs.
    pub fn new(
        config: ZingoConfig,
        chain_height: BlockHeight,
        overwrite: bool,
    ) -> Result<Self, LightClientError> {
        Self::create_from_wallet(
            LightWallet::new(config.chain, WalletBase::FreshEntropy, chain_height)?,
            config,
            overwrite,
        )
    }

    /// Creates a LightClient from a `wallet` and `config`.
    /// Will fail if a wallet file already exists in the given data directory unless `overwrite` is `true`.
    pub fn create_from_wallet(
        wallet: LightWallet,
        config: ZingoConfig,
        overwrite: bool,
    ) -> Result<Self, LightClientError> {
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            if !overwrite && dbg!(config.wallet_path_exists()) {
                return Err(LightClientError::FileError(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("Cannot save to given data directory as a wallet file already exists at:\n{}",
                    config.get_wallet_pathbuf().to_string_lossy())
                )));
            }
        }
        Ok(LightClient {
            config,
            wallet: Arc::new(Mutex::new(wallet)),
            sync_mode: Arc::new(AtomicU8::new(SyncMode::NotRunning as u8)),
            sync_handle: None,
            save_active: Arc::new(AtomicBool::new(false)),
            save_handle: None,
            latest_proposal: None,
        })
    }

    /// TODO: Add Doc Comment Here!
    pub fn create_from_wallet_path(config: ZingoConfig) -> Result<Self, LightClientError> {
        let wallet_path = if config.wallet_path_exists() {
            config.get_wallet_path()
        } else {
            return Err(LightClientError::FileError(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "Cannot read wallet. No file at {}",
                    config.get_wallet_path().display()
                ),
            )));
        };

        let buffer = BufReader::new(File::open(wallet_path)?);

        Self::create_from_wallet(LightWallet::read(buffer, config.chain)?, config, false)
    }

    /// TODO: Add Doc Comment Here!
    pub fn config(&self) -> &ZingoConfig {
        &self.config
    }

    /// Generates a new unified address from the given `addr_type`.
    // TODO: move to wallet
    pub async fn do_new_address(&mut self, addr_type: &str) -> Result<JsonValue, String> {
        //TODO: Placeholder interface
        let desired_receivers = ReceiverSelection {
            sapling: addr_type.contains('z'),
            orchard: addr_type.contains('o'),
            transparent: addr_type.contains('t'),
        };

        let mut wallet = self.wallet.lock().await;
        let new_address = wallet
            .generate_unified_address(desired_receivers)
            .map_err(|e| e.to_string())?;
        wallet.save_required = true;

        Ok(array![new_address.encode(&self.config.chain)])
    }

    /// TODO: Add Doc Comment Here!
    pub fn set_server(&self, server: http::Uri) {
        *self.config.lightwalletd_uri.write().unwrap() = server
    }

    pub(crate) async fn update_current_price(&self) -> String {
        // Get the zec price from the server
        match get_recent_median_price_from_gemini().await {
            Ok(price) => {
                self.wallet.lock().await.set_latest_zec_price(price).await;
                price.to_string()
            }
            Err(s) => {
                error!("Error fetching latest price: {}", s);
                s.to_string()
            }
        }
    }
}

// TODO: derive error and move to lightclient error module
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

// TODO: derive error and move to lightclient error module
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
    use crate::{
        config::{ChainType, RegtestNetwork, ZingoConfig},
        lightclient::{describe::UAReceivers, error::LightClientError},
        wallet::LightWallet,
    };
    use tempfile::TempDir;
    use testvectors::seeds::CHIMNEY_BETTER_SEED;

    use crate::{lightclient::LightClient, wallet::WalletBase};

    #[tokio::test]
    async fn new_wallet_from_phrase() {
        let temp_dir = TempDir::new().unwrap();
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let config = ZingoConfig::build(ChainType::Regtest(regtest_network))
            .set_wallet_dir(temp_dir.path().to_path_buf())
            .create();
        let mut lc = LightClient::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::MnemonicPhrase(CHIMNEY_BETTER_SEED.to_string()),
                0.into(),
            )
            .unwrap(),
            config.clone(),
            false,
        )
        .unwrap();

        lc.save_task().await;
        lc.wait_for_save().await;

        let lc_file_exists_error = LightClient::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::MnemonicPhrase(CHIMNEY_BETTER_SEED.to_string()),
                0.into(),
            )
            .unwrap(),
            config,
            false,
        )
        .unwrap_err();

        assert!(matches!(
            lc_file_exists_error,
            LightClientError::FileError(_)
        ));

        // The first t address and z address should be derived
        let addresses = lc.do_addresses(UAReceivers::All).await;
        assert_eq!(
            "zregtestsapling1etnl5s47cqves0g5hk2dx5824rme4xv4aeauwzp4d6ys3qxykt5sw5rnaqh9syxry8vgxr7x3x4"
                .to_string(),
            addresses[0]["receivers"]["sapling"]
        );
        assert_eq!(
            "tmYd5GP6JxUxTUcz98NLPumEotvaMPaXytz".to_string(),
            addresses[0]["receivers"]["transparent"]
        );
    }
}
