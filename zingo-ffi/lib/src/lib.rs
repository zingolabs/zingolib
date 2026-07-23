uniffi::include_scaffolding!("zingo");

#[macro_use]
extern crate lazy_static;
extern crate android_logger;

#[cfg(target_os = "android")]
use android_logger::{Config, FilterBuilder};
#[cfg(target_os = "android")]
use log::Level;

use std::any::Any;
use std::backtrace::Backtrace;
use std::num::NonZeroU32;
use std::panic::{self, PanicHookInfo, UnwindSafe};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::Once;
use std::sync::RwLock;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use json::object;
use once_cell::sync::Lazy;
use rustls::crypto::{CryptoProvider, ring::default_provider};

use zcash_address::unified::{Container, Encoding, Ufvk};
use zcash_keys::address::Address;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_protocol::consensus::{NetworkType, NetworkUpgrade, Parameters};
use zip32::AccountId;

use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
use pepper_sync::error::SyncModeError;
use pepper_sync::keys::transparent;
use pepper_sync::wallet::{KeyIdInterface, SyncMode};
use tokio::runtime::Runtime;
use zcash_address::ZcashAddress;
use zcash_protocol::memo::MemoBytes;
use zcash_protocol::value::Zatoshis;
use zingo_netutils::{GrpcIndexer, Indexer};
use zingolib::config::{
    ChainType, ClientConfig, DEFAULT_INDEXER_URI, DEFAULT_INDEXER_URI_TESTNET, WalletConfig,
    construct_indexer_uri, lib_birthday,
};
use zingolib::data::PollReport;
use zingolib::data::proposal::total_fee;
use zingolib::data::receivers::Receivers;
use zingolib::data::receivers::transaction_request_from_receivers;
use zingolib::lightclient::LightClient;
use zingolib::lightclient::error::{LightClientError, SendError};
use zingolib::lightclient::migrate::SplitStep;
use zingolib::utils::{conversion::address_from_str, conversion::txid_from_hex_encoded_str};
use zingolib::wallet::WalletSettings;
use zingolib::wallet::keys::{
    WalletAddressRef,
    unified::{ReceiverSelection, UnifiedKeyStore},
};
use zingolib::wallet::migration::{
    MigrationParams, MigrationPhase, RecommendedAction, parts::SigningStrategy, split::plan_hash,
};

use zingo_common_components::protocol::ActivationHeights;

const INDEXER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum ZingolibError {
    #[error("Error: Lightclient is not initialized")]
    LightclientNotInitialized,
    #[error("Error: Lightclient lock poisoned")]
    LightclientLockPoisoned,
    #[error("Error: panic: {0}")]
    Panic(String),
    #[error("Error: saving wallet: {0}")]
    Save(String),
    #[error("Error: initializing wallet: {0}")]
    Init(String),
    #[error("Error: sync: {0}")]
    Sync(String),
    #[error("Error: rescan: {0}")]
    Rescan(String),
    #[error("Error: read: {0}")]
    Read(String),
    #[error("Error: send: {0}")]
    Send(String),
    #[error("Error: shield: {0}")]
    Shield(String),
    #[error("Error: invalid input: {0}")]
    InvalidInput(String),
    #[error("Error: wallet: {0}")]
    Wallet(String),
    #[error("Error: indexer: {0}")]
    Indexer(String),
    #[error("Error: offline: no indexer configured")]
    Offline,
    #[error("Error: progress side-channel lock poisoned")]
    SideChannelPoisoned,
    #[error("Error: no migration in progress")]
    MigrationNotInProgress,
    #[error("Error: a migration is already in progress")]
    MigrationAlreadyInProgress,
    #[error("Error: consent stale: {0}")]
    MigrationConsentStale(String),
    #[error("Error: cadence fixed: {0}")]
    MigrationCadenceFixed(String),
    #[error("Error: note splitting: {0}")]
    MigrationSplit(String),
    #[error("Error: migration: {0}")]
    Migration(String),
}

impl ZingolibError {
    fn init(e: impl ToString) -> Self {
        Self::Init(e.to_string())
    }

    fn sync(e: impl ToString) -> Self {
        Self::Sync(e.to_string())
    }

    fn read(e: impl ToString) -> Self {
        Self::Read(e.to_string())
    }
}

/// The one pure funnel from zingolib's error taxonomy to the FFI's typed
/// variants. Exhaustive at every level on purpose: a new zingolib variant
/// fails compilation here instead of degrading to prose in the data channel.
fn ffi_error(e: LightClientError) -> ZingolibError {
    use zingolib::lightclient::error::MigrationError;
    let text = e.to_string();
    match e {
        LightClientError::SyncLaunchError
        | LightClientError::SyncNotRunning
        | LightClientError::SyncError(_)
        | LightClientError::SyncModeError(_) => ZingolibError::Sync(text),
        LightClientError::SendError(inner) => match inner {
            SendError::ProposeShieldError(_) | SendError::CalculateShieldError(_) => {
                ZingolibError::Shield(text)
            }
            SendError::ProposeSendError(_)
            | SendError::CalculateSendError(_)
            | SendError::RetargetError(_)
            | SendError::NoStoredProposal
            | SendError::TransmissionError(_) => ZingolibError::Send(text),
        },
        LightClientError::ClientError(_) | LightClientError::IndexerError(_) => {
            ZingolibError::Indexer(text)
        }
        LightClientError::FileError(_) => ZingolibError::Save(text),
        LightClientError::WalletError(_) => ZingolibError::Wallet(text),
        LightClientError::Offline => ZingolibError::Offline,
        LightClientError::MigrationError(inner) => match inner {
            MigrationError::NoMigration => ZingolibError::MigrationNotInProgress,
            MigrationError::AlreadyInProgress => ZingolibError::MigrationAlreadyInProgress,
            MigrationError::ConsentStale => ZingolibError::MigrationConsentStale(text),
            MigrationError::CadenceFixed => ZingolibError::MigrationCadenceFixed(text),
            MigrationError::PreSignedUnavailable => ZingolibError::Migration(text),
            MigrationError::SplitDidNotConverge(_)
            | MigrationError::SplitTransactionFailed(_)
            | MigrationError::SplitConfirmationTimeout => ZingolibError::MigrationSplit(text),
        },
    }
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
    col: Option<u32>,
    backtrace: Option<String>,
}

static LAST_PANIC: Lazy<Mutex<PanicReport>> = Lazy::new(|| Mutex::new(PanicReport::default()));

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

            let (file, line, col) = info
                .location()
                .map(|l| (Some(l.file().to_string()), Some(l.line()), Some(l.column())))
                .unwrap_or((None, None, None));

            let bt = Backtrace::force_capture().to_string();

            set_last_panic(PanicReport {
                msg: payload,
                file,
                line,
                col,
                backtrace: Some(bt),
            });
        }));
    });
}

fn clean_backtrace(bt_raw: &str) -> String {
    const DROP: &[&str] = &["<unknown>"];

    let mut out = String::new();

    for line in bt_raw.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if DROP.iter().any(|d| l.contains(d)) {
            continue;
        }

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

// Live progress of the in-flight immediate Orchard->Ironwood drain, held in a
// side channel *outside* the LIGHTCLIENT lock. `drain_orchard_to_ironwood`
// holds LIGHTCLIENT.write() for its whole `block_on`, so a `drain_status` poll
// that read the lightclient would deadlock behind it. Instead the drain stashes
// a cloned `DrainProgressHandle` (an independent Arc<Mutex<Option<DrainStatus>>>)
// here; the poller reads only this global and never contends for the drain's
// wallet/lightclient lock. `None` between drains.
lazy_static! {
    static ref DRAIN_PROGRESS: RwLock<Option<zingolib::lightclient::migrate::DrainProgressHandle>> =
        RwLock::new(None);
}

// Live progress of the in-flight scheduled-parts execute batch (phase 2 of the
// private migration), held in a side channel outside the LIGHTCLIENT lock for
// the same reason as DRAIN_PROGRESS: `execute_due_parts` holds LIGHTCLIENT.write()
// for its whole `block_on`, so an `execute_due_parts_status` poll reading the
// lightclient would deadlock behind it. The batch stashes a cloned
// `BatchProgressHandle` (an independent Arc) here. `None` between batches.
lazy_static! {
    static ref BATCH_PROGRESS: RwLock<Option<zingolib::lightclient::migrate::BatchProgressHandle>> =
        RwLock::new(None);
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
    f(&mut guard)
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

/// Runs `f` against the initialized global lightclient, under the panic
/// guard, with the two infrastructure failures mapped to their typed
/// variants: a poisoned lock is `LightclientLockPoisoned` (unlike
/// `with_lightclient_write`, which recovers), and an absent client is
/// `LightclientNotInitialized`.
fn with_initialized_lightclient<T, F>(f: F) -> Result<T, ZingolibError>
where
    F: FnOnce(&mut LightClient) -> Result<T, ZingolibError> + UnwindSafe,
{
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        match &mut *guard {
            Some(lightclient) => f(lightclient),
            None => Err(ZingolibError::LightclientNotInitialized),
        }
    })
}

/// Read-lock sibling of [`with_initialized_lightclient`], for functions that
/// must not block behind a long-held write lock (planning and status reads).
fn with_initialized_lightclient_read<T, F>(f: F) -> Result<T, ZingolibError>
where
    F: FnOnce(&LightClient) -> Result<T, ZingolibError> + UnwindSafe,
{
    with_panic_guard(|| {
        let guard = LIGHTCLIENT
            .read()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        match &*guard {
            Some(lightclient) => f(lightclient),
            None => Err(ZingolibError::LightclientNotInitialized),
        }
    })
}

struct ConnectionParams {
    chain_type: ChainType,
    wallet_settings: WalletSettings,
    /// `None` in Offline mode: no Indexer is ever configured and the client
    /// stays Indexerless (zingolib ADR 0001). `Some(uri)` when a real server
    /// was selected.
    lightwalletd_uri: Option<http::Uri>,
}

fn build_connection_params(
    uri: String,
    chain_hint: String,
    performance_level: String,
    min_confirmations: u32,
) -> Result<ConnectionParams, ZingolibError> {
    let chain_type = match chain_hint.as_str() {
        "main" => ChainType::Mainnet,
        "test" => ChainType::Testnet,
        "regtest" => ChainType::Regtest(ActivationHeights::default()),
        _ => return Err(ZingolibError::init("Not a valid chain hint!")),
    };

    // Offline Mode = empty uri → no Indexer is ever configured; the client
    // stays Indexerless (zingolib ADR 0001), and `require_indexer()` gates
    // sync/send with `Offline`. A real server yields `Some(uri)`.
    let lightwalletd_uri = if uri.is_empty() {
        None
    } else {
        Some(
            construct_indexer_uri(Some(uri))
                .map_err(|e| ZingolibError::init(format!("Invalid lightwalletd uri: {e}")))?,
        )
    };
    let performancetype = match performance_level.as_str() {
        "Maximum" => PerformanceLevel::Maximum,
        "High" => PerformanceLevel::High,
        "Medium" => PerformanceLevel::Medium,
        "Low" => PerformanceLevel::Low,
        _ => return Err(ZingolibError::init("Not a valid performance level!")),
    };
    let wallet_settings = WalletSettings {
        sync_config: SyncConfig {
            transparent_address_discovery: TransparentAddressDiscovery::minimal(),
            performance_level: performancetype,
        },
        min_confirmations: NonZeroU32::try_from(min_confirmations)
            .map_err(|_| ZingolibError::init("min_confirmations must be greater than 0"))?,
    };

    Ok(ConnectionParams {
        chain_type,
        wallet_settings,
        lightwalletd_uri,
    })
}

fn build_client_config(
    params: &ConnectionParams,
    wallet_config: WalletConfig,
) -> Result<ClientConfig, ZingolibError> {
    let builder = ClientConfig::builder()
        .set_chain_type(params.chain_type)
        .set_wallet_dir(PathBuf::new())
        .set_wallet_config(wallet_config);
    // Offline (no uri) → leave the client Indexerless. Only configure the
    // Indexer when a real server was selected. Mirrors zingo-cli.
    let builder = match params.lightwalletd_uri.clone() {
        Some(uri) => builder.set_indexer_uri(uri),
        None => builder,
    };
    builder.build().map_err(ZingolibError::init)
}

/// The shared spine of the wallet-init FFI functions: tear down any live
/// client, resolve the connection parameters, build the wallet the caller
/// describes, store the new client, and report through `finish` (the seed
/// or the UFVK). Each `init_*` entry point supplies only its
/// `WalletConfig` and its report.
fn init_lightclient(
    server_uri: String,
    chain_hint: String,
    performance_level: String,
    min_confirmations: u32,
    make_wallet_config: impl FnOnce(&ConnectionParams) -> Result<WalletConfig, ZingolibError>
    + UnwindSafe,
    finish: fn() -> Result<String, ZingolibError>,
) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        reset_lightclient();
        let params =
            build_connection_params(server_uri, chain_hint, performance_level, min_confirmations)?;
        let wallet_config = make_wallet_config(&params)?;
        let config = build_client_config(&params, wallet_config)?;
        let lightclient = RT
            .block_on(LightClient::new(config, false))
            .map_err(ZingolibError::init)?;
        let _ = store_client(lightclient);

        finish()
    })
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
    birthday: u32,
    chain_hint: String,
    performance_level: String,
    min_confirmations: u32,
) -> Result<String, ZingolibError> {
    init_lightclient(
        server_uri,
        chain_hint,
        performance_level,
        min_confirmations,
        |params| {
            // Online: ask the Indexer for the chain tip. Offline
            // (Indexerless): there is no server to query, so fall back to
            // zingolib's Library Birthday — a per-chain height already mined
            // when the linked zingolib release was cut, hence always a safe
            // floor for a newly-generated seed (see zingolib ADR 0007). A
            // caller-supplied `birthday > 0` still wins as an explicit
            // override. Mirrors zingo-cli's offline new-wallet path.
            let chain_height = match &params.lightwalletd_uri {
                Some(uri) => {
                    let uri = uri.clone();
                    RT.block_on(async move {
                        let mut indexer = GrpcIndexer::new(uri).await.map_err(|e| e.to_string())?;
                        indexer
                            .get_latest_block(INDEXER_REQUEST_TIMEOUT)
                            .await
                            .map_err(|e| e.to_string())
                    })
                    .map_err(ZingolibError::init)?
                    .height as u32
                }
                None => {
                    if birthday > 0 {
                        birthday
                    } else {
                        lib_birthday(params.chain_type)
                    }
                }
            };
            Ok(WalletConfig::NewSeed {
                no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
                chain_height,
                wallet_settings: params.wallet_settings.clone(),
            })
        },
        get_seed,
    )
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
    init_lightclient(
        server_uri,
        chain_hint,
        performance_level,
        min_confirmations,
        move |params| {
            Ok(WalletConfig::MnemonicPhrase {
                mnemonic_phrase: seed,
                no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
                birthday,
                wallet_settings: params.wallet_settings.clone(),
            })
        },
        get_seed,
    )
}

pub fn init_from_ufvk(
    ufvk: String,
    birthday: u32,
    server_uri: String,
    chain_hint: String,
    performance_level: String,
    min_confirmations: u32,
) -> Result<String, ZingolibError> {
    init_lightclient(
        server_uri,
        chain_hint,
        performance_level,
        min_confirmations,
        move |params| {
            Ok(WalletConfig::Ufvk {
                ufvk,
                birthday,
                wallet_settings: params.wallet_settings.clone(),
            })
        },
        get_ufvk,
    )
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

        let decoded_bytes = match STANDARD.decode(&base64_data) {
            Ok(b) => b,
            Err(e) => {
                // The undecodable payload is wallet material; describe it by
                // size only, never by content (audit Issues A and K).
                return Err(ZingolibError::Init(format!(
                    "Decoding Base64: {}, Size: {}",
                    e,
                    base64_data.len()
                )));
            }
        };

        // Offline (empty server uri) has no server, so the caller-supplied
        // `chain_hint` is meaningless — and the wallet already stores its own
        // chain. Try each chain and keep the one the wallet deserializes under,
        // so an Offline open works regardless of any residual chain value (a
        // mainnet wallet opened while settings still say "test", and vice
        // versa). Online we honor the hint strictly: a chain that disagrees
        // with the selected server is a genuine mismatch and must error.
        let chain_hints: Vec<String> = if server_uri.is_empty() {
            vec![
                "main".to_string(),
                "test".to_string(),
                "regtest".to_string(),
            ]
        } else {
            vec![chain_hint]
        };

        // `LightClient::from_bytes` deserializes the wallet straight from memory.
        // The native layer (Kotlin/Swift) owns all wallet persistence and ships
        // the bytes across the FFI; nothing here touches the filesystem. It reads
        // (and chain-validates) the wallet BEFORE building the indexer, so a chain
        // mismatch fails fast and cheaply — no network is ever dialed, which is
        // what makes trying several chains offline essentially free.
        //
        // (This whole path replaced the previous staging-to-`std::env::temp_dir()`
        // workaround that satisfied v5's `WalletConfig::Read` variant, which failed
        // with `Permission denied (os error 13)` on Android where `TMPDIR` is unset
        // and the app UID cannot write to `/tmp`.)
        let mut built: Option<(LightClient, ConnectionParams)> = None;
        let mut last_error = ZingolibError::init("could not read the wallet with any chain");
        for hint in chain_hints {
            let params = match build_connection_params(
                server_uri.clone(),
                hint,
                performance_level.clone(),
                min_confirmations,
            ) {
                Ok(p) => p,
                Err(e) => {
                    last_error = e;
                    continue;
                }
            };
            let config = match build_client_config(&params, WalletConfig::Read) {
                Ok(c) => c,
                Err(e) => {
                    last_error = e;
                    continue;
                }
            };
            match RT.block_on(LightClient::from_bytes(decoded_bytes.clone(), config)) {
                Ok(l) => {
                    built = Some((l, params));
                    break;
                }
                Err(e) => {
                    last_error = ZingolibError::init(e);
                    continue;
                }
            }
        }

        let (lightclient, params) = match built {
            Some(v) => v,
            None => return Err(last_error),
        };

        // Override the wallet's settings with the caller-supplied ones, since the
        // serialized wallet carries whatever settings it was last saved with.
        let has_seed = RT.block_on(async {
            let mut wallet = lightclient.wallet().write().await;
            wallet.wallet_settings = params.wallet_settings.clone();
            wallet.mnemonic_phrase().is_some()
        });

        let _ = store_client(lightclient);

        if has_seed { get_seed() } else { get_ufvk() }
    })
}

/// Maps a wallet-save result onto the FFI channels structurally: bytes are
/// the wallet export, an absent buffer means no save was needed and crosses
/// as null, and failure travels on the error channel. No shape of success
/// can resemble failure, so no boundary ever classifies content
/// (zingolabs/zingo-mobile#1151; audit Issue Q).
fn map_wallet_save(
    save_result: std::io::Result<Option<Vec<u8>>>,
) -> Result<Option<Vec<u8>>, ZingolibError> {
    save_result.map_err(|e| ZingolibError::Save(e.to_string()))
}

pub fn save_wallet_bytes() -> Result<Option<Vec<u8>>, ZingolibError> {
    // Return the wallet as raw bytes; the platforms own the file format and
    // encode base64 at their write sites.
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let mut wallet = lightclient.wallet().write().await;
            map_wallet_save(wallet.save())
        })
    })
}

/// The save-path contract (zingolabs/zingo-mobile#1151; audit Issue Q), now
/// structural at the boundary: bytes are the export, null means no save was
/// needed, and failure is typed. The historical attack string — a base64
/// wallet export beginning with "error" — is no longer representable,
/// because no string crosses this boundary at all.
#[cfg(test)]
mod wallet_export_tests {
    use super::*;

    #[test]
    fn save_failure_travels_on_the_error_channel() {
        let error = map_wallet_save(Err(std::io::Error::other("disk full")))
            .expect_err("a failed save must be typed, not prose in the data channel");
        assert!(
            matches!(error, ZingolibError::Save(_)),
            "the failure must be the typed Save variant: {error}"
        );
    }

    #[test]
    fn wallet_bytes_cross_the_data_channel_verbatim() {
        let wallet_bytes = vec![0xde, 0xad, 0xbe, 0xef];
        let crossed = map_wallet_save(Ok(Some(wallet_bytes.clone())))
            .expect("wallet bytes travel on the data channel");
        assert_eq!(crossed, Some(wallet_bytes));
    }

    #[test]
    fn no_save_needed_crosses_as_null() {
        let crossed = map_wallet_save(Ok(None)).expect("no-save is not a failure");
        assert_eq!(crossed, None);
    }
}

/// The init-path data/error channel contract (zingo-mobile#1151): domain
/// failures travel on the error channel as the typed `Init` variant, never
/// as prose in the data channel. Every case here fails before any network
/// dial, so the tests run host-side with no infrastructure.
#[cfg(test)]
mod init_error_channel_tests {
    use super::*;

    #[test]
    fn invalid_server_uri_travels_on_the_error_channel() {
        let error = init_new(
            "http://an invalid uri with spaces".to_string(),
            0,
            "main".to_string(),
            "Medium".to_string(),
            1,
        )
        .expect_err("an invalid lightwalletd uri must be typed, not prose in the data channel");
        assert!(
            matches!(error, ZingolibError::Init(_)),
            "the failure must be the typed Init variant: {error}"
        );
    }

    #[test]
    fn invalid_performance_level_travels_on_the_error_channel() {
        let error = init_from_seed(
            "unvalidated at this point".to_string(),
            1,
            String::new(),
            "main".to_string(),
            "NotALevel".to_string(),
            1,
        )
        .expect_err("an invalid performance level must be typed, not prose in the data channel");
        assert!(
            matches!(error, ZingolibError::Init(_)),
            "the failure must be the typed Init variant: {error}"
        );
    }

    #[test]
    fn ufvk_restore_failure_travels_on_the_error_channel() {
        let error = init_from_ufvk(
            "unvalidated at this point".to_string(),
            1,
            String::new(),
            "main".to_string(),
            "NotALevel".to_string(),
            1,
        )
        .expect_err("an invalid performance level must be typed, not prose in the data channel");
        assert!(
            matches!(error, ZingolibError::Init(_)),
            "the failure must be the typed Init variant: {error}"
        );
    }

    #[test]
    fn undecodable_wallet_base64_travels_on_the_error_channel() {
        let error = init_from_b64(
            "!!!not-base64!!!".to_string(),
            String::new(),
            "main".to_string(),
            "Medium".to_string(),
            1,
        )
        .expect_err("undecodable wallet bytes must be typed, not prose in the data channel");
        assert!(
            matches!(error, ZingolibError::Init(_)),
            "the failure must be the typed Init variant: {error}"
        );
        assert!(
            !error.to_string().contains("!!!not-base64!!!"),
            "the failure must not embed the payload it could not decode: {error}"
        );
    }
}

/// The sync/rescan data/error channel contract (zingo-mobile#1151): with no
/// initialized client, every call fails typed — never as prose in the data
/// channel. The domain arms (Sync, Rescan) need a live wallet and server, so
/// they are covered by the platform tests; these pin the one failure
/// reachable host-side. nextest runs each test in its own process, so the
/// LIGHTCLIENT global is reliably uninitialized.
#[cfg(test)]
mod sync_error_channel_tests {
    use super::*;

    fn assert_uninitialized(result: Result<String, ZingolibError>, ffi: &str) {
        let error = result.expect_err("an uninitialized client must fail typed");
        assert!(
            matches!(error, ZingolibError::LightclientNotInitialized),
            "{ffi} must fail with the typed uninitialized variant: {error}"
        );
    }

    #[test]
    fn run_sync_fails_typed_without_a_client() {
        assert_uninitialized(run_sync(), "run_sync");
    }

    #[test]
    fn pause_sync_fails_typed_without_a_client() {
        assert_uninitialized(pause_sync(), "pause_sync");
    }

    #[test]
    fn status_sync_fails_typed_without_a_client() {
        assert_uninitialized(status_sync(), "status_sync");
    }

    #[test]
    fn poll_sync_fails_typed_without_a_client() {
        assert_uninitialized(poll_sync(), "poll_sync");
    }

    #[test]
    fn run_rescan_fails_typed_without_a_client() {
        assert_uninitialized(run_rescan(), "run_rescan");
    }
}

/// The read-path data/error channel contract (zingo-mobile#1151): a read
/// getter's failure travels typed on the error channel — never as prose in
/// the data channel. The wallet-read domain arms need a live wallet, so the
/// platform tests cover them; these pin the failures reachable host-side:
/// the uninitialized client for every getter, and the pre-dial uri parse
/// for the server probe.
#[cfg(test)]
mod read_error_channel_tests {
    use super::*;

    fn assert_uninitialized(result: Result<String, ZingolibError>, ffi: &str) {
        let error = result.expect_err("an uninitialized client must fail typed");
        assert!(
            matches!(error, ZingolibError::LightclientNotInitialized),
            "{ffi} must fail with the typed uninitialized variant: {error}"
        );
    }

    #[test]
    fn get_balance_fails_typed_without_a_client() {
        assert_uninitialized(get_balance(), "get_balance");
    }

    #[test]
    fn get_spendable_balance_total_fails_typed_without_a_client() {
        assert_uninitialized(get_spendable_balance_total(), "get_spendable_balance_total");
    }

    #[test]
    fn get_value_transfers_fails_typed_without_a_client() {
        assert_uninitialized(get_value_transfers(), "get_value_transfers");
    }

    #[test]
    fn get_messages_fails_typed_without_a_client() {
        assert_uninitialized(get_messages(String::new()), "get_messages");
    }

    #[test]
    fn invalid_server_uri_travels_on_the_error_channel() {
        let error = get_latest_block_server("http://an invalid uri with spaces".to_string())
            .expect_err("an unparseable server uri must be typed, not prose in the data channel");
        assert!(
            matches!(error, ZingolibError::Read(_)),
            "the failure must be the typed Read variant: {error}"
        );
    }

    // Reproduces the exact `From<ValueTransfers>` shape (array nested under
    // "value_transfers") to prove the migrated-amount splice targets the right
    // field: only send-to-self transfers with a mapped txid are overwritten.
    #[test]
    fn splice_migrated_values_overwrites_only_mapped_self_sends() {
        let mut json_vts = json::object! {
            "value_transfers" => json::array![
                json::object!{ "kind" => "send-to-self", "txid" => "aaa", "value" => 0 },
                json::object!{ "kind" => "sent", "txid" => "bbb", "value" => 100 },
                json::object!{ "kind" => "send-to-self", "txid" => "ccc", "value" => 0 },
            ]
        };
        let mut migrated_by_txid = std::collections::HashMap::new();
        migrated_by_txid.insert("aaa".to_string(), 500u64); // a migration
        // "ccc" is a self-send but not a migration (no mapping) -> untouched.

        splice_migrated_values(&mut json_vts, &migrated_by_txid);

        assert_eq!(
            json_vts["value_transfers"][0]["value"].as_u64(),
            Some(500),
            "the mapped send-to-self must carry the migrated amount"
        );
        assert_eq!(
            json_vts["value_transfers"][1]["value"].as_u64(),
            Some(100),
            "a plain send must be left alone"
        );
        assert_eq!(
            json_vts["value_transfers"][2]["value"].as_u64(),
            Some(0),
            "an unmapped self-send must be left alone"
        );
    }
}

pub fn get_developer_donation_address() -> Result<String, ZingolibError> {
    with_panic_guard(|| Ok(zingolib::DEVELOPER_DONATION_ADDRESS.to_string()))
}

pub fn get_zennies_for_zingo_donation_address() -> Result<String, ZingolibError> {
    with_panic_guard(|| Ok(zingolib::ZENNIES_FOR_ZINGO_DONATION_ADDRESS.to_string()))
}

pub fn set_crypto_default_provider_to_ring() -> Result<String, ZingolibError> {
    with_panic_guard(|| match CryptoProvider::get_default() {
        Some(_) => Ok("true".to_string()),
        None => match default_provider().install_default() {
            Ok(_) => Ok("true".to_string()),
            Err(_) => Err(ZingolibError::Init(
                "failed to install the ring crypto provider".to_string(),
            )),
        },
    })
}

pub fn get_latest_block_server(server_uri: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let lightwalletd_uri: http::Uri = server_uri
            .parse()
            .map_err(|e| ZingolibError::read(format!("failed to parse uri. {e}")))?;
        RT.block_on(async move {
            let mut indexer = GrpcIndexer::new(lightwalletd_uri)
                .await
                .map_err(ZingolibError::read)?;
            let block_id = indexer
                .get_latest_block(INDEXER_REQUEST_TIMEOUT)
                .await
                .map_err(ZingolibError::read)?;
            Ok(block_id.height.to_string())
        })
    })
}

pub fn get_latest_block_wallet() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet().read().await;
                object! { "height" => json::JsonValue::from(wallet.sync_state.last_known_chain_height().map_or(0, u32::from))}.pretty(2)
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

/// Overwrites the `value` of each Orchard->Ironwood migration value transfer
/// with the amount migrated. `json_vts` is the `{ "value_transfers": [ ... ] }`
/// object produced by `From<ValueTransfers>`; migrations are the send-to-self
/// transfers keyed in `migrated_by_txid`. zingolib reports `value == 0` for
/// self-sends, so without this the migrated amount is invisible.
///
/// Note the array is nested under `"value_transfers"` — `members_mut()` on the
/// outer object yields an empty iterator, so we must index in first.
fn splice_migrated_values(
    json_vts: &mut json::JsonValue,
    migrated_by_txid: &std::collections::HashMap<String, u64>,
) {
    for vt in json_vts["value_transfers"].members_mut() {
        if vt["kind"].as_str() != Some("send-to-self") {
            continue;
        }
        let txid = vt["txid"].as_str().unwrap_or_default().to_string();
        if let Some(&migrated) = migrated_by_txid.get(&txid) {
            vt["value"] = json::JsonValue::from(migrated);
        }
    }
}

pub fn get_value_transfers() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let wallet = lightclient.wallet().read().await;

            // An Orchard -> Ironwood migration is a send-to-self, which zingolib
            // reports with `value == 0` because `total_value_sent` excludes
            // self-addressed value. To surface how much was migrated, recover it
            // from the transaction's self-received ironwood notes and splice it
            // into the matching value transfer's `value`. Keyed by txid; only
            // self-sends funded from Orchard that land ironwood notes qualify.
            let migrated_by_txid: std::collections::HashMap<String, u64> =
                match wallet.transaction_summaries(true).await {
                    Ok(summaries) => summaries
                        .0
                        .iter()
                        .filter(|s| {
                            s.kind.to_string() == "send-to-self"
                                && s.pools_sent_from.iter().any(|p| p.to_string() == "Orchard")
                                && !s.ironwood_notes.is_empty()
                        })
                        .map(|s| {
                            (
                                s.txid.to_string(),
                                s.ironwood_notes.iter().map(|n| n.value).sum::<u64>(),
                            )
                        })
                        .collect(),
                    Err(e) => return Err(ZingolibError::read(e)),
                };

            match wallet.value_transfers(true).await {
                Ok(value_transfers) => {
                    let mut json_vts = json::JsonValue::from(value_transfers);
                    splice_migrated_values(&mut json_vts, &migrated_by_txid);
                    Ok(json_vts.pretty(2))
                }
                Err(e) => Err(ZingolibError::read(e)),
            }
        })
    })
}

pub fn poll_sync() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| match lightclient.poll_sync() {
        PollReport::NoHandle => Ok("Sync task has not been launched.".to_string()),
        PollReport::NotReady => Ok("Sync task is not complete.".to_string()),
        PollReport::Ready(result) => match result {
            Ok(sync_result) => Ok(json::object! {
                "sync_complete" => json::JsonValue::from(sync_result)
            }
            .pretty(2)),
            Err(e) => Err(ZingolibError::sync(e)),
        },
    })
}

fn run_sync() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        if lightclient.sync_mode() == SyncMode::Paused {
            // resume_sync can race: sync_mode() was Paused a moment ago but the
            // task may have advanced before we got here. Return the error typed
            // instead of `expect` — panicking would poison LIGHTCLIENT.
            match lightclient.resume_sync() {
                Ok(_) => Ok("Resuming sync task...".to_string()),
                Err(e) => Err(ZingolibError::sync(e)),
            }
        } else {
            RT.block_on(async {
                match lightclient.sync().await {
                    Ok(_) => Ok("Launching sync task...".to_string()),
                    // Launching is idempotent: a concurrent launch means the
                    // desired state — a running sync — already holds, so it
                    // reports as status, not failure. Before the typed-error
                    // migration this crossed as in-band prose every consumer
                    // ignored; throwing it broke the send integration test's
                    // (historically benign) second launch.
                    Err(LightClientError::SyncModeError(SyncModeError::SyncAlreadyRunning)) => {
                        Ok("Sync task already running.".to_string())
                    }
                    Err(e) => Err(ZingolibError::sync(e)),
                }
            })
        }
    })
}

pub fn pause_sync() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| match lightclient.pause_sync() {
        Ok(_) => Ok("Pausing sync task...".to_string()),
        Err(e) => Err(ZingolibError::sync(e)),
    })
}

fn status_sync() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async {
            let wallet = lightclient.wallet().read().await;
            match pepper_sync::sync_status(&*wallet).await {
                Ok(status) => Ok(json::JsonValue::from(status).pretty(2)),
                Err(e) => Err(ZingolibError::sync(e)),
            }
        })
    })
}

pub fn run_rescan() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            match lightclient.rescan().await {
                Ok(_) => Ok("Launching rescan...".to_string()),
                Err(e) => Err(ZingolibError::Rescan(e.to_string())),
            }
        })
    })
}

pub fn info_server() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let info = lightclient.info().await.map_err(ffi_error)?;
            // Read from the WALLET's chain rather than the server's, so it
            // agrees with `chain_name_short` and stays right when the two
            // disagree.
            let ironwood_activation = ironwood_activation_height(lightclient.chain_type());
            let mut val = json::JsonValue::from(info);
            val["ironwood_activation_height"] = match ironwood_activation {
                Some(height) => height.into(),
                None => json::JsonValue::Null,
            };
            Ok(val.pretty(2))
        })
    })
}

/// Ironwood (NU6.3) activation height for `chain`, straight from zingolib's
/// consensus parameters — mainnet and testnet delegate to `zcash_protocol`'s
/// table, and regtest reports whatever its `ActivationHeights` were built
/// with. Sourcing it here keeps the app on exactly the schedule the wallet
/// backend transacts against, instead of a copy that has to be kept in step by
/// hand.
fn ironwood_activation_height(chain: ChainType) -> Option<u32> {
    chain
        .activation_height(NetworkUpgrade::Nu6_3)
        .map(u32::from)
}

/// The loaded wallet's chain as the short token the JS layer uses
/// (`ChainNameEnum`: "main" / "test" / "regtest"). Read straight from the
/// wallet, so it is reliable even Offline (no server).
fn chain_name_short(chain: ChainType) -> &'static str {
    match chain {
        ChainType::Mainnet => "main",
        ChainType::Testnet => "test",
        ChainType::Regtest(_) => "regtest",
    }
}

// TODO: rename "get_seed_phrase" or "get_mnemonic_phrase"
// or if other recovery info is being used could rename "get_recovery_info" ?
pub fn get_seed() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let wallet = lightclient.wallet().read().await;
            let recovery_info = wallet.recovery_info().ok_or_else(|| {
                ZingolibError::Read(
                    "get seed. no mnemonic found. wallet loaded from key.".to_string(),
                )
            })?;
            // Surface the wallet's own chain alongside the recovery
            // info so the JS layer can track it even Offline.
            let mut val = serde_json::to_value(&recovery_info).unwrap_or(serde_json::Value::Null);
            if let Some(obj) = val.as_object_mut() {
                obj.insert(
                    "chain_name".to_string(),
                    serde_json::Value::String(chain_name_short(wallet.chain_type()).to_string()),
                );
            }
            serde_json::to_string_pretty(&val)
                .map_err(|_| ZingolibError::Read("get seed. failed to serialize".to_string()))
        })
    })
}

pub fn get_ufvk() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let wallet = lightclient.wallet().read().await;
            let ufvk: UnifiedFullViewingKey = wallet
                .unified_key_store
                .get(&AccountId::ZERO)
                .expect("account 0 must always exist")
                .try_into()
                .map_err(|e| ZingolibError::Read(format!("{e}")))?;
            Ok(object! {
                "ufvk" => ufvk.encode(&wallet.chain_type()),
                "birthday" => u32::from(wallet.birthday()),
                "chain_name" => chain_name_short(wallet.chain_type())
            }
            .pretty(2))
        })
    })
}

pub fn change_server(server_uri: String) -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        let uri = if server_uri.is_empty() {
            // Offline: no server. `http::Uri::default()` is scheme-less and
            // `set_indexer_uri` rejects it ("bad uri: invalid scheme"), so
            // hand it the chain's default indexer URI instead —
            // syntactically valid, and never actually dialed while offline.
            let default = match lightclient.chain_type() {
                ChainType::Mainnet => DEFAULT_INDEXER_URI,
                ChainType::Testnet => DEFAULT_INDEXER_URI_TESTNET,
                ChainType::Regtest(_) => DEFAULT_INDEXER_URI,
            };
            construct_indexer_uri(Some(default.to_string()))
                .map_err(|_| ZingolibError::InvalidInput("invalid server uri".to_string()))?
        } else {
            construct_indexer_uri(Some(server_uri))
                .map_err(|_| ZingolibError::InvalidInput("invalid server uri".to_string()))?
        };
        RT.block_on(async move {
            lightclient
                .set_indexer_uri(uri)
                .await
                .map_err(|e| ffi_error(e.into()))?;
            Ok("server set".to_string())
        })
    })
}

pub fn wallet_kind() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet().read().await;
                if wallet.mnemonic_phrase().is_some() {
                    object! {"kind" => "Loaded from seed or mnemonic phrase",
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
            Err(ZingolibError::InvalidInput(
                "the address is empty".to_string(),
            ))
        } else {
            fn make_decoded_chain_pair(
                address: &str,
            ) -> Option<(zcash_client_backend::address::Address, ChainType)> {
                [
                    ChainType::Mainnet,
                    ChainType::Testnet,
                    ChainType::Regtest(ActivationHeights::default()),
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
            Err(ZingolibError::InvalidInput("the ufvk is empty".to_string()))
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
    with_panic_guard(|| Ok(zingolib::git_description().to_string()))
}

pub fn get_messages(address: String) -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            match lightclient
                .messages_containing(Some(address.as_str()))
                .await
            {
                Ok(value_transfers) => Ok(json::JsonValue::from(value_transfers).pretty(2)),
                Err(e) => Err(ZingolibError::read(e)),
            }
        })
    })
}

pub fn get_balance() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            match lightclient.account_balance(AccountId::ZERO).await {
                Ok(bal) => Ok(json::JsonValue::from(bal).pretty(2)),
                Err(e) => Err(ZingolibError::read(e)),
            }
        })
    })
}

pub fn get_total_memobytes_to_address() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            match lightclient.do_total_memobytes_to_address().await {
                Ok(total_memo_bytes) => Ok(json::JsonValue::from(total_memo_bytes).pretty(2)),
                Err(e) => Err(ZingolibError::read(e)),
            }
        })
    })
}

pub fn get_total_value_to_address() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            match lightclient.do_total_value_to_address().await {
                Ok(total_values) => Ok(json::JsonValue::from(total_values).pretty(2)),
                Err(e) => Err(ZingolibError::read(e)),
            }
        })
    })
}

pub fn get_total_spends_to_address() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            match lightclient.do_total_spends_to_address().await {
                Ok(total_spends) => Ok(json::JsonValue::from(total_spends).pretty(2)),
                Err(e) => Err(ZingolibError::read(e)),
            }
        })
    })
}

pub fn zec_price() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let mut wallet = lightclient.wallet().write().await;
            let price = wallet
                .update_current_price()
                .await
                .map_err(ZingolibError::read)?;
            Ok(object! { "current_price" => price }.pretty(2))
        })
    })
}

pub fn remove_transaction(txid: String) -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        let txid = txid_from_hex_encoded_str(&txid)
            .map_err(|e| ZingolibError::InvalidInput(e.to_string()))?;
        RT.block_on(async move {
            let mut wallet = lightclient.wallet().write().await;
            wallet
                .remove_failed_transaction(txid)
                .map_err(|e| ZingolibError::Wallet(e.to_string()))?;
            Ok("Successfully removed transaction.".to_string())
        })
    })
}

// we don't use this anymore...
pub fn get_spendable_balance_with_address(
    address: String,
    zennies: String,
) -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        let address = address_from_str(&address)
            .map_err(|_| ZingolibError::InvalidInput("unknown address format".to_string()))?;
        let zennies = zennies.parse().map_err(|_| {
            ZingolibError::InvalidInput("failed to parse zennies setting".to_string())
        })?;
        RT.block_on(async move {
            let bal = lightclient
                .max_send_value(address, zennies, AccountId::ZERO)
                .await
                .map_err(|e| ffi_error(SendError::from(e).into()))?;
            Ok(object! { "spendable_balance" => bal.into_u64() }.pretty(2))
        })
    })
}

pub fn get_spendable_balance_total() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let wallet = lightclient.wallet().read().await;
            let spendable_balance = wallet
                .shielded_spendable_balance(AccountId::ZERO, false)
                .map_err(ZingolibError::read)?;
            Ok(object! {
                "spendable_balance" => spendable_balance.into_u64(),
            }
            .pretty(2))
        })
    })
}

pub fn set_option_wallet() -> Result<String, ZingolibError> {
    with_panic_guard(|| Err(ZingolibError::Wallet("unimplemented".to_string())))
}

pub fn get_option_wallet() -> Result<String, ZingolibError> {
    with_panic_guard(|| Err(ZingolibError::Wallet("unimplemented".to_string())))
}

pub fn get_unified_addresses() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move { lightclient.unified_addresses_json().await.pretty(2) }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn get_transparent_addresses() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(
                RT.block_on(
                    async move { lightclient.transparent_addresses_json().await.pretty(2) },
                ),
            )
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn create_new_unified_address(receivers: String) -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let mut wallet = lightclient.wallet().write().await;
            let network = wallet.chain_type();
            let receivers_available = ReceiverSelection {
                orchard: receivers.contains('o'),
                sapling: receivers.contains('z'),
            };
            let (id, unified_address) = wallet
                .generate_unified_address(receivers_available, AccountId::ZERO)
                .map_err(|e| ZingolibError::Wallet(e.to_string()))?;
            Ok(json::object! {
                "account" => u32::from(AccountId::ZERO),
                "address_index" => id.address_index,
                "has_orchard" => unified_address.has_orchard(),
                "has_sapling" => unified_address.has_sapling(),
                "has_transparent" => unified_address.has_transparent(),
                "encoded_address" => unified_address.encode(&network),
            }
            .pretty(2))
        })
    })
}

pub fn create_new_transparent_address() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let mut wallet = lightclient.wallet().write().await;
            let network = wallet.chain_type();
            let (id, transparent_address) = wallet
                .generate_transparent_address(AccountId::ZERO, true)
                .map_err(|e| ZingolibError::Wallet(e.to_string()))?;
            Ok(json::object! {
                "account" => u32::from(id.account_id()),
                "address_index" => id.address_index().index(),
                "scope" => id.scope().to_string(),
                "encoded_address" => transparent::encode_address(&network,  transparent_address),
            }
            .pretty(2))
        })
    })
}

pub fn check_my_address(address: String) -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let wallet = lightclient.wallet().read().await;
            let address_ref = wallet
                .is_address_derived_by_keys(&address)
                .map_err(|e| ZingolibError::Wallet(e.to_string()))?;
            Ok(address_ref
                .map_or(
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
                )
                .pretty(2))
        })
    })
}

pub fn get_wallet_save_required() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let save_required = lightclient.is_save_required().await;
                object! { "save_required" => save_required }.pretty(2)
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

pub fn set_config_wallet_to_test() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let mut wallet = lightclient.wallet().write().await;
                wallet.wallet_settings.min_confirmations = NonZeroU32::try_from(1).unwrap();
                wallet.wallet_settings.sync_config.performance_level = PerformanceLevel::Medium;
                wallet.mark_dirty();
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
    with_initialized_lightclient(|lightclient| {
        let performancetype = match performance_level.as_str() {
            "Maximum" => PerformanceLevel::Maximum,
            "High" => PerformanceLevel::High,
            "Medium" => PerformanceLevel::Medium,
            "Low" => PerformanceLevel::Low,
            _ => {
                return Err(ZingolibError::InvalidInput(
                    "not a valid performance level".to_string(),
                ));
            }
        };
        let min_confirmations = NonZeroU32::try_from(min_confirmations).map_err(|_| {
            ZingolibError::InvalidInput("min_confirmations must be greater than 0".to_string())
        })?;
        RT.block_on(async move {
            let mut wallet = lightclient.wallet().write().await;
            wallet.wallet_settings.min_confirmations = min_confirmations;
            wallet.wallet_settings.sync_config.performance_level = performancetype;
            wallet.mark_dirty();
            Ok("Successfully set config wallet to prod.".to_string())
        })
    })
}

pub fn get_config_wallet_performance() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet().read().await;
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
        let mut guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                let wallet = lightclient.wallet().read().await;
                let current_version = wallet.current_version();
                let read_version = wallet.read_version();
                object! {
                    "current_version" => current_version,
                    "read_version" => read_version
                }
                .pretty(2)
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
        .map_err(|_| format!("creating output. Memo '{:?}' is too long", memo_str))
}

pub fn send(send_json: String) -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let json_args = json::parse(&send_json)
                .map_err(|_| ZingolibError::InvalidInput("it is not a valid JSON".to_string()))?;

            let mut receivers = Receivers::new();
            for j in json_args.members() {
                let recipient_address = match j["address"].as_str() {
                    Some(addr) => ZcashAddress::try_from_encoded(addr).map_err(|e| {
                        ZingolibError::InvalidInput(format!("invalid address: {e}"))
                    })?,
                    None => {
                        return Err(ZingolibError::InvalidInput("missing address".to_string()));
                    }
                };

                let amount = match j["amount"].as_u64() {
                    Some(a) => Zatoshis::from_u64(a)
                        .map_err(|e| ZingolibError::InvalidInput(format!("invalid amount: {e}")))?,
                    None => {
                        return Err(ZingolibError::InvalidInput("missing amount".to_string()));
                    }
                };

                let memo =
                    match j["memo"].as_str() {
                        Some(m) => Some(interpret_memo_string(m.to_string()).map_err(|e| {
                            ZingolibError::InvalidInput(format!("invalid memo: {e}"))
                        })?),
                        None => None,
                    };

                receivers.push(zingolib::data::receivers::Receiver {
                    recipient_address,
                    amount,
                    memo,
                });
            }

            let request = transaction_request_from_receivers(receivers)
                .map_err(|e| ZingolibError::InvalidInput(format!("request error: {e}")))?;

            let proposal = lightclient
                .propose_send(request, AccountId::ZERO)
                .await
                .map_err(|e| ffi_error(SendError::from(e).into()))?;
            let fee = total_fee(&proposal).map_err(|e| ZingolibError::Send(e.to_string()))?;
            Ok(object! { "fee" => fee.into_u64() }.pretty(2))
        })
    })
}

pub fn shield() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let proposal = lightclient
                .propose_shield(AccountId::ZERO)
                .await
                .map_err(|e| ffi_error(SendError::from(e).into()))?;
            if proposal.steps().len() != 1 {
                return Err(ZingolibError::InvalidInput(
                    "shielding transactions should not have multiple proposal steps".to_string(),
                ));
            }
            let step = proposal.steps().first();
            let value_to_shield = step
                .balance()
                .proposed_change()
                .iter()
                .try_fold(Zatoshis::ZERO, |acc, c| acc + c.value())
                .ok_or_else(|| {
                    ZingolibError::InvalidInput(
                        "shield amount outside valid range of zatoshis".to_string(),
                    )
                })?;
            let fee = step.balance().fee_required();
            Ok(object! {
                "value_to_shield" => value_to_shield.into_u64(),
                "fee" => fee.into_u64(),
            }
            .pretty(2))
        })
    })
}

pub fn confirm() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let txids = lightclient
                .send_stored_proposal(true)
                .await
                .map_err(ffi_error)?;
            Ok(
                object! { "txids" => txids.iter().map(|txid| txid.to_string()).collect::<Vec<_>>() }
                    .pretty(2),
            )
        })
    })
}

/// Plans an immediate drain of the account's Orchard pool into Ironwood. Pure
/// and deterministic: nothing is signed or broadcast, so the plan can be shown
/// to the user for consent first. Mirror of `send`'s propose phase.
///
/// Returns, on success, the drain plan as JSON:
/// `{ transactions: [{ inputs: [u64], output: u64, fee: u64 }], migrated,
/// fee, stranded }` (all zatoshis). Each transaction spends its `inputs`
/// Orchard notes into a single Ironwood `output`; the fee is `sum(inputs) -
/// output`. An empty `transactions` array means there is nothing worth
/// migrating.
pub fn plan_orchard_drain() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let plan = lightclient
                .plan_orchard_drain(AccountId::ZERO)
                .await
                .map_err(ffi_error)?;
            let transactions = plan
                .transactions
                .iter()
                .map(|tx| {
                    object! {
                        "inputs" => tx.inputs.clone(),
                        "output" => tx.output,
                        "fee" => tx.fee(),
                    }
                })
                .collect::<Vec<_>>();
            Ok(object! {
                "transactions" => transactions,
                "migrated" => plan.migrated,
                "fee" => plan.fee,
                "stranded" => plan.stranded,
            }
            .pretty(2))
        })
    })
}

/// Spends every spendable Orchard note in the account into the Ironwood pool in
/// one round of transactions, broadcasting them all at once. This is the
/// *migrate immediately* path (`plan_orchard_drain` executed): the amount
/// crossing the pool boundary is visible on-chain, so the caller must have
/// disclosed that. Mirror of `confirm`'s broadcast phase.
///
/// Uses zingolib's `drain_orchard_to_ironwood_presynced`: this app owns the
/// sync lifecycle (a background sync runs continuously), so the drain must NOT
/// launch its own sync — the syncing variant `drain_orchard_to_ironwood`
/// collides with the running sync and fails with `SyncAlreadyRunning`. It
/// drains against current wallet state instead, matching `send`/`shield`.
///
/// Returns, on success, `{ txids: [..], migrated, fee, stranded }` (values in
/// zatoshis). Notes worth at most the sweep minimum are left behind and
/// reported as `stranded`.
pub fn drain_orchard_to_ironwood() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        // Publish this drain's progress handle to the DRAIN_PROGRESS side
        // channel *before* the block_on. We hold LIGHTCLIENT.write() for the
        // whole drain, so a concurrent `drain_status` poll must read this
        // handle (an independent Arc) rather than the lightclient. The handle
        // reads idle (`null`) until the drain arms it after planning.
        if let Ok(mut progress) = DRAIN_PROGRESS.write() {
            *progress = Some(lightclient.drain_progress_handle());
        }
        let out = RT.block_on(async move {
            // Pause our continuous background sync before the drain plans:
            // the presynced drain requires a SyncPauseGuard so the plan and
            // the build observe one stable wallet state (dropping `sync` at
            // the end of this block resumes the engine). Without it, planning
            // could select notes the running sync then mutates underneath,
            // between the plan and the build.
            let sync = lightclient
                .pause_sync_scoped()
                .map_err(|e| ffi_error(e.into()))?;
            let summary = lightclient
                .drain_orchard_to_ironwood_presynced(AccountId::ZERO, &sync)
                .await
                .map_err(ffi_error)?;
            Ok(object! {
                "txids" => summary.txids.iter().map(|txid| txid.to_string()).collect::<Vec<_>>(),
                "migrated" => summary.migrated,
                "fee" => summary.fee,
                "stranded" => summary.stranded,
            }
            .pretty(2))
        });
        // Drop the stale handle. The drain's own scope guard already reset
        // the snapshot to idle on the way out, so a late poll reads `null`
        // whether or not this clear has landed yet.
        if let Ok(mut progress) = DRAIN_PROGRESS.write() {
            *progress = None;
        }
        out
    })
}

/// A snapshot of the in-flight immediate drain's progress, for rendering
/// "Building i/N" then "Broadcasting i/N" after the user presses Accept. Reads
/// the DRAIN_PROGRESS side channel only, never LIGHTCLIENT, so it stays
/// responsive while `drain_orchard_to_ironwood` holds the lightclient lock
/// across its whole build-and-broadcast loop.
///
/// Returns, while a drain runs:
/// `{ total, built, sent, phase }` where `phase` is `"building"` or
/// `"transmitting"` and the counts are `0..=total`. Returns JSON `null` when no
/// drain is in flight (before it starts, or once it has finished).
pub fn drain_status() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        // Snapshot under a brief read lock, then drop it: the drain only touches
        // DRAIN_PROGRESS at its very start and end, so this never blocks on the
        // long-running drain the way a LIGHTCLIENT read would.
        let status = {
            let progress = DRAIN_PROGRESS
                .read()
                .map_err(|_| ZingolibError::SideChannelPoisoned)?;
            progress.as_ref().and_then(|handle| handle.status())
        };
        Ok(match status {
            Some(s) => {
                use zingolib::lightclient::migrate::DrainPhase;
                object! {
                    "total" => s.total,
                    "built" => s.built,
                    "sent" => s.sent,
                    "phase" => match s.phase {
                        DrainPhase::Building => "building",
                        DrainPhase::Transmitting => "transmitting",
                    },
                }
                .pretty(2)
            }
            None => json::JsonValue::Null.pretty(2),
        })
    })
}

// ----- ZIP 318 private migration (note splitting + scheduled parts) -----

/// Lowercase-hex rendering of a consent hash, the form the plan JSON carries
/// and `start_ironwood_migration` accepts back.
fn hash32_to_hex(hash: &[u8; 32]) -> String {
    hash.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Total inverse of [`hash32_to_hex`]: `None` for anything but 64 hex digits.
fn hex_to_hash32(hex_str: &str) -> Option<[u8; 32]> {
    hex::decode(hex_str).ok()?.try_into().ok()
}

fn migration_phase_json(phase: &MigrationPhase) -> json::JsonValue {
    match phase {
        MigrationPhase::Planned => object! { "kind" => "planned" },
        MigrationPhase::NoteSplitting {
            round,
            pending_txids,
        } => object! {
            "kind" => "note_splitting",
            "round" => *round,
            "pending_txids" => pending_txids
                .iter()
                .map(|txid| txid.to_string())
                .collect::<Vec<_>>(),
        },
        MigrationPhase::PartsScheduled => object! { "kind" => "parts_scheduled" },
        MigrationPhase::Complete { residual } => object! {
            "kind" => "complete",
            "residual" => *residual,
        },
    }
}

/// Plans the ZIP 318 private migration from the account's current spendable
/// Orchard notes. Pure and deterministic: nothing is signed or sent, so the
/// plan can be rendered for consent. Mirror of `plan_orchard_drain` for the
/// split path.
///
/// Returns, on success, the plan as JSON: `{ split_rounds:
/// [[{ inputs: [u64], outputs: [u64], fee: u64 }]], parts: [u64], split_fee,
/// parts_fee, stranded, plan_hash }` (values in zatoshis, `plan_hash` in hex).
/// Consent must disclose `stranded`. Pass `plan_hash` back to
/// `start_ironwood_migration` unchanged.
pub fn plan_ironwood_migration() -> Result<String, ZingolibError> {
    with_initialized_lightclient_read(|lightclient| {
        RT.block_on(async move {
            let plan = lightclient
                .plan_ironwood_migration(AccountId::ZERO)
                .await
                .map_err(ffi_error)?;
            let params = {
                let wallet = lightclient.wallet().read().await;
                MigrationParams::provisional(wallet.chain_type())
            };
            let split_rounds = plan
                .split_rounds
                .iter()
                .map(|round| {
                    round
                        .iter()
                        .map(|tx| {
                            object! {
                                "inputs" => tx.inputs.clone(),
                                "outputs" => tx.outputs.clone(),
                                "fee" => tx.fee(),
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            Ok(object! {
                "split_rounds" => split_rounds,
                "parts" => plan.parts.clone(),
                "split_fee" => plan.split_fee(),
                "parts_fee" => plan.parts_fee(&params),
                "stranded" => plan.stranded,
                "plan_hash" => hash32_to_hex(&plan_hash(&plan)),
            }
            .pretty(2))
        })
    })
}

/// Records the user's consent to the exact plan they were shown (by its
/// `plan_hash` from `plan_ironwood_migration`) and persists the migration
/// state. Nothing is broadcast here; `continue_note_splitting` drives the
/// rounds afterwards.
///
/// `per_bucket` caps how many parts share one broadcast window; `null` keeps
/// zingolib's default, and `reschedule_parts` can change it any time before
/// the first part is signed. Signing is always `LazyAtBoundary` (the only
/// sound strategy while ZIP 244 commits the anchor into the signature hash).
///
/// Returns `{ started: true }`; failure throws typed — notably
/// `MigrationConsentStale` when the wallet's notes changed since planning
/// (replan and re-show).
pub fn start_ironwood_migration(
    plan_hash_hex: String,
    per_bucket: Option<u32>,
) -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        let hash = hex_to_hash32(&plan_hash_hex)
            .ok_or_else(|| ZingolibError::InvalidInput("invalid plan hash".to_string()))?;
        RT.block_on(async move {
            lightclient
                .start_ironwood_migration(
                    AccountId::ZERO,
                    SigningStrategy::LazyAtBoundary,
                    hash,
                    per_bucket,
                )
                .await
                .map_err(ffi_error)?;
            Ok(object! { "started" => true }.pretty(2))
        })
    })
}

/// Drives one step of note splitting: builds, proves and broadcasts the next
/// round of Orchard self-sends, or reports what the round is waiting on.
/// zingolib pauses the running sync itself for the critical section. Call
/// after a sync tick and keep looping until `splitting_complete`.
///
/// Like the drain, this holds the lightclient for the whole prove+broadcast,
/// so dispatch it on the concurrent native queue, never the main one.
///
/// Returns one of:
/// `{ step: "round_broadcast", round, txids: [..] }` (sync until they
/// confirm, then call again),
/// `{ step: "awaiting_confirmation", pending: [..] }` (nothing written; an
/// empty `pending` means confirmed but the anchor hasn't reached the outputs
/// — sync and retry either way), or
/// `{ step: "splitting_complete" }` (parts bound and scheduled).
pub fn continue_note_splitting() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let step = lightclient
                .continue_note_splitting()
                .await
                .map_err(ffi_error)?;
            Ok(match step {
                SplitStep::RoundBroadcast { round, txids } => object! {
                    "step" => "round_broadcast",
                    "round" => round,
                    "txids" => txids
                        .iter()
                        .map(|txid| txid.to_string())
                        .collect::<Vec<_>>(),
                },
                SplitStep::AwaitingConfirmation { pending } => object! {
                    "step" => "awaiting_confirmation",
                    "pending" => pending
                        .iter()
                        .map(|txid| txid.to_string())
                        .collect::<Vec<_>>(),
                },
                SplitStep::SplittingComplete => object! {
                    "step" => "splitting_complete",
                },
            }
            .pretty(2))
        })
    })
}

/// Sets how many parts share each broadcast window and re-buckets every part
/// under the new cadence with fresh randomization. Callable any time between
/// consent and the first signed part; afterwards it fails typed with
/// `MigrationCadenceFixed`. After a successful call the old schedule is void:
/// re-read `migration_status` and re-arm the platform scheduler.
///
/// Returns `{ rescheduled: true }`; failure throws typed.
pub fn reschedule_parts(per_bucket: u32) -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            lightclient
                .reschedule_parts(per_bucket)
                .await
                .map_err(ffi_error)?;
            Ok(object! { "rescheduled" => true }.pretty(2))
        })
    })
}

/// The migration's progress, arranged for direct rendering. ZIP 318 requires
/// showing `orchard_confirmed_spendable` (the Orchard-pool figure
/// specifically) while a migration is in flight.
///
/// Returns `{ orchard_confirmed_spendable, phase, parts_total,
/// parts_confirmed, value_total, value_migrated, per_bucket, bucket_modulus,
/// next_wakes: [{ bucket_index, boundary, part_ids, denominations,
/// estimated_unix_time, estimated_target_unix_time }] }`. `phase` is `null`
/// when no migration is in progress, else `{ kind }` with per-kind fields
/// (`round`/`pending_txids` while note splitting, `residual` when complete).
/// `denominations` (zatoshis) mirror `part_ids` element-for-element, so a
/// schedule screen renders each window's batch without a second call.
pub fn migration_status() -> Result<String, ZingolibError> {
    with_initialized_lightclient_read(|lightclient| {
        RT.block_on(async move {
            let status = lightclient.migration_status().await.map_err(ffi_error)?;
            // Join each wake's part ids to their denominations (and
            // pick up the effective cadence) from the persisted
            // migration state; WakePoint alone carries only ids.
            let (denoms_by_id, per_bucket, bucket_modulus) = {
                let wallet = lightclient.wallet().read().await;
                match &wallet.migration {
                    Some(state) => (
                        state
                            .parts
                            .iter()
                            .map(|part| (part.id.0, part.denomination))
                            .collect::<std::collections::HashMap<_, _>>(),
                        Some(state.params.k_max),
                        state.params.bucket_modulus,
                    ),
                    None => (
                        std::collections::HashMap::new(),
                        None,
                        MigrationParams::provisional(wallet.chain_type()).bucket_modulus,
                    ),
                }
            };
            let next_wakes = status
                .next_wakes
                .iter()
                .map(|wake| {
                    object! {
                        "bucket_index" => wake.bucket_index,
                        "boundary" => u32::from(wake.boundary),
                        "part_ids" => wake
                            .part_ids
                            .iter()
                            .map(|id| id.0)
                            .collect::<Vec<_>>(),
                        "denominations" => wake
                            .part_ids
                            .iter()
                            .map(|id| denoms_by_id.get(&id.0).copied().unwrap_or(0))
                            .collect::<Vec<_>>(),
                        "estimated_unix_time" => wake.estimated_unix_time,
                        "estimated_target_unix_time" => wake.estimated_target_unix_time,
                    }
                })
                .collect::<Vec<_>>();
            Ok(object! {
                "orchard_confirmed_spendable" => status.orchard_confirmed_spendable,
                "phase" => match &status.phase {
                    Some(phase) => migration_phase_json(phase),
                    None => json::JsonValue::Null,
                },
                "parts_total" => status.parts_total,
                "parts_confirmed" => status.parts_confirmed,
                "value_total" => status.value_total,
                "value_migrated" => status.value_migrated,
                "per_bucket" => per_bucket,
                "bucket_modulus" => bucket_modulus,
                "next_wakes" => next_wakes,
            }
            .pretty(2))
        })
    })
}

/// Classifies every part against the local chain view, applies what is safe
/// unattended (promotions, expiries, rebuilds, completion) and returns what
/// needs the app. Call on every launch; never syncs, offline-safe.
///
/// Returns `{ actions: [{ action, ... }] }`. The app-facing ones:
/// `continue_note_splitting` / `retry_split { txid }` (drive the splitting
/// loop), `await_split_confirmation` (sync first), `prompt_catch_up { parts }`
/// (folded into the next execute tap in manual mode), `replan_remainder`
/// (fresh consent required). The rest report what was applied unattended.
pub fn reconcile_migration() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let report = lightclient.reconcile_migration().await.map_err(ffi_error)?;
            let actions = report
                .actions
                .iter()
                .map(|action| match action {
                    RecommendedAction::AwaitSplitConfirmation => {
                        object! { "action" => "await_split_confirmation" }
                    }
                    RecommendedAction::RetrySplit { txid } => object! {
                        "action" => "retry_split",
                        "txid" => txid.to_string(),
                    },
                    RecommendedAction::ContinueNoteSplitting => {
                        object! { "action" => "continue_note_splitting" }
                    }
                    RecommendedAction::PromoteConfirmed { part, .. } => object! {
                        "action" => "promote_confirmed",
                        "part" => part.0,
                    },
                    RecommendedAction::Rebuild { part } => object! {
                        "action" => "rebuild",
                        "part" => part.0,
                    },
                    RecommendedAction::MarkInvalidated { part } => object! {
                        "action" => "mark_invalidated",
                        "part" => part.0,
                    },
                    RecommendedAction::ReplanRemainder => {
                        object! { "action" => "replan_remainder" }
                    }
                    RecommendedAction::PromptCatchUp { parts, .. } => object! {
                        "action" => "prompt_catch_up",
                        "parts" => parts.iter().map(|id| id.0).collect::<Vec<_>>(),
                    },
                    RecommendedAction::MarkComplete { residual } => object! {
                        "action" => "mark_complete",
                        "residual" => *residual,
                    },
                })
                .collect::<Vec<_>>();
            Ok(object! { "actions" => actions }.pretty(2))
        })
    })
}

/// Serializes a completed execute batch's report for the client.
fn batch_report_json(report: &zingolib::lightclient::migrate::BatchReport) -> json::JsonValue {
    use zingolib::lightclient::migrate::PartSendResult;
    let outcomes = report
        .outcomes
        .iter()
        .map(|o| {
            let result = match &o.result {
                PartSendResult::Sent(txid) => object! {
                    "kind" => "sent",
                    "txid" => txid.to_string(),
                },
                PartSendResult::Slid => object! { "kind" => "slid" },
                PartSendResult::NotDue {
                    estimated_unix_time,
                } => object! {
                    "kind" => "not_due",
                    "estimated_unix_time" => *estimated_unix_time,
                },
                PartSendResult::Failed { error } => object! {
                    "kind" => "failed",
                    "error" => error.clone(),
                },
            };
            object! {
                "part" => o.part.0,
                "denomination" => o.denomination,
                "result" => result,
            }
        })
        .collect::<Vec<_>>();
    object! {
        "outcomes" => outcomes,
        "halted" => match &report.halted {
            Some(e) => json::JsonValue::from(e.clone()),
            None => json::JsonValue::Null,
        },
    }
}

/// Sends everything the scheduled migration owes right now, in one
/// user-triggered batch: the current window's due parts plus any missed
/// windows', folded in. Sends are sequenced `spacing_ms` apart, never
/// simultaneous. This is the phase-2 execute tap for the private path — the
/// user opens the app from a window's reminder, the app syncs, then calls this
/// once to send the whole batch.
///
/// zingolib pauses the running sync itself for the critical section. Like the
/// drain, it holds the lightclient for the whole prove+broadcast, so dispatch
/// it on the concurrent native queue, never the main one; poll
/// `execute_due_parts_status` concurrently for progress.
///
/// Returns `{ outcomes: [{ part, denomination, result }], halted }`, where each
/// `result` is `{ kind: "sent", txid }`, `{ kind: "slid" }`,
/// `{ kind: "not_due", estimated_unix_time }`, or `{ kind: "failed", error }`,
/// and `halted` is the error that stopped the batch early or `null`. An empty
/// `outcomes` with `halted` null means nothing was owed. An infrastructure
/// failure (offline, no migration in progress) throws typed.
pub fn execute_due_parts(spacing_ms: u64) -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        // Publish the batch progress handle to the BATCH_PROGRESS side
        // channel *before* the block_on. We hold LIGHTCLIENT.write() for the
        // whole batch, so a concurrent `execute_due_parts_status` poll must
        // read this handle (an independent Arc) rather than the lightclient.
        // The handle reads idle (`null`) until execute_due_parts arms it
        // after folding in overdue parts.
        if let Ok(mut progress) = BATCH_PROGRESS.write() {
            *progress = Some(lightclient.batch_progress_handle());
        }
        let out = RT.block_on(async move {
            let report = lightclient
                .execute_due_parts(std::time::Duration::from_millis(spacing_ms))
                .await
                .map_err(ffi_error)?;
            Ok(batch_report_json(&report).pretty(2))
        });
        // Drop the stale handle. The batch's own scope guard already reset
        // the snapshot to idle on the way out, so a late poll reads `null`
        // whether or not this clear has landed yet.
        if let Ok(mut progress) = BATCH_PROGRESS.write() {
            *progress = None;
        }
        out
    })
}

/// A snapshot of the in-flight execute batch's progress, for rendering
/// "Sending i/N". Reads the BATCH_PROGRESS side channel only, never LIGHTCLIENT,
/// so it stays responsive while `execute_due_parts` holds the lightclient lock
/// across its whole prove-and-broadcast loop.
///
/// Returns, while a batch runs: `{ total, resolved, sent, phase }` where `phase`
/// is `"sending"` or `"spacing"` and the counts are `0..=total`. Returns JSON
/// `null` when no batch is in flight (before it starts, or once it finishes).
pub fn execute_due_parts_status() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        // Snapshot under a brief read lock, then drop it: the batch only touches
        // BATCH_PROGRESS at its very start and end, so this never blocks on the
        // long-running batch the way a LIGHTCLIENT read would.
        let status = {
            let progress = BATCH_PROGRESS
                .read()
                .map_err(|_| ZingolibError::SideChannelPoisoned)?;
            progress.as_ref().and_then(|handle| handle.status())
        };
        Ok(match status {
            Some(s) => {
                use zingolib::lightclient::migrate::BatchPhase;
                object! {
                    "total" => s.total,
                    "resolved" => s.resolved,
                    "sent" => s.sent,
                    "phase" => match s.phase {
                        BatchPhase::Sending => "sending",
                        BatchPhase::Spacing => "spacing",
                    },
                }
                .pretty(2)
            }
            None => json::JsonValue::Null.pretty(2),
        })
    })
}

/// Abandons the in-progress migration: confirmed parts stand, pending ones
/// are dropped and their notes released. The wallet then offers a fresh plan
/// for whatever Orchard balance remains.
///
/// Returns `{ cancelled: true }`; failure throws typed.
pub fn cancel_ironwood_migration() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            lightclient
                .cancel_ironwood_migration()
                .await
                .map_err(ffi_error)?;
            Ok(object! { "cancelled" => true }.pretty(2))
        })
    })
}
