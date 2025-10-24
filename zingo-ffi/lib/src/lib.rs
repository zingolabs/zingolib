uniffi::include_scaffolding!("zingo");

#[macro_use]
extern crate lazy_static;
extern crate android_logger;

#[cfg(target_os = "android")]
use android_logger::{Config, FilterBuilder};
#[cfg(target_os = "android")]
use log::Level;

use std::num::NonZeroU32;
use std::str::FromStr;
use std::sync::RwLock;
use std::any::Any;
use std::panic::{self, PanicHookInfo, UnwindSafe};
use std::backtrace::Backtrace;
use std::sync::Once;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bip0039::Mnemonic;
use json::object;
use rustls::crypto::{CryptoProvider, ring::default_provider};

use zcash_address::unified::{Container, Encoding, Ufvk};
use zcash_keys::address::Address;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::zip32::AccountId;
use zcash_protocol::consensus::NetworkType;

use pepper_sync::keys::transparent;
use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
use pepper_sync::wallet::{KeyIdInterface, SyncMode};
use zingolib::config::{ChainType, ZingoConfig, construct_lightwalletd_uri};
use zingolib::data::PollReport;
use zingolib::lightclient::LightClient;
use zingolib::utils::{conversion::address_from_str, conversion::txid_from_hex_encoded_str};
use zingolib::wallet::keys::{
    WalletAddressRef,
    unified::{ReceiverSelection, UnifiedKeyStore},
};
use zingolib::wallet::{LightWallet, WalletBase, WalletSettings};
use zingolib::data::receivers::Receivers;
use zcash_address::ZcashAddress;
use zcash_protocol::{value::Zatoshis};
use tokio::runtime::Runtime;
use zcash_primitives::memo::MemoBytes;
use zingolib::data::receivers::transaction_request_from_receivers;
use zingolib::data::proposal::total_fee;
use zingolib::testutils;

#[derive(Debug, thiserror::Error)]
pub enum ZingolibError {
    #[error("Error: Lightclient is not initialized")]
    LightclientNotInitialized,
    #[error("Error: Lightclient lock poisoned")]
    LightclientLockPoisoned,
    #[error("Error: panic: {0}")]
    Panic(String),
}

pub fn with_panic_guard<T, F>(f: F) -> Result<T, ZingolibError>
where
    F: FnOnce() -> Result<T, ZingolibError> + UnwindSafe,
{
    install_panic_hook_once();
    match panic::catch_unwind(f) {
        Ok(res) => res,
        Err(payload) => Err(ZingolibError::Panic(format_panic_text(payload))),
    }
}

#[derive(Clone, Default)]
struct PanicReport {
    msg: String,
    file: Option<String>,
    line: Option<u32>,
    col:  Option<u32>,
    backtrace: Option<String>,
}

static LAST_PANIC: Lazy<Mutex<PanicReport>> =
    Lazy::new(|| Mutex::new(PanicReport::default()));

fn set_last_panic(report: PanicReport) {
    if let Ok(mut r) = LAST_PANIC.lock() {
        *r = report;
    }
}

fn take_last_panic() -> PanicReport {
    if let Ok(mut r) = LAST_PANIC.lock() {
        let out = r.clone();
        *r = PanicReport::default();
        out
    } else {
        PanicReport::default()
    }
}

static PANIC_HOOK_ONCE: Once = Once::new();

fn install_panic_hook_once() {
    PANIC_HOOK_ONCE.call_once(|| {
        panic::set_hook(Box::new(|info: &PanicHookInfo<'_>| {
            let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                info.to_string()
            };

            let (file, line, col) = info.location()
                .map(|l| (Some(l.file().to_string()), Some(l.line()), Some(l.column())))
                .unwrap_or((None, None, None));

            let bt = Backtrace::force_capture().to_string();

            set_last_panic(PanicReport {
                msg: payload,
                file, line, col,
                backtrace: Some(bt),
            });
        }));
    });
}

fn clean_backtrace(bt_raw: &str) -> String {
    const DROP: &[&str] = &[
        "<unknown>"
    ];

    let mut out = String::new();

    for line in bt_raw.lines() {
        let l = line.trim();
        if l.is_empty() { continue; }
        if DROP.iter().any(|d| l.contains(d)) { continue; }

        out.push_str(line);
        out.push('\n');
    }

    out
}

fn format_panic_text(payload: Box<dyn Any + Send>) -> String {
    let rpt = take_last_panic();

    let fallback = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    };

    let mut out = String::new();

    if let (Some(f), Some(l), Some(c)) = (rpt.file.as_ref(), rpt.line, rpt.col) {
        out.push_str(&format!("{f}:{l}:{c}: "));
    }
    if !rpt.msg.is_empty() {
        out.push_str(&rpt.msg);
    } else {
        out.push_str(&fallback);
    }

    if let Some(bt) = rpt.backtrace {
        let cleaned = clean_backtrace(&bt);
        if !cleaned.is_empty() {
            out.push_str("\nBacktrace:\n");
            out.push_str(&cleaned);
        }
    }

    out
}

// We'll use a RwLock to store a global lightclient instance,
// so we don't have to keep creating it. We need to store it here, in rust
// because we can't return such a complex structure back to JS
lazy_static! {
    static ref LIGHTCLIENT: RwLock<Option<LightClient>> = RwLock::new(None);
}

lazy_static! {
    pub static ref RT: Runtime = tokio::runtime::Runtime::new().unwrap();
}

fn with_lightclient_write<F, R>(f: F) -> R
where
    F: FnOnce(&mut Option<LightClient>) -> R,
{
    let mut guard = match LIGHTCLIENT.write() {
        Ok(g) => g,
        Err(poisoned) => {
            log::warn!("LIGHTCLIENT RwLock poisoned; recovering and clearing poison");
            let g = poisoned.into_inner();
            LIGHTCLIENT.clear_poison();
            g
        }
    };
    f(&mut *guard)
}

fn reset_lightclient() {
    with_lightclient_write(|slot| {
        *slot = None;
    });
}

fn store_client(lightclient: LightClient) -> Result<(), ZingolibError> {
    with_lightclient_write(|slot| {
        *slot = Some(lightclient);
    });
    Ok(())
}

fn construct_uri_load_config(
    uri: String,
    chain_hint: String,
    performance_level: String,
    min_confirmations: u32,
) -> Result<(ZingoConfig, http::Uri), String> {
    // if uri is empty -> Offline Mode.
    let lightwalletd_uri = construct_lightwalletd_uri(Some(uri));

    let chaintype = match chain_hint.as_str() {
        "main" => ChainType::Mainnet,
        "test" => ChainType::Testnet,
        "regtest" => ChainType::Regtest(testutils::default_regtest_heights()),
        _ => return Err("Error: Not a valid chain hint!".to_string()),
    };
    let performancetype = match performance_level.as_str() {
        "Maximum" => PerformanceLevel::Maximum,
        "High" => PerformanceLevel::High,
        "Medium" => PerformanceLevel::Medium,
        "Low" => PerformanceLevel::Low,
        _ => return Err("Error: Not a valid performance level!".to_string()),
    };
    let config = match zingolib::config::load_clientconfig(
        lightwalletd_uri.clone(),
        None,
        chaintype,
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                performance_level: performancetype,
            },
            min_confirmations: NonZeroU32::try_from(min_confirmations).unwrap(),
        },
        NonZeroU32::try_from(1).expect("hard-coded integer"),
    ) {
        Ok(c) => c,
        Err(e) => {
            return Err(format!("Error: Config load: {e}"));
        }
    };

    Ok((config, lightwalletd_uri))
}

pub fn init_logging() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        // this is only for Android
        #[cfg(target_os = "android")]
        android_logger::init_once(
            Config::default().with_min_level(Level::Trace).with_filter(
                FilterBuilder::new()
                    .parse("debug,hello::crate=zingolib")
                    .build(),
            ),
        );
        Ok("OK".to_string())
    })
}

pub fn init_new(
    server_uri: String,
    chain_hint: String,
    performance_level: String,
    min_confirmations: u32,
) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        reset_lightclient();
        let (config, lightwalletd_uri) = match construct_uri_load_config(server_uri, chain_hint, performance_level, min_confirmations) {
            Ok(c) => c,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let chain_height = match RT
            .block_on(async move {
                zingolib::grpc_connector::get_latest_block(lightwalletd_uri)
                    .await
                    .map(|block_id| BlockHeight::from_u32(block_id.height as u32))
        }) {
            Ok(h) => h,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let lightclient = match LightClient::new(
            config,
            chain_height - 100,
            false,
        ) {
            Ok(l) => l,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let _ = store_client(lightclient);

        get_seed()
    })
}

// TODO: change `seed` to `seed_phrase` or `mnemonic_phrase`
pub fn init_from_seed(
    seed: String,
    birthday: u32,
    server_uri: String,
    chain_hint: String,
    performance_level: String,
    min_confirmations: u32,
) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        reset_lightclient();
        let (config, _lightwalletd_uri) = match construct_uri_load_config(server_uri, chain_hint, performance_level, min_confirmations) {
            Ok(c) => c,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let mnemonic = match Mnemonic::from_phrase(seed) {
            Ok(m) => m,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let wallet = match LightWallet::new(
            config.chain,
            WalletBase::Mnemonic {
                mnemonic,
                no_of_accounts: config.no_of_accounts,
            },
            BlockHeight::from_u32(birthday),
            config.wallet_settings.clone(),
        ) {
            Ok(w) => w,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let lightclient = match LightClient::create_from_wallet(wallet, config, false) {
            Ok(l) => l,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let _ = store_client(lightclient);

        get_seed()
    })
}

pub fn init_from_ufvk(
    ufvk: String,
    birthday: u32,
    server_uri: String,
    chain_hint: String,
    performance_level: String,
    min_confirmations: u32,
) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        reset_lightclient();
        let (config, _lightwalletd_uri) = match construct_uri_load_config(server_uri, chain_hint, performance_level, min_confirmations) {
            Ok(c) => c,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let wallet = match LightWallet::new(
            config.chain,
            WalletBase::Ufvk(ufvk),
            BlockHeight::from_u32(birthday),
            config.wallet_settings.clone(),
        ) {
            Ok(w) => w,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let lightclient = match LightClient::create_from_wallet(wallet, config, false) {
            Ok(l) => l,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let _ = store_client(lightclient);

        get_ufvk()
    })
}

pub fn init_from_b64(
    base64_data: String,
    server_uri: String,
    chain_hint: String,
    performance_level: String,
    min_confirmations: u32,
) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        reset_lightclient();
        let (config, _lightwalletd_uri) = match construct_uri_load_config(server_uri, chain_hint, performance_level, min_confirmations) {
            Ok(c) => c,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let decoded_bytes = match STANDARD.decode(&base64_data) {
            Ok(b) => b,
            Err(e) => {
                return Ok(format!(
                    "Error: Decoding Base64: {}, Size: {}, Content: {}",
                    e,
                    base64_data.len(),
                    base64_data
                ));
            }
        };

        let wallet = match LightWallet::read(&decoded_bytes[..], config.chain) {
            Ok(w) => w,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let has_seed = wallet.mnemonic().is_some();
        let lightclient = match LightClient::create_from_wallet(wallet, config, false) {
            Ok(l) => l,
            Err(e) => return Ok(format!("Error: {e}")),
        };
        let _ = store_client(lightclient);

        if has_seed { get_seed() } else { get_ufvk() }
    })
}

pub fn save_to_b64() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        // Return the wallet as a base64 encoded string
        if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
            // we need to use STANDARD because swift is expecting the encoded String with padding
            // I tried with STANDARD_NO_PAD and the decoding return `nil`.
            Ok(RT.block_on(async move {
                let mut wallet = lightclient.wallet.write().await;
                match wallet.save() {
                    Ok(Some(wallet_bytes)) => STANDARD.encode(wallet_bytes),
                    // TODO: check this is better than a custom error when save is not required (empty buffer)
                    Ok(None) => "".to_string(),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn check_b64(base64_data: String,) -> String {
    match STANDARD.decode(&base64_data) {
        Ok(_) => "true".to_string(),
        Err(_) => "false".to_string(),
    }
}

pub fn get_developer_donation_address() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        Ok(zingolib::config::DEVELOPER_DONATION_ADDRESS.to_string())
    })
}

pub fn get_zennies_for_zingo_donation_address() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        Ok(zingolib::config::ZENNIES_FOR_ZINGO_DONATION_ADDRESS.to_string())
    })
}

pub fn set_crypto_default_provider_to_ring() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        Ok(CryptoProvider::get_default().map_or_else(
            || match default_provider().install_default() {
                Ok(_) => "true".to_string(),
                Err(_) => "Error: Failed to install crypto provider".to_string(),
            },
            |_| "true".to_string(),
        ))
    })
}

pub fn get_latest_block_server(server_uri: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let lightwalletd_uri: http::Uri = match server_uri.parse() {
            Ok(uri) => uri,
            Err(e) => {
                return Ok(format!("Error: failed to parse uri. {e}"));
            }
        };
        Ok(match RT
            .block_on(async move { zingolib::grpc_connector::get_latest_block(lightwalletd_uri).await })
        {
            Ok(block_id) => block_id.height.to_string(),
            Err(e) => format!("Error: {e}"),
        })
    })
}

pub fn get_latest_block_wallet() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet.write().await;
                object! { "height" => json::JsonValue::from(wallet.sync_state.wallet_height().map(u32::from).unwrap_or(0))}.pretty(2)
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_value_transfers() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet.read().await;
                match wallet.value_transfers(true).await
                {
                    Ok(value_transfers) => json::JsonValue::from(value_transfers).pretty(2),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn poll_sync() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(match lightclient.poll_sync() {
                PollReport::NoHandle => "Sync task has not been launched.".to_string(),
                PollReport::NotReady => "Sync task is not complete.".to_string(),
                PollReport::Ready(result) => match result {
                    Ok(sync_result) => {
                        json::object! { "sync_complete" => json::JsonValue::from(sync_result) }
                            .pretty(2)
                    }
                    Err(e) => format!("Error: {e}"),
                },
            })
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

fn run_sync() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            if lightclient.sync_mode() == SyncMode::Paused {
                lightclient.resume_sync().expect("sync should be paused");
                Ok("Resuming sync task...".to_string())
            } else {
                Ok(RT.block_on(async {
                    match lightclient.sync().await {
                        Ok(_) => "Launching sync task...".to_string(),
                        Err(e) => format!("Error: {e}"),
                    }
                }))
            }
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn pause_sync() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(match lightclient.pause_sync() {
                Ok(_) => "Pausing sync task...".to_string(),
                Err(e) => format!("Error: {e}"),
            })
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

fn status_sync() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async {
                let wallet = lightclient.wallet.read().await;
                match pepper_sync::sync_status(&*wallet).await {
                    Ok(status) => json::JsonValue::from(status).pretty(2),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn run_rescan() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                match lightclient.rescan().await {
                    Ok(_) => "Launching rescan...".to_string(),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn info_server() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move { lightclient.do_info().await }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

// TODO: rename "get_seed_phrase" or "get_mnemonic_phrase"
// or if other recovery info is being used could rename "get_recovery_info" ?
pub fn get_seed() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet.read().await;
                match wallet.recovery_info() {
                    Some(recovery_info) => serde_json::to_string_pretty(&recovery_info)
                        .unwrap_or_else(|_| "Error: get seed. failed to serialize".to_string()),
                    None => "Error: get seed. no mnemonic found. wallet loaded from key.".to_string(),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_ufvk() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet.read().await;
                let ufvk: UnifiedFullViewingKey = match wallet
                    .unified_key_store
                    .get(&AccountId::ZERO)
                    .expect("account 0 must always exist")
                    .try_into()
                {
                    Ok(ufvk) => ufvk,
                    Err(e) => return format!("Error: {e}"),
                };
                object! {
                    "ufvk" => ufvk.encode(&wallet.network),
                    "birthday" => u32::from(wallet.birthday)
                }
                .pretty(2)
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn change_server(server_uri: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            if server_uri.is_empty() {
                lightclient.set_server(http::Uri::default());
                Ok("server set (default)".to_string())
            } else {
                match http::Uri::from_str(&server_uri) {
                    Ok(uri) => {
                        lightclient.set_server(uri);
                        Ok("server set".to_string())
                    },
                    Err(_) => Ok("Error: invalid server uri".to_string()),
                }
            }
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn wallet_kind() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet.read().await;
                if wallet.mnemonic().is_some() {
                    object! {"kind" => "Loaded from seed or mnemonic phrase)",
                            "transparent" => true,
                            "sapling" => true,
                            "orchard" => true,
                    }
                    .pretty(2)
                } else {
                    match wallet
                        .unified_key_store
                        .get(&AccountId::ZERO)
                        .expect("account 0 must always exist")
                    {
                        UnifiedKeyStore::Spend(_) => object! {
                            "kind" => "Loaded from unified spending key",
                            "transparent" => true,
                            "sapling" => true,
                            "orchard" => true,
                        }
                        .pretty(2),
                        UnifiedKeyStore::View(ufvk) => object! {
                            "kind" => "Loaded from unified full viewing key",
                            "transparent" => ufvk.transparent().is_some(),
                            "sapling" => ufvk.sapling().is_some(),
                            "orchard" => ufvk.orchard().is_some(),
                        }
                        .pretty(2),
                        UnifiedKeyStore::Empty => object! {
                            "kind" => "No keys found",
                            "transparent" => false,
                            "sapling" => false,
                            "orchard" => false,
                        }
                        .pretty(2),
                    }
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn parse_address(address: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        if address.is_empty() {
            Ok("Error: The address is empty".to_string())
        } else {
            fn make_decoded_chain_pair(
                address: &str,
            ) -> Option<(zcash_client_backend::address::Address, ChainType)> {
                [
                    ChainType::Mainnet,
                    ChainType::Testnet,
                    ChainType::Regtest(testutils::default_regtest_heights()),
                ]
                .iter()
                .find_map(|chain| Address::decode(chain, address).zip(Some(*chain)))
            }
            if let Some((recipient_address, chain_name)) = make_decoded_chain_pair(&address) {
                let chain_name_string = match chain_name {
                    ChainType::Mainnet => "main",
                    ChainType::Testnet => "test",
                    ChainType::Regtest(_) => "regtest",
                };
                Ok(match recipient_address {
                    Address::Sapling(_) => object! {
                        "status" => "success",
                        "chain_name" => chain_name_string,
                        "address_kind" => "sapling",
                    }
                    .pretty(2),
                    Address::Transparent(_) => object! {
                        "status" => "success",
                        "chain_name" => chain_name_string,
                        "address_kind" => "transparent",
                    }
                    .pretty(2),
                    Address::Tex(_) => object! {
                        "status" => "success",
                        "chain_name" => chain_name_string,
                        "address_kind" => "tex",
                    }
                    .pretty(2),
                    Address::Unified(ua) => {
                        let mut receivers_available = vec![];
                        if ua.sapling().is_some() {
                            receivers_available.push("sapling")
                        }
                        if ua.transparent().is_some() {
                            receivers_available.push("transparent")
                        }
                        if ua.orchard().is_some() {
                            receivers_available.push("orchard");
                            object! {
                                "status" => "success",
                                "chain_name" => chain_name_string,
                                "address_kind" => "unified",
                                "receivers_available" => receivers_available,
                                "only_orchard_ua" => zcash_keys::address::UnifiedAddress::from_receivers(ua.orchard().cloned(), None, None).expect("To construct UA").encode(&chain_name),
                            }
                            .pretty(2)
                        } else {
                            object! {
                                "status" => "success",
                                "chain_name" => chain_name_string,
                                "address_kind" => "unified",
                                "receivers_available" => receivers_available,
                            }
                            .pretty(2)
                        }
                    }
                })
            } else {
                Ok(object! {
                    "status" => "Invalid address",
                    "chain_name" => json::JsonValue::Null,
                    "address_kind" => json::JsonValue::Null,
                }
                .pretty(2))
            }
        }
    })
}

pub fn parse_ufvk(ufvk: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        if ufvk.is_empty() {
            Ok("Error: The ufvk is empty".to_string())
        } else {
            Ok(json::stringify_pretty(
                match Ufvk::decode(&ufvk) {
                    Ok((network, ufvk)) => {
                        let mut pools_available = vec![];
                        for fvk in ufvk.items_as_parsed() {
                            match fvk {
                                zcash_address::unified::Fvk::Orchard(_) => {
                                    pools_available.push("orchard")
                                }
                                zcash_address::unified::Fvk::Sapling(_) => {
                                    pools_available.push("sapling")
                                }
                                zcash_address::unified::Fvk::P2pkh(_) => {
                                    pools_available.push("transparent")
                                }
                                zcash_address::unified::Fvk::Unknown { .. } => pools_available.push(
                                    "Error: Unknown future protocol. Perhaps you're using old software",
                                ),
                            }
                        }
                        object! {
                            "status" => "success",
                            "chain_name" => match network {
                                NetworkType::Main => "main",
                                NetworkType::Test => "test",
                                NetworkType::Regtest => "regtest",
                            },
                            "address_kind" => "ufvk",
                            "pools_available" => pools_available,
                        }
                    }
                    Err(_) => {
                        object! {
                            "status" => "Invalid viewkey",
                            "chain_name" => json::JsonValue::Null,
                            "address_kind" => json::JsonValue::Null
                        }
                    }
                },
                2,
            ))
        }
    })
}

pub fn get_version() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        Ok(zingolib::git_description().to_string())
    })
}

pub fn get_messages(address: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                match lightclient
                    .messages_containing(Some(address.as_str()))
                    .await
                {
                    Ok(value_transfers) => json::JsonValue::from(value_transfers).pretty(2),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_balance() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                match lightclient
                    .account_balance(AccountId::ZERO)
                    .await
                {
                    Ok(bal) => json::JsonValue::from(bal).pretty(2),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_total_memobytes_to_address() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                match lightclient.do_total_memobytes_to_address().await {
                    Ok(total_memo_bytes) => json::JsonValue::from(total_memo_bytes).pretty(2),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_total_value_to_address() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                match lightclient.do_total_value_to_address().await {
                    Ok(total_values) => json::JsonValue::from(total_values).pretty(2),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_total_spends_to_address() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                match lightclient.do_total_spends_to_address().await {
                    Ok(total_spends) => json::JsonValue::from(total_spends).pretty(2),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn zec_price(tor: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let Ok(tor_bool) = tor.parse() else {
                    return "Error: failed to parse tor setting.".to_string();
                };
                let client_check = match (tor_bool, lightclient.tor_client()) {
                    (true, Some(tc)) => Ok(Some(tc)),
                    (true, None) => Err(()),
                    (false, _) => Ok(None),
                };
                let tor_client = match client_check {
                    Ok(tc) => tc,
                    Err(_) => {
                        return "Error: no tor client found. please create a tor client.".to_string();
                    }
                };

                let mut wallet = lightclient.wallet.write().await;
                match wallet.update_current_price(tor_client).await
                {
                    Ok(price) => object! { "current_price" => price }.pretty(2),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn resend_transaction(txid: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            let txid = match txid_from_hex_encoded_str(&txid) {
                Ok(txid) => txid,
                Err(e) => return Ok(format!("Error: {e}")),
            };
            Ok(RT.block_on(async move {
                match lightclient.resend(txid).await {
                    Ok(_) => "Successfully resent transaction.".to_string(),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn remove_transaction(txid: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            let txid = match txid_from_hex_encoded_str(&txid) {
                Ok(txid) => txid,
                Err(e) => return Ok(format!("Error: {e}")),
            };
            Ok(RT.block_on(async move {
                let mut wallet = lightclient.wallet.write().await;
                match wallet.remove_unconfirmed_transaction(txid)
                {
                    Ok(_) => "Successfully removed transaction.".to_string(),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

// we don't use this anymore...
pub fn get_spendable_balance_with_address(address: String, zennies: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            let Ok(address) = address_from_str(&address) else {
                return Ok("Error: unknown address format".to_string());
            };
            let Ok(zennies) = zennies.parse() else {
                return Ok("Error: failed to parse zennies setting.".to_string());
            };
            Ok(RT.block_on(async move {
                match lightclient
                    .max_send_value(address, zennies, AccountId::ZERO)
                    .await
                {
                    Ok(bal) => {
                        object! { "spendable_balance" => bal.into_u64() }.pretty(2)
                    }
                    Err(e) => format!("error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_spendable_balance_total() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet.write().await;
                let spendable_balance =
                    match wallet.shielded_spendable_balance(AccountId::ZERO, false) {
                        Ok(bal) => bal,
                        Err(e) => return format!("Error: {e}"),
                    };
                object! {
                    "spendable_balance" => spendable_balance.into_u64(),
                }
                .pretty(2)
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn set_option_wallet() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        Ok("Error: unimplemented".to_string())
    })
}

pub fn get_option_wallet() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        Ok("Error: unimplemented".to_string())
    })
}

pub fn create_tor_client(data_dir: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            if lightclient.tor_client().is_some() {
                return Ok("Tor client already exists.".to_string());
            }
            Ok(match RT.block_on(async move {
                lightclient.create_tor_client(Some(data_dir.into())).await
            }) {
                Ok(_) => "Successfully created tor client.".to_string(),
                Err(e) => format!("Error: creating tor client: {e}"),
            })
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn remove_tor_client() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            if lightclient.tor_client().is_none() {
                return Ok("Tor client is not active.".to_string());
            }
            RT.block_on(async move {
                lightclient.remove_tor_client().await;
            });
            Ok("Successfully removed tor client.".to_string())
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_unified_addresses() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move { lightclient.unified_addresses_json().await.pretty(2) }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_transparent_addresses() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move { lightclient.transparent_addresses_json().await.pretty(2) }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn create_new_unified_address(receivers: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let mut wallet = lightclient.wallet.write().await;
                let network = wallet.network;
                let receivers_available = ReceiverSelection {
                    orchard: receivers.contains('o'),
                    sapling: receivers.contains('z'),
                };
                match wallet.generate_unified_address(receivers_available, AccountId::ZERO) {
                    Ok((id, unified_address)) => {
                        json::object! {
                            "account" => u32::from(AccountId::ZERO),
                            "address_index" => id.address_index,
                            "has_orchard" => unified_address.has_orchard(),
                            "has_sapling" => unified_address.has_sapling(),
                            "has_transparent" => unified_address.has_transparent(),
                            "encoded_address" => unified_address.encode(&network),
                        }.pretty(2)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn create_new_transparent_address() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let mut wallet = lightclient.wallet.write().await;
                let network = wallet.network;
                match wallet.generate_transparent_address(AccountId::ZERO, true) {
                    Ok((id, transparent_address)) => {
                        json::object! {
                            "account" => u32::from(id.account_id()),
                            "address_index" => id.address_index().index(),
                            "scope" => id.scope().to_string(),
                            "encoded_address" => transparent::encode_address(&network,  transparent_address),
                        }.pretty(2)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn check_my_address(address: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet.read().await;
                match wallet.is_address_derived_by_keys(&address) {
                    Ok(address_ref) => address_ref.map_or(
                        json::object! { "is_wallet_address" => false },
                        |address_ref| match address_ref {
                            WalletAddressRef::Unified {
                                account_id,
                                address_index,
                                has_orchard,
                                has_sapling,
                                has_transparent,
                                encoded_address,
                            } => json::object! {
                                "is_wallet_address" => true,
                                "address_type" => "unified".to_string(),
                                "address_index" => address_index,
                                "account_id" => u32::from(account_id),
                                "has_orchard" => has_orchard,
                                "has_sapling" => has_sapling,
                                "has_transparent" => has_transparent,
                                "encoded_address" => encoded_address,
                            },
                            WalletAddressRef::OrchardInternal {
                                account_id,
                                diversifier_index,
                                encoded_address,
                            } => json::object! {
                                "is_wallet_address" => true,
                                "address_type" => "orchard_internal".to_string(),
                                "account_id" => u32::from(account_id),
                                "diversifier_index" => u128::from(diversifier_index).to_string(),
                                "encoded_address" => encoded_address,
                            },
                            WalletAddressRef::SaplingExternal {
                                account_id,
                                diversifier_index,
                                encoded_address,
                            } => json::object! {
                                "is_wallet_address" => true,
                                "address_type" => "sapling".to_string(),
                                "account_id" => u32::from(account_id),
                                "diversifier_index" => u128::from(diversifier_index).to_string(),
                                "encoded_address" => encoded_address,
                            },
                            WalletAddressRef::Transparent {
                                account_id,
                                scope,
                                address_index,
                                encoded_address,
                            } => json::object! {
                                "is_wallet_address" => true,
                                "address_type" => "transparent".to_string(),
                                "account_id" => u32::from(account_id),
                                "scope" => scope.to_string(),
                                "address_index" => address_index.index(),
                                "encoded_address" => encoded_address,
                            },
                        },
                    ).pretty(2),
                    Err(e) => format!("Error: {e}"),
                }
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_wallet_save_required() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet.read().await;
                object! { "save_required" => wallet.save_required }.pretty(2)
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn set_config_wallet_to_test() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let mut wallet = lightclient.wallet.write().await;
                wallet.wallet_settings.min_confirmations = NonZeroU32::try_from(1).unwrap();
                wallet.wallet_settings.sync_config.performance_level = PerformanceLevel::Medium;
                wallet.save_required = true;
                "Successfully set config wallet to test. (1 - Medium)".to_string()
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn set_config_wallet_to_prod(
    performance_level: String,
    min_confirmations: u32,
) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let performancetype = match performance_level.as_str() {
                    "Maximum" => PerformanceLevel::Maximum,
                    "High" => PerformanceLevel::High,
                    "Medium" => PerformanceLevel::Medium,
                    "Low" => PerformanceLevel::Low,
                    _ => return "Error: Not a valid performance level!".to_string(),
                };
                let mut wallet = lightclient.wallet.write().await;
                wallet.wallet_settings.min_confirmations = NonZeroU32::try_from(min_confirmations).unwrap();
                wallet.wallet_settings.sync_config.performance_level = performancetype;
                wallet.save_required = true;
                "Successfully set config wallet to prod.".to_string()
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_config_wallet_performance() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet.read().await;
                let performance_level = match wallet.wallet_settings.sync_config.performance_level {
                    PerformanceLevel::Low => "Low",
                    PerformanceLevel::Medium => "Medium",
                    PerformanceLevel::High => "High",
                    PerformanceLevel::Maximum => "Maximum",
                };
                object! { "performance_level" => performance_level }.pretty(2)
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_wallet_version() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet.read().await;
                let current_version = wallet.current_version();
                let read_version = wallet.read_version();
                object! { 
                    "current_version" => current_version,
                    "read_version" => read_version
                }.pretty(2)
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

// internal use
fn interpret_memo_string(memo_str: String) -> Result<MemoBytes, String> {
    // If the string starts with an "0x", and contains only hex chars ([a-f0-9]+) then
    // interpret it as a hex
    let s_bytes = if memo_str.to_lowercase().starts_with("0x") {
        match hex::decode(&memo_str[2..memo_str.len()]) {
            Ok(data) => data,
            Err(_) => Vec::from(memo_str.as_bytes()),
        }
    } else {
        Vec::from(memo_str.as_bytes())
    };

    MemoBytes::from_bytes(&s_bytes)
        .map_err(|_| format!("Error: creating output. Memo '{:?}' is too long", memo_str))
}

pub fn send(send_json: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let json_args = match json::parse(&send_json) {
                    Ok(parsed) => parsed,
                    Err(_) => return "Error: it is not a valid JSON".to_string(),
                };

                let mut receivers = Receivers::new();
                for j in json_args.members() {
                    let recipient_address = match j["address"].as_str() {
                        Some(addr) => match ZcashAddress::try_from_encoded(addr) {
                            Ok(a) => a,
                            Err(e) => return format!("Error: Invalid address: {e}"),
                        },
                        None => return "Error: Missing address".to_string(),
                    };

                    let amount = match j["amount"].as_u64() {
                        Some(a) => match Zatoshis::from_u64(a) {
                            Ok(a) => a,
                            Err(e) => return format!("Error: Invalid amount: {e}"),
                        },
                        None => return "Missing amount".to_string(),
                    };

                    let memo = if let Some(m) = j["memo"].as_str() {
                        match interpret_memo_string(m.to_string()) {
                            Ok(memo_bytes) => Some(memo_bytes),
                            Err(e) => return format!("Error: Invalid memo: {e}"),
                        }
                    } else {
                        None
                    };

                    receivers.push(zingolib::data::receivers::Receiver {
                        recipient_address,
                        amount,
                        memo,
                    });
                }

                let request = match transaction_request_from_receivers(receivers)
                {
                    Ok(request) => request,
                    Err(e) => return format!("Error: Request Error: {e}"),
                };

                match lightclient
                    .propose_send(request, AccountId::ZERO)
                    .await
                {
                    Ok(proposal) => {
                        let fee = match total_fee(&proposal) {
                            Ok(fee) => fee,
                            Err(e) => return object! { "error" => e.to_string() }.pretty(2),
                        };
                        object! { "fee" => fee.into_u64() }
                    }
                    Err(e) => {
                        object! { "error" => e.to_string() }
                    }
                }
                .pretty(2)
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn shield() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                match lightclient.propose_shield(AccountId::ZERO).await {
                    Ok(proposal) => {
                        if proposal.steps().len() != 1 {
                            return object! { "error" => "shielding transactions should not have multiple proposal steps" }.pretty(2);
                        }
                        let step = proposal.steps().first();
                        let Some(value_to_shield) = step
                            .balance()
                            .proposed_change()
                            .iter()
                            .try_fold(Zatoshis::ZERO, |acc, c| acc + c.value()) else {
                                return object! { "error" => "shield amount outside valid range of zatoshis" }
                                    .pretty(2);
                        };
                        let fee = step.balance().fee_required();
                        object! {
                            "value_to_shield" => value_to_shield.into_u64(),
                            "fee" => fee.into_u64(),
                        }
                    }
                    Err(e) => {
                        object! { "error" => e.to_string() }
                    }
                }
                .pretty(2)
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn confirm() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT.write().map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                match lightclient
                    .send_stored_proposal()
                    .await {
                    Ok(txids) => {
                        object! { "txids" => txids.iter().map(|txid| txid.to_string()).collect::<Vec<_>>() }
                    }
                    Err(e) => {
                        object! { "error" => e.to_string() }
                    }
                }
                .pretty(2)
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}