uniffi::include_scaffolding!("zingo");

include!(concat!(env!("OUT_DIR"), "/zm_description.rs"));

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
use zcash_client_backend::proposal::Proposal;
use zcash_protocol::memo::MemoBytes;
use zcash_protocol::value::Zatoshis;
use zcash_protocol::{PoolType, ShieldedPool};
use zingolib::netutils::{GrpcIndexer, Indexer};
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
    MigrationParams, MigrationPhase, RecommendedAction, parts::PartState, parts::SigningStrategy,
    split::plan_hash,
};

use zingo_common_components::protocol::ActivationHeights;

const INDEXER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// Bounds the pending-URI redial inside run_sync, which holds the LIGHTCLIENT
// write lock: every other FFI call waits behind it, so the dial gets seconds,
// not the OS connect timeout.
const PENDING_INDEXER_DIAL_TIMEOUT: Duration = Duration::from_secs(5);

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
    #[error("Error: mixnet: {0}")]
    Mixnet(String),
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
        LightClientError::PriceError(_) | LightClientError::PriceFetchRequiresMixnet => {
            ZingolibError::Read(text)
        }
        LightClientError::MixnetNotReady(_) => ZingolibError::Mixnet(text),
        // A deliberate verdict (#1229): exhausting the eligible Correspondents
        // is a server-topology problem, not a mixnet refusal — switching the
        // synchronization endpoint changes eligibility, so the app's
        // switch-and-retry routing can genuinely help. Mapping it to Mixnet
        // would stamp it with the owned refusal marker and turn it
        // never-retry.
        LightClientError::NoEligibleCorrespondent
        | LightClientError::IneligibleProbeTarget(_)
        | LightClientError::MigrationTransmissionTargetIsSyncEndpoint { .. } => {
            ZingolibError::Indexer(text)
        }
        LightClientError::MigrationError(inner) => match inner {
            MigrationError::NoMigration => ZingolibError::MigrationNotInProgress,
            MigrationError::AlreadyInProgress => ZingolibError::MigrationAlreadyInProgress,
            MigrationError::ConsentStale => ZingolibError::MigrationConsentStale(text),
            MigrationError::CadenceFixed => ZingolibError::MigrationCadenceFixed(text),
            MigrationError::ScheduledMigrationExists => ZingolibError::MigrationAlreadyInProgress,
            MigrationError::PreSignedUnavailable
            | MigrationError::NoteSplittingRequired
            | MigrationError::DifferentAccount
            | MigrationError::IronwoodEraTooYoung { .. } => ZingolibError::Migration(text),
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
// a cloned `ImmediateMigrationProgressHandle` (an independent
// Arc<Mutex<Option<ImmediateMigrationStatus>>>) here; the poller reads only
// this global and never contends for the drain's wallet/lightclient lock.
// `None` between drains.
lazy_static! {
    static ref DRAIN_PROGRESS: RwLock<Option<zingolib::lightclient::migrate::ImmediateMigrationProgressHandle>> =
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

// An optional dedicated migration-transmission endpoint, read at every client
// construction. The library owns the curated Correspondent pool and always
// excludes the synchronization operator (ADR 0022), so a mixnet migration works
// without any app input. `Some` names one dedicated endpoint distinct from the
// sync operator; `None` (the default) lets the library draw its curated pool.
static MIGRATION_TRANSMISSION_URI: Lazy<RwLock<Option<http::Uri>>> =
    Lazy::new(|| RwLock::new(None));

static PENDING_INDEXER_URI: Lazy<RwLock<Option<http::Uri>>> = Lazy::new(|| RwLock::new(None));

// Live progress of the in-flight Phase 1 note-splitting round, the split-path
// counterpart to DRAIN_PROGRESS. `quick_split` holds LIGHTCLIENT.write() for
// its whole `block_on`, so a `split_status` poll reads this cloned
// `SplitProgressHandle` (an independent Arc) instead of the lightclient. `None`
// between rounds.
lazy_static! {
    static ref SPLIT_PROGRESS: RwLock<Option<zingolib::lightclient::migrate::SplitProgressHandle>> =
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
    if let Ok(mut pending) = PENDING_INDEXER_URI.write() {
        *pending = None;
    }
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

#[cfg(test)]
mod lock_discipline_tests;

/// Parses the schedule suffix of a `regtest:<schedule>` chain hint into
/// activation heights.
///
/// The format is the harness convention (`regtest-launcher`, and the shape
/// of `REGTEST_FIXTURE_HEIGHTS_CLI_STRING`): comma-separated `key=height`
/// entries applied left to right over the bare-"regtest" default schedule,
/// where `height` is a block height or `off`, and `all` sets every upgrade
/// through NU6.3 at once. Unknown keys and malformed heights are errors,
/// never silently dropped — a wrong schedule fails loudly here instead of
/// as a consensus-branch-id rejection at broadcast.
fn parse_regtest_activation_heights(spec: &str) -> Result<ActivationHeights, String> {
    let default = ActivationHeights::default();
    let mut overwinter = default.overwinter();
    let mut sapling = default.sapling();
    let mut blossom = default.blossom();
    let mut heartwood = default.heartwood();
    let mut canopy = default.canopy();
    let mut nu5 = default.nu5();
    let mut nu6 = default.nu6();
    let mut nu6_1 = default.nu6_1();
    let mut nu6_2 = default.nu6_2();
    let mut nu6_3 = default.nu6_3();
    let mut nu7 = default.nu7();

    for entry in spec.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (key, value) = entry
            .split_once('=')
            .ok_or_else(|| format!("expected key=height, got \"{entry}\""))?;
        let height = match value.trim() {
            "off" => None,
            v => Some(
                v.parse::<u32>()
                    .map_err(|_| format!("invalid height \"{v}\" for \"{}\"", key.trim()))?,
            ),
        };
        match key.trim() {
            "all" => {
                overwinter = height;
                sapling = height;
                blossom = height;
                heartwood = height;
                canopy = height;
                nu5 = height;
                nu6 = height;
                nu6_1 = height;
                nu6_2 = height;
                nu6_3 = height;
            }
            "overwinter" => overwinter = height,
            "sapling" => sapling = height,
            "blossom" => blossom = height,
            "heartwood" => heartwood = height,
            "canopy" => canopy = height,
            "nu5" => nu5 = height,
            "nu6" => nu6 = height,
            "nu6_1" => nu6_1 = height,
            "nu6_2" => nu6_2 = height,
            "nu6_3" => nu6_3 = height,
            "nu7" => nu7 = height,
            other => return Err(format!("unknown network upgrade \"{other}\"")),
        }
    }

    Ok(ActivationHeights::builder()
        .set_overwinter(overwinter)
        .set_sapling(sapling)
        .set_blossom(blossom)
        .set_heartwood(heartwood)
        .set_canopy(canopy)
        .set_nu5(nu5)
        .set_nu6(nu6)
        .set_nu6_1(nu6_1)
        .set_nu6_2(nu6_2)
        .set_nu6_3(nu6_3)
        .set_nu7(nu7)
        .build())
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
        hint => match hint.strip_prefix("regtest:") {
            // A regtest chain has no universal schedule: the node that was
            // launched is the only authority on its activation heights
            // (infrastructure ADR 0003), so the harness that launched it
            // passes the schedule through the hint. Bare "regtest" keeps
            // the historical all-at-height-1 default.
            Some(schedule) => {
                ChainType::Regtest(parse_regtest_activation_heights(schedule).map_err(|e| {
                    ZingolibError::init(format!("Invalid regtest activation heights: {e}"))
                })?)
            }
            None => return Err(ZingolibError::init("Not a valid chain hint!")),
        },
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
    // Point migration transmission at a dedicated endpoint when the app set
    // one. Absent it, the library draws its curated Correspondent pool.
    let migration_uri = MIGRATION_TRANSMISSION_URI
        .read()
        .ok()
        .and_then(|uri| uri.clone());
    let builder = match migration_uri {
        Some(uri) => builder.set_migration_transmission_uri(uri),
        None => builder,
    };
    builder.build().map_err(ZingolibError::init)
}

/// Set an optional dedicated migration-transmission endpoint, applied at the
/// next client construction. The library owns the curated Correspondent pool
/// and always excludes the synchronization operator, so migration-over-mixnet
/// works with no input here.
///
/// Accepts `{ transmissionUri: "<uri>" }` to name one dedicated endpoint
/// distinct from the sync operator, or `null` / `{}` / the legacy
/// `{ candidates, allowSyncEndpoint }` shape to clear it and use the curated
/// pool. The legacy pool no longer feeds routing: the library embeds the pool.
pub fn set_broadcast_candidates(candidates_json: String) -> Result<String, ZingolibError> {
    let parsed = json::parse(&candidates_json)
        .map_err(|e| ZingolibError::InvalidInput(format!("invalid candidates json: {e}")))?;
    let transmission_uri = match parsed["transmissionUri"].as_str() {
        Some(uri) => Some(construct_indexer_uri(Some(uri.to_string())).map_err(|e| {
            ZingolibError::InvalidInput(format!("invalid transmission uri {uri}: {e}"))
        })?),
        None => None,
    };
    let is_set = transmission_uri.is_some();
    *MIGRATION_TRANSMISSION_URI
        .write()
        .map_err(|_| ZingolibError::LightclientLockPoisoned)? = transmission_uri;
    Ok(object! { "transmission_uri_set" => is_set }.pretty(2))
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

                Err(LightClientError::ClientError(_)) => {
                    let mut offline_params = params;
                    let pending_uri = offline_params.lightwalletd_uri.take();
                    let config = match build_client_config(&offline_params, WalletConfig::Read) {
                        Ok(c) => c,
                        Err(e) => {
                            last_error = e;
                            continue;
                        }
                    };
                    match RT.block_on(LightClient::from_bytes(decoded_bytes.clone(), config)) {
                        Ok(l) => {
                            if let Ok(mut pending) = PENDING_INDEXER_URI.write() {
                                *pending = pending_uri;
                            }
                            built = Some((l, offline_params));
                            break;
                        }
                        Err(e) => {
                            last_error = ZingolibError::init(e);
                            continue;
                        }
                    }
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

/// The classification seam contract (#1229): the app routes send failures
/// by our own error variants' display prefixes, so each zingolib failure
/// must land in the variant whose routing verdict it deserves. A mixnet
/// refusal is never-retry; the excluded-indexer exhaustion is a
/// server-topology problem where switching servers genuinely changes
/// eligibility, so it must NOT carry the mixnet marker.
#[cfg(test)]
mod ffi_error_routing_tests {
    use super::*;

    #[test]
    fn a_mixnet_refusal_maps_to_the_owned_mixnet_marker() {
        let mapped = ffi_error(LightClientError::MixnetNotReady(
            zingolib::nym::MixnetNotReady::Bootstrapping,
        ));
        assert!(
            matches!(&mapped, ZingolibError::Mixnet(_)),
            "a refusal must carry the owned mixnet marker: {mapped:?}"
        );
        assert!(mapped.to_string().starts_with("Error: mixnet:"));
    }

    #[test]
    fn excluded_indexer_exhaustion_is_an_indexer_failure_not_a_refusal() {
        let mapped = ffi_error(LightClientError::NoEligibleCorrespondent);
        assert!(
            matches!(&mapped, ZingolibError::Indexer(_)),
            "exhaustion routes as server-suspect, so it must not wear the mixnet marker: {mapped:?}"
        );
        assert!(mapped.to_string().starts_with("Error: indexer:"));
    }
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

/// The regtest schedule handoff: a `regtest:<schedule>` hint must carry
/// the launched node's activation heights into the wallet verbatim
/// (infrastructure ADR 0003 — the validator is the only heights
/// authority), and a malformed schedule must fail at parse time, never
/// surface later as a consensus-branch-id rejection at broadcast.
#[cfg(test)]
mod regtest_activation_heights_tests {
    use super::*;

    #[test]
    fn empty_schedule_matches_the_bare_regtest_default() {
        let parsed = parse_regtest_activation_heights("").expect("empty schedule is valid");
        assert_eq!(parsed, ActivationHeights::default());
    }

    #[test]
    fn fixture_schedule_parses_to_the_mixed_chain() {
        // The harness fixture shape: pre-NU5 upgrades at 1, NU5/NU6 at 2,
        // the NU6.x line at 5, NU7 unscheduled.
        let parsed =
            parse_regtest_activation_heights("all=1,nu5=2,nu6=2,nu6_1=5,nu6_2=5,nu6_3=5,nu7=off")
                .expect("the fixture schedule is valid");
        assert_eq!(parsed.canopy(), Some(1));
        assert_eq!(parsed.nu5(), Some(2));
        assert_eq!(parsed.nu6(), Some(2));
        assert_eq!(parsed.nu6_1(), Some(5));
        assert_eq!(parsed.nu6_2(), Some(5));
        assert_eq!(parsed.nu6_3(), Some(5));
        assert_eq!(parsed.nu7(), None);
    }

    #[test]
    fn entries_apply_left_to_right_over_all() {
        let parsed = parse_regtest_activation_heights("all=3,nu6_3=7").expect("override is valid");
        assert_eq!(parsed.nu6(), Some(3));
        assert_eq!(parsed.nu6_3(), Some(7));
    }

    #[test]
    fn unknown_upgrade_is_an_error_not_a_silent_drop() {
        let error = parse_regtest_activation_heights("nu99=1")
            .expect_err("an unknown upgrade must be rejected");
        assert!(error.contains("nu99"), "error names the bad key: {error}");
    }

    #[test]
    fn malformed_height_is_an_error() {
        let error = parse_regtest_activation_heights("nu5=soon")
            .expect_err("a non-numeric height must be rejected");
        assert!(error.contains("soon"), "error names the bad value: {error}");
    }

    #[test]
    fn hint_without_schedule_prefix_still_fails_init() {
        let error = match build_connection_params(
            String::new(),
            "shmegtest".to_string(),
            "Medium".to_string(),
            1,
        ) {
            Err(error) => error,
            Ok(_) => panic!("an unknown chain hint must be rejected"),
        };
        assert!(matches!(error, ZingolibError::Init(_)));
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
    fn reset_clears_the_pending_indexer_uri() {
        if let Ok(mut pending) = PENDING_INDEXER_URI.write() {
            *pending = Some(http::Uri::from_static("https://example.com:9067"));
        }
        reset_lightclient();
        let pending = PENDING_INDEXER_URI.read().expect("pending uri lock");
        assert!(
            pending.is_none(),
            "a client teardown must not leave a stale pending URI behind"
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
    // field: only `migration` transfers with a mapped txid are overwritten.
    #[test]
    fn splice_migrated_values_overwrites_only_mapped_migrations() {
        let mut json_vts = json::object! {
            "value_transfers" => json::array![
                json::object!{ "kind" => "migration", "txid" => "aaa", "value" => 0 },
                json::object!{ "kind" => "sent", "txid" => "bbb", "value" => 100 },
                json::object!{ "kind" => "migration", "txid" => "ccc", "value" => 0 },
            ]
        };
        let mut migrated_by_txid = std::collections::HashMap::new();
        migrated_by_txid.insert("aaa".to_string(), 500u64);
        // "ccc" is a migration without a mapped amount -> untouched.

        splice_migrated_values(&mut json_vts, &migrated_by_txid);

        assert_eq!(
            json_vts["value_transfers"][0]["value"].as_u64(),
            Some(500),
            "the mapped migration must carry the migrated amount"
        );
        assert_eq!(
            json_vts["value_transfers"][1]["value"].as_u64(),
            Some(100),
            "a plain send must be left alone"
        );
        assert_eq!(
            json_vts["value_transfers"][2]["value"].as_u64(),
            Some(0),
            "an unmapped migration must be left alone"
        );
    }
}

/// Pins the behavior of [`ffi_error`], the one funnel from zingolib's error
/// taxonomy to the FFI's typed variants. The match itself is exhaustive, so
/// a new zingolib variant fails compilation; these tests pin which variant
/// each input maps to, so a remapping (or a regression to prose in the data
/// channel) turns the suite red rather than silently shifting the codes the
/// bridges and TypeScript branch on. Inputs are limited to variants
/// constructible without wallet or network state; the Shield and Indexer
/// arms carry deeply nested foreign payloads and stay guarded by
/// exhaustiveness alone.
#[cfg(test)]
mod error_funnel_tests {
    use super::*;
    use zcash_protocol::TxId;
    use zingolib::lightclient::error::{MigrationError, TransmissionError};
    use zingolib::wallet::error::WalletError;

    /// (input, is-the-pinned-variant, the pinned variant's name).
    type FunnelCase = (LightClientError, fn(&ZingolibError) -> bool, &'static str);

    #[test]
    fn the_funnel_maps_each_constructible_input_to_its_pinned_variant() {
        let cases: Vec<FunnelCase> = vec![
            (
                LightClientError::SyncLaunchError,
                |e| matches!(e, ZingolibError::Sync(_)),
                "Sync",
            ),
            (
                LightClientError::SyncNotRunning,
                |e| matches!(e, ZingolibError::Sync(_)),
                "Sync",
            ),
            (
                LightClientError::SendError(SendError::NoStoredProposal),
                |e| matches!(e, ZingolibError::Send(_)),
                "Send",
            ),
            (
                LightClientError::SendError(SendError::TransmissionError(
                    TransmissionError::TransmissionFailed("node said no".to_string()),
                )),
                |e| matches!(e, ZingolibError::Send(_)),
                "Send",
            ),
            (
                LightClientError::FileError(std::io::Error::other("disk full")),
                |e| matches!(e, ZingolibError::Save(_)),
                "Save",
            ),
            (
                LightClientError::WalletError(WalletError::MnemonicNotFound),
                |e| matches!(e, ZingolibError::Wallet(_)),
                "Wallet",
            ),
            (
                LightClientError::Offline,
                |e| matches!(e, ZingolibError::Offline),
                "Offline",
            ),
            (
                LightClientError::MigrationError(MigrationError::NoMigration),
                |e| matches!(e, ZingolibError::MigrationNotInProgress),
                "MigrationNotInProgress",
            ),
            (
                LightClientError::MigrationError(MigrationError::AlreadyInProgress),
                |e| matches!(e, ZingolibError::MigrationAlreadyInProgress),
                "MigrationAlreadyInProgress",
            ),
            (
                LightClientError::MigrationError(MigrationError::ConsentStale),
                |e| matches!(e, ZingolibError::MigrationConsentStale(_)),
                "MigrationConsentStale",
            ),
            (
                LightClientError::MigrationError(MigrationError::CadenceFixed),
                |e| matches!(e, ZingolibError::MigrationCadenceFixed(_)),
                "MigrationCadenceFixed",
            ),
            (
                LightClientError::MigrationError(MigrationError::PreSignedUnavailable),
                |e| matches!(e, ZingolibError::Migration(_)),
                "Migration",
            ),
            (
                LightClientError::MigrationError(MigrationError::SplitDidNotConverge(3)),
                |e| matches!(e, ZingolibError::MigrationSplit(_)),
                "MigrationSplit",
            ),
            (
                LightClientError::MigrationError(MigrationError::SplitTransactionFailed(
                    TxId::from_bytes([0u8; 32]),
                )),
                |e| matches!(e, ZingolibError::MigrationSplit(_)),
                "MigrationSplit",
            ),
            (
                LightClientError::MigrationError(MigrationError::SplitConfirmationTimeout),
                |e| matches!(e, ZingolibError::MigrationSplit(_)),
                "MigrationSplit",
            ),
        ];
        for (input, is_expected, expected_name) in cases {
            let input_text = input.to_string();
            let mapped = ffi_error(input);
            assert!(
                is_expected(&mapped),
                "{input_text:?} must map to the {expected_name} variant, got: {mapped}"
            );
        }
    }

    #[test]
    fn the_funnel_preserves_the_upstream_message_verbatim_in_the_variant() {
        // The bridges forward the Display text as the rejection message, so
        // the funnel must not paraphrase or truncate the upstream prose.
        let upstream = LightClientError::SendError(SendError::TransmissionError(
            TransmissionError::TransmissionFailed("node said no".to_string()),
        ));
        let upstream_text = upstream.to_string();
        let mapped = ffi_error(upstream);
        assert!(
            mapped.to_string().contains(&upstream_text),
            "the mapped error must carry the upstream text \"{upstream_text}\": {mapped}"
        );
    }
}

/// Pins [`hex_to_hash32`] and [`hash32_to_hex`], the consent-hash boundary
/// that `start_ironwood_migration` trusts before anything is signed. The
/// parser was rewritten from per-pair `from_str_radix` (which accepted
/// `+`-signed pairs and could panic on multi-byte UTF-8 slicing) to
/// `hex::decode`; the rejection cases pin that tightening as deliberate.
#[cfg(test)]
mod consent_hash_tests {
    use super::*;

    #[test]
    fn a_hash_round_trips_through_hex_and_back() {
        let hash: [u8; 32] = core::array::from_fn(|i| i as u8);
        let hex = hash32_to_hex(&hash);
        assert_eq!(hex.len(), 64);
        assert_eq!(hex_to_hash32(&hex), Some(hash));
    }

    #[test]
    fn uppercase_hex_decodes_to_the_same_hash() {
        let hash = [0xabu8; 32];
        let hex = hash32_to_hex(&hash).to_uppercase();
        assert_eq!(hex_to_hash32(&hex), Some(hash));
    }

    #[test]
    fn anything_but_64_hex_digits_is_rejected() {
        let valid = hash32_to_hex(&[0u8; 32]);
        let rejected = [
            String::new(),
            valid[..62].to_string(),      // too short (even length)
            valid[..63].to_string(),      // odd length
            format!("{valid}00"),         // too long
            format!("zz{}", &valid[2..]), // non-hex digits
            format!("+a{}", &valid[2..]), // signed pair the old parser accepted
            format!("é{}", &valid[2..]),  // multi-byte UTF-8: must not panic
        ];
        for input in rejected {
            assert_eq!(
                hex_to_hash32(&input),
                None,
                "{input:?} must be rejected, not coerced into a consent hash"
            );
        }
    }
}

/// The migration read-path contract: with no initialized client, the
/// read-lock entry points fail typed — never as prose in the data channel.
/// These pin `with_initialized_lightclient_read`, which the migration
/// getters introduced. nextest runs each test in its own process, so the
/// LIGHTCLIENT global is reliably uninitialized.
#[cfg(test)]
mod migration_error_channel_tests {
    use super::*;

    fn assert_uninitialized(result: Result<String, ZingolibError>, ffi: &str) {
        let error = result.expect_err("an uninitialized client must fail typed");
        assert!(
            matches!(error, ZingolibError::LightclientNotInitialized),
            "{ffi} must fail with the typed uninitialized variant: {error}"
        );
    }

    #[test]
    fn plan_ironwood_migration_fails_typed_without_a_client() {
        assert_uninitialized(plan_ironwood_migration(), "plan_ironwood_migration");
    }

    #[test]
    fn migration_status_fails_typed_without_a_client() {
        assert_uninitialized(migration_status(), "migration_status");
    }
}

/// The parse/stub data/error channel contract: input the caller got wrong
/// travels as the typed InvalidInput variant, and unimplemented endpoints
/// fail typed — never `Ok("Error: ...")` prose in the data channel, which
/// is exactly what these calls returned before the typed surface.
#[cfg(test)]
mod parse_and_stub_error_channel_tests {
    use super::*;

    #[test]
    fn empty_address_travels_on_the_error_channel() {
        let error = parse_address(String::new())
            .expect_err("an empty address must be typed, not prose in the data channel");
        assert!(
            matches!(error, ZingolibError::InvalidInput(_)),
            "the failure must be the typed InvalidInput variant: {error}"
        );
    }

    #[test]
    fn empty_ufvk_travels_on_the_error_channel() {
        let error = parse_ufvk(String::new())
            .expect_err("an empty viewkey must be typed, not prose in the data channel");
        assert!(
            matches!(error, ZingolibError::InvalidInput(_)),
            "the failure must be the typed InvalidInput variant: {error}"
        );
    }

    #[test]
    fn unimplemented_option_stubs_fail_typed() {
        for (result, ffi) in [
            (set_option_wallet(), "set_option_wallet"),
            (get_option_wallet(), "get_option_wallet"),
        ] {
            let error = result.expect_err("an unimplemented endpoint must fail typed");
            assert!(
                matches!(error, ZingolibError::Wallet(_)),
                "{ffi} must fail with the typed Wallet variant: {error}"
            );
        }
    }

    #[test]
    fn crypto_provider_install_succeeds_and_is_idempotent() {
        for attempt in 1..=2 {
            let value = set_crypto_default_provider_to_ring()
                .expect("installing the ring provider must succeed");
            assert_eq!(value, "true", "attempt {attempt} must resolve plain data");
        }
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
    with_initialized_lightclient_read(|lightclient| {
        Ok(RT.block_on(async move {
            let wallet = lightclient.wallet().read().await;
            object! { "height" => json::JsonValue::from(wallet.sync_state.last_known_chain_height().map_or(0, u32::from))}.pretty(2)
        }))
    })
}

/// Overwrites the `value` of each Orchard->Ironwood migration value transfer
/// with the amount migrated. `json_vts` is the `{ "value_transfers": [ ... ] }`
/// object produced by `From<ValueTransfers>`; zingolib classifies these
/// transfers with `kind: "migration"` and the migrated amount lives in
/// `migrated_by_txid`. zingolib reports `value == 0` for self-sends (a
/// migration is one), so without this the migrated amount is invisible.
///
/// Note the array is nested under `"value_transfers"` — `members_mut()` on the
/// outer object yields an empty iterator, so we must index in first.
fn splice_migrated_values(
    json_vts: &mut json::JsonValue,
    migrated_by_txid: &std::collections::HashMap<String, u64>,
) {
    for vt in json_vts["value_transfers"].members_mut() {
        if vt["kind"].as_str() != Some("migration") {
            continue;
        }
        let txid = vt["txid"].as_str().unwrap_or_default().to_string();
        if let Some(&migrated) = migrated_by_txid.get(&txid) {
            vt["value"] = json::JsonValue::from(migrated);
        }
    }
}

pub fn get_value_transfers() -> Result<String, ZingolibError> {
    with_initialized_lightclient_read(|lightclient| {
        RT.block_on(async move {
            let wallet = lightclient.wallet().read().await;

            // An Orchard -> Ironwood migration is a send-to-self, which zingolib
            // reports with `value == 0` because `total_value_sent` excludes
            // self-addressed value. To surface how much was migrated, recover it
            // from the transaction's self-received ironwood notes and splice it
            // into the matching value transfer's `value`. Keyed by txid, using
            // zingolib's own migration predicate so the map stays in lockstep
            // with the value transfers it classifies as `migration`.
            let migrated_by_txid: std::collections::HashMap<String, u64> =
                match wallet.transaction_summaries(true).await {
                    Ok(summaries) => summaries
                        .0
                        .iter()
                        .filter(|s| s.is_orchard_to_ironwood_migration())
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
            Ok(sync_result) => {
                // A bucket boundary's tree state is witnessable only while its
                // checkpoint is retained, so the witness has to be taken on the
                // sync that crosses it. `await_sync` does this for its own
                // callers; we drive sync through the poll, so it falls here.
                // No-op without a scheduled migration.
                RT.block_on(lightclient.capture_migration_witnesses())
                    .map_err(ffi_error)?;
                Ok(json::object! {
                    "sync_complete" => json::JsonValue::from(sync_result)
                }
                .pretty(2))
            }
            Err(e) => Err(ZingolibError::sync(e)),
        },
    })
}

/// One bounded redial of the URI left pending by `init_from_b64`'s
/// Indexerless fallback. Success attaches the Indexer and clears the pending
/// URI; failure leaves it for the next sync tick. A no-op when no URI is
/// pending.
fn attach_pending_indexer(lightclient: &mut LightClient) {
    let pending_uri = PENDING_INDEXER_URI
        .read()
        .ok()
        .and_then(|pending| pending.clone());
    let Some(uri) = pending_uri else { return };
    let attached = RT.block_on(async {
        tokio::time::timeout(
            PENDING_INDEXER_DIAL_TIMEOUT,
            lightclient.set_indexer_uri(uri),
        )
        .await
    });
    if matches!(attached, Ok(Ok(())))
        && let Ok(mut pending) = PENDING_INDEXER_URI.write()
    {
        *pending = None;
    }
}

fn run_sync() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        attach_pending_indexer(lightclient);
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
    with_initialized_lightclient_read(|lightclient| {
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
    with_initialized_lightclient_read(|lightclient| {
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
    with_initialized_lightclient_read(|lightclient| {
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
            // An explicit server choice supersedes any URI left pending by the
            // init fallback.
            if let Ok(mut pending) = PENDING_INDEXER_URI.write() {
                *pending = None;
            }
            Ok("server set".to_string())
        })
    })
}

pub fn wallet_kind() -> Result<String, ZingolibError> {
    with_initialized_lightclient_read(|lightclient| {
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
                                "shielded_only_ua" => zcash_keys::address::UnifiedAddress::from_receivers(ua.orchard().cloned(), None, None).expect("To construct UA").encode(&chain_name),
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
    with_panic_guard(|| {
        Ok(format!(
            "{}-{}",
            zingolib::git_description(),
            zm_description()
        ))
    })
}

pub fn get_messages(address: String) -> Result<String, ZingolibError> {
    with_initialized_lightclient_read(|lightclient| {
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
    with_initialized_lightclient_read(|lightclient| {
        RT.block_on(async move {
            match lightclient.account_balance(AccountId::ZERO).await {
                Ok(bal) => Ok(json::JsonValue::from(bal).pretty(2)),
                Err(e) => Err(ZingolibError::read(e)),
            }
        })
    })
}

pub fn get_total_memobytes_to_address() -> Result<String, ZingolibError> {
    with_initialized_lightclient_read(|lightclient| {
        RT.block_on(async move {
            match lightclient.do_total_memobytes_to_address().await {
                Ok(total_memo_bytes) => Ok(json::JsonValue::from(total_memo_bytes).pretty(2)),
                Err(e) => Err(ZingolibError::read(e)),
            }
        })
    })
}

pub fn get_total_value_to_address() -> Result<String, ZingolibError> {
    with_initialized_lightclient_read(|lightclient| {
        RT.block_on(async move {
            match lightclient.do_total_value_to_address().await {
                Ok(total_values) => Ok(json::JsonValue::from(total_values).pretty(2)),
                Err(e) => Err(ZingolibError::read(e)),
            }
        })
    })
}

pub fn get_total_spends_to_address() -> Result<String, ZingolibError> {
    with_initialized_lightclient_read(|lightclient| {
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
            // zingolib fetches price only over the mixnet (ADR 0011). This
            // wallet treats the price oracle as non-sensitive, so a mixnet
            // deliberately switched off fetches over clearnet instead. A mixnet
            // that is on but not ready still refuses, so an enabled private
            // route never silently drops to clearnet.
            let usd = match lightclient.update_current_price().await {
                Ok(fetch) => fetch.usd,
                Err(LightClientError::PriceFetchRequiresMixnet) => {
                    zingo_price::race_current_price(None)
                        .await
                        .map_err(ZingolibError::read)?
                        .price
                        .price_usd
                }
                Err(e) => return Err(ffi_error(e)),
            };
            Ok(object! { "current_price" => usd }.pretty(2))
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
    with_initialized_lightclient_read(|lightclient| {
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
    with_initialized_lightclient_read(|lightclient| {
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
    with_initialized_lightclient_read(|lightclient| {
        Ok(RT.block_on(async move { lightclient.unified_addresses_json().await.pretty(2) }))
    })
}

pub fn get_transparent_addresses() -> Result<String, ZingolibError> {
    with_initialized_lightclient_read(|lightclient| {
        Ok(RT.block_on(async move { lightclient.transparent_addresses_json().await.pretty(2) }))
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
    with_initialized_lightclient_read(|lightclient| {
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
    with_initialized_lightclient_read(|lightclient| {
        Ok(RT.block_on(async move {
            let save_required = lightclient.is_save_required().await;
            object! { "save_required" => save_required }.pretty(2)
        }))
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
    with_initialized_lightclient_read(|lightclient| {
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
    })
}

pub fn get_wallet_version() -> Result<String, ZingolibError> {
    with_initialized_lightclient_read(|lightclient| {
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

fn shielded_pool_name(pool: ShieldedPool) -> &'static str {
    match pool {
        ShieldedPool::Sapling => "sapling",
        ShieldedPool::Orchard => "orchard",
        ShieldedPool::Ironwood => "ironwood",
    }
}

fn pool_name(pool: PoolType) -> &'static str {
    match pool {
        PoolType::Transparent => "transparent",
        PoolType::Shielded(shielded) => shielded_pool_name(shielded),
    }
}

fn push_pool(pools: &mut Vec<&'static str>, pool: &'static str) {
    if !pools.contains(&pool) {
        pools.push(pool);
    }
}

fn proposal_source_pools<FeeRuleT, NoteRef>(
    proposal: &Proposal<FeeRuleT, NoteRef>,
) -> Vec<&'static str> {
    let mut pools = Vec::new();
    for step in proposal.steps() {
        if !step.transparent_inputs().is_empty() {
            push_pool(&mut pools, "transparent");
        }
        if let Some(inputs) = step.shielded_inputs() {
            for note in inputs.notes() {
                push_pool(&mut pools, shielded_pool_name(note.note().pool()));
            }
        }
    }
    pools
}

fn proposal_destination_pools<FeeRuleT, NoteRef>(
    proposal: &Proposal<FeeRuleT, NoteRef>,
) -> Vec<&'static str> {
    let mut pools = Vec::new();
    for step in proposal.steps() {
        for pool in step.payment_pools().values() {
            push_pool(&mut pools, pool_name(*pool));
        }
    }
    pools
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
            Ok(object! {
                "fee" => fee.into_u64(),
                "source_pools" => proposal_source_pools(&proposal),
                "destination_pools" => proposal_destination_pools(&proposal),
            }
            .pretty(2))
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
/// fee, residual }` (all zatoshis). Each transaction spends its `inputs`
/// Orchard notes into a single Ironwood `output`; the fee is `sum(inputs) -
/// output`. An empty `transactions` array means there is nothing worth
/// migrating.
pub fn plan_orchard_drain() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        RT.block_on(async move {
            let plan = lightclient
                .plan_immediate_migration(AccountId::ZERO)
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
                "residual" => plan.residual,
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
/// Uses zingolib's `quick_immediate_migration`, the send-shaped mobile entry
/// point. It pauses our continuous background sync internally, plans against
/// current wallet state, builds and transmits every drain transaction, then
/// restores sync before it returns (`resume_sync: true`). No self-sync, no
/// reconcile loop, matching `send`/`shield`. The older
/// `migrate_immediately_presynced` (guard parameter) is a Rust-internal form
/// and no longer crosses the FFI.
///
/// Returns, on success, `{ txids: [..], migrated, fee, residual }` (values in
/// zatoshis). Notes worth at most the sweep minimum are left behind and
/// reported as `residual`.
pub fn drain_orchard_to_ironwood() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        // Publish this drain's progress handle to the DRAIN_PROGRESS side
        // channel *before* the block_on. We hold LIGHTCLIENT.write() for the
        // whole drain, so a concurrent `drain_status` poll must read this
        // handle (an independent Arc) rather than the lightclient. The handle
        // reads idle (`null`) until the drain arms it after planning.
        if let Ok(mut progress) = DRAIN_PROGRESS.write() {
            *progress = Some(lightclient.immediate_migration_progress_handle());
        }
        let out = RT.block_on(async move {
            // quick_immediate_migration holds the pause internally: it plans,
            // builds, and transmits under one stable wallet state, then resumes
            // sync on return. resume_sync: true is the normal case.
            let summary = lightclient
                .quick_immediate_migration(AccountId::ZERO, true)
                .await
                .map_err(ffi_error)?;
            Ok(object! {
                "txids" => summary.txids.iter().map(|txid| txid.to_string()).collect::<Vec<_>>(),
                "migrated" => summary.migrated,
                "fee" => summary.fee,
                "residual" => summary.residual,
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
                use zingolib::lightclient::migrate::ImmediateMigrationPhase;
                object! {
                    "total" => s.total,
                    "built" => s.built,
                    "sent" => s.sent,
                    "phase" => match s.phase {
                        ImmediateMigrationPhase::Building => "building",
                        ImmediateMigrationPhase::Transmitting => "transmitting",
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
/// parts_fee, residual, plan_hash }` (values in zatoshis, `plan_hash` in hex).
/// Consent must disclose `residual`. Pass `plan_hash` back to
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
                "residual" => plan.residual,
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
                SplitStep::RoundTransmitted { round, txids } => object! {
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

/// Executes one round of Phase 1 note splitting as a send-shaped call — the
/// mobile entry point for the private path's splitting, the counterpart to
/// `quick_immediate_migration` for the immediate path (ADR 0016). Unlike the old
/// `start_ironwood_migration` + `continue_note_splitting` driver it persists no
/// migration state: it pauses sync internally, plans against current confirmed
/// notes, builds and transmits one round, then restores sync before it returns
/// (`resume_sync: true`). Each call re-plans, and "a round is still in flight"
/// is derived from the wallet's pending transactions, not a stored phase.
///
/// One call does one round. Loop it — sync to confirmation between calls —
/// until `complete`, then run `start_ironwood_migration` for Phase 2 (the notes
/// are fully split by then, so it binds and schedules the parts at once).
/// Refuses with `MigrationAlreadyInProgress` while a scheduled migration is
/// active. Preview with `plan_ironwood_migration`; poll `split_status` for
/// per-transaction progress.
///
/// Returns one of:
/// `{ outcome: "round", txids: [..] }` (sync until they confirm, then call
/// again), `{ outcome: "awaiting_confirmation" }` (a prior round has not
/// confirmed yet — sync and retry, no double-broadcast), or
/// `{ outcome: "complete" }` (every note is part-ready).
pub fn quick_split() -> Result<String, ZingolibError> {
    with_initialized_lightclient(|lightclient| {
        // Arm the SPLIT_PROGRESS side channel before the block_on: we hold
        // LIGHTCLIENT.write() for the whole round, so a concurrent
        // `split_status` poll must read this handle (an independent Arc). It
        // reads idle (`null`) until the round arms it after planning.
        if let Ok(mut progress) = SPLIT_PROGRESS.write() {
            *progress = Some(lightclient.split_progress_handle());
        }
        let out = RT.block_on(async move {
            let outcome = lightclient
                .quick_split(AccountId::ZERO, true)
                .await
                .map_err(ffi_error)?;
            use zingolib::lightclient::migrate::SplitOutcome;
            Ok(match outcome {
                SplitOutcome::Round { txids } => object! {
                    "outcome" => "round",
                    "txids" => txids
                        .iter()
                        .map(|txid| txid.to_string())
                        .collect::<Vec<_>>(),
                },
                SplitOutcome::AwaitingConfirmation => object! {
                    "outcome" => "awaiting_confirmation",
                },
                SplitOutcome::Complete => object! {
                    "outcome" => "complete",
                },
            }
            .pretty(2))
        });
        if let Ok(mut progress) = SPLIT_PROGRESS.write() {
            *progress = None;
        }
        out
    })
}

/// A snapshot of the in-flight splitting round's progress, for rendering
/// "built i/N" then "sent i/N" within a `quick_split` call. The Phase 1 mirror
/// of `drain_status`: reads the SPLIT_PROGRESS side channel only, never
/// LIGHTCLIENT, so it stays responsive while `quick_split` holds the lock.
///
/// Returns, while a round runs: `{ total, built, sent, phase }` where `phase` is
/// `"building"` or `"transmitting"` and the counts are `0..=total`. Returns JSON
/// `null` when no round is in flight.
pub fn split_status() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let status = {
            let progress = SPLIT_PROGRESS
                .read()
                .map_err(|_| ZingolibError::SideChannelPoisoned)?;
            progress.as_ref().and_then(|handle| handle.status())
        };
        Ok(match status {
            Some(s) => {
                use zingolib::lightclient::migrate::SplitPhase;
                object! {
                    "total" => s.total,
                    "built" => s.built,
                    "sent" => s.sent,
                    "phase" => match s.phase {
                        SplitPhase::Building => "building",
                        SplitPhase::Transmitting => "transmitting",
                    },
                }
                .pretty(2)
            }
            None => json::JsonValue::Null.pretty(2),
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
/// parts_confirmed, parts_broadcast, value_total, value_migrated,
/// per_bucket, bucket_modulus,
/// upcoming_windows: [{ bucket_index, boundary, part_ids, denominations,
/// window_opens_unix_time, latest_target_unix_time }], due_now }`.
/// `parts_broadcast` counts parts submitted but not yet mined (the sent,
/// in-flight batch); they sit outside `upcoming_windows` and `due_now` and
/// clear into the confirmed figures as parts mine. `phase` is `null`
/// when no migration is in progress, else `{ kind }` with per-kind fields
/// (`round`/`pending_txids` while note splitting, `residual` when complete).
/// `denominations` (zatoshis) mirror `part_ids` element-for-element, so a
/// schedule screen renders each window's batch without a second call.
/// `due_now` is the batch the client can broadcast this instant (`{ boundary,
/// part_ids, denominations }`) or `null` when a send would build nothing; it
/// carries the current window, which `upcoming_windows` (future windows only)
/// omits.
pub fn migration_status() -> Result<String, ZingolibError> {
    with_initialized_lightclient_read(|lightclient| {
        RT.block_on(async move {
            let status = lightclient.migration_status().await.map_err(ffi_error)?;
            // Join each window's part ids to their denominations (and
            // pick up the effective cadence) from the persisted
            // migration state; BroadcastWindow alone carries only ids.
            let (denoms_by_id, per_bucket, bucket_modulus, parts_broadcast) = {
                let wallet = lightclient.wallet().read().await;
                match &wallet.migration {
                    Some(state) => {
                        // Parts submitted to the network but not yet mined: the
                        // sent, in-flight batch. The confirmed figures gate on
                        // mining, so without this a client cannot tell "batch
                        // sent, confirming" from "batch never sent" (both leave
                        // due_now null and parts_confirmed unchanged).
                        let parts_broadcast = state
                            .parts
                            .iter()
                            .filter(|part| matches!(part.state, PartState::Broadcast))
                            .count() as u32;
                        (
                            state
                                .parts
                                .iter()
                                .map(|part| (part.id.0, part.denomination))
                                .collect::<std::collections::HashMap<_, _>>(),
                            Some(state.params.k_max),
                            state.params.bucket_modulus,
                            parts_broadcast,
                        )
                    }
                    None => (
                        std::collections::HashMap::new(),
                        None,
                        MigrationParams::provisional(wallet.chain_type()).bucket_modulus,
                        0,
                    ),
                }
            };
            let upcoming_windows = status
                .upcoming_windows
                .iter()
                .map(|window| {
                    object! {
                        "bucket_index" => window.bucket_index,
                        "boundary" => u32::from(window.boundary),
                        "part_ids" => window
                            .part_ids
                            .iter()
                            .map(|id| id.0)
                            .collect::<Vec<_>>(),
                        "denominations" => window
                            .part_ids
                            .iter()
                            .map(|id| denoms_by_id.get(&id.0).copied().unwrap_or(0))
                            .collect::<Vec<_>>(),
                        "window_opens_unix_time" => window.window_opens_unix_time,
                        "latest_target_unix_time" => window.latest_target_unix_time,
                    }
                })
                .collect::<Vec<_>>();
            // The window the chain is currently inside, which upcoming_windows
            // structurally omits (it lists future windows only). `null` when a
            // send this instant would build nothing, so the client's Send
            // action gates on it being present.
            let due_now = match &status.due_now {
                Some(batch) => object! {
                    "boundary" => u32::from(batch.boundary),
                    "part_ids" => batch
                        .part_ids
                        .iter()
                        .map(|id| id.0)
                        .collect::<Vec<_>>(),
                    "denominations" => batch.denominations.clone(),
                },
                None => json::JsonValue::Null,
            };
            Ok(object! {
                "orchard_confirmed_spendable" => status.orchard_confirmed_spendable,
                "phase" => match &status.phase {
                    Some(phase) => migration_phase_json(phase),
                    None => json::JsonValue::Null,
                },
                "parts_total" => status.parts_total,
                "parts_confirmed" => status.parts_confirmed,
                "parts_broadcast" => parts_broadcast,
                "value_total" => status.value_total,
                "value_migrated" => status.value_migrated,
                "per_bucket" => per_bucket,
                "bucket_modulus" => bucket_modulus,
                "upcoming_windows" => upcoming_windows,
                "due_now" => due_now,
            }
            .pretty(2))
        })
    })
}

/// The window calendar, for a schedule screen's grid, independent of any
/// migration. The window the chain tip sits in is always present (zero tallies
/// when nothing is scheduled there); every scheduled window, past and future,
/// carries its confirmation progress. So the grid and its "you are here" render
/// before consent, and the same call fills per-window progress afterwards.
///
/// Returns a JSON array of `{ bucket_index, boundary, close, is_current,
/// parts_total, parts_confirmed, value_total, value_migrated }` (heights in
/// blocks with `close` exclusive, values in zatoshis), earliest first. Returns
/// JSON `null` when the wallet has never synced.
pub fn window_timeline() -> Result<String, ZingolibError> {
    with_initialized_lightclient_read(|lightclient| {
        RT.block_on(async move {
            let timeline = lightclient.window_timeline().await.map_err(ffi_error)?;
            Ok(match timeline {
                Some(windows) => {
                    let reports = windows
                        .iter()
                        .map(|w| {
                            object! {
                                "bucket_index" => w.bucket_index,
                                "boundary" => u32::from(w.boundary),
                                "close" => u32::from(w.close),
                                "is_current" => w.is_current,
                                "parts_total" => w.parts_total,
                                "parts_confirmed" => w.parts_confirmed,
                                "value_total" => w.value_total,
                                "value_migrated" => w.value_migrated,
                            }
                        })
                        .collect::<Vec<_>>();
                    json::JsonValue::from(reports).pretty(2)
                }
                None => json::JsonValue::Null.pretty(2),
            })
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
                    window_opens_unix_time,
                } => object! {
                    "kind" => "not_due",
                    "window_opens_unix_time" => *window_opens_unix_time,
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
/// `{ kind: "not_due", window_opens_unix_time }`, or `{ kind: "failed", error }`,
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

/// The Mixnet Mode tri-state-plus-died as the strings the app layer shows.
fn mixnet_mode_string(mode: zingolib::nym::MixnetMode) -> &'static str {
    match mode {
        zingolib::nym::MixnetMode::SwitchedOff => "off",
        zingolib::nym::MixnetMode::Bootstrapping => "bootstrapping",
        zingolib::nym::MixnetMode::Ready => "ready",
        // A never-attached transport (spawn/attach failed) is not consent to
        // clearnet; report it as `died` so the app fails closed and reconnects
        // rather than opening the mixnet-only surfaces.
        zingolib::nym::MixnetMode::Died | zingolib::nym::MixnetMode::Unattached => "died",
    }
}

/// Attach Mixnet Mode to an already-running, platform-hosted SOCKS5 endpoint
/// (the UniFFI proxy shim's address). Readiness is validated by a data round
/// trip; poll [`mixnet_mode`] for `bootstrapping` -> `ready`, or `died`.
pub fn attach_mixnet(socks5_addr: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            RT.block_on(async move {
                lightclient
                    .attach_mixnet(&socks5_addr)
                    .await
                    .map_err(|e| ZingolibError::Mixnet(e.to_string()))?;
                Ok(
                    object! { "mixnet_mode" => mixnet_mode_string(lightclient.mixnet_mode()) }
                        .pretty(2),
                )
            })
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

/// Enable Mixnet Mode by spawning the bundled nym-proxy binary at
/// `proxy_path` (the exec fallback; Android-attached and iOS builds use
/// [`attach_mixnet`] instead).
pub fn enable_mixnet(proxy_path: String) -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            RT.block_on(async move {
                lightclient
                    .enable_mixnet::<zingolib::nym::PrioritisePrivacy>(std::path::Path::new(
                        &proxy_path,
                    ))
                    .await
                    .map_err(|e| ZingolibError::Mixnet(e.to_string()))?;
                Ok(
                    object! { "mixnet_mode" => mixnet_mode_string(lightclient.mixnet_mode()) }
                        .pretty(2),
                )
            })
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

/// Disable Mixnet Mode: the user's deliberate per-session consent to
/// clearnet for the send and price surfaces.
pub fn disable_mixnet() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let mut guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &mut *guard {
            Ok(RT.block_on(async move {
                lightclient.disable_mixnet().await;
                object! { "mixnet_mode" => "off" }.pretty(2)
            }))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

/// The current Mixnet Mode: `off`, `bootstrapping`, `ready` (with the local
/// SOCKS5 address), or `died` (unconsented proxy loss; sends refuse — run
/// [`attach_mixnet`] or [`enable_mixnet`] to recover).
pub fn mixnet_mode() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &*guard {
            let mut status = object! {
                "mixnet_mode" => mixnet_mode_string(lightclient.mixnet_mode()),
            };
            if let Some(addr) = lightclient.mixnet_socks5_addr() {
                status["socks5_addr"] = addr.into();
            }
            Ok(status.pretty(2))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

/// The proxy's latest bootstrap progress line while Mixnet Mode is
/// bootstrapping, so the app can narrate the connect race; empty otherwise.
pub fn mixnet_bootstrap_detail() -> Result<String, ZingolibError> {
    with_panic_guard(|| {
        let guard = LIGHTCLIENT
            .write()
            .map_err(|_| ZingolibError::LightclientLockPoisoned)?;
        if let Some(lightclient) = &*guard {
            let detail = lightclient.mixnet_bootstrap_detail().unwrap_or_default();
            Ok(object! { "detail" => detail }.pretty(2))
        } else {
            Err(ZingolibError::LightclientNotInitialized)
        }
    })
}

/// The canonical IP-correlation disclaimer (ZIP-0318): Mixnet Mode covers only
/// transaction broadcast and price-fetch, and synchronization still exposes
/// the client IP to the sync indexer. One constant from zingolib so every
/// frontend renders the same text; infallible because it needs no wallet and
/// has no failure path.
pub fn mixnet_ip_correlation_disclaimer() -> String {
    zingolib::nym::IP_CORRELATION_DISCLAIMER.to_string()
}
