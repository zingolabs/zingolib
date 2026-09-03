//! Command definitions and dispatch for zingo-cli.

mod error;
#[cfg(test)]
mod tests;
mod utils;

use std::convert::TryInto;
use std::num::NonZeroU32;
use std::str::FromStr;
use std::sync::LazyLock;

use indoc::indoc;
use json::object;
use pepper_sync::config::PerformanceLevel;
use pepper_sync::keys::transparent;
use tokio::runtime::Runtime;

use zcash_address::unified::{Container, Encoding, Ufvk};
use zcash_keys::address::Address;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_protocol::TxId;
use zcash_protocol::consensus::NetworkType;
use zcash_protocol::value::Zatoshis;

use pepper_sync::wallet::{IronwoodNote, KeyIdInterface, OrchardNote, SaplingNote, SyncMode};
use zingo_common_components::protocol::ActivationHeights;
use zingolib::data::{PollReport, proposal};
use zingolib::lightclient::migrate::{
    ImmediateMigrationPhase, ImmediateMigrationStatus, PartSendResult, SplitOutcome, SplitPhase,
    SplitStatus, SplitStep,
};
use zingolib::lightclient::{LightClient, SaveShutdown, TransmitProgressHandle};
use zingolib::utils::conversion::txid_from_hex_encoded_str;
use zingolib::wallet::keys::WalletAddressRef;
use zingolib::wallet::keys::unified::{ReceiverSelection, UnifiedKeyStore};
use zingolib::wallet::migration::{self, MigrationPhase};

pub static RT: LazyLock<Runtime> = LazyLock::new(|| tokio::runtime::Runtime::new().unwrap());

use zingolib::netutils::time::PROGRESS_HEARTBEAT_INTERVAL;

async fn with_heartbeat<T>(
    label: &str,
    interval: std::time::Duration,
    fallback: &str,
    latest: impl Fn() -> Option<String>,
    mut emit: impl FnMut(String),
    operation: impl Future<Output = T>,
) -> T {
    let started = tokio::time::Instant::now();
    let mut ticker = tokio::time::interval_at(started + interval, interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut operation = std::pin::pin!(operation);
    loop {
        tokio::select! {
            output = &mut operation => return output,
            _ = ticker.tick() => {
                let detail = latest().unwrap_or_else(|| fallback.to_string());
                emit(format!(
                    "{label}: {detail} ({}s elapsed)",
                    started.elapsed().as_secs()
                ));
            }
        }
    }
}

/// One read over every live progress side channel, cloned from the client
/// before dispatch so the narration closure never touches the `&mut` borrow
/// an operation holds.
struct ProgressPeek {
    transmit: TransmitProgressHandle,
    batch: zingolib::lightclient::migrate::BatchProgressHandle,
    drain: zingolib::lightclient::migrate::ImmediateMigrationProgressHandle,
    split: zingolib::lightclient::migrate::SplitProgressHandle,
    #[cfg(feature = "nym")]
    mixnet: tokio::sync::watch::Receiver<zingolib::mixnet::MixnetStatus>,
}

impl ProgressPeek {
    fn from_client(lightclient: &LightClient) -> Self {
        Self {
            transmit: lightclient.transmit_progress_handle(),
            batch: lightclient.batch_progress_handle(),
            drain: lightclient.immediate_migration_progress_handle(),
            split: lightclient.split_progress_handle(),
            #[cfg(feature = "nym")]
            mixnet: lightclient.subscribe_mixnet_status(),
        }
    }

    fn latest(&self) -> Option<String> {
        if let Some(line) = self.transmit.latest() {
            return Some(line);
        }
        if let Some(status) = self.batch.status() {
            return Some(batch_progress_line(&status));
        }
        if let Some(status) = self.drain.status() {
            return Some(drain_progress_line(&status));
        }
        if let Some(status) = self.split.status() {
            return Some(split_progress_line(&status));
        }
        #[cfg(feature = "nym")]
        if let Some(detail) = self.mixnet.borrow().bootstrap_detail.clone() {
            return Some(detail);
        }
        None
    }
}

/// The result-is-a-txid-list rendering, kept in one place so every
/// transmitting body shares it.
async fn transmit_txids<T: ToString, E: std::error::Error + Send + Sync + 'static>(
    operation: impl Future<Output = Result<impl IntoIterator<Item = T>, E>>,
) -> Result<String, CommandError> {
    match operation.await {
        Ok(txids) => {
            let txids: Vec<T> = txids.into_iter().collect();
            Ok(object! { "txids" => txids_json(&txids) }.pretty(JSON_INDENT))
        }
        Err(e) => Err(not_yet_typed(e)),
    }
}

/// Typed failure of a CLI command, rendered to prose exactly once, on
/// stderr, at the dispatch seam.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Migration(#[from] MigrationCommandError),
    #[cfg(feature = "nym")]
    #[error(transparent)]
    Network(#[from] NetworkCommandError),
    #[error("the `{0}` command runs only at the interactive prompt")]
    ReplOnly(String),
    /// Transitional quarantine for commands whose failure is not yet
    /// typed, carrying the failure itself so the dispatch renderer still
    /// walks its whole source chain.
    #[error(transparent)]
    NotYetTyped(Box<dyn std::error::Error + Send + Sync + 'static>),
}

/// Separates one link of a rendered cause chain from the next at the
/// dispatch seam, which gives each link its own line.
const DISPATCH_CHAIN_SEPARATOR: &str = "\ncaused by: ";

/// Renders `error` and then every link of its source chain, one `caused
/// by:` line per link, over the one sanctioned chain walk.
pub(crate) fn render_error_chain(error: &(impl std::error::Error + 'static)) -> String {
    zingo_net_diag::chain_texts(error).join(DISPATCH_CHAIN_SEPARATOR)
}

/// A usage failure carrying the standard "Try 'help `<command>`'" pointer,
/// with the command name drawn from the caller instead of re-typed prose.
fn usage(command: &str, detail: impl std::fmt::Display) -> CommandError {
    CommandError::NotYetTyped(
        format!("{detail}\nTry 'help {command}' for correct usage and examples.").into(),
    )
}

/// The indent width of every JSON object the CLI prints.
const JSON_INDENT: u16 = 2;

/// Wraps a failure in the transitional [`CommandError::NotYetTyped`] variant, source chain and all.
fn not_yet_typed(e: impl std::error::Error + Send + Sync + 'static) -> CommandError {
    CommandError::NotYetTyped(Box::new(e))
}

async fn addresses(lightclient: &mut LightClient) -> Result<String, CommandError> {
    Ok(lightclient
        .unified_addresses_json()
        .await
        .pretty(JSON_INDENT))
}

async fn balance(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.account_balance(zip32::AccountId::ZERO).await {
        Ok(bal) => Ok(bal.to_string()),
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn birthday(lightclient: &mut LightClient) -> Result<String, CommandError> {
    Ok(lightclient.birthday().to_string())
}

async fn calculate(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.calculate_stored_proposal().await {
        Ok(txids) => Ok(object! {
            "txids" => txids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>(),
        }
        .pretty(JSON_INDENT)),
        Err(e) => Err(not_yet_typed(e)),
    }
}

/// The `change_server` argument: an empty string names the default uri,
/// as it did when the body parsed the argument itself.
fn parse_server_uri(raw: &str) -> Result<http::Uri, String> {
    if raw.is_empty() {
        return Ok(http::Uri::default());
    }
    http::Uri::from_str(raw).map_err(|_| "invalid server uri".to_string())
}

async fn change_server(
    uri: Option<http::Uri>,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    match lightclient.set_indexer_uri(uri.unwrap_or_default()).await {
        Ok(()) => Ok("server set".to_string()),
        Err(e) => Err(not_yet_typed(e)),
    }
}

/// Renders the wallet's judgment of an address into the `check_address` JSON.
fn address_check_json(address_ref: Option<WalletAddressRef>) -> json::JsonValue {
    address_ref.map_or(
        json::object! { "is_wallet_address" => "false".to_string() },
        |address_ref| match address_ref {
            WalletAddressRef::Unified {
                account_id,
                address_index,
                has_orchard,
                has_sapling,
                has_transparent,
                encoded_address,
            } => json::object! {
                "is_wallet_address" => "true".to_string(),
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
                "is_wallet_address" => "true".to_string(),
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
                "is_wallet_address" => "true".to_string(),
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
                "is_wallet_address" => "true".to_string(),
                "address_type" => "transparent".to_string(),
                "account_id" => u32::from(account_id),
                "scope" => scope.to_string(),
                "address_index" => address_index.index(),
                "encoded_address" => encoded_address,
            },
        },
    )
}

async fn check_address(
    address: &str,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    match lightclient
        .wallet()
        .read()
        .await
        .is_address_derived_by_keys(address)
    {
        Ok(address_ref) => Ok(address_check_json(address_ref).pretty(JSON_INDENT)),
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn clear(lightclient: &mut LightClient) -> Result<String, CommandError> {
    lightclient.wallet().write().await.clear_all();
    Ok(object! { "result" => "success" }.pretty(JSON_INDENT))
}

async fn confirm(lightclient: &mut LightClient) -> Result<String, CommandError> {
    transmit_txids(lightclient.send_stored_proposal(true)).await
}

#[cfg(feature = "nym")]
async fn current_price(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.update_current_price().await {
        Ok(fetch) => {
            let route = match &fetch.route {
                zingolib::lightclient::PriceFetchRoute::Mixnet { via_socks5 } => {
                    format!("over the mixnet via {via_socks5}")
                }
                zingolib::lightclient::PriceFetchRoute::Clearnet => "over clearnet".to_string(),
            };
            Ok(format!(
                "current price: {} USD (source: {}, rtt: {} ms, fetched {})",
                fetch.usd,
                fetch.source.name(),
                fetch.round_trip.as_millis(),
                route
            ))
        }
        Err(e) => Err(not_yet_typed(e)),
    }
}

#[cfg(not(feature = "nym"))]
async fn current_price(_lightclient: &mut LightClient) -> Result<String, CommandError> {
    Ok(
        "This build has no price fetch: price travels only over the Nym mixnet (ADR 0011), \
         and this build switched off the default mixnet support at build time. Rebuild with \
         default features (plain `cargo build`, or `makers run-cli`) to compile it in."
            .to_string(),
    )
}

async fn delete(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.delete_wallet_file().await {
        Ok(()) => Ok(object! {
            "result" => "success",
            "wallet_path" => lightclient.wallet_path().to_str().expect("should be valid UTF-8"),
        }
        .pretty(JSON_INDENT)),
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn export_ufvk(lightclient: &mut LightClient) -> Result<String, CommandError> {
    let ufvk: UnifiedFullViewingKey = match lightclient
        .wallet()
        .read()
        .await
        .unified_key_store
        .get(&zip32::AccountId::ZERO)
        .expect("account 0 must always exist")
        .try_into()
    {
        Ok(ufvk) => ufvk,
        Err(e) => return Err(not_yet_typed(e)),
    };
    Ok(object! {
        "ufvk" => ufvk.encode(&lightclient.chain_type()),
        "birthday" => lightclient.birthday()
    }
    .pretty(JSON_INDENT))
}

async fn height(lightclient: &mut LightClient) -> Result<String, CommandError> {
    Ok(object! {
        "height" => json::JsonValue::from(
            lightclient
                .wallet()
                .read()
                .await
                .sync_state
                .last_known_chain_height()
                .map_or(0, u32::from)
        )
    }
    .pretty(JSON_INDENT))
}

fn help(command: Option<&str>) -> Result<String, CommandError> {
    Ok(format_help(crate::Communications::Online, command))
}

async fn info(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.info().await {
        Ok(info) => Ok(json::JsonValue::from(info).pretty(JSON_INDENT)),
        Err(e) => Err(not_yet_typed(e)),
    }
}

/// Borrows the send family's arguments for the `utils::parse_*` grammars,
/// a JSON-or-positional hybrid clap does not own yet.
fn as_strs(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

async fn max_send_value(
    name: &str,
    args: &[String],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let (address, zennies_for_zingo) =
        utils::parse_max_send_value_args(&as_strs(args)).map_err(|e| usage(name, e))?;
    match lightclient
        .max_send_value(address, zennies_for_zingo, zip32::AccountId::ZERO)
        .await
    {
        Ok(bal) => Ok(object! { "max_send_value" => bal.into_u64() }.pretty(JSON_INDENT)),
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn memobytes_to_address(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.do_total_memobytes_to_address().await {
        Ok(total_memo_bytes) => Ok(json::JsonValue::from(total_memo_bytes).pretty(JSON_INDENT)),
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn messages(
    filter: Option<&str>,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    match lightclient.messages_containing(filter).await {
        Ok(value_transfers) => Ok(json::JsonValue::from(value_transfers).pretty(JSON_INDENT)),
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn migrate(lightclient: &mut LightClient) -> Result<String, CommandError> {
    Ok(run_migrate(lightclient).await?)
}

async fn migration(
    sub: MigrationSubCommand,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    Ok(run_migration(sub, lightclient).await?)
}

/// The `new_address` argument: `o`, `z`, or both, naming the receivers the
/// new unified address carries (transparent receivers are `new_taddress`'s).
fn parse_receiver_selection(raw: &str) -> Result<ReceiverSelection, String> {
    if raw.is_empty()
        || raw
            .chars()
            .any(|receiver| receiver != 'o' && receiver != 'z')
    {
        return Err("the address type must be o, z, or oz".to_string());
    }
    Ok(ReceiverSelection {
        orchard: raw.contains('o'),
        sapling: raw.contains('z'),
    })
}

async fn new_address(
    receivers: ReceiverSelection,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let chain_type = lightclient.chain_type();
    let mut wallet = lightclient.wallet().write().await;
    match wallet.generate_unified_address(receivers, zip32::AccountId::ZERO) {
        Ok((id, unified_address)) => Ok(json::object! {
            "account" => u32::from(zip32::AccountId::ZERO), // used concrete type instead of u32 to simplify upgrading CLI to multi-account
            "address_index" => id.address_index,
            "has_orchard" => unified_address.has_orchard(),
            "has_sapling" => unified_address.has_sapling(),
            "has_transparent" => unified_address.has_transparent(),
            "encoded_address" => unified_address.encode(&chain_type),
        }
        .pretty(JSON_INDENT)),
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn taddress(
    lightclient: &mut LightClient,
    enforce_no_gap: bool,
) -> Result<String, CommandError> {
    let chain_type = lightclient.chain_type();
    let mut wallet = lightclient.wallet().write().await;
    match wallet.generate_transparent_address(zip32::AccountId::ZERO, enforce_no_gap) {
        Ok((id, transparent_address)) => Ok(json::object! {
            "account" => u32::from(id.account_id()),
            "address_index" => id.address_index().index(),
            "scope" => id.scope().to_string(),
            "encoded_address" => transparent::encode_address(&chain_type, transparent_address),
        }
        .pretty(JSON_INDENT)),
        Err(e) => Err(not_yet_typed(e)),
    }
}

/// The one optional argument of `notes` and `coins`: `all`, widening the
/// listing to the spent outputs.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputScope {
    All,
}

async fn notes(
    scope: Option<OutputScope>,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let all_notes = scope.is_some();
    let wallet = lightclient.wallet().read().await;
    Ok(json::object! {
        "ironwood_notes" => json::JsonValue::from(wallet.note_summaries::<IronwoodNote>(all_notes)),
        "orchard_notes" => json::JsonValue::from(wallet.note_summaries::<OrchardNote>(all_notes)),
        "sapling_notes" => json::JsonValue::from(wallet.note_summaries::<SaplingNote>(all_notes)),
    }
    .pretty(JSON_INDENT))
}

async fn coins(
    scope: Option<OutputScope>,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let all_coins = scope.is_some();
    Ok(json::object! {
        "transparent_coins" => json::JsonValue::from(
            lightclient.wallet().read().await.coin_summaries(all_coins)
        ),
    }
    .pretty(JSON_INDENT))
}

async fn quicksend(
    name: &str,
    args: &[String],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let receivers = utils::parse_send_args(&as_strs(args)).map_err(|e| usage(name, e))?;
    let request = zingolib::data::receivers::transaction_request_from_receivers(receivers)
        .map_err(|e| usage(name, e))?;
    match lightclient
        .quick_send_reported(request, zip32::AccountId::ZERO, true)
        .await
    {
        Ok(reports) => Ok(object! {
            "txids" => txids_json(&reports.iter().map(|report| report.txid).collect::<Vec<_>>()),
            "transmissions" => reports.iter().map(render_transmit_report).collect::<Vec<_>>(),
        }
        .pretty(JSON_INDENT)),
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn quickshield(lightclient: &mut LightClient) -> Result<String, CommandError> {
    transmit_txids(lightclient.quick_shield(zip32::AccountId::ZERO)).await
}

async fn quit(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.shutdown_save_task().await {
        Ok(SaveShutdown::ShutDown) => eprintln!("Save task shutdown successfully."),
        Ok(SaveShutdown::NotRunning) => eprintln!("No save task was running."),
        Err(e) => eprintln!("Error: save failed. {}", render_error_chain(&e)),
    }
    Ok("Zingo CLI quit successfully.".to_string())
}

async fn recovery_info(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.wallet().read().await.recovery_info() {
        Some(backup_info) => Ok(backup_info.to_string()),
        None => Err(CommandError::NotYetTyped(
            "no mnemonic found. wallet loaded from key.".into(),
        )),
    }
}

/// A transaction id argument, hex-encoded as `calculate` and the wallet's
/// listings print it.
fn parse_txid(raw: &str) -> Result<TxId, String> {
    txid_from_hex_encoded_str(raw).map_err(|e| e.to_string())
}

async fn remove_transaction(
    txid: TxId,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    match lightclient
        .wallet()
        .write()
        .await
        .remove_failed_transaction(txid)
    {
        Ok(()) => Ok("Successfully removed failed transaction.".to_string()),
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn rescan(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.rescan().await {
        Ok(()) => Ok("Launching rescan...".to_string()),
        Err(e) => Err(not_yet_typed(e)),
    }
}

/// A parsed `save` command, its arguments parsed completely at the clap
/// derive grammar before any wallet access.
#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SaveSubCommand {
    #[command(about = "Launch the task that persists the wallet as its state changes")]
    Run,
    #[command(about = "Check the save task for a recorded failure, restarting it on one")]
    Check,
    #[command(about = "Shut the save task down")]
    Shutdown,
}

async fn save(sub: SaveSubCommand, lightclient: &mut LightClient) -> Result<String, CommandError> {
    match sub {
        SaveSubCommand::Run => {
            lightclient.save_task().await;
            Ok("Launching save task...".to_string())
        }
        SaveSubCommand::Check => match lightclient.check_save_error().await {
            Ok(()) => Ok(String::new()),
            Err(e) => Err(CommandError::NotYetTyped(
                format!(
                    "save failed. {}\nRestarting save task...",
                    render_error_chain(&e)
                )
                .into(),
            )),
        },
        SaveSubCommand::Shutdown => match lightclient.shutdown_save_task().await {
            Ok(SaveShutdown::ShutDown) => Ok("Save task shutdown successfully.".to_string()),
            Ok(SaveShutdown::NotRunning) => Ok("No save task was running.".to_string()),
            Err(e) => Err(CommandError::NotYetTyped(
                format!("save failed. {}", render_error_chain(&e)).into(),
            )),
        },
    }
}

async fn send(
    name: &str,
    args: &[String],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let receivers = utils::parse_send_args(&as_strs(args)).map_err(|e| usage(name, e))?;
    let request = zingolib::data::receivers::transaction_request_from_receivers(receivers)
        .map_err(|e| usage(name, e))?;
    match lightclient
        .propose_send(request, zip32::AccountId::ZERO)
        .await
    {
        Ok(proposal) => match zingolib::data::proposal::total_fee(&proposal) {
            Ok(fee) => Ok(object! { "fee" => fee.into_u64() }.pretty(JSON_INDENT)),
            Err(e) => Err(not_yet_typed(e)),
        },
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn send_all(
    name: &str,
    args: &[String],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let (address, zennies_for_zingo, memo) =
        utils::parse_send_all_args(&as_strs(args)).map_err(|e| usage(name, e))?;
    match lightclient
        .propose_send_all(address, zennies_for_zingo, memo, zip32::AccountId::ZERO)
        .await
    {
        Ok(proposal) => {
            let amount = proposal::total_payment_amount(&proposal).map_err(not_yet_typed)?;
            let fee = proposal::total_fee(&proposal).map_err(not_yet_typed)?;
            Ok(object! {
                "amount" => amount.into_u64(),
                "fee" => fee.into_u64(),
            }
            .pretty(JSON_INDENT))
        }
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn sends_to_address(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.do_total_spends_to_address().await {
        Ok(total_spends) => Ok(json::JsonValue::from(total_spends).pretty(JSON_INDENT)),
        Err(e) => Err(not_yet_typed(e)),
    }
}

/// A parsed `settings` command, naming which setting to write and its
/// value, or reading the settings out when no setting is named.
#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq)]
#[command(rename_all = "snake_case")]
pub(crate) enum SettingsSubCommand {
    #[command(about = "Set the sync performance level")]
    Performance {
        #[arg(value_enum)]
        level: PerformanceLevelArg,
    },
    #[command(about = "Set how many confirmations a note needs to be spendable")]
    MinConfirmations {
        #[arg(value_name = "count")]
        count: NonZeroU32,
    },
    #[command(about = "Set the gap limit for transparent address and fund discovery")]
    TransparentGapLimit {
        #[arg(value_name = "gap limit")]
        gap_limit: u8,
    },
}

/// The sync performance levels as a clap grammar, minting CLI value names
/// for pepper-sync's [`PerformanceLevel`] and converting.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerformanceLevelArg {
    Low,
    Medium,
    High,
    Maximum,
}

impl From<PerformanceLevelArg> for PerformanceLevel {
    fn from(level: PerformanceLevelArg) -> Self {
        match level {
            PerformanceLevelArg::Low => PerformanceLevel::Low,
            PerformanceLevelArg::Medium => PerformanceLevel::Medium,
            PerformanceLevelArg::High => PerformanceLevel::High,
            PerformanceLevelArg::Maximum => PerformanceLevel::Maximum,
        }
    }
}

async fn settings(
    sub: Option<SettingsSubCommand>,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let mut wallet = lightclient.wallet().write().await;

    let Some(sub) = sub else {
        return Ok(format!(
            r"
performance: {}
min confirmations: {}
transparent gap limit: {}
            ",
            wallet.wallet_settings.sync_config.performance_level,
            wallet.wallet_settings.min_confirmations,
            wallet
                .wallet_settings
                .sync_config
                .transparent_address_discovery
                .gap_limit
        ));
    };

    match sub {
        SettingsSubCommand::Performance { level } => {
            wallet.wallet_settings.sync_config.performance_level = level.into();
        }
        SettingsSubCommand::MinConfirmations { count } => {
            wallet.wallet_settings.min_confirmations = count;
        }
        SettingsSubCommand::TransparentGapLimit { gap_limit } => {
            wallet
                .wallet_settings
                .sync_config
                .transparent_address_discovery
                .gap_limit = gap_limit;
        }
    }

    wallet.mark_dirty();

    Ok("Successfully updated settings.".to_string())
}

async fn shield(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.propose_shield(zip32::AccountId::ZERO).await {
        Ok(proposal) => {
            if proposal.steps().len() != 1 {
                return Err(CommandError::NotYetTyped(
                    "shielding transactions should not have multiple proposal steps".into(),
                ));
            }
            let step = proposal.steps().first();
            let Some(value_to_shield) = step
                .balance()
                .proposed_change()
                .iter()
                .try_fold(Zatoshis::ZERO, |acc, c| acc + c.value())
            else {
                return Err(CommandError::NotYetTyped(
                    "shield amount outside valid range of zatoshis".into(),
                ));
            };
            let fee = step.balance().fee_required();
            Ok(object! {
                "value_to_shield" => value_to_shield.into_u64(),
                "fee" => fee.into_u64(),
            }
            .pretty(JSON_INDENT))
        }
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn spendable_balance(lightclient: &mut LightClient) -> Result<String, CommandError> {
    let wallet = lightclient.wallet().read().await;
    let spendable_balance = wallet
        .shielded_spendable_balance(zip32::AccountId::ZERO, false)
        .map_err(not_yet_typed)?;
    Ok(object! {
        "spendable_balance" => spendable_balance.into_u64(),
    }
    .pretty(JSON_INDENT))
}

/// A parsed `sync` command, its arguments parsed completely at the clap
/// derive grammar before any wallet access.
#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SyncSubCommand {
    #[command(about = "Start the sync task, or resume it when paused")]
    Run,
    #[command(about = "Pause the sync task's scanning")]
    Pause,
    #[command(about = "Shut the sync task down early")]
    Stop,
    #[command(about = "Report sync progress")]
    Status,
    #[command(about = "Report the finished sync's result. Not meant to be called by hand")]
    Poll,
}

async fn sync(sub: SyncSubCommand, lightclient: &mut LightClient) -> Result<String, CommandError> {
    match sub {
        SyncSubCommand::Run => {
            if lightclient.sync_mode() == SyncMode::Paused {
                lightclient.resume_sync().expect("sync should be paused");
                Ok("Resuming sync task...".to_string())
            } else {
                match lightclient.sync().await {
                    Ok(()) => Ok("Launching sync task...".to_string()),
                    Err(zingolib::lightclient::error::LightClientError::SyncModeError(
                        pepper_sync::error::SyncModeError::SyncAlreadyRunning,
                    )) => Ok("Sync task already running.".to_string()),
                    Err(e) => Err(not_yet_typed(e)),
                }
            }
        }
        SyncSubCommand::Pause => match lightclient.pause_sync() {
            Ok(()) => Ok("Pausing sync task...".to_string()),
            Err(e) => Err(not_yet_typed(e)),
        },
        SyncSubCommand::Stop => match lightclient.stop_sync() {
            Ok(()) => Ok("Stopping sync task...".to_string()),
            Err(e) => Err(not_yet_typed(e)),
        },
        SyncSubCommand::Status => {
            let status = match lightclient.latest_sync_status() {
                Some(status) if lightclient.sync_mode() != SyncMode::NotRunning => status,
                _ => pepper_sync::sync_status(&*lightclient.wallet().read().await)
                    .await
                    .map_err(not_yet_typed)?,
            };
            Ok(json::JsonValue::from(status).pretty(JSON_INDENT))
        }
        SyncSubCommand::Poll => match lightclient.poll_sync() {
            PollReport::NoHandle => Ok("Sync task has not been launched.".to_string()),
            PollReport::NotReady => Ok("Sync task is not complete.".to_string()),
            PollReport::Ready(result) => match result {
                Ok(sync_result) => Ok(sync_result.to_string()),
                Err(e) => Err(not_yet_typed(e)),
            },
        },
    }
}

async fn t_addresses(lightclient: &mut LightClient) -> Result<String, CommandError> {
    Ok(lightclient
        .transparent_addresses_json()
        .await
        .pretty(JSON_INDENT))
}

async fn transactions(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.transaction_summaries(false).await {
        Ok(transactions) => Ok(transactions.to_string()),
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn transmit(
    requested: Vec<TxId>,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let txids = if requested.is_empty() {
        // All Calculated transactions, ordered by target height and
        // then txid for determinism.
        let wallet = lightclient.wallet().read().await;
        let mut calculated: Vec<_> = wallet
            .wallet_transactions
            .iter()
            .filter(|(_, transaction)| {
                matches!(
                    transaction.status(),
                    zingo_status::confirmation_status::ConfirmationStatus::Calculated(_)
                )
            })
            .map(|(txid, transaction)| (transaction.status().get_height(), *txid))
            .collect();
        calculated.sort_by_key(|&(height, txid)| (height, txid.as_ref().to_owned()));
        calculated.into_iter().map(|(_, txid)| txid).collect()
    } else {
        requested
    };

    let Some(txids) = nonempty::NonEmpty::from_vec(txids) else {
        return Err(CommandError::NotYetTyped(
            "no calculated transactions to transmit".into(),
        ));
    };

    transmit_txids(lightclient.transmit_calculated(txids)).await
}

async fn value_to_address(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.do_total_value_to_address().await {
        Ok(total_values) => Ok(json::JsonValue::from(total_values).pretty(JSON_INDENT)),
        Err(e) => Err(not_yet_typed(e)),
    }
}

async fn value_transfers(lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.value_transfers(false).await {
        Ok(value_transfers) => Ok(value_transfers.to_string()),
        Err(e) => Err(not_yet_typed(e)),
    }
}

fn version() -> Result<String, CommandError> {
    Ok(zingolib::git_description().to_string())
}

async fn wallet_kind(lightclient: &mut LightClient) -> Result<String, CommandError> {
    if lightclient.mnemonic_phrase().is_some() {
        return Ok(object! {"kind" => "Loaded from mnemonic (seed or phrase)",
                "transparent" => true,
                "sapling" => true,
                "orchard" => true,
        }
        .pretty(4));
    }
    Ok(
        match lightclient
            .wallet()
            .read()
            .await
            .unified_key_store
            .get(&zip32::AccountId::ZERO)
            .expect("account 0 must always exist")
        {
            UnifiedKeyStore::Spend(_) => object! {
                "kind" => "Loaded from unified spending key",
                "transparent" => true,
                "sapling" => true,
                "orchard" => true,
            }
            .pretty(4),
            UnifiedKeyStore::View(ufvk) => object! {
                "kind" => "Loaded from unified full viewing key",
                "transparent" => ufvk.transparent().is_some(),
                "sapling" => ufvk.sapling().is_some(),
                "orchard" => ufvk.orchard().is_some(),
            }
            .pretty(4),
            UnifiedKeyStore::Empty => object! {
                "kind" => "No keys found",
                "transparent" => false,
                "sapling" => false,
                "orchard" => false,
            }
            .pretty(4),
        },
    )
}

fn parse_address(address: &str) -> Result<String, CommandError> {
    fn make_decoded_chain_pair(
        address: &str,
    ) -> Option<(
        zcash_client_backend::address::Address,
        zingolib::config::ChainType,
    )> {
        [
            zingolib::config::ChainType::Mainnet,
            zingolib::config::ChainType::Testnet,
            zingolib::config::ChainType::Regtest(ActivationHeights::default()),
        ]
        .iter()
        .find_map(|chain| Address::decode(chain, address).zip(Some(*chain)))
    }
    Ok(
        if let Some((recipient_address, chain_name)) = make_decoded_chain_pair(address) {
            #[allow(unreachable_patterns)]
            let chain_name_string = match chain_name {
                zingolib::config::ChainType::Mainnet => "main",
                zingolib::config::ChainType::Testnet => "test",
                zingolib::config::ChainType::Regtest(_) => "regtest",
                _ => unreachable!("Invalid chain type"),
            };
            match recipient_address {
                Address::Sapling(_) => object! {
                    "status" => "success",
                    "chain_name" => chain_name_string,
                    "address_kind" => "sapling",
                }
                .to_string(),
                Address::Transparent(_) => object! {
                    "status" => "success",
                    "chain_name" => chain_name_string,
                    "address_kind" => "transparent",
                }
                .to_string(),
                Address::Tex(_) => object! {
                    "status" => "success",
                    "chain_name" => chain_name_string,
                    "address_kind" => "tex",
                }
                .to_string(),
                Address::Unified(ua) => {
                    let mut receivers_available = vec![];
                    if ua.sapling().is_some() {
                        receivers_available.push("sapling");
                    }
                    if ua.transparent().is_some() {
                        receivers_available.push("transparent");
                    }
                    if ua.orchard().is_some() {
                        receivers_available.push("orchard");
                        object! {
                        "status" => "success",
                        "chain_name" => chain_name_string,
                        "address_kind" => "unified",
                        "receivers_available" => receivers_available,
                        "only_orchard_ua" => zcash_keys::address::UnifiedAddress::from_receivers(ua.orchard().copied(), None, None).expect("To construct UA").encode(&chain_name),
                    }
                    .to_string()
                    } else {
                        object! {
                            "status" => "success",
                            "chain_name" => chain_name_string,
                            "address_kind" => "unified",
                            "receivers_available" => receivers_available,
                        }
                        .to_string()
                    }
                }
            }
        } else {
            object! {
                "status" => "Invalid address",
                "chain_name" => json::JsonValue::Null,
                "address_kind" => json::JsonValue::Null,
            }
            .to_string()
        },
    )
}

fn parse_viewkey(viewkey: &str) -> Result<String, CommandError> {
    Ok(json::stringify_pretty(
        match Ufvk::decode(viewkey) {
            Ok((network, ufvk)) => {
                let mut pools_available = vec![];
                for fvk in ufvk.items_as_parsed() {
                    match fvk {
                        zcash_address::unified::Fvk::Orchard(_) => {
                            pools_available.push("orchard");
                        }
                        zcash_address::unified::Fvk::Sapling(_) => {
                            pools_available.push("sapling");
                        }
                        zcash_address::unified::Fvk::P2pkh(_) => {
                            pools_available.push("transparent");
                        }
                        zcash_address::unified::Fvk::Unknown { .. } => pools_available
                            .push("Unknown future protocol. Perhaps you're using old software"),
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
        4,
    ))
}

async fn drain(
    sub: DrainSubCommand,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    Ok(run_drain(sub, lightclient).await?)
}

async fn split(
    sub: SplitSubCommand,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    Ok(run_split(sub, lightclient).await?)
}

#[cfg(feature = "nym")]
async fn network(
    sub: Option<NetworkSubCommand>,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    Ok(network_command(sub.unwrap_or(NetworkSubCommand::Status), lightclient).await?)
}

/// This consumer's platform hints for provisioning the `nym-proxy` binary:
/// the explicit flag value and the executable-sibling bundled directory
/// (where the `bundle-nym-proxy` workbench tool places the binary).
/// [`zingolib::mixnet::provision`] owns the precedence rule and its tests
/// (ADR 0024); this names only what zingolib cannot know by itself. Shared
/// by the session driver call at startup and the `network on` command.
#[cfg(feature = "nym")]
pub(crate) fn spawn_hints(explicit: Option<&str>) -> zingolib::mixnet::provision::SpawnHints<'_> {
    use zingolib::mixnet::provision::{self, SpawnHints};
    SpawnHints {
        explicit,
        bundled_dir: provision::executable_sibling_dir(),
    }
}

/// Resolve the `nym-proxy` binary path from this consumer's
/// [`spawn_hints`], for the `network on` command's in-session enable.
#[cfg(feature = "nym")]
pub(crate) fn resolve_proxy_path(explicit: Option<&str>) -> String {
    zingolib::mixnet::provision::resolve_proxy_path(&spawn_hints(explicit))
}

/// Typed failure of the `network` command family. The family exists only
/// with the mixnet capability compiled in: a build without it has no
/// `network` command, because Offline Mode is the only mode such a build
/// can be in (ADR 0026).
#[cfg(feature = "nym")]
#[derive(Debug, thiserror::Error)]
pub enum NetworkCommandError {
    /// `network probe` runs only over the mixnet route; this carries the
    /// typed refusal naming the transport state and its remedy.
    #[error(transparent)]
    Probe(#[from] zingolib::lightclient::error::LightClientError),
    /// The `network on` consent act could not resolve any indexer URI while
    /// switching the session to Online Mode; the session stays offline.
    /// Reachable only from the quarantined clearnet resolution.
    #[cfg(feature = "clearnet-test-mode")]
    #[error("no indexer could be resolved for going online")]
    ServerResolution(#[from] crate::server_select_clearnet::ResolveServerError),
    /// The `network on` consent act selected an indexer, but the connection
    /// failed; the session stays offline. Reachable only from the
    /// quarantined clearnet resolution.
    #[cfg(feature = "clearnet-test-mode")]
    #[error("failed to connect to '{uri}' while switching to Online Mode")]
    GoOnline {
        uri: String,
        source: zingolib::netutils::GetClientError,
    },
    #[error("failed to start the nym proxy at '{path}'")]
    ProxyStart {
        path: String,
        source: zingolib::mixnet::acquire::TransportError,
    },
    /// The proxy spawned but its bootstrap reached a terminal failure while
    /// the command waited; re-enabling spawns a fresh proxy.
    #[error("the mixnet bootstrap failed: {report}. Re-enable with `network on`.")]
    Bootstrap { report: String },
}

/// A parsed `network` command, its arguments parsed completely at the clap
/// derive grammar before any wallet access.
#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq)]
pub(crate) enum NetworkSubCommand {
    #[command(about = "Report the mixnet state: off, bootstrapping, or ready")]
    Status,
    #[command(about = "Start the nym-proxy child and route transmissions through the mixnet")]
    On {
        #[arg(value_name = "proxy_path")]
        path: Option<String>,
    },
    #[command(
        about = "Disconnect every network capability of the session, keeping any stored consent"
    )]
    Off,
    #[command(about = "Probe indexer liveness over the mixnet route")]
    Probe {
        #[arg(value_name = "indexer_uri", value_parser = parse_probe_target)]
        target: Option<http::Uri>,
    },
    #[command(about = "Show per-indexer attempts across sessions")]
    History,
}

/// https on port 443 only in a mixnet build — the one endpoint shape the
/// exit policy carries — so the grammar refuses anything else up front,
/// while a build without the feature defers to the typed refusal.
fn parse_probe_target(raw: &str) -> Result<http::Uri, String> {
    let uri = raw
        .parse::<http::Uri>()
        .map_err(|_| "not a valid indexer uri to probe".to_string())?;
    #[cfg(feature = "nym")]
    if !zingolib::mixnet::probe::probe_eligible(&uri) {
        return Err("probe targets must be https on port 443".to_string());
    }
    Ok(uri)
}

#[cfg(feature = "nym")]
use zingolib::netutils::time::PROBE_LEG_TIMEOUT;

/// Render one mixnet liveness probe. Pure, pinned by unit tests.
#[cfg(feature = "nym")]
fn render_mixnet_probe(probe: &zingolib::mixnet::probe::MixnetProbe) -> String {
    let leg = |leg: &zingolib::mixnet::probe::ProbeLeg| match &leg.outcome {
        Ok(success) => format!("ok in {}ms: height {}", leg.millis, success.height),
        Err(failure) => format!("FAILED after {}ms: {failure}", leg.millis),
    };
    format!("{}\n  mixnet:   {}", probe.host, leg(&probe.leg))
}

/// Renders this session's accumulated per-indexer record for `network history`.
#[cfg(feature = "nym")]
fn nym_history_command(lightclient: &LightClient) -> String {
    render_history(
        &lightclient.indexer_history_handle().load(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0),
    )
}

/// Render this session's per-indexer history as per-host, per-route
/// aggregates, most-attempted hosts first. Pure over the loaded attempts and
/// a caller-supplied "now" so tests pin the ages.
#[cfg(feature = "nym")]
fn render_history(
    attempts: &[zingolib::lightclient::indexer_history::IndexerAttempt],
    now_unix_secs: u64,
) -> String {
    use std::collections::BTreeMap;

    use zingolib::lightclient::indexer_history::AttemptRoute;

    if attempts.is_empty() {
        return "No indexer history recorded yet.".to_string();
    }

    struct RouteStats {
        attempts: usize,
        ok: usize,
        last_unix_secs: u64,
        last_ok: bool,
    }
    let mut hosts: BTreeMap<String, BTreeMap<&'static str, RouteStats>> = BTreeMap::new();
    for attempt in attempts {
        let route = match attempt.route {
            AttemptRoute::Clearnet => "clearnet",
            AttemptRoute::Mixnet => "mixnet",
        };
        let stats = hosts
            .entry(attempt.host.to_string())
            .or_default()
            .entry(route)
            .or_insert(RouteStats {
                attempts: 0,
                ok: 0,
                last_unix_secs: 0,
                last_ok: false,
            });
        stats.attempts += 1;
        if attempt.outcome.is_ok() {
            stats.ok += 1;
        }
        if attempt.unix_secs >= stats.last_unix_secs {
            stats.last_unix_secs = attempt.unix_secs;
            stats.last_ok = attempt.outcome.is_ok();
        }
    }

    let age = |unix_secs: u64| -> String {
        let elapsed = now_unix_secs.saturating_sub(unix_secs);
        match elapsed {
            0..60 => format!("{elapsed}s"),
            60..3600 => format!("{}m", elapsed / 60),
            3600..86400 => format!("{}h", elapsed / 3600),
            _ => format!("{}d", elapsed / 86400),
        }
    };

    let mut lines = vec!["Indexer history (all sessions):".to_string()];
    for (host, routes) in &hosts {
        let mut summaries: Vec<String> = Vec::new();
        for (route, stats) in routes {
            summaries.push(format!(
                "{route} {}/{} ok, last {} {} ago",
                stats.ok,
                stats.attempts,
                if stats.last_ok { "ok" } else { "failed" },
                age(stats.last_unix_secs),
            ));
        }
        lines.push(format!("  {host}: {}", summaries.join("; ")));
    }
    lines.join("\n")
}

/// Render the `network status` line for a Mixnet Mode, the live bootstrap
/// progress while bootstrapping, and the local SOCKS5 address when ready.
/// Pure, so the user-facing mode strings are pinned by unit tests and
/// reusable by any other frontend.
#[cfg(feature = "nym")]
fn render_status(
    mode: zingolib::mixnet::Indicator,
    socks5_addr: Option<&str>,
    bootstrap_detail: Option<&str>,
) -> String {
    use zingolib::mixnet::Indicator;

    match mode {
        Indicator::Unattached => "Mixnet Mode: unattached. The mixnet has not been enabled, \
             and no consent to clearnet has been given: send and price-fetch refuse. Run \
             `network on` to enable the mixnet, or `network off` to use clearnet."
            .to_string(),
        Indicator::SwitchedOff => {
            "Mixnet Mode: switched off (send and price-fetch use clearnet)".to_string()
        }
        Indicator::Bootstrapping => match bootstrap_detail {
            Some(detail) => format!(
                "Mixnet Mode: bootstrapping, {detail} (send and price-fetch are unavailable \
                 until ready)"
            ),
            None => "Mixnet Mode: bootstrapping (send and price-fetch are unavailable until ready)"
                .to_string(),
        },
        Indicator::Ready => match socks5_addr {
            Some(addr) => format!("Mixnet Mode: ready (SOCKS5 {addr})"),
            None => "Mixnet Mode: ready".to_string(),
        },
        Indicator::PreviouslyProvenThisEpoch => match socks5_addr {
            Some(addr) => format!(
                "Mixnet Mode: previously proven this epoch (SOCKS5 {addr}; the exit's \
                 proof is stale until a round trip of this session confirms it)"
            ),
            None => "Mixnet Mode: previously proven this epoch".to_string(),
        },
        Indicator::Died => "Mixnet Mode: died. The proxy exited unexpectedly. Send and \
             price-fetch refuse and will not fall back to clearnet. Run `network on` to \
             restart the proxy."
            .to_string(),
    }
}

/// The IP-correlation disclaimer this frontend shows alongside every Mixnet
/// Mode status.
#[cfg(feature = "nym")]
const IP_CORRELATION_DISCLAIMER: &str = "\
IP-correlation risk: Mixnet Mode covers only transaction transmission and \
price-fetch. Wallet synchronization always uses the ordinary connection, so \
the sync indexer (and any network operator on that path) sees your IP \
address and can correlate it with the transactions you transmit; reusing the \
same IP across sessions can reveal your wallet's total balance to that \
operator. To hide your IP during synchronization as well, route the wallet \
through a system-level VPN or NymVPN. See ZIP-0318.";

/// Render the Mixnet Mode status line followed by the IP-correlation
/// disclaimer.
#[cfg(feature = "nym")]
fn render_status_with_disclaimer(
    mode: zingolib::mixnet::Indicator,
    socks5_addr: Option<&str>,
    bootstrap_detail: Option<&str>,
) -> String {
    format!(
        "{}\n\n{}",
        render_status(mode, socks5_addr, bootstrap_detail),
        IP_CORRELATION_DISCLAIMER,
    )
}

/// The terminal readings of one bootstrap wait.
#[cfg(feature = "nym")]
#[derive(Debug, PartialEq, Eq)]
enum BootstrapOutcome {
    Ready {
        exits: Vec<zingolib::mixnet::ExitNodeId>,
    },
    Failed {
        report: String,
    },
}

/// Renders the bound Exit Nodes for the `network on` success report,
/// shortening each identity for the terminal.
#[cfg(feature = "nym")]
pub(crate) fn render_exit_nodes(exits: &[zingolib::mixnet::ExitNodeId]) -> String {
    fn shorten(identity: &str) -> String {
        if identity.chars().count() > 15 {
            let head: String = identity.chars().take(12).collect();
            format!("{head}…")
        } else {
            identity.to_string()
        }
    }
    let named: Vec<String> = exits.iter().map(|exit| shorten(exit.as_str())).collect();
    match named.len() {
        0 => String::new(),
        1 => format!(" Exit Node bound: {}.", named[0]),
        _ => format!(" Exit Nodes bound: {}.", named.join(", ")),
    }
}

/// Waits on the status subscription until the bootstrap reaches a terminal
/// mode, so `network on` reports an outcome instead of a promise to poll.
#[cfg(feature = "nym")]
async fn await_bootstrap_outcome(
    mut rx: tokio::sync::watch::Receiver<zingolib::mixnet::MixnetStatus>,
) -> BootstrapOutcome {
    use zingolib::mixnet::Indicator;
    let mut was_bootstrapping = false;
    loop {
        let status = rx.borrow_and_update().clone();
        match status.mode {
            Indicator::Ready | Indicator::PreviouslyProvenThisEpoch => {
                return BootstrapOutcome::Ready {
                    exits: status.exits.clone(),
                };
            }
            Indicator::Died => {
                let cause = status
                    .death
                    .as_ref()
                    .and_then(|death| death.detail.as_ref())
                    .map(|detail| format!(": {detail}"))
                    .unwrap_or_default();
                return BootstrapOutcome::Failed {
                    report: format!("the mixnet transport died{cause}"),
                };
            }
            Indicator::Bootstrapping => was_bootstrapping = true,
            Indicator::Unattached | Indicator::SwitchedOff if was_bootstrapping => {
                return BootstrapOutcome::Failed {
                    report: format!("the bootstrap ended in mode {}", status.mode),
                };
            }
            Indicator::Unattached | Indicator::SwitchedOff => {}
        }
        if rx.changed().await.is_err() {
            return BootstrapOutcome::Failed {
                report: "the mixnet status channel closed".to_string(),
            };
        }
    }
}

/// The body of the `network` command; the command exists only with the
/// mixnet transport compiled in (ADR 0026).
#[cfg(feature = "nym")]
async fn network_command(
    sub: NetworkSubCommand,
    lightclient: &mut LightClient,
) -> Result<String, NetworkCommandError> {
    match sub {
        NetworkSubCommand::Status => {
            let socks5 = lightclient
                .mixnet_socks5_addr()
                .map(|addr| addr.to_string());
            Ok(render_status_with_disclaimer(
                lightclient.read_mixnet_indicator(),
                socks5.as_deref(),
                lightclient.mixnet_bootstrap_detail().as_deref(),
            ))
        }
        NetworkSubCommand::On { path } => {
            // In an offline session, `network on` is itself the
            // Connectivity Consent act (ADR 0026, amending ADR 0025's
            // act list): the session switches to Online Mode for this
            // session only by bootstrapping the mixnet. It engages no
            // clearnet indexer link; the quarantined clearnet resolution
            // survives only under `clearnet-test-mode`.
            #[cfg(feature = "clearnet-test-mode")]
            let went_online = if lightclient.indexer_uri().is_none() {
                let (server, _ranked) =
                    crate::server_select_clearnet::resolve_ranked_server().await?;
                lightclient
                    .set_indexer_uri(server.clone())
                    .await
                    .map_err(|source| NetworkCommandError::GoOnline {
                        uri: server.to_string(),
                        source,
                    })?;
                Some(server)
            } else {
                None
            };
            #[cfg(not(feature = "clearnet-test-mode"))]
            let went_online: Option<http::Uri> = None;
            let path = resolve_proxy_path(path.as_deref());
            // `network on` waits out the standing client's proven birth: the
            // command returns with a client whose exit carried a round trip.
            lightclient
                .enable_mixnet(std::path::Path::new(&path))
                .await
                .map_err(|source| NetworkCommandError::ProxyStart {
                    path: path.clone(),
                    source,
                })?;
            // The enable itself waited out the proven birth — the six-birth
            // budget bounds the wait, the supervisor's lifecycle timeout
            // bounds each bootstrap inside it, and the dispatch seam's
            // progress heartbeat narrates it — so the session channel holds
            // the settled outcome and reading it does not block.
            let outcome = await_bootstrap_outcome(lightclient.subscribe_mixnet_status()).await;
            let readiness = match outcome {
                BootstrapOutcome::Ready { exits } => format!(
                    "Mixnet Mode ready; the nym proxy at '{path}' serves send and \
                     price-fetch over the mixnet.{}",
                    render_exit_nodes(&exits)
                ),
                BootstrapOutcome::Failed { report } => {
                    return Err(NetworkCommandError::Bootstrap { report });
                }
            };
            Ok(match went_online {
                Some(server) => format!(
                    "WARNING: this consent act switched the session to ONLINE MODE \
                     (this session only); indexer '{server}'. {readiness}"
                ),
                None => readiness,
            })
        }
        NetworkSubCommand::Off => {
            // Zero-emission teardown: the session drops to the unconsented
            // posture, never to clearnet transmit, and the stored standing
            // consent is untouched (`--forget-online` is the erasure act).
            lightclient.go_offline().await;
            Ok(
                "Network off: the nym proxy is stopped, the Indexer connection is dropped, \
                 and in-flight sync is aborted. Nothing network-visible is emitted until \
                 `network on` re-consents for this session. The stored Connectivity \
                 Consent record is untouched: a standing consent, if recorded, attaches \
                 the next launch again (`--forget-online` erases it)."
                    .to_string(),
            )
        }
        NetworkSubCommand::Probe { target } => {
            // Probing runs only over the mixnet route; the typed refusal
            // below names the transport state and its remedy.
            let probes = lightclient
                .probe_destinations(target, PROBE_LEG_TIMEOUT)
                .await?;
            Ok(probes
                .iter()
                .map(render_mixnet_probe)
                .collect::<Vec<_>>()
                .join("\n"))
        }
        NetworkSubCommand::History => Ok(nym_history_command(lightclient)),
    }
}

/// One transmitted transaction's attestation as JSON: the route it
/// traveled, the endpoint that accepted it, and the round-trip time.
fn render_transmit_report(report: &zingolib::lightclient::send::TransmitReport) -> json::JsonValue {
    use zingolib::lightclient::send::TransmitRoute;
    let rtt_ms = u64::try_from(report.round_trip.as_millis()).unwrap_or(u64::MAX);
    match &report.route {
        TransmitRoute::Clearnet { indexer } => object! {
            "txid" => report.txid.to_string(),
            "over_mixnet" => false,
            "indexer" => indexer.clone(),
            "rtt_ms" => rtt_ms,
        },
        TransmitRoute::Mixnet {
            destination,
            via_socks5,
        } => object! {
            "txid" => report.txid.to_string(),
            "over_mixnet" => true,
            "destination" => destination.clone(),
            "via_socks5" => via_socks5.clone(),
            "rtt_ms" => rtt_ms,
        },
    }
}

fn render_migration_phase(phase: &MigrationPhase) -> String {
    match phase {
        MigrationPhase::Planned => "planned".to_string(),
        MigrationPhase::NoteSplitting { round, .. } => {
            format!("note splitting (round {round})")
        }
        MigrationPhase::PartsScheduled => "parts scheduled".to_string(),
        MigrationPhase::Complete { residual } => {
            format!("complete ({residual} zatoshis residual)")
        }
    }
}

/// Typed failure of the migration command family: the discriminant lives in
/// the type, and prose is produced at exactly one rendering site per command.
#[derive(Debug, thiserror::Error)]
pub enum MigrationCommandError {
    #[error("sync failed")]
    Sync(#[source] zingolib::lightclient::error::LightClientError),
    #[error(transparent)]
    Client(#[from] zingolib::lightclient::error::LightClientError),
}

/// A parsed migration command, its arguments parsed completely at the clap
/// derive grammar before any wallet access.
#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq)]
pub(crate) enum MigrationSubCommand {
    #[command(about = "Compute the plan and print its hash, sending nothing")]
    Plan,
    #[command(about = "Record consent to the plan with that hash and begin")]
    Start {
        #[arg(value_name = "plan_hash_hex", value_parser = parse_plan_hash)]
        plan_hash: [u8; 32],
        #[arg(long, value_name = "parts")]
        per_bucket: Option<u32>,
    },
    #[command(about = "Sync, then drive one splitting step")]
    Continue,
    #[command(about = "Reset parts-per-window and redraw the schedule")]
    Cadence {
        #[arg(value_name = "parts")]
        per_bucket: u32,
    },
    #[command(about = "Sync, then send every part owed right now in one spaced batch")]
    Execute {
        #[arg(value_name = "spacing_seconds", default_value = "30", value_parser = parse_spacing)]
        spacing: std::time::Duration,
    },
    #[command(about = "Sync, then transmit whatever the current window has due")]
    Auto,
    #[command(about = "Report the balance, phase, part counts, and coming windows")]
    Status,
    #[command(about = "List each window's block range, position, and confirmations")]
    Windows,
    #[command(about = "Check the schedule against the chain and apply what is safe")]
    Reconcile,
    #[command(about = "Send overdue parts now, spaced by the given seconds")]
    Catchup {
        #[arg(value_name = "spacing_seconds", default_value = "30", value_parser = parse_spacing)]
        spacing: std::time::Duration,
    },
    #[command(about = "Abandon the migration, keeping its confirmed parts")]
    Cancel,
}

fn parse_plan_hash(hash_hex: &str) -> Result<[u8; 32], String> {
    hex::decode(hash_hex)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| "the plan hash must be 64 hex characters".to_string())
}

fn parse_spacing(seconds: &str) -> Result<std::time::Duration, String> {
    seconds
        .parse::<u64>()
        .map(std::time::Duration::from_secs)
        .map_err(|_| "spacing must be a number of seconds".to_string())
}

/// Renders displayable ids as a JSON array of strings.
fn txids_json<T: ToString>(txids: &[T]) -> json::JsonValue {
    txids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .into()
}

/// Runs the `migrate` command. Its errors cross the dispatch seam as
/// [`CommandError::Migration`].
async fn run_migrate(lightclient: &mut LightClient) -> Result<String, MigrationCommandError> {
    let summary = lightclient
        .migrate_to_ironwood(zip32::AccountId::ZERO)
        .await?;
    Ok(object! {
        "split_txids" => txids_json(&summary.split_txids),
        "part_txids" => txids_json(&summary.part_txids),
        "residual" => summary.residual,
    }
    .pretty(JSON_INDENT))
}

/// Runs one `migration` sub-command.
async fn run_migration(
    sub: MigrationSubCommand,
    lightclient: &mut LightClient,
) -> Result<String, MigrationCommandError> {
    Ok(match sub {
        MigrationSubCommand::Plan => {
            let plan = lightclient
                .plan_ironwood_migration(zip32::AccountId::ZERO)
                .await?;
            object! {
                "split_rounds" => plan.split_rounds.len(),
                "split_transactions" => plan.split_rounds.iter().map(Vec::len).sum::<usize>(),
                "split_fee" => plan.split_fee(),
                "parts" => plan.parts.clone(),
                "residual" => plan.residual,
                "plan_hash" => hex::encode(migration::plan_hash(&plan)),
            }
            .pretty(JSON_INDENT)
        }
        MigrationSubCommand::Start {
            plan_hash,
            per_bucket,
        } => {
            lightclient
                .start_ironwood_migration(
                    zip32::AccountId::ZERO,
                    migration::SigningStrategy::LazyAtBoundary,
                    plan_hash,
                    per_bucket,
                )
                .await?;
            "Migration started.".to_string()
        }
        MigrationSubCommand::Continue => {
            lightclient
                .sync_and_await()
                .await
                .map_err(MigrationCommandError::Sync)?;
            match lightclient.continue_note_splitting().await? {
                SplitStep::RoundTransmitted { round, txids } => object! {
                    "round" => round,
                    "split_txids" => txids_json(&txids),
                }
                .pretty(JSON_INDENT),
                SplitStep::AwaitingConfirmation { pending } if pending.is_empty() => {
                    "Round confirmed; waiting for the anchor to reach its outputs. \
                     Sync and retry."
                        .to_string()
                }
                SplitStep::AwaitingConfirmation { pending } => object! {
                    "awaiting_confirmation" => txids_json(&pending),
                }
                .pretty(JSON_INDENT),
                SplitStep::SplittingComplete => {
                    "Note splitting complete; parts are scheduled.".to_string()
                }
            }
        }
        MigrationSubCommand::Cadence { per_bucket } => {
            lightclient.reschedule_parts(per_bucket).await?;
            format!("Cadence set to {per_bucket} per window; the schedule was re-drawn.")
        }
        MigrationSubCommand::Execute { spacing } => {
            lightclient
                .sync_and_await()
                .await
                .map_err(MigrationCommandError::Sync)?;
            let report = lightclient.execute_due_parts(spacing).await?;
            object! {
                "outcomes" => report
                    .outcomes
                    .iter()
                    .map(|outcome| object! {
                        "part" => outcome.part.0,
                        "denomination" => outcome.denomination,
                        "result" => match &outcome.result {
                            PartSendResult::Sent(txid) => object! { "sent" => txid.to_string() },
                            PartSendResult::Slid => object! { "slid" => true },
                            PartSendResult::NotDue { window_opens_unix_time } => {
                                object! { "not_due_until" => *window_opens_unix_time }
                            }
                            PartSendResult::Failed { error } => {
                                object! { "failed" => error.clone() }
                            }
                        },
                    })
                    .collect::<Vec<_>>(),
                "halted" => report.halted,
            }
            .pretty(JSON_INDENT)
        }
        MigrationSubCommand::Auto => {
            lightclient
                .sync_and_await()
                .await
                .map_err(MigrationCommandError::Sync)?;
            let txids = lightclient.auto_transmit_if_due().await?;
            if txids.is_empty() {
                "No parts due yet.".to_string()
            } else {
                object! { "transmitted" => txids_json(&txids) }.pretty(JSON_INDENT)
            }
        }
        MigrationSubCommand::Status => {
            let status = lightclient.migration_status().await?;
            object! {
                "orchard_confirmed_spendable" => status.orchard_confirmed_spendable,
                "phase" => status.phase.as_ref().map(render_migration_phase),
                "parts_total" => status.parts_total,
                "parts_confirmed" => status.parts_confirmed,
                "value_total" => status.value_total,
                "value_migrated" => status.value_migrated,
                "upcoming_windows" => status
                    .upcoming_windows
                    .iter()
                    .map(|window| object! {
                        "bucket_index" => window.bucket_index,
                        "boundary" => u32::from(window.boundary),
                        "part_ids" => window.part_ids.iter().map(|id| id.0).collect::<Vec<_>>(),
                        "window_opens_unix_time" => window.window_opens_unix_time,
                        "latest_target_unix_time" => window.latest_target_unix_time,
                    })
                    .collect::<Vec<_>>(),
                "due_now" => status.due_now.as_ref().map(|batch| object! {
                    "boundary" => u32::from(batch.boundary),
                    "part_ids" => batch.part_ids.iter().map(|id| id.0).collect::<Vec<_>>(),
                    "denominations" => batch.denominations.clone(),
                }),
            }
            .pretty(JSON_INDENT)
        }
        MigrationSubCommand::Windows => {
            let timeline = lightclient.window_timeline().await?;
            match timeline {
                None => "Wallet has no chain height yet; sync first.".to_string(),
                Some(windows) => object! {
                    "windows" => windows
                        .iter()
                        .map(|window| object! {
                            "bucket_index" => window.bucket_index,
                            "opens" => u32::from(window.boundary),
                            "closes" => u32::from(window.close),
                            "is_current" => window.is_current,
                            "parts_confirmed" => window.parts_confirmed,
                            "parts_total" => window.parts_total,
                            "value_migrated" => window.value_migrated,
                            "value_total" => window.value_total,
                        })
                        .collect::<Vec<_>>(),
                }
                .pretty(JSON_INDENT),
            }
        }
        MigrationSubCommand::Reconcile => {
            let report = lightclient.reconcile_migration().await?;
            object! {
                "assessments" => report
                    .assessments
                    .iter()
                    .map(|assessment| object! {
                        "part" => assessment.id.0,
                        "class" => format!("{:?}", assessment.class),
                    })
                    .collect::<Vec<_>>(),
                "actions" => report
                    .actions
                    .iter()
                    .map(|action| format!("{action:?}"))
                    .collect::<Vec<_>>(),
            }
            .pretty(JSON_INDENT)
        }
        MigrationSubCommand::Catchup { spacing } => {
            let txids = lightclient.catch_up_migration(spacing).await?;
            if txids.is_empty() {
                "No overdue parts.".to_string()
            } else {
                object! { "part_txids" => txids_json(&txids) }.pretty(JSON_INDENT)
            }
        }
        MigrationSubCommand::Cancel => {
            lightclient.cancel_ironwood_migration().await?;
            "Migration canceled.".to_string()
        }
    })
}

/// A parsed `drain` sub-command, at the clap derive grammar.
#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq)]
pub(crate) enum DrainSubCommand {
    #[command(about = "Preview the drain from current wallet state, sending nothing")]
    Plan,
    #[command(about = "Build, sign, and transmit the drain")]
    Now,
}

/// A parsed `split` sub-command, at the clap derive grammar.
#[derive(clap::Subcommand, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SplitSubCommand {
    #[command(about = "Preview the remaining rounds, sending nothing")]
    Plan,
    #[command(about = "Run one splitting round")]
    Now,
}

/// Renders an in-flight execute batch snapshot as the heartbeat's detail
/// line, the same [`zingolib::lightclient::migrate::BatchStatus`] a mobile
/// progress screen polls during `execute_due_parts`.
fn batch_progress_line(status: &zingolib::lightclient::migrate::BatchStatus) -> String {
    use zingolib::lightclient::migrate::BatchPhase;
    match status.phase {
        BatchPhase::Sending => format!(
            "resolved {}/{} (sent {})",
            status.resolved, status.total, status.sent
        ),
        BatchPhase::Spacing => format!(
            "resolved {}/{} (sent {}), waiting out the spacing",
            status.resolved, status.total, status.sent
        ),
    }
}

/// Renders an in-flight drain snapshot as the heartbeat's detail line:
/// "built i/N" while proving and signing, "sent i/N" while transmitting.
fn drain_progress_line(status: &ImmediateMigrationStatus) -> String {
    match status.phase {
        ImmediateMigrationPhase::Building => format!("built {}/{}", status.built, status.total),
        ImmediateMigrationPhase::Transmitting => format!("sent {}/{}", status.sent, status.total),
    }
}

/// Renders an in-flight note-splitting round snapshot as the heartbeat's
/// detail line, mirroring [`drain_progress_line`].
fn split_progress_line(status: &SplitStatus) -> String {
    match status.phase {
        SplitPhase::Building => format!("built {}/{}", status.built, status.total),
        SplitPhase::Transmitting => format!("sent {}/{}", status.sent, status.total),
    }
}

/// Runs `drain plan` or `drain now`.
///
/// `plan` previews from wallet state and sends nothing. `now` transmits, and
/// writes progress lines to stderr while it runs.
///
/// Returns the summary as JSON.
async fn run_drain(
    sub: DrainSubCommand,
    lightclient: &mut LightClient,
) -> Result<String, MigrationCommandError> {
    Ok(match sub {
        DrainSubCommand::Plan => {
            let plan = lightclient
                .plan_immediate_migration(zip32::AccountId::ZERO)
                .await?;
            object! {
                "transactions" => plan.transactions.len(),
                "migrated" => plan.migrated,
                "fee" => plan.fee,
                "residual" => plan.residual,
            }
            .pretty(JSON_INDENT)
        }
        DrainSubCommand::Now => {
            let summary = lightclient
                .quick_immediate_migration(zip32::AccountId::ZERO, true)
                .await?;
            object! {
                "txids" => txids_json(&summary.txids),
                "migrated" => summary.migrated,
                "fee" => summary.fee,
                "residual" => summary.residual,
            }
            .pretty(JSON_INDENT)
        }
    })
}

/// Runs `split plan` or `split now`.
///
/// `plan` previews the remaining rounds and sends nothing. `now` runs one
/// round, writing progress lines to stderr while it runs. It returns the
/// round's txids, or a message explaining why nothing was sent.
async fn run_split(
    sub: SplitSubCommand,
    lightclient: &mut LightClient,
) -> Result<String, MigrationCommandError> {
    Ok(match sub {
        SplitSubCommand::Plan => {
            let plan = lightclient.plan_note_split(zip32::AccountId::ZERO).await?;
            object! {
                "split_rounds" => plan.split_rounds.len(),
                "split_transactions" => plan.split_rounds.iter().map(Vec::len).sum::<usize>(),
                "split_fee" => plan.split_fee(),
                "parts" => plan.parts.clone(),
                "residual" => plan.residual,
            }
            .pretty(JSON_INDENT)
        }
        SplitSubCommand::Now => {
            match lightclient
                .quick_split(zip32::AccountId::ZERO, true)
                .await?
            {
                SplitOutcome::Round { txids } => {
                    object! { "split_txids" => txids_json(&txids) }.pretty(JSON_INDENT)
                }
                SplitOutcome::AwaitingConfirmation => {
                    "A previous round has not confirmed yet; nothing was sent. Sync and retry."
                        .to_string()
                }
                SplitOutcome::Complete => {
                    "Every note is part-ready; splitting is complete.".to_string()
                }
            }
        }
    })
}

/// Every command the CLI dispatches, in alphabetical order: the single
/// source of the dispatchable names, help texts, and typed arguments.
#[derive(clap::Subcommand, Clone, PartialEq)]
#[command(rename_all = "snake_case")]
pub(crate) enum CliCommand {
    #[command(
        about = "List unified addresses in the wallet.",
        long_about = indoc! {r"
            List the wallet's unified addresses.
        "}
    )]
    Addresses,
    #[command(
        about = "Return the wallet ZEC balance for each pool (account 0).",
        long_about = indoc! {r"
            Return the wallet ZEC balance for each pool (account 0).
        "}
    )]
    Balance,
    #[command(
        about = "Returns block height wallet was created",
        long_about = indoc! {r"
            Print the height the wallet was created at.
        "}
    )]
    Birthday,
    #[command(
        about = "Sign the latest proposal without transmitting it.",
        long_about = concat!(
            "Sign the latest proposal without transmitting it, for offline signing. No\n",
            "Indexer needed. The transactions are stored Calculated, for 'transmit' to send\n",
            "later. Needs a proposal from 'send', 'send_all' or 'shield' first.\n",
            "\n",
            "In Offline mode the expiry is retargeted to the last height before the next\n",
            "network upgrade, the longest life a pre-signed Zcash transaction can have.\n",
            "Treat a Calculated transaction as live value in flight until it is transmitted,\n",
            "expires, or another transaction spends its inputs.\n",
            "\n",
            "Example:\n",
            "    send ",
            crate::examples::sapling_address!(),
            " ",
            crate::examples::amount_zatoshis!(),
            " \"",
            crate::examples::memo!(),
            "\"\n",
            "    calculate\n",
            "    transmit\n",
        )
    )]
    Calculate,
    #[command(
        about = "Change indexer server",
        long_about = concat!(
            "Change the indexer server.\n",
            "\n",
            "Example:\n",
            "change_server ",
            crate::examples::server_uri!(),
            "\n",
        )
    )]
    ChangeServer {
        #[arg(value_parser = parse_server_uri)]
        uri: Option<http::Uri>,
    },
    #[command(
        about = "Checks if the given encoded address is derived by the wallet's keys.",
        long_about = indoc! {r"
            Check whether an encoded address derives from the wallet's keys.
        "}
    )]
    CheckAddress { address: String },
    #[command(
        about = "Clear the wallet state, rolling back the wallet to an empty state.",
        long_about = indoc! {r"
            Drop every note, coin and transaction, leaving the wallet to sync from scratch.
        "}
    )]
    Clear,
    #[command(
        about = "Show the wallet's coins (transparent outputs).",
        long_about = indoc! {r"
            Show the wallet's coins (transparent outputs). `all` includes spent ones.
        "}
    )]
    Coins {
        #[arg(value_enum)]
        scope: Option<OutputScope>,
    },
    #[command(
        about = "Build and transmit the latest proposal, then resume sync.",
        long_about = concat!(
            "Build and transmit the latest proposal, then resume sync. Needs a proposal\n",
            "from 'send', 'send_all' or 'shield' first.\n",
            "\n",
            "Example:\n",
            "    send ",
            crate::examples::sapling_address!(),
            " ",
            crate::examples::amount_zatoshis!(),
            " \"",
            crate::examples::memo!(),
            "\"\n",
            "    confirm\n",
        )
    )]
    Confirm,
    #[command(
        about = "Updates and returns current price of ZEC.",
        long_about = indoc! {r"
            Fetch the current ZEC price over the Nym mixnet. USD only.

            The fetch races all three price sources (gemini, kraken,
            coingecko) through the tunnel and reports the first answer,
            naming the winning source and the round-trip time.

            Price travels only over the mixnet (ADR 0011): the fetch runs while
            Mixnet Mode is ready and refuses in every other state, including
            switched off — the clearnet consent covers sends, never price,
            because the price source is a third party outside the Zcash
            ecosystem. A build without the `nym` feature has no price fetch.
        "}
    )]
    CurrentPrice,
    #[command(
        about = "Delete wallet file from disk",
        long_about = indoc! {r"
            Delete the wallet file from disk.
        "}
    )]
    Delete,
    #[command(
        about = "Send all Orchard funds into the Ironwood pool now (immediate ZIP 318 path).",
        long_about = indoc! {r"
            Send every spendable Orchard note into the Ironwood pool now, ZIP 318's
            immediate path.

            Privacy disclosure (ZIP 318): this puts the wallet's real amounts on-chain
            at once, correlated with each other and with this wallet's activity. The
            `migration` command is the private alternative.

            `plan` previews from current wallet state: transaction count, the total
            landing in Ironwood, fees, and the residual dust left behind because moving
            it costs more than it carries. Nothing is signed or sent.
            `now` builds, signs and transmits. Sync first, since like any send this
            does not synchronize. Safe to repeat: a partial failure leaves the unsent
            notes spendable and a second run sends only the remainder.
        "}
    )]
    Drain {
        #[command(subcommand)]
        sub: DrainSubCommand,
    },
    #[command(
        about = "Export unified full viewing key for the wallet.",
        long_about = indoc! {r"
            Export the wallet's unified full viewing key. To back up spend capability,
            use `recovery_info` instead.
        "}
    )]
    ExportUfvk,
    #[command(
        about = "Print the chain height as of the wallet's last request to the server.",
        long_about = indoc! {r"
            Print the chain height as of the wallet's last request to the server.
        "}
    )]
    Height,
    #[command(
        about = "Lists all available commands",
        long_about = indoc! {r"
            List every command, or show one command's help.
        "}
    )]
    Help { command: Option<String> },
    #[command(
        about = "Get the indexer server's info",
        long_about = indoc! {r"
            Print the connected indexer's info.
        "}
    )]
    Info,
    #[command(
        about = "Print the most the wallet can send to a given address.",
        long_about = indoc! {r"
            Print the most the wallet can send to an address: shielded spendable
            balance less the fee. Mid-sync this can trail the confirmed balance.
            `zennies_for_zingo` also budgets 1_000_000 ZAT to the ZingoLabs developer
            address.
        "},
        override_usage = concat!(
            "max_send_value <address>\n",
            "       max_send_value { \"address\": \"<address>\", \"zennies_for_zingo\": <true|false> }",
        )
    )]
    MaxSendValue { args: Vec<String> },
    #[command(
        about = "Show by address memo_bytes transfers for this seed.",
        long_about = indoc! {r"
            Print total memo bytes sent, keyed by address.
        "}
    )]
    MemobytesToAddress,
    #[command(
        about = "List memos for this wallet.",
        long_about = indoc! {r"
            List the wallet's memo-bearing value transfers. An address filters to that
            destination, any other string filters to memos containing it, and no
            argument shows every memo. Received messages are matched on the memo's
            reply-to address.
        "}
    )]
    Messages { filter: Option<String> },
    #[command(
        about = "Migrate all Orchard funds to the Ironwood pool in one interactive run.",
        long_about = indoc! {r"
            Migrate all Orchard funds to the Ironwood pool in one interactive run.

            Runs ZIP 318's two phases back to back: note-splitting rounds of Orchard
            self-sends, each awaited to confirmation, then one migration transaction per
            part, transmitted immediately.

            Privacy disclosure (ZIP 318): parts go out alongside each other and
            alongside synchronization, so the server can correlate them with this
            wallet's activity. The `migration` command spreads them across
            anchor-height buckets instead.
        "}
    )]
    Migrate,
    #[command(
        about = "Drive the scheduled Orchard to Ironwood migration",
        long_about = indoc! {r"
            Drive the scheduled Orchard to Ironwood migration (ZIP 318).

            `plan` computes the plan (rounds, parts, fees, residual dust) from the
            wallet's spendable Orchard notes and prints its hash. Nothing is sent.
            `start` records consent to the plan with that hash and begins. --per-bucket
            caps how many parts share a transmission window: lower is more private, higher
            is faster. Fails if the notes changed since planning.
            `continue` syncs, then drives one splitting step, transmitting the next
            round of self-sends or, once every note is part-ready, binding the parts and
            scheduling them. Repeat, syncing between rounds, until it reports them
            scheduled.
            `cadence` resets parts-per-window and redraws the schedule. Usable until the
            first part is signed, so the choice can wait for splitting to end.
            `execute` syncs, then sends everything owed right now in one batch, the
            current window's due parts plus any missed windows', spaced by the given
            seconds (default 30). Reports each part's outcome. The manual counterpart
            to `auto`.
            `auto` syncs, then transmits whatever the current window has due. Run it
            periodically to drive the migration hands-off.
            `status` reports the Orchard confirmed-spendable balance, the phase, part
            counts and values, and the coming windows.
            `windows` lists each window's block range, whether the chain is inside it,
            and how many parts and how much value confirmed. The current window is
            reported even with no migration running.
            `reconcile` checks the persisted schedule against the chain and applies what
            is safe unattended. Run it after every sync.
            `catchup` sends overdue parts now, spaced by the given seconds (default 30).
            Disclosure (ZIP 318): sending at catch-up time correlates the transmissions
            with this wallet's activity.
            `cancel` abandons the migration. Confirmed parts stand, pending ones are
            dropped and their notes released.
        "}
    )]
    Migration {
        #[command(subcommand)]
        sub: MigrationSubCommand,
    },
    // Without the mixnet capability there is no `network` command at all:
    // Offline Mode is the only mode such a build can be in (ADR 0026), so
    // no command may exist that could change the session's posture.
    #[cfg(feature = "nym")]
    #[command(
        about = "Control the network posture and mixnet transport; `network on` switches an \
                 unconsented offline session to ONLINE MODE.",
        long_about = indoc! {r"
            Control the session's network posture and its mixnet transport
            (the mixnet is Nym; the name is implicit).

            With Mixnet Mode on, send and price-fetch route over the mixnet and
            fail closed while it bootstraps, never falling back to clearnet.

            WARNING: in an unconsented offline session, `network on` is itself
            the consent act: it switches the session to ONLINE MODE, for this
            session only. The session selects an indexer over the same curated
            ranking that `--online` uses at launch, and only then bootstraps
            the mixnet (ADR 0026). A deliberate --offline session does not
            offer this command; relaunch without --offline instead.

            `status` reports off, bootstrapping or ready. `on` starts the
            nym-proxy child, taking the binary from the given path, else
            $ZINGO_NYM_PROXY, else one bundled beside this binary, else PATH.
            `off` disconnects every network capability of the session, keeping
            any stored standing consent; `network on` re-consents (ADR 0032).
            `probe` runs GetLatestBlock over the mixnet route to establish an
            indexer's liveness; it requires the mixnet and touches no
            clearnet endpoint. `history` shows the indexer attempts this
            session recorded; nothing survives the session that made it.
        "}
    )]
    Network {
        #[command(subcommand)]
        sub: Option<NetworkSubCommand>,
    },
    #[command(
        about = "Create a new unified address.",
        long_about = indoc! {r"
            Create a new unified address, with an orchard receiver, a sapling one, or
            both. No transparent receivers: use `new_taddress` for those.
        "}
    )]
    NewAddress {
        #[arg(value_name = "o|z|oz", value_parser = parse_receiver_selection)]
        receivers: ReceiverSelection,
    },
    #[command(
        about = "Create a new transparent address.",
        long_about = indoc! {r"
            Create a new transparent address.
        "}
    )]
    NewTaddress,
    #[command(
        about = "Create a new transparent address (even if the last one did not receive any funds).",
        long_about = indoc! {r"
            Create a new transparent address even if the last one never received funds.

            This bypasses the no-gap rule, which exists because recovery from seed may
            not discover addresses beyond a gap. Funds sent to skipped addresses can go
            missing after a restore unless you rescan or raise the gap limit, so you are
            taking on tracking the unused ones yourself.
        "}
    )]
    NewTaddressAllowGap,
    #[command(
        about = "Show the wallet's notes (shielded outputs).",
        long_about = indoc! {r"
            Show the wallet's notes (shielded outputs). `all` includes spent ones.
        "}
    )]
    Notes {
        #[arg(value_enum)]
        scope: Option<OutputScope>,
    },
    #[command(
        about = "Parse an address",
        long_about = concat!(
            "Parse an address.\n",
            "\n",
            "Example\n",
            "parse_address ",
            crate::examples::transparent_address!(),
            "\n",
        )
    )]
    ParseAddress { address: String },
    #[command(
        about = "Parse a view_key.",
        long_about = concat!(
            "Parse a viewing key.\n",
            "\n",
            "Example\n",
            "parse_viewkey ",
            crate::examples::unified_viewing_key!(),
            "\n",
        )
    )]
    ParseViewkey { viewkey: String },
    #[command(
        about = "Send ZEC, fusing `send` and `confirm`.",
        long_about = concat!(
            "Send ZEC, fusing `send` and `confirm`. The fee comes out of your balance\n",
            "and you never see it before the transaction goes out.\n",
            "\n",
            "Example:\n",
            "    quicksend ",
            crate::examples::sapling_address!(),
            " ",
            crate::examples::amount_zatoshis!(),
            " \"",
            crate::examples::memo!(),
            "\"\n",
        ),
        override_usage = concat!(
            "quicksend <address> <zatoshis> \"<optional memo>\"\n",
            "       quicksend '[{\"address\":\"<address>\", \"amount\":<zatoshis>, \"memo\":\"<optional memo>\"}, ...]'",
        )
    )]
    Quicksend { args: Vec<String> },
    #[command(
        about = "Shield transparent funds, fusing `shield` and `confirm`.",
        long_about = indoc! {r"
            Shield transparent funds into the ironwood pool, fusing `shield` and
            `confirm`. The fee comes out of your balance and you never see it before
            the transaction goes out.
        "}
    )]
    Quickshield,
    #[command(
        alias = "exit",
        about = "Quit the light client, saving state to disk.",
        long_about = indoc! {r"
            Quit the light client, saving state to disk. `exit` is an alias.
        "}
    )]
    Quit,
    #[command(
        about = "Print the wallet's seed phrase, birthday and account count.",
        long_about = indoc! {r"
            Print the wallet's seed phrase, birthday and account count.

            The seed phrase recovers the whole wallet. Save it carefully, share it with
            nobody.
        "}
    )]
    RecoveryInfo,
    #[command(
        about = "Remove a failed transaction from the wallet.",
        long_about = indoc! {r"
            Remove a failed transaction from the wallet. Manual on purpose, so a failed
            send keeps its memos until you decide to drop them.
        "}
    )]
    RemoveTransaction {
        #[arg(value_parser = parse_txid)]
        txid: TxId,
    },
    #[command(
        about = "Clear all chain-derived wallet data and sync again from the birthday.",
        long_about = indoc! {r"
            Clear all chain-derived wallet data and sync again from the birthday.
        "}
    )]
    Rescan,
    #[command(
        about = "Launch the save task. Not meant to be called by hand.",
        long_about = indoc! {r"
            Launch the task that persists the wallet as its state changes. Not meant to
            be called by hand.
        "}
    )]
    Save {
        #[command(subcommand)]
        sub: SaveSubCommand,
    },
    #[command(
        about = "Propose a transfer of ZEC, for 'confirm' to transmit.",
        long_about = concat!(
            "Propose a transfer of ZEC. Shows the fee, then 'confirm' transmits it.\n",
            "\n",
            "Example:\n",
            "    send ",
            crate::examples::sapling_address!(),
            " ",
            crate::examples::amount_zatoshis!(),
            " \"",
            crate::examples::memo!(),
            "\"\n",
            "    confirm\n",
        ),
        override_usage = concat!(
            "send <address> <zatoshis> \"<optional memo>\"\n",
            "       send '[{\"address\":\"<address>\", \"amount\":<zatoshis>, \"memo\":\"<optional memo>\"}, ...]'",
        )
    )]
    Send { args: Vec<String> },
    #[command(
        about = "Propose a transfer of all shielded ZEC to one address, for 'confirm' to transmit.",
        long_about = concat!(
            "Propose a transfer of every shielded ZEC to one address. Shows the fee,\n",
            "then 'confirm' transmits it. `zennies_for_zingo` adds 1_000_000 ZAT to the\n",
            "zingolabs developer address per transaction.\n",
            "\n",
            "Skips transparent funds: shield those first, see `help shield`.\n",
            "\n",
            "Example:\n",
            "    send_all ",
            crate::examples::sapling_address!(),
            " \"",
            crate::examples::send_all_memo!(),
            "\"\n",
            "    confirm\n",
        ),
        override_usage = concat!(
            "send_all <address> \"<optional memo>\"\n",
            "       send_all '{ \"address\": \"<address>\", \"memo\": \"<optional memo>\", \"zennies_for_zingo\": <true|false> }'",
        )
    )]
    SendAll { args: Vec<String> },
    #[command(
        about = "Show by address number of sends for this seed.",
        long_about = indoc! {r"
            Print the number of sends, keyed by address.
        "}
    )]
    SendsToAddress,
    #[command(
        about = "Show ranked indexer servers and response times",
        long_about = "Show ranked indexer servers and their get_info() response times."
    )]
    Servers,
    #[command(
        about = "Show or set wallet settings.",
        long_about = indoc! {r"
            Show or set wallet settings. With no argument, prints them all. To set one,
            name it and give a value.

            performance            low | medium | high | maximum
            min_confirmations      1 or greater
            transparent_gap_limit  0-255
        "}
    )]
    Settings {
        #[command(subcommand)]
        sub: Option<SettingsSubCommand>,
    },
    #[command(
        about = "Propose a shield of transparent funds, for 'confirm' to transmit.",
        long_about = indoc! {r"
            Propose a shield of transparent funds into the ironwood pool. Shows the
            fee, then 'confirm' transmits it.
        "}
    )]
    Shield,
    #[command(
        about = "Display the wallet's spendable balance.",
        long_about = indoc! {r"
            Print the wallet's spendable balance.
        "}
    )]
    SpendableBalance,
    #[command(
        about = "Split Orchard notes into ZIP 318 part sizes, one round per call.",
        long_about = indoc! {r"
            Resize the wallet's Orchard notes into ZIP 318 part sizes, one round of
            Orchard self-sends per call.

            The rounds reveal no value and may run before NU6.3 activation. Each call
            replans from the wallet's confirmed notes and persists no migration state.
            Refused while a scheduled migration is active, since that flow does its own
            splitting.

            `plan` previews the remaining rounds, transactions, fees, resulting
            denominations and residual dust. Nothing is signed or sent, and zero rounds
            means every note is already part-ready.
            `now` runs one round. Sync first, and again between rounds until each
            round's self-sends confirm. It reports when a prior round is still
            confirming and when splitting is done.
        "}
    )]
    Split {
        #[command(subcommand)]
        sub: SplitSubCommand,
    },
    #[command(
        about = "Sync the wallet to the latest state of the blockchain.",
        long_about = indoc! {r"
            Sync the wallet to the chain tip.

            `run` starts or resumes. `pause` halts scanning. `stop` shuts sync down
            early. `status` reports progress. `poll` returns the result once complete,
            and is not meant to be called by hand.
        "}
    )]
    Sync {
        #[command(subcommand)]
        sub: SyncSubCommand,
    },
    #[command(
        about = "List transparent addresses in the wallet.",
        long_about = indoc! {r"
            List the wallet's transparent addresses.
        "}
    )]
    TAddresses,
    #[command(
        about = "List the wallet's transaction summaries by block height.",
        long_about = indoc! {r"
            List the wallet's transaction summaries by block height.
        "}
    )]
    Transactions,
    #[command(
        about = "Transmit calculated transactions to the Indexer.",
        long_about = concat!(
            "Transmit calculated transactions to the Indexer. With no arguments, sends\n",
            "every Calculated transaction in target-height order. Pass txids in the order\n",
            "'calculate' printed them to fix the order yourself, which multi-step proposals\n",
            "such as TEX sends require.\n",
            "\n",
            "Anything you leave untransmitted stays live value in flight until it expires or\n",
            "its inputs are spent.\n",
            "\n",
            "Example:\n",
            "    transmit\n",
        )
    )]
    Transmit {
        #[arg(value_parser = parse_txid)]
        txids: Vec<TxId>,
    },
    #[command(
        about = "Show by address value transfers for this seed.",
        long_about = indoc! {r"
            Print total value sent, keyed by address.
        "}
    )]
    ValueToAddress,
    #[command(
        about = "List all value transfers for this wallet.",
        long_about = indoc! {r"
            List the wallet's value transfers, each one a transaction's notes to a
            single receiver.
        "}
    )]
    ValueTransfers,
    #[command(
        about = "Get version of build code",
        long_about = indoc! {r"
            Print the build's git describe --dirty.
        "}
    )]
    Version,
    #[command(
        about = "Displays the kind of wallet currently loaded",
        long_about = indoc! {r"
            Print the loaded wallet's kind. For a UFVK, lists the supported pools.
            Spend-capable wallets always spend from all three.
            "}
    )]
    WalletKind,
}

impl CliCommand {
    /// The variant's bare identifier, the single source `Debug` and `name`
    /// render, so neither can materialize an argument.
    fn ident(&self) -> &'static str {
        match self {
            CliCommand::Addresses => "Addresses",
            CliCommand::Balance => "Balance",
            CliCommand::Birthday => "Birthday",
            CliCommand::Calculate => "Calculate",
            CliCommand::ChangeServer { .. } => "ChangeServer",
            CliCommand::CheckAddress { .. } => "CheckAddress",
            CliCommand::Clear => "Clear",
            CliCommand::Coins { .. } => "Coins",
            CliCommand::Confirm => "Confirm",
            CliCommand::CurrentPrice => "CurrentPrice",
            CliCommand::Delete => "Delete",
            CliCommand::Drain { .. } => "Drain",
            CliCommand::ExportUfvk => "ExportUfvk",
            CliCommand::Height => "Height",
            CliCommand::Help { .. } => "Help",
            CliCommand::Info => "Info",
            CliCommand::MaxSendValue { .. } => "MaxSendValue",
            CliCommand::MemobytesToAddress => "MemobytesToAddress",
            CliCommand::Messages { .. } => "Messages",
            CliCommand::Migrate => "Migrate",
            CliCommand::Migration { .. } => "Migration",
            #[cfg(feature = "nym")]
            CliCommand::Network { .. } => "Network",
            CliCommand::NewAddress { .. } => "NewAddress",
            CliCommand::NewTaddress => "NewTaddress",
            CliCommand::NewTaddressAllowGap => "NewTaddressAllowGap",
            CliCommand::Notes { .. } => "Notes",
            CliCommand::ParseAddress { .. } => "ParseAddress",
            CliCommand::ParseViewkey { .. } => "ParseViewkey",
            CliCommand::Quicksend { .. } => "Quicksend",
            CliCommand::Quickshield => "Quickshield",
            CliCommand::Quit => "Quit",
            CliCommand::RecoveryInfo => "RecoveryInfo",
            CliCommand::RemoveTransaction { .. } => "RemoveTransaction",
            CliCommand::Rescan => "Rescan",
            CliCommand::Save { .. } => "Save",
            CliCommand::Send { .. } => "Send",
            CliCommand::SendAll { .. } => "SendAll",
            CliCommand::SendsToAddress => "SendsToAddress",
            CliCommand::Servers => "Servers",
            CliCommand::Settings { .. } => "Settings",
            CliCommand::Shield => "Shield",
            CliCommand::SpendableBalance => "SpendableBalance",
            CliCommand::Split { .. } => "Split",
            CliCommand::Sync { .. } => "Sync",
            CliCommand::TAddresses => "TAddresses",
            CliCommand::Transactions => "Transactions",
            CliCommand::Transmit { .. } => "Transmit",
            CliCommand::ValueToAddress => "ValueToAddress",
            CliCommand::ValueTransfers => "ValueTransfers",
            CliCommand::Version => "Version",
            CliCommand::WalletKind => "WalletKind",
        }
    }

    /// The command's minted name, derived from the variant identifier, so
    /// a log line can carry the name and never the arguments.
    pub(crate) fn name(&self) -> String {
        let ident = self.ident();
        let mut name = String::with_capacity(ident.len() + 2);
        for c in ident.chars() {
            if c.is_ascii_uppercase() {
                if !name.is_empty() {
                    name.push('_');
                }
                name.push(c.to_ascii_lowercase());
            } else {
                name.push(c);
            }
        }
        name
    }

    /// True when the command's body touches the wallet, deciding which
    /// section of `help` lists the command.
    pub(crate) fn requires_wallet(&self) -> bool {
        match self {
            // These reach the indexer or the mixnet, never the wallet, so
            // they are wallet-free even though they are not offline.
            CliCommand::ChangeServer { .. }
            | CliCommand::CurrentPrice
            | CliCommand::Help { .. }
            | CliCommand::Info
            | CliCommand::ParseAddress { .. }
            | CliCommand::ParseViewkey { .. }
            | CliCommand::Servers
            | CliCommand::Version => false,
            #[cfg(feature = "nym")]
            CliCommand::Network { .. } => true,
            CliCommand::Addresses
            | CliCommand::Balance
            | CliCommand::Birthday
            | CliCommand::Calculate
            | CliCommand::CheckAddress { .. }
            | CliCommand::Clear
            | CliCommand::Coins { .. }
            | CliCommand::Confirm
            | CliCommand::Delete
            | CliCommand::Drain { .. }
            | CliCommand::ExportUfvk
            | CliCommand::Height
            | CliCommand::MaxSendValue { .. }
            | CliCommand::MemobytesToAddress
            | CliCommand::Messages { .. }
            | CliCommand::Migrate
            | CliCommand::Migration { .. }
            | CliCommand::NewAddress { .. }
            | CliCommand::NewTaddress
            | CliCommand::NewTaddressAllowGap
            | CliCommand::Notes { .. }
            | CliCommand::Quicksend { .. }
            | CliCommand::Quickshield
            | CliCommand::Quit
            | CliCommand::RecoveryInfo
            | CliCommand::RemoveTransaction { .. }
            | CliCommand::Rescan
            | CliCommand::Save { .. }
            | CliCommand::Send { .. }
            | CliCommand::SendAll { .. }
            | CliCommand::SendsToAddress
            | CliCommand::Settings { .. }
            | CliCommand::Shield
            | CliCommand::SpendableBalance
            | CliCommand::Split { .. }
            | CliCommand::Sync { .. }
            | CliCommand::TAddresses
            | CliCommand::Transactions
            | CliCommand::Transmit { .. }
            | CliCommand::ValueToAddress
            | CliCommand::ValueTransfers
            | CliCommand::WalletKind => true,
        }
    }

    /// True when executing the command reaches a transmit seam — a
    /// transaction Transmission, the price fetch, or the mixnet probe — the
    /// class the Online consent covers and the readiness gate holds.
    pub(crate) fn transmits(&self) -> bool {
        match self {
            CliCommand::Confirm
            | CliCommand::CurrentPrice
            | CliCommand::Migrate
            | CliCommand::Quicksend { .. }
            | CliCommand::Quickshield
            | CliCommand::Transmit { .. } => true,
            #[cfg(feature = "nym")]
            CliCommand::Network { sub } => matches!(sub, Some(NetworkSubCommand::Probe { .. })),
            CliCommand::Drain { sub } => matches!(sub, DrainSubCommand::Now),
            CliCommand::Split { sub } => matches!(sub, SplitSubCommand::Now),
            CliCommand::Migration { sub } => matches!(
                sub,
                MigrationSubCommand::Start { .. }
                    | MigrationSubCommand::Continue
                    | MigrationSubCommand::Execute { .. }
                    | MigrationSubCommand::Auto
                    | MigrationSubCommand::Catchup { .. }
            ),
            CliCommand::Addresses
            | CliCommand::Balance
            | CliCommand::Birthday
            | CliCommand::Calculate
            | CliCommand::ChangeServer { .. }
            | CliCommand::CheckAddress { .. }
            | CliCommand::Clear
            | CliCommand::Coins { .. }
            | CliCommand::Delete
            | CliCommand::ExportUfvk
            | CliCommand::Height
            | CliCommand::Help { .. }
            | CliCommand::Info
            | CliCommand::MaxSendValue { .. }
            | CliCommand::MemobytesToAddress
            | CliCommand::Messages { .. }
            | CliCommand::NewAddress { .. }
            | CliCommand::NewTaddress
            | CliCommand::NewTaddressAllowGap
            | CliCommand::Notes { .. }
            | CliCommand::ParseAddress { .. }
            | CliCommand::ParseViewkey { .. }
            | CliCommand::Quit
            | CliCommand::RecoveryInfo
            | CliCommand::RemoveTransaction { .. }
            | CliCommand::Rescan
            | CliCommand::Save { .. }
            | CliCommand::Send { .. }
            | CliCommand::SendAll { .. }
            | CliCommand::SendsToAddress
            | CliCommand::Servers
            | CliCommand::Settings { .. }
            | CliCommand::Shield
            | CliCommand::SpendableBalance
            | CliCommand::Sync { .. }
            | CliCommand::TAddresses
            | CliCommand::Transactions
            | CliCommand::ValueToAddress
            | CliCommand::ValueTransfers
            | CliCommand::Version
            | CliCommand::WalletKind => false,
        }
    }

    /// True when the command cannot do its work offline: it either transmits
    /// or speaks to the sync Indexer. A one-shot `--online <command>` is only
    /// valid for such a command; an offline-capable command after `--online`
    /// is refused early, since the flag would grant a connection the command
    /// never uses.
    pub(crate) fn requires_online(&self) -> bool {
        self.transmits() || self.requires_indexer()
    }

    /// True when the command speaks to the sync Indexer over the session
    /// route, so a missing Indexer refuses it with the typed Offline error.
    pub(crate) fn requires_indexer(&self) -> bool {
        match self {
            CliCommand::ChangeServer { .. } | CliCommand::Info | CliCommand::Rescan => true,
            CliCommand::Sync { sub } => matches!(sub, SyncSubCommand::Run),
            _ => false,
        }
    }

    /// True when `mode` suppresses the command: it leaves `help` and is
    /// refused if typed, the network family surviving only where `network
    /// on` remains the consent act.
    pub(crate) fn suppressed(&self, mode: crate::Communications) -> bool {
        match mode {
            crate::Communications::Online => false,
            crate::Communications::DeliberateOffline => {
                #[cfg(feature = "nym")]
                if matches!(self, CliCommand::Network { .. }) {
                    return true;
                }
                self.transmits() || self.requires_indexer()
            }
            crate::Communications::UnconsentedOffline => {
                #[cfg(feature = "nym")]
                if matches!(self, CliCommand::Network { .. }) {
                    return false;
                }
                self.transmits() || self.requires_indexer()
            }
        }
    }

    /// Runs the send family's deferred string grammar eagerly, so both
    /// parse boundaries refuse a malformed payload before any wallet work.
    pub(crate) fn validate_deferred_grammar(&self) -> Result<(), String> {
        match self {
            CliCommand::Send { args } | CliCommand::Quicksend { args } => {
                utils::parse_send_args(&as_strs(args)).map(|_| ())
            }
            CliCommand::SendAll { args } => utils::parse_send_all_args(&as_strs(args)).map(|_| ()),
            CliCommand::MaxSendValue { args } => {
                utils::parse_max_send_value_args(&as_strs(args)).map(|_| ())
            }
            _ => return Ok(()),
        }
        .map_err(|e| usage(&self.name(), e).to_string())
    }
}

/// Renders only the variant identifier, so memos and key material among
/// the arguments never reach a `Debug` surface.
impl std::fmt::Debug for CliCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.ident())
    }
}

/// The whole command line one dispatch parses: a command name and its
/// arguments, with no binary name in front, so the REPL and the one-shot
/// entry share this grammar.
#[derive(clap::Parser, Debug)]
#[command(
    name = "",
    no_binary_name = true,
    disable_help_subcommand = true,
    about = "Enter one command per line. `help` lists the commands.",
    override_usage = "<COMMAND> [ARGS]"
)]
struct CommandLine {
    #[command(subcommand)]
    command: CliCommand,
}

/// [`CommandLine`]'s clap model, built once and cloned per use, so a REPL
/// line never pays the grammar's construction again.
static COMMAND_MODEL: std::sync::LazyLock<clap::Command> = std::sync::LazyLock::new(|| {
    use clap::CommandFactory as _;

    let mut model = CommandLine::command();
    model.build();
    model
});

/// One sample value per variant, held complete by the tests' set-equality
/// pin, so derived listings cover the whole grammar.
fn every_command() -> Vec<CliCommand> {
    vec![
        CliCommand::Addresses,
        CliCommand::Balance,
        CliCommand::Birthday,
        CliCommand::Calculate,
        CliCommand::ChangeServer { uri: None },
        CliCommand::CheckAddress {
            address: String::new(),
        },
        CliCommand::Clear,
        CliCommand::Coins { scope: None },
        CliCommand::Confirm,
        CliCommand::CurrentPrice,
        CliCommand::Delete,
        CliCommand::Drain {
            sub: DrainSubCommand::Plan,
        },
        CliCommand::ExportUfvk,
        CliCommand::Height,
        CliCommand::Help { command: None },
        CliCommand::Info,
        CliCommand::MaxSendValue { args: Vec::new() },
        CliCommand::MemobytesToAddress,
        CliCommand::Messages { filter: None },
        CliCommand::Migrate,
        CliCommand::Migration {
            sub: MigrationSubCommand::Plan,
        },
        #[cfg(feature = "nym")]
        CliCommand::Network { sub: None },
        CliCommand::NewAddress {
            receivers: ReceiverSelection {
                orchard: true,
                sapling: false,
            },
        },
        CliCommand::NewTaddress,
        CliCommand::NewTaddressAllowGap,
        CliCommand::Notes { scope: None },
        CliCommand::ParseAddress {
            address: String::new(),
        },
        CliCommand::ParseViewkey {
            viewkey: String::new(),
        },
        CliCommand::Quicksend { args: Vec::new() },
        CliCommand::Quickshield,
        CliCommand::Quit,
        CliCommand::RecoveryInfo,
        CliCommand::RemoveTransaction {
            txid: TxId::from_bytes([0; 32]),
        },
        CliCommand::Rescan,
        CliCommand::Save {
            sub: SaveSubCommand::Run,
        },
        CliCommand::Send { args: Vec::new() },
        CliCommand::SendAll { args: Vec::new() },
        CliCommand::SendsToAddress,
        CliCommand::Servers,
        CliCommand::Settings { sub: None },
        CliCommand::Shield,
        CliCommand::SpendableBalance,
        CliCommand::Split {
            sub: SplitSubCommand::Plan,
        },
        CliCommand::Sync {
            sub: SyncSubCommand::Status,
        },
        CliCommand::TAddresses,
        CliCommand::Transactions,
        CliCommand::Transmit { txids: Vec::new() },
        CliCommand::ValueToAddress,
        CliCommand::ValueTransfers,
        CliCommand::Version,
        CliCommand::WalletKind,
    ]
}

/// The wallet-free commands, filtered from [`every_command`] by
/// [`CliCommand::requires_wallet`], so the set has that single statement.
fn wallet_free_commands() -> Vec<CliCommand> {
    every_command()
        .into_iter()
        .filter(|command| !command.requires_wallet())
        .collect()
}

/// Renders the two-section help listing, or one command's long help, from
/// [`CommandLine`]'s clap model, offering only what `mode` leaves
/// unsuppressed so help reflects the live session posture.
pub fn format_help(mode: crate::Communications, command: Option<&str>) -> String {
    let mut model = COMMAND_MODEL.clone();
    let offered: Vec<String> = every_command()
        .into_iter()
        .filter(|sample| !sample.suppressed(mode))
        .map(|sample| sample.name())
        .collect();
    let Some(command) = command else {
        let standalone_names: Vec<String> = wallet_free_commands()
            .iter()
            .map(CliCommand::name)
            .collect();
        let listing = |standalone: bool| {
            model
                .get_subcommands()
                .filter(|sub| offered.iter().any(|name| name == sub.get_name()))
                .filter(|sub| {
                    standalone_names.iter().any(|name| name == sub.get_name()) == standalone
                })
                .map(|sub| {
                    format!(
                        "  {} - {}",
                        sub.get_name(),
                        sub.get_about().map(ToString::to_string).unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut lines = vec!["Standalone commands (no wallet required):".to_string()];
        lines.extend(listing(true));
        lines.push(String::new());
        lines.push("Wallet commands:".to_string());
        lines.extend(listing(false));
        return lines.join("\n");
    };
    match model.find_subcommand_mut(command) {
        Some(sub) if offered.iter().any(|name| name == sub.get_name()) => {
            sub.render_long_help().to_string()
        }
        Some(_) | None => format!("Command {command} not found"),
    }
}

/// Parses one command line into a [`CliCommand`], rendering a refusal as
/// clap prints it, so a malformed REPL line never reaches the command loop.
pub(crate) fn parse_command_tokens(tokens: &[String]) -> Result<CliCommand, String> {
    use clap::FromArgMatches as _;

    COMMAND_MODEL
        .clone()
        .try_get_matches_from(tokens)
        .and_then(|matches| CommandLine::from_arg_matches(&matches))
        .map(|CommandLine { command }| command)
        .map_err(|error| error.to_string())
        .and_then(|command| command.validate_deferred_grammar().map(|()| command))
}

/// Dispatches an already-parsed command against the wallet under the
/// progress heartbeat: every command narrates its latest progress line on
/// the shared cadence while it runs, so no command is silent past one
/// interval, and no body wires its own narration.
pub(crate) async fn dispatch_parsed(
    command: CliCommand,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    #[cfg(feature = "nym")]
    if command.transmits() {
        wait_out_bootstrap(lightclient).await;
    }
    let label = command.name();
    let peek = ProgressPeek::from_client(lightclient);
    with_heartbeat(
        &label,
        PROGRESS_HEARTBEAT_INTERVAL,
        "working",
        move || peek.latest(),
        |line| eprintln!("{line}"),
        run_parsed(command, lightclient),
    )
    .await
}

/// While Mixnet Mode is Bootstrapping, wait for it to leave that state
/// within the transmit readiness budget, reporting a heartbeat at each
/// interval; every other mode returns at once, leaving the route
/// resolver at the transmit seam as the sole refusal authority.
#[cfg(feature = "nym")]
async fn wait_out_bootstrap(lightclient: &LightClient) {
    use zingolib::mixnet::Indicator;
    use zingolib::netutils::time::{TRANSMIT_HEARTBEAT_INTERVAL, TRANSMIT_READINESS_BUDGET};

    let mut status_rx = lightclient.subscribe_mixnet_status();
    let started = tokio::time::Instant::now();
    let deadline = started + TRANSMIT_READINESS_BUDGET;
    while status_rx.borrow_and_update().mode == Indicator::Bootstrapping {
        tokio::select! {
            changed = status_rx.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            _ = tokio::time::sleep(TRANSMIT_HEARTBEAT_INTERVAL) => {
                eprintln!(
                    "the mixnet is bootstrapping ({}s of the {}s readiness budget)",
                    started.elapsed().as_secs(),
                    TRANSMIT_READINESS_BUDGET.as_secs(),
                );
            }
            _ = tokio::time::sleep_until(deadline) => return,
        }
    }
}

/// The exhaustive match every frontend reaches, whether it parsed its
/// command at the REPL, at the process's own argument parse, or from a
/// string.
async fn run_parsed(
    command: CliCommand,
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let name = command.name();
    match command {
        CliCommand::Addresses => addresses(lightclient).await,
        CliCommand::Balance => balance(lightclient).await,
        CliCommand::Birthday => birthday(lightclient).await,
        CliCommand::Calculate => calculate(lightclient).await,
        CliCommand::ChangeServer { uri } => change_server(uri, lightclient).await,
        CliCommand::CheckAddress { address } => check_address(&address, lightclient).await,
        CliCommand::Clear => clear(lightclient).await,
        CliCommand::Coins { scope } => coins(scope, lightclient).await,
        CliCommand::Confirm => confirm(lightclient).await,
        CliCommand::CurrentPrice => current_price(lightclient).await,
        CliCommand::Delete => delete(lightclient).await,
        CliCommand::Drain { sub } => drain(sub, lightclient).await,
        CliCommand::ExportUfvk => export_ufvk(lightclient).await,
        CliCommand::Height => height(lightclient).await,
        CliCommand::Help { command: named } => help(named.as_deref()),
        CliCommand::Info => info(lightclient).await,
        CliCommand::MaxSendValue { args } => max_send_value(&name, &args, lightclient).await,
        CliCommand::MemobytesToAddress => memobytes_to_address(lightclient).await,
        CliCommand::Messages { filter } => messages(filter.as_deref(), lightclient).await,
        CliCommand::Migrate => migrate(lightclient).await,
        CliCommand::Migration { sub } => migration(sub, lightclient).await,
        CliCommand::NewAddress { receivers } => new_address(receivers, lightclient).await,
        CliCommand::NewTaddress => taddress(lightclient, true).await,
        CliCommand::NewTaddressAllowGap => taddress(lightclient, false).await,
        CliCommand::Notes { scope } => notes(scope, lightclient).await,
        #[cfg(feature = "nym")]
        CliCommand::Network { sub } => network(sub, lightclient).await,
        CliCommand::ParseAddress { address } => parse_address(&address),
        CliCommand::ParseViewkey { viewkey } => parse_viewkey(&viewkey),
        CliCommand::Quicksend { args } => quicksend(&name, &args, lightclient).await,
        CliCommand::Quickshield => quickshield(lightclient).await,
        CliCommand::Quit => quit(lightclient).await,
        CliCommand::RecoveryInfo => recovery_info(lightclient).await,
        CliCommand::RemoveTransaction { txid } => remove_transaction(txid, lightclient).await,
        CliCommand::Rescan => rescan(lightclient).await,
        CliCommand::Save { sub } => save(sub, lightclient).await,
        CliCommand::Send { args } => send(&name, &args, lightclient).await,
        CliCommand::SendAll { args } => send_all(&name, &args, lightclient).await,
        CliCommand::SendsToAddress => sends_to_address(lightclient).await,
        CliCommand::Servers => Err(CommandError::ReplOnly(name)),
        CliCommand::Settings { sub } => settings(sub, lightclient).await,
        CliCommand::Shield => shield(lightclient).await,
        CliCommand::SpendableBalance => spendable_balance(lightclient).await,
        CliCommand::Split { sub } => split(sub, lightclient).await,
        CliCommand::Sync { sub } => sync(sub, lightclient).await,
        CliCommand::TAddresses => t_addresses(lightclient).await,
        CliCommand::Transactions => transactions(lightclient).await,
        CliCommand::Transmit { txids } => transmit(txids, lightclient).await,
        CliCommand::ValueToAddress => value_to_address(lightclient).await,
        CliCommand::ValueTransfers => value_transfers(lightclient).await,
        CliCommand::Version => version(),
        CliCommand::WalletKind => wallet_kind(lightclient).await,
    }
}
