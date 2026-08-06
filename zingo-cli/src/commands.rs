//! Command definitions and dispatch for zingo-cli.
//!
//! Every command is one row of [`COMMANDS`], the static spec table: its
//! name, help texts, wallet requirement, and the async function that runs
//! it. The sync world crosses into async exactly once, at
//! [`do_user_command`] (ADR 0030), and a failure leaves a command only as
//! a [`CommandError`], never as error prose in the result channel
//! (ADR 0031).

mod error;
#[cfg(test)]
mod tests;
mod utils;

use std::convert::TryInto;
use std::num::NonZeroU32;
use std::pin::Pin;
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
use zcash_protocol::consensus::NetworkType;
use zcash_protocol::value::Zatoshis;

use pepper_sync::wallet::{IronwoodNote, KeyIdInterface, OrchardNote, SaplingNote, SyncMode};
use zingo_common_components::protocol::ActivationHeights;
use zingolib::data::{PollReport, proposal};
use zingolib::lightclient::migrate::{
    ImmediateMigrationPhase, ImmediateMigrationStatus, PartSendResult, SplitOutcome, SplitPhase,
    SplitStatus, SplitStep,
};
use zingolib::lightclient::{LightClient, TransmitProgressHandle};
use zingolib::utils::conversion::txid_from_hex_encoded_str;
use zingolib::wallet::keys::WalletAddressRef;
use zingolib::wallet::keys::unified::{ReceiverSelection, UnifiedKeyStore};
use zingolib::wallet::migration::{self, MigrationPhase};

pub static RT: LazyLock<Runtime> = LazyLock::new(|| tokio::runtime::Runtime::new().unwrap());

use zingolib::netutils::time::TRANSMIT_HEARTBEAT_INTERVAL;

async fn with_transmit_heartbeat<T>(
    label: &str,
    latest: impl Fn() -> Option<String>,
    mut emit: impl FnMut(String),
    operation: impl Future<Output = T>,
) -> T {
    let started = tokio::time::Instant::now();
    let mut ticker = tokio::time::interval_at(
        started + TRANSMIT_HEARTBEAT_INTERVAL,
        TRANSMIT_HEARTBEAT_INTERVAL,
    );
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut operation = std::pin::pin!(operation);
    loop {
        tokio::select! {
            output = &mut operation => return output,
            _ = ticker.tick() => {
                let detail = latest().unwrap_or_else(|| "transmitting".to_string());
                emit(format!(
                    "{label}: {detail} ({}s elapsed)",
                    started.elapsed().as_secs()
                ));
            }
        }
    }
}

/// Runs `operation` under the transmit heartbeat with the stderr sink: the
/// one place a command names Narration's channel.
async fn narrated<T>(
    label: &str,
    latest: impl Fn() -> Option<String>,
    operation: impl Future<Output = T>,
) -> T {
    with_transmit_heartbeat(label, latest, |line| eprintln!("{line}"), operation).await
}

/// [`narrated`] over the transmit progress handle, for the send-family
/// commands. Taking the handle by value lets a call site clone it from the
/// client in argument position, before the operation's `&mut` borrow begins.
async fn transmit_narrated<T>(
    label: &str,
    progress: TransmitProgressHandle,
    operation: impl Future<Output = T>,
) -> T {
    narrated(label, move || progress.latest(), operation).await
}

/// Typed failure of a CLI command. The dispatch seam renders these to
/// prose exactly once, on stderr, for string frontends (ADR 0031). Typed
/// frontends consume them directly via [`do_user_command_result`].
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Migration(#[from] MigrationCommandError),
    #[error(transparent)]
    Nym(#[from] NymCommandError),
    #[error("Unknown command : {0}. Type 'help' for a list of commands")]
    UnknownCommand(String),
    #[error("the `{0}` command runs only at the interactive prompt")]
    ReplOnly(&'static str),
    /// Transitional quarantine for commands whose failure prose is not
    /// yet typed: the message is stored WITHOUT the "Error: " prefix
    /// (the renderer adds it). Every construction site is a candidate
    /// for a dedicated variant, and none may ever be string-matched.
    #[error("{0}")]
    NotYetTyped(String),
}

/// A usage failure carrying the standard "Try 'help <command>'" pointer,
/// with the command name drawn from the caller instead of re-typed prose.
fn usage(command: &str, detail: impl std::fmt::Display) -> CommandError {
    CommandError::NotYetTyped(format!(
        "{detail}\nTry 'help {command}' for correct usage and examples."
    ))
}

/// The future a wallet command's body returns: boxed so every row of the
/// spec table shares one type, borrowing the arguments and the client.
type CommandFuture<'a> = Pin<Box<dyn Future<Output = Result<String, CommandError>> + 'a>>;

type WalletRunFn = for<'a> fn(&'a [&'a str], &'a mut LightClient) -> CommandFuture<'a>;

/// What runs a command, and therefore what it may touch and where `help`
/// lists it. A `Standalone` body never receives the wallet, so its
/// wallet-freedom is enforced by construction; `Repl` marks a command the
/// interactive REPL dispatches itself, holding state this seam does not.
enum CommandBody {
    /// Wallet-free and synchronous; listed under "Standalone commands".
    Standalone(fn(&[&str]) -> Result<String, CommandError>),
    /// Needs the loaded wallet; listed under "Wallet commands".
    Wallet(WalletRunFn),
    /// Owned by the interactive REPL; listed under "Standalone commands".
    Repl,
}

/// One row of the command table: the single source of a command's name,
/// help texts, and body. The name is minted once, by the `wallet!` and
/// `standalone!` macros, from the body function's identifier.
pub struct CommandSpec {
    pub name: &'static str,
    help: &'static str,
    short_help: &'static str,
    body: CommandBody,
}

macro_rules! wallet {
    (
        $body:ident,
        short_help: $short_help:expr,
        help: $help:expr,
    ) => {
        CommandSpec {
            name: stringify!($body),
            help: $help,
            short_help: $short_help,
            body: CommandBody::Wallet({
                fn wrap<'a>(
                    args: &'a [&'a str],
                    lightclient: &'a mut LightClient,
                ) -> CommandFuture<'a> {
                    Box::pin($body(args, lightclient))
                }
                wrap
            }),
        }
    };
}

macro_rules! standalone {
    (
        $body:ident,
        short_help: $short_help:expr,
        help: $help:expr,
    ) => {
        CommandSpec {
            name: stringify!($body),
            help: $help,
            short_help: $short_help,
            body: CommandBody::Standalone($body),
        }
    };
}

/// The help text of the named command, for bodies that echo their own
/// usage. Empty for a name not in the table.
fn help_for(name: &str) -> &'static str {
    COMMANDS
        .iter()
        .find(|spec| spec.name == name)
        .map(|spec| spec.help)
        .unwrap_or_default()
}

async fn addresses(_args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    Ok(lightclient.unified_addresses_json().await.pretty(2))
}

async fn balance(_args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.account_balance(zip32::AccountId::ZERO).await {
        Ok(bal) => Ok(bal.to_string()),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn birthday(_args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    Ok(lightclient.birthday().to_string())
}

async fn calculate(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    if !args.is_empty() {
        return Err(usage("calculate", error::CommandError::InvalidArguments));
    }
    match lightclient.calculate_stored_proposal().await {
        Ok(txids) => Ok(object! {
            "txids" => txids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>(),
        }
        .pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn change_server(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    async fn set(lightclient: &mut LightClient, uri: http::Uri) -> Result<String, CommandError> {
        match lightclient.set_indexer_uri(uri).await {
            Ok(()) => Ok("server set".to_string()),
            Err(e) => Err(CommandError::NotYetTyped(format!(
                "failed to set server: {e}"
            ))),
        }
    }
    match args.len() {
        0 => set(lightclient, http::Uri::default()).await,
        1 => match http::Uri::from_str(args[0]) {
            Ok(uri) => set(lightclient, uri).await,
            Err(_) => match args[0] {
                "" => set(lightclient, http::Uri::default()).await,
                _ => Err(CommandError::NotYetTyped("invalid server uri".to_string())),
            },
        },
        _ => Err(CommandError::NotYetTyped(
            help_for("change_server").to_string(),
        )),
    }
}

async fn check_address(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    if args.len() != 1 {
        return Err(CommandError::NotYetTyped(
            "no address specified. try 'help check_address' for correct usage and examples."
                .to_string(),
        ));
    }
    match lightclient
        .wallet()
        .read()
        .await
        .is_address_derived_by_keys(args[0])
    {
        Ok(address_ref) => Ok(address_ref
            .map_or(
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
            .pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn clear(_args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    lightclient.wallet().write().await.clear_all();
    Ok(object! { "result" => "success" }.pretty(2))
}

async fn confirm(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    if !args.is_empty() {
        return Err(usage("confirm", error::CommandError::InvalidArguments));
    }
    match transmit_narrated(
        "confirm",
        lightclient.transmit_progress_handle(),
        lightclient.send_stored_proposal(true),
    )
    .await
    {
        Ok(txids) => Ok(object! {
            "txids" => txids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>(),
        }
        .pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

#[cfg(feature = "nym")]
async fn current_price(
    _args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    match lightclient.update_current_price().await {
        Ok(fetch) => Ok(format!(
            "current price: {} USD (source: {}, rtt: {} ms, fetched over the mixnet via {})",
            fetch.usd,
            fetch.source.name(),
            fetch.round_trip.as_millis(),
            fetch.via_socks5
        )),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

#[cfg(not(feature = "nym"))]
async fn current_price(
    _args: &[&str],
    _lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    Ok(
        "This build has no price fetch: price travels only over the Nym mixnet (ADR 0011). \
         Rebuild zingo-cli with `--features nym`."
            .to_string(),
    )
}

async fn delete(_args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.delete_wallet_file().await {
        Ok(()) => Ok(object! {
            "result" => "success",
            "wallet_path" => lightclient.wallet_path().to_str().expect("should be valid UTF-8"),
        }
        .pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn export_ufvk(
    _args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
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
        Err(e) => return Err(CommandError::NotYetTyped(e.to_string())),
    };
    Ok(object! {
        "ufvk" => ufvk.encode(&lightclient.chain_type()),
        "birthday" => lightclient.birthday()
    }
    .pretty(2))
}

async fn height(_args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
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
    .pretty(2))
}

fn help(args: &[&str]) -> Result<String, CommandError> {
    Ok(format_help(args))
}

async fn info(_args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.info().await {
        Ok(info) => Ok(json::JsonValue::from(info).pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn max_send_value(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let (address, zennies_for_zingo) =
        utils::parse_max_send_value_args(args).map_err(|e| usage("max_send_value", e))?;
    match lightclient
        .max_send_value(address, zennies_for_zingo, zip32::AccountId::ZERO)
        .await
    {
        Ok(bal) => Ok(object! { "max_send_value" => bal.into_u64() }.pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn memobytes_to_address(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    if args.len() > 1 {
        return Err(CommandError::NotYetTyped(format!(
            "didn't understand arguments\n{}",
            help_for("memobytes_to_address")
        )));
    }
    match lightclient.do_total_memobytes_to_address().await {
        Ok(total_memo_bytes) => Ok(json::JsonValue::from(total_memo_bytes).pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn messages(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    if args.len() > 1 {
        return Err(CommandError::NotYetTyped(
            "invalid arguments\nTry 'help messages' for correct usage and examples".to_string(),
        ));
    }
    match lightclient.messages_containing(args.first().copied()).await {
        Ok(value_transfers) => Ok(json::JsonValue::from(value_transfers).pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn migrate(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    Ok(run_migrate(args, lightclient).await?)
}

async fn migration(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    use clap::Parser as _;
    let MigrationCli { sub } = MigrationCli::try_parse_from(args.iter().copied())
        .map_err(|error| CommandError::NotYetTyped(error.to_string()))?;
    Ok(run_migration(sub, lightclient).await?)
}

async fn new_address(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    if args.len() != 1 || (!args[0].contains('o') && !args[0].contains('z')) {
        return Err(CommandError::NotYetTyped(format!(
            "No address type specified\n{}",
            help_for("new_address")
        )));
    }
    let chain_type = lightclient.chain_type();
    let mut wallet = lightclient.wallet().write().await;
    let receivers = ReceiverSelection {
        orchard: args[0].contains('o'),
        sapling: args[0].contains('z'),
    };
    match wallet.generate_unified_address(receivers, zip32::AccountId::ZERO) {
        Ok((id, unified_address)) => Ok(json::object! {
            "account" => u32::from(zip32::AccountId::ZERO), // used concrete type instead of u32 to simplify upgrading CLI to multi-account
            "address_index" => id.address_index,
            "has_orchard" => unified_address.has_orchard(),
            "has_sapling" => unified_address.has_sapling(),
            "has_transparent" => unified_address.has_transparent(),
            "encoded_address" => unified_address.encode(&chain_type),
        }
        .pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn new_taddress(
    _args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let chain_type = lightclient.chain_type();
    let mut wallet = lightclient.wallet().write().await;
    match wallet.generate_transparent_address(zip32::AccountId::ZERO, true) {
        Ok((id, transparent_address)) => Ok(json::object! {
            "account" => u32::from(id.account_id()),
            "address_index" => id.address_index().index(),
            "scope" => id.scope().to_string(),
            "encoded_address" => transparent::encode_address(&chain_type, transparent_address),
        }
        .pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn new_taddress_allow_gap(
    _args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let chain_type = lightclient.chain_type();
    let mut wallet = lightclient.wallet().write().await;
    match wallet.generate_transparent_address(zip32::AccountId::ZERO, false) {
        Ok((id, transparent_address)) => Ok(json::object! {
            "account" => u32::from(id.account_id()),
            "address_index" => id.address_index().index(),
            "scope" => id.scope().to_string(),
            "encoded_address" => transparent::encode_address(&chain_type, transparent_address),
        }
        .pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn notes(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    let all_notes = match args {
        [] => false,
        ["all"] => true,
        [a] => {
            return Err(CommandError::NotYetTyped(format!(
                "Invalid argument \"{a}\". Specify 'all' to include spent notes"
            )));
        }
        _ => {
            return Err(CommandError::NotYetTyped(help_for("notes").to_string()));
        }
    };
    let wallet = lightclient.wallet().read().await;
    Ok(json::object! {
        "ironwood_notes" => json::JsonValue::from(wallet.note_summaries::<IronwoodNote>(all_notes)),
        "orchard_notes" => json::JsonValue::from(wallet.note_summaries::<OrchardNote>(all_notes)),
        "sapling_notes" => json::JsonValue::from(wallet.note_summaries::<SaplingNote>(all_notes)),
    }
    .pretty(2))
}

async fn coins(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    let all_coins = match args {
        [] => false,
        ["all"] => true,
        [a] => {
            return Err(CommandError::NotYetTyped(format!(
                "Invalid argument \"{a}\". Specify 'all' to include spent coins"
            )));
        }
        _ => {
            return Err(CommandError::NotYetTyped(help_for("coins").to_string()));
        }
    };
    Ok(json::object! {
        "transparent_coins" => json::JsonValue::from(
            lightclient.wallet().read().await.coin_summaries(all_coins)
        ),
    }
    .pretty(2))
}

async fn quicksend(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    let receivers = utils::parse_send_args(args).map_err(|e| usage("quicksend", e))?;
    let request = zingolib::data::receivers::transaction_request_from_receivers(receivers)
        .map_err(|e| usage("quicksend", e))?;
    match transmit_narrated(
        "quicksend",
        lightclient.transmit_progress_handle(),
        lightclient.quick_send_reported(request, zip32::AccountId::ZERO, true),
    )
    .await
    {
        Ok(reports) => Ok(object! {
            "txids" => reports.iter().map(|report| report.txid.to_string()).collect::<Vec<_>>(),
            "transmissions" => reports.iter().map(render_transmit_report).collect::<Vec<_>>(),
        }
        .pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn quickshield(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    if !args.is_empty() {
        return Err(usage("shield", error::CommandError::InvalidArguments));
    }
    match transmit_narrated(
        "quickshield",
        lightclient.transmit_progress_handle(),
        lightclient.quick_shield(zip32::AccountId::ZERO),
    )
    .await
    {
        Ok(txids) => Ok(object! {
            "txids" => txids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>(),
        }
        .pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn quit(_args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    match lightclient.shutdown_save_task().await {
        Ok(()) => eprintln!("Save task shutdown successfully."),
        Err(e) => eprintln!("Error: save failed. {e}"),
    }
    Ok("Zingo CLI quit successfully.".to_string())
}

async fn recovery_info(
    _args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    match lightclient.wallet().read().await.recovery_info() {
        Some(backup_info) => Ok(backup_info.to_string()),
        None => Err(CommandError::NotYetTyped(
            "no mnemonic found. wallet loaded from key.".to_string(),
        )),
    }
}

async fn remove_transaction(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    if args.len() != 1 {
        return Err(CommandError::NotYetTyped(
            "remove command expects 1 argument. Type \"help remove\" for usage.".to_string(),
        ));
    }
    let txid =
        txid_from_hex_encoded_str(args[0]).map_err(|e| CommandError::NotYetTyped(e.to_string()))?;
    match lightclient
        .wallet()
        .write()
        .await
        .remove_failed_transaction(txid)
    {
        Ok(()) => Ok("Successfully removed failed transaction.".to_string()),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn rescan(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    if !args.is_empty() {
        return Err(CommandError::NotYetTyped(
            "rescan command expects no arguments. Type \"rescan help\" for usage.".to_string(),
        ));
    }
    match lightclient.rescan().await {
        Ok(()) => Ok("Launching rescan...".to_string()),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

#[derive(clap::Parser, Debug, PartialEq, Eq)]
#[command(name = "save", no_binary_name = true)]
struct SaveCli {
    #[command(subcommand)]
    sub: SaveSubCommand,
}

/// A parsed `save` command. Arguments parse completely into this enum,
/// at the clap derive grammar, before any wallet access.
#[derive(clap::Subcommand, Debug, PartialEq, Eq)]
enum SaveSubCommand {
    Run,
    Check,
    Shutdown,
}

async fn save(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    use clap::Parser as _;
    let SaveCli { sub } = SaveCli::try_parse_from(args.iter().copied())
        .map_err(|error| CommandError::NotYetTyped(error.to_string()))?;
    match sub {
        SaveSubCommand::Run => {
            lightclient.save_task().await;
            Ok("Launching save task...".to_string())
        }
        SaveSubCommand::Check => match lightclient.check_save_error().await {
            Ok(()) => Ok(String::new()),
            Err(e) => Err(CommandError::NotYetTyped(format!(
                "save failed. {e}\nRestarting save task..."
            ))),
        },
        SaveSubCommand::Shutdown => match lightclient.shutdown_save_task().await {
            Ok(()) => Ok("Save task shutdown successfully.".to_string()),
            Err(e) => Err(CommandError::NotYetTyped(format!("save failed. {e}"))),
        },
    }
}

async fn send(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    let receivers = utils::parse_send_args(args).map_err(|e| usage("send", e))?;
    let request = zingolib::data::receivers::transaction_request_from_receivers(receivers)
        .map_err(|e| usage("send", e))?;
    match lightclient
        .propose_send(request, zip32::AccountId::ZERO)
        .await
    {
        Ok(proposal) => match zingolib::data::proposal::total_fee(&proposal) {
            Ok(fee) => Ok(object! { "fee" => fee.into_u64() }.pretty(2)),
            Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
        },
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn send_all(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    let (address, zennies_for_zingo, memo) =
        utils::parse_send_all_args(args).map_err(|e| usage("sendall", e))?;
    match lightclient
        .propose_send_all(address, zennies_for_zingo, memo, zip32::AccountId::ZERO)
        .await
    {
        Ok(proposal) => {
            let amount = proposal::total_payment_amount(&proposal)
                .map_err(|e| CommandError::NotYetTyped(e.to_string()))?;
            let fee = proposal::total_fee(&proposal)
                .map_err(|e| CommandError::NotYetTyped(e.to_string()))?;
            Ok(object! {
                "amount" => amount.into_u64(),
                "fee" => fee.into_u64(),
            }
            .pretty(2))
        }
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn sends_to_address(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    if args.len() > 1 {
        return Err(CommandError::NotYetTyped(format!(
            "didn't understand arguments\n{}",
            help_for("sends_to_address")
        )));
    }
    match lightclient.do_total_spends_to_address().await {
        Ok(total_spends) => Ok(json::JsonValue::from(total_spends).pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn settings(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    let mut wallet = lightclient.wallet().write().await;

    if args.is_empty() {
        return Ok(format!(
            r"
performance: {}
min confirmations: {}
            ",
            wallet.wallet_settings.sync_config.performance_level,
            wallet.wallet_settings.min_confirmations,
        ));
    }

    let invalid_arguments = || {
        CommandError::NotYetTyped(
            "invalid arguments\nTry 'help settings' for correct usage and examples".to_string(),
        )
    };
    match args[0] {
        "performance" => {
            wallet.wallet_settings.sync_config.performance_level =
                match args.get(1).copied().ok_or_else(invalid_arguments)? {
                    "low" => PerformanceLevel::Low,
                    "medium" => PerformanceLevel::Medium,
                    "high" => PerformanceLevel::High,
                    "maximum" => PerformanceLevel::Maximum,
                    _ => return Err(invalid_arguments()),
                }
        }
        "min_confirmations" => {
            let min_confirmations = args
                .get(1)
                .and_then(|value| value.parse::<u32>().ok())
                .and_then(|m| NonZeroU32::try_from(m).ok())
                .ok_or_else(invalid_arguments)?;
            wallet.wallet_settings.min_confirmations = min_confirmations;
        }
        _ => return Err(invalid_arguments()),
    }

    wallet.mark_dirty();

    Ok("Successfully updated settings.".to_string())
}

async fn shield(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    if !args.is_empty() {
        return Err(usage("shield", error::CommandError::InvalidArguments));
    }
    match lightclient.propose_shield(zip32::AccountId::ZERO).await {
        Ok(proposal) => {
            if proposal.steps().len() != 1 {
                return Err(CommandError::NotYetTyped(
                    "shielding transactions should not have multiple proposal steps".to_string(),
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
                    "shield amount outside valid range of zatoshis".to_string(),
                ));
            };
            let fee = step.balance().fee_required();
            Ok(object! {
                "value_to_shield" => value_to_shield.into_u64(),
                "fee" => fee.into_u64(),
            }
            .pretty(2))
        }
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn spendable_balance(
    _args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let wallet = lightclient.wallet().read().await;
    let spendable_balance = wallet
        .shielded_spendable_balance(zip32::AccountId::ZERO, false)
        .map_err(|e| CommandError::NotYetTyped(e.to_string()))?;
    Ok(object! {
        "spendable_balance" => spendable_balance.into_u64(),
    }
    .pretty(2))
}

#[derive(clap::Parser, Debug, PartialEq, Eq)]
#[command(name = "sync", no_binary_name = true)]
struct SyncCli {
    #[command(subcommand)]
    sub: SyncSubCommand,
}

/// A parsed `sync` command. Arguments parse completely into this enum,
/// at the clap derive grammar, before any wallet access.
#[derive(clap::Subcommand, Debug, PartialEq, Eq)]
enum SyncSubCommand {
    Run,
    Pause,
    Stop,
    Status,
    Poll,
}

async fn sync(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    use clap::Parser as _;
    let SyncCli { sub } = SyncCli::try_parse_from(args.iter().copied())
        .map_err(|error| CommandError::NotYetTyped(error.to_string()))?;
    match sub {
        SyncSubCommand::Run => {
            if lightclient.sync_mode() == SyncMode::Paused {
                lightclient.resume_sync().expect("sync should be paused");
                Ok("Resuming sync task...".to_string())
            } else {
                match lightclient.sync().await {
                    Ok(()) => Ok("Launching sync task...".to_string()),
                    Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
                }
            }
        }
        SyncSubCommand::Pause => match lightclient.pause_sync() {
            Ok(()) => Ok("Pausing sync task...".to_string()),
            Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
        },
        SyncSubCommand::Stop => match lightclient.stop_sync() {
            Ok(()) => Ok("Stopping sync task...".to_string()),
            Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
        },
        SyncSubCommand::Status => {
            match pepper_sync::sync_status(&*lightclient.wallet().read().await).await {
                Ok(status) => Ok(json::JsonValue::from(status).pretty(2)),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            }
        }
        SyncSubCommand::Poll => match lightclient.poll_sync() {
            PollReport::NoHandle => Ok("Sync task has not been launched.".to_string()),
            PollReport::NotReady => Ok("Sync task is not complete.".to_string()),
            PollReport::Ready(result) => match result {
                Ok(sync_result) => Ok(sync_result.to_string()),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            },
        },
    }
}

async fn t_addresses(
    _args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    Ok(lightclient.transparent_addresses_json().await.pretty(2))
}

async fn transactions(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    if !args.is_empty() {
        return Err(CommandError::NotYetTyped(
            "invalid arguments\nTry 'help transactions' for correct usage and examples".to_string(),
        ));
    }
    match lightclient.transaction_summaries(false).await {
        Ok(transactions) => Ok(transactions.to_string()),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn transmit(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    let txids = if args.is_empty() {
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
        args.iter()
            .map(|arg| txid_from_hex_encoded_str(arg))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| usage("transmit", e))?
    };

    let Some(txids) = nonempty::NonEmpty::from_vec(txids) else {
        return Err(CommandError::NotYetTyped(
            "no calculated transactions to transmit".to_string(),
        ));
    };

    match transmit_narrated(
        "transmit",
        lightclient.transmit_progress_handle(),
        lightclient.transmit_calculated(txids),
    )
    .await
    {
        Ok(txids) => Ok(object! {
            "txids" => txids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>(),
        }
        .pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn value_to_address(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    if args.len() > 1 {
        return Err(CommandError::NotYetTyped(format!(
            "didn't understand arguments\n{}",
            help_for("value_to_address")
        )));
    }
    match lightclient.do_total_value_to_address().await {
        Ok(total_values) => Ok(json::JsonValue::from(total_values).pretty(2)),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

async fn value_transfers(
    _args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    match lightclient.value_transfers(false).await {
        Ok(value_transfers) => Ok(value_transfers.to_string()),
        Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
    }
}

fn version(_args: &[&str]) -> Result<String, CommandError> {
    Ok(zingolib::git_description().to_string())
}

async fn wallet_kind(
    _args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
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

fn parse_address(args: &[&str]) -> Result<String, CommandError> {
    if args.len() > 1 || args.is_empty() {
        return Err(CommandError::NotYetTyped(
            help_for("parse_address").to_string(),
        ));
    }
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
        if let Some((recipient_address, chain_name)) = make_decoded_chain_pair(args[0]) {
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

fn parse_viewkey(args: &[&str]) -> Result<String, CommandError> {
    match args.len() {
        1 => Ok(json::stringify_pretty(
            match Ufvk::decode(args[0]) {
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
        )),
        _ => Err(CommandError::NotYetTyped(
            help_for("parse_viewkey").to_string(),
        )),
    }
}

async fn drain(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    use clap::Parser as _;
    let DrainCli { sub } = DrainCli::try_parse_from(args.iter().copied())
        .map_err(|error| CommandError::NotYetTyped(error.to_string()))?;
    Ok(run_drain(sub, lightclient).await?)
}

async fn split(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    use clap::Parser as _;
    let SplitCli { sub } = SplitCli::try_parse_from(args.iter().copied())
        .map_err(|error| CommandError::NotYetTyped(error.to_string()))?;
    Ok(run_split(sub, lightclient).await?)
}

#[cfg(feature = "nym")]
async fn nym(args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
    use clap::Parser as _;
    let NymCli { sub } = NymCli::try_parse_from(args.iter().copied())
        .map_err(|error| CommandError::NotYetTyped(error.to_string()))?;
    Ok(nym_command(sub.unwrap_or(NymSubCommand::Status), lightclient).await?)
}

#[cfg(not(feature = "nym"))]
async fn nym(_args: &[&str], _lightclient: &mut LightClient) -> Result<String, CommandError> {
    Err(CommandError::Nym(NymCommandError::FeatureAbsent))
}

/// This consumer's platform hints for provisioning the `nym-proxy` binary:
/// the explicit flag value and the executable-sibling bundled directory
/// (where the `bundle-nym-proxy` workbench tool places the binary).
/// [`zingolib::nym::provision`] owns the precedence rule and its tests
/// (ADR 0024); this names only what zingolib cannot know by itself. Shared
/// by the session driver call at startup and the `nym on` command.
#[cfg(feature = "nym")]
pub(crate) fn spawn_hints(explicit: Option<&str>) -> zingolib::nym::provision::SpawnHints<'_> {
    use zingolib::nym::provision::{self, SpawnHints};
    SpawnHints {
        explicit,
        bundled_dir: provision::executable_sibling_dir(),
    }
}

/// Resolve the `nym-proxy` binary path from this consumer's
/// [`spawn_hints`], for the `nym on` command's in-session enable.
#[cfg(feature = "nym")]
pub(crate) fn resolve_proxy_path(explicit: Option<&str>) -> String {
    zingolib::nym::provision::resolve_proxy_path(&spawn_hints(explicit))
}

/// Typed failure of the `nym` command family. Each variant exists only in
/// the build that can produce it, so the enum's shape follows the feature.
/// The parse failures that once lived here are clap's now: arguments
/// refuse at the derive grammar, before any wallet access.
#[derive(Debug, thiserror::Error)]
pub enum NymCommandError {
    #[cfg(not(feature = "nym"))]
    #[error("This build has no Nym mixnet support. Rebuild zingo-cli with `--features nym`.")]
    FeatureAbsent,
    #[cfg(feature = "nym")]
    #[error("failed to start the nym proxy at '{path}': {source}")]
    ProxyStart {
        path: String,
        source: zingolib::nym::MixnetProxyError,
    },
}

#[cfg(feature = "nym")]
#[derive(clap::Parser, Debug, PartialEq, Eq)]
#[command(name = "nym", no_binary_name = true)]
struct NymCli {
    /// A bare `nym` reads as `nym status`.
    #[command(subcommand)]
    sub: Option<NymSubCommand>,
}

/// A parsed `nym` command. Arguments parse completely into this enum,
/// at the clap derive grammar, before any wallet access.
#[cfg(feature = "nym")]
#[derive(clap::Subcommand, Debug, PartialEq, Eq)]
enum NymSubCommand {
    Status,
    On {
        path: Option<String>,
    },
    Off,
    Probe {
        #[arg(value_parser = parse_probe_target)]
        target: Option<http::Uri>,
    },
    History,
}

/// https-only: the mixnet leg refuses a plaintext target at dial time,
/// so the grammar rejects it up front with a clear message.
#[cfg(feature = "nym")]
fn parse_probe_target(raw: &str) -> Result<http::Uri, String> {
    let uri = raw
        .parse::<http::Uri>()
        .map_err(|_| "not a valid indexer uri to probe".to_string())?;
    if uri.scheme_str() != Some("https") {
        return Err("indexers must be https".to_string());
    }
    Ok(uri)
}

#[cfg(feature = "nym")]
use zingolib::netutils::time::PROBE_LEG_TIMEOUT;

/// Render one paired probe: the two legs side by side, so a mixnet-specific
/// failure (clearnet ok, mixnet failed) reads at a glance. Pure, pinned by
/// unit tests.
#[cfg(feature = "nym")]
fn render_paired_probe(probe: &zingolib::nym::probe::PairedProbe) -> String {
    let leg = |leg: &zingolib::nym::probe::ProbeLeg| match &leg.outcome {
        Ok(success) => format!(
            "ok in {}ms: chain {}, height {}",
            leg.millis, success.chain, success.height
        ),
        Err(failure) => format!("FAILED after {}ms: {failure}", leg.millis),
    };
    let mixnet = match &probe.mixnet {
        Some(mixnet_leg) => leg(mixnet_leg),
        None => "skipped (mixnet proxy not ready)".to_string(),
    };
    format!(
        "{}\n  clearnet: {}\n  mixnet:   {}",
        probe.host,
        leg(&probe.clearnet),
        mixnet
    )
}

/// Renders the accumulated record for `nym history` when the indexer diary is
/// compiled in, reminding an opted-out session how recording starts.
#[cfg(all(feature = "nym", feature = "nym-diary"))]
fn nym_history_command(lightclient: &LightClient) -> String {
    let handle = lightclient.indexer_history_handle();
    let mut rendered = render_history(
        &handle.load(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0),
    );
    if !handle.is_recording() {
        rendered.push_str("\n(recording is off this session; start with --indexer-diary)");
    }
    rendered
}

/// The `nym history` body when the indexer diary is not compiled in.
#[cfg(all(feature = "nym", not(feature = "nym-diary")))]
fn nym_history_command(_lightclient: &LightClient) -> String {
    "This build has no indexer diary. Rebuild zingo-cli with `--features nym-diary`, then \
     opt a session in with --indexer-diary to record per-indexer history."
        .to_string()
}

/// Render the accumulated per-indexer history as per-host, per-route
/// aggregates, most-attempted hosts first. Pure over the loaded attempts and
/// a caller-supplied "now" so tests pin the ages.
#[cfg(all(feature = "nym", feature = "nym-diary"))]
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
            .entry(attempt.host.clone())
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

/// Render the `nym status` line for a Mixnet Mode, the live bootstrap
/// progress while bootstrapping, and the local SOCKS5 address when ready.
/// Pure, so the user-facing mode strings are pinned by unit tests and
/// reusable by any other frontend.
#[cfg(feature = "nym")]
fn render_status(
    mode: zingolib::nym::MixnetMode,
    socks5_addr: Option<&str>,
    bootstrap_detail: Option<&str>,
) -> String {
    use zingolib::nym::MixnetMode;

    match mode {
        MixnetMode::Unattached => "Mixnet Mode: unattached. The mixnet has not been enabled, \
             and no consent to clearnet has been given: send and price-fetch refuse. Run \
             `nym on` to enable the mixnet, or `nym off` to use clearnet."
            .to_string(),
        MixnetMode::SwitchedOff => {
            "Mixnet Mode: switched off (send and price-fetch use clearnet)".to_string()
        }
        MixnetMode::Bootstrapping => match bootstrap_detail {
            Some(detail) => format!(
                "Mixnet Mode: bootstrapping, {detail} (send and price-fetch are unavailable \
                 until ready)"
            ),
            None => "Mixnet Mode: bootstrapping (send and price-fetch are unavailable until ready)"
                .to_string(),
        },
        MixnetMode::Ready => match socks5_addr {
            Some(addr) => format!("Mixnet Mode: ready (SOCKS5 {addr})"),
            None => "Mixnet Mode: ready".to_string(),
        },
        MixnetMode::Died => "Mixnet Mode: died. The proxy exited unexpectedly. Send and \
             price-fetch refuse and will not fall back to clearnet. Run `nym on` to \
             restart the proxy."
            .to_string(),
    }
}

/// The complete `nym status` output: the Mixnet Mode line followed by the
/// IP-correlation disclaimer. The disclaimer always accompanies the status
/// (ZIP-0318), because Mixnet Mode obfuscates only send and price-fetch while
/// synchronization stays on the ordinary connector, so a bare "ready" must
/// never be read as end-to-end IP protection. The canonical text lives in
/// [`zingolib::nym::IP_CORRELATION_DISCLAIMER`] so every frontend shows the same
/// wording.
#[cfg(feature = "nym")]
fn render_status_with_disclaimer(
    mode: zingolib::nym::MixnetMode,
    socks5_addr: Option<&str>,
    bootstrap_detail: Option<&str>,
) -> String {
    format!(
        "{}\n\n{}",
        render_status(mode, socks5_addr, bootstrap_detail),
        zingolib::nym::IP_CORRELATION_DISCLAIMER,
    )
}

/// The body of the `nym` command when the mixnet transport is compiled in.
#[cfg(feature = "nym")]
async fn nym_command(
    sub: NymSubCommand,
    lightclient: &mut LightClient,
) -> Result<String, NymCommandError> {
    match sub {
        NymSubCommand::Status => Ok(render_status_with_disclaimer(
            lightclient.mixnet_mode(),
            lightclient.mixnet_socks5_addr().as_deref(),
            lightclient.mixnet_bootstrap_detail().as_deref(),
        )),
        NymSubCommand::On { path } => {
            let path = resolve_proxy_path(path.as_deref());
            lightclient
                .enable_mixnet(std::path::Path::new(&path))
                .await
                .map_err(|source| NymCommandError::ProxyStart {
                    path: path.clone(),
                    source,
                })?;
            Ok(format!(
                "Mixnet Mode enabling; the nym proxy at '{path}' is bootstrapping. \
                 Run `nym status` to check readiness."
            ))
        }
        NymSubCommand::Off => {
            lightclient.disable_mixnet().await;
            Ok("Mixnet Mode disabled; send and price-fetch will use clearnet.".to_string())
        }
        NymSubCommand::Probe { target } => {
            let probes = lightclient
                .probe_broadcast_indexers(target, PROBE_LEG_TIMEOUT)
                .await;
            Ok(probes
                .iter()
                .map(render_paired_probe)
                .collect::<Vec<_>>()
                .join("\n"))
        }
        NymSubCommand::History => Ok(nym_history_command(lightclient)),
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
            witness,
            via_socks5,
        } => object! {
            "txid" => report.txid.to_string(),
            "over_mixnet" => true,
            "witness" => witness.clone(),
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

/// Typed failure of the migration command family, following the audit Issue-Q
/// pattern PR #2464 established: the discriminant lives in the type, and
/// prose is produced at exactly one rendering site per command. The parse
/// failures that once lived here are clap's now: arguments refuse at the
/// derive grammar, before any wallet access.
#[derive(Debug, thiserror::Error)]
pub enum MigrationCommandError {
    #[error("migrate command expects no arguments. Type \"help migrate\" for usage.")]
    UnexpectedArguments,
    #[error("sync failed: {0}")]
    Sync(zingolib::lightclient::error::LightClientError),
    #[error("{0}")]
    Client(#[from] zingolib::lightclient::error::LightClientError),
}

#[derive(clap::Parser, Debug, PartialEq, Eq)]
#[command(name = "migration", no_binary_name = true)]
struct MigrationCli {
    #[command(subcommand)]
    sub: MigrationSubCommand,
}

/// A parsed migration command. Arguments parse completely into this
/// enum, at the clap derive grammar, before any wallet access.
#[derive(clap::Subcommand, Debug, PartialEq, Eq)]
enum MigrationSubCommand {
    Plan,
    Start {
        #[arg(value_parser = parse_plan_hash)]
        plan_hash: [u8; 32],
        #[arg(long)]
        per_bucket: Option<u32>,
    },
    Continue,
    Cadence {
        per_bucket: u32,
    },
    Execute {
        #[arg(value_name = "spacing_seconds", default_value = "30", value_parser = parse_spacing)]
        spacing: std::time::Duration,
    },
    Auto,
    Status,
    Windows,
    Reconcile,
    Catchup {
        #[arg(value_name = "spacing_seconds", default_value = "30", value_parser = parse_spacing)]
        spacing: std::time::Duration,
    },
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
async fn run_migrate(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, MigrationCommandError> {
    if !args.is_empty() {
        return Err(MigrationCommandError::UnexpectedArguments);
    }
    let summary = transmit_narrated(
        "migrate",
        lightclient.transmit_progress_handle(),
        lightclient.migrate_to_ironwood(zip32::AccountId::ZERO),
    )
    .await?;
    Ok(object! {
        "split_txids" => txids_json(&summary.split_txids),
        "part_txids" => txids_json(&summary.part_txids),
        "residual" => summary.residual,
    }
    .pretty(2))
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
            .pretty(2)
        }
        MigrationSubCommand::Start {
            plan_hash,
            per_bucket,
        } => {
            transmit_narrated(
                "migration start",
                lightclient.transmit_progress_handle(),
                lightclient.start_ironwood_migration(
                    zip32::AccountId::ZERO,
                    migration::SigningStrategy::LazyAtBoundary,
                    plan_hash,
                    per_bucket,
                ),
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
                SplitStep::RoundBroadcast { round, txids } => object! {
                    "round" => round,
                    "split_txids" => txids_json(&txids),
                }
                .pretty(2),
                SplitStep::AwaitingConfirmation { pending } if pending.is_empty() => {
                    "Round confirmed; waiting for the anchor to reach its outputs. \
                     Sync and retry."
                        .to_string()
                }
                SplitStep::AwaitingConfirmation { pending } => object! {
                    "awaiting_confirmation" => txids_json(&pending),
                }
                .pretty(2),
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
            let progress = lightclient.batch_progress_handle();
            let report = narrated(
                "migration execute",
                move || progress.status().as_ref().map(batch_progress_line),
                lightclient.execute_due_parts(spacing),
            )
            .await?;
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
            .pretty(2)
        }
        MigrationSubCommand::Auto => {
            lightclient
                .sync_and_await()
                .await
                .map_err(MigrationCommandError::Sync)?;
            let txids = lightclient.auto_broadcast_if_due().await?;
            if txids.is_empty() {
                "No parts due yet.".to_string()
            } else {
                object! { "broadcast" => txids_json(&txids) }.pretty(2)
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
            .pretty(2)
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
                .pretty(2),
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
            .pretty(2)
        }
        MigrationSubCommand::Catchup { spacing } => {
            let txids = transmit_narrated(
                "migration catchup",
                lightclient.transmit_progress_handle(),
                lightclient.catch_up_migration(spacing),
            )
            .await?;
            if txids.is_empty() {
                "No overdue parts.".to_string()
            } else {
                object! { "part_txids" => txids_json(&txids) }.pretty(2)
            }
        }
        MigrationSubCommand::Cancel => {
            lightclient.cancel_ironwood_migration().await?;
            "Migration canceled.".to_string()
        }
    })
}

#[derive(clap::Parser, Debug, PartialEq, Eq)]
#[command(name = "drain", no_binary_name = true)]
struct DrainCli {
    #[command(subcommand)]
    sub: DrainSubCommand,
}

/// A parsed `drain` sub-command, at the clap derive grammar.
#[derive(clap::Subcommand, Debug, PartialEq, Eq)]
enum DrainSubCommand {
    Plan,
    Now,
}

#[derive(clap::Parser, Debug, PartialEq, Eq)]
#[command(name = "split", no_binary_name = true)]
struct SplitCli {
    #[command(subcommand)]
    sub: SplitSubCommand,
}

/// A parsed `split` sub-command, at the clap derive grammar.
#[derive(clap::Subcommand, Debug, PartialEq, Eq)]
enum SplitSubCommand {
    Plan,
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
/// "built i/N" while proving and signing, "sent i/N" while broadcasting.
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
/// `plan` previews from wallet state and sends nothing. `now` broadcasts, and
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
            .pretty(2)
        }
        DrainSubCommand::Now => {
            let progress = lightclient.immediate_migration_progress_handle();
            let summary = narrated(
                "drain",
                move || progress.status().as_ref().map(drain_progress_line),
                lightclient.quick_immediate_migration(zip32::AccountId::ZERO, true),
            )
            .await?;
            object! {
                "txids" => txids_json(&summary.txids),
                "migrated" => summary.migrated,
                "fee" => summary.fee,
                "residual" => summary.residual,
            }
            .pretty(2)
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
            .pretty(2)
        }
        SplitSubCommand::Now => {
            let progress = lightclient.split_progress_handle();
            match narrated(
                "split",
                move || progress.status().as_ref().map(split_progress_line),
                lightclient.quick_split(zip32::AccountId::ZERO, true),
            )
            .await?
            {
                SplitOutcome::Round { txids } => {
                    object! { "split_txids" => txids_json(&txids) }.pretty(2)
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

/// Every command, in alphabetical order: the single source of the
/// dispatchable names, help texts, and bodies (ADR 0030). `help` renders
/// its listing straight from this table, so declaration order is display
/// order.
static COMMANDS: &[CommandSpec] = &[
    wallet! {
        addresses,
        short_help: "List unified addresses in the wallet.",
        help: indoc! {r"
            List the wallet's unified addresses.

            Usage:
            addresses
        "},
    },
    wallet! {
        balance,
        short_help: "Return the wallet ZEC balance for each pool (account 0).",
        help: indoc! {r"
            Return the wallet ZEC balance for each pool (account 0).
        "},
    },
    wallet! {
        birthday,
        short_help: "Returns block height wallet was created",
        help: indoc! {r"
            Print the height the wallet was created at.

            Usage:
            birthday
        "},
    },
    wallet! {
        calculate,
        short_help: "Sign the latest proposal without transmitting it.",
        help: concat!(
            "Sign the latest proposal without transmitting it, for offline signing. No\n",
            "Indexer needed. The transactions are stored Calculated, for 'transmit' to send\n",
            "later. Needs a proposal from 'send', 'send_all' or 'shield' first.\n",
            "\n",
            "In Offline mode the expiry is retargeted to the last height before the next\n",
            "network upgrade, the longest life a pre-signed Zcash transaction can have.\n",
            "Treat a Calculated transaction as live value in flight until it is transmitted,\n",
            "expires, or another transaction spends its inputs.\n",
            "\n",
            "Usage:\n",
            "    calculate\n",
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
        ),
    },
    wallet! {
        change_server,
        short_help: "Change indexer server",
        help: concat!(
            "Change the indexer server.\n",
            "\n",
            "Usage:\n",
            "change_server <server_uri>\n",
            "\n",
            "Example:\n",
            "change_server ",
            crate::examples::server_uri!(),
            "\n",
        ),
    },
    wallet! {
        check_address,
        short_help: "Checks if the given encoded address is derived by the wallet's keys.",
        help: indoc! {r"
            Check whether an encoded address derives from the wallet's keys.

            Usage:
            check_address <encoded_address>
        "},
    },
    wallet! {
        clear,
        short_help: "Clear the wallet state, rolling back the wallet to an empty state.",
        help: indoc! {r"
            Drop every note, coin and transaction, leaving the wallet to sync from scratch.

            Usage:
            clear
        "},
    },
    wallet! {
        coins,
        short_help: "Show the wallet's coins (transparent outputs).",
        help: indoc! {r"
            Show the wallet's coins (transparent outputs). `all` includes spent ones.

            Usage:
            coins [all]
        "},
    },
    wallet! {
        confirm,
        short_help: "Build and transmit the latest proposal, then resume sync.",
        help: concat!(
            "Build and transmit the latest proposal, then resume sync. Needs a proposal\n",
            "from 'send', 'send_all' or 'shield' first.\n",
            "\n",
            "Usage:\n",
            "    confirm\n",
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
    },
    wallet! {
        current_price,
        short_help: "Updates and returns current price of ZEC.",
        help: indoc! {r"
            Fetch the current ZEC price over the Nym mixnet. USD only.

            The fetch races all three price sources (gemini, kraken,
            coingecko) through the tunnel and reports the first answer,
            naming the winning source and the round-trip time.

            Price travels only over the mixnet (ADR 0011): the fetch runs while
            Mixnet Mode is ready and refuses in every other state, including
            switched off — the clearnet consent covers sends, never price,
            because the price source is a third party outside the Zcash
            ecosystem. A build without the `nym` feature has no price fetch.

            Usage:
            current_price
        "},
    },
    wallet! {
        delete,
        short_help: "Delete wallet file from disk",
        help: indoc! {r"
            Delete the wallet file from disk.

            Usage:
            delete
        "},
    },
    wallet! {
        drain,
        short_help: "Send all Orchard funds into the Ironwood pool now (immediate ZIP 318 path).",
        help: indoc! {r"
            Send every spendable Orchard note into the Ironwood pool now, ZIP 318's
            immediate path.

            Privacy disclosure (ZIP 318): this puts the wallet's real amounts on-chain
            at once, correlated with each other and with this wallet's activity. The
            `migration` command is the private alternative.

            `plan` previews from current wallet state: transaction count, the total
            landing in Ironwood, fees, and the residual dust left behind because moving
            it costs more than it carries. Nothing is signed or sent.
            `now` builds, signs and broadcasts. Sync first, since like any send this
            does not synchronize. Safe to repeat: a partial failure leaves the unsent
            notes spendable and a second run sends only the remainder.

            Usage:
            drain plan | now
        "},
    },
    wallet! {
        export_ufvk,
        short_help: "Export unified full viewing key for the wallet.",
        help: indoc! {r"
            Export the wallet's unified full viewing key. To back up spend capability,
            use `recovery_info` instead.

            Usage:
            export_ufvk
        "},
    },
    wallet! {
        height,
        short_help: "Print the chain height as of the wallet's last request to the server.",
        help: indoc! {r"
            Print the chain height as of the wallet's last request to the server.

            Usage:
            height
        "},
    },
    standalone! {
        help,
        short_help: "Lists all available commands",
        help: indoc! {r"
            List every command, or show one command's help.

            Usage:
            help [command]
        "},
    },
    wallet! {
        info,
        short_help: "Get the indexer server's info",
        help: indoc! {r"
            Print the connected indexer's info.

            Usage:
            info
        "},
    },
    wallet! {
        max_send_value,
        short_help: "Print the most the wallet can send to a given address.",
        help: indoc! {r#"
            Print the most the wallet can send to an address: shielded spendable
            balance less the fee. Mid-sync this can trail the confirmed balance.
            `zennies_for_zingo` also budgets 1_000_000 ZAT to the ZingoLabs developer
            address.

            Usage:
            max_send_value <address>
            max_send_value { "address": "<address>", "zennies_for_zingo": <true|false> }
        "#},
    },
    wallet! {
        memobytes_to_address,
        short_help: "Show by address memo_bytes transfers for this seed.",
        help: indoc! {r"
            Print total memo bytes sent, keyed by address.

            Usage:
            memobytes_to_address
        "},
    },
    wallet! {
        messages,
        short_help: "List memos for this wallet.",
        help: indoc! {r"
            List the wallet's memo-bearing value transfers. An address filters to that
            correspondent, any other string filters to memos containing it, and no
            argument shows every memo. Received messages are matched on the memo's
            reply-to address.

            Usage:
            messages [address | string]
        "},
    },
    wallet! {
        migrate,
        short_help: "Migrate all Orchard funds to the Ironwood pool in one interactive run.",
        help: indoc! {r"
            Migrate all Orchard funds to the Ironwood pool in one interactive run.

            Runs ZIP 318's two phases back to back: note-splitting rounds of Orchard
            self-sends, each awaited to confirmation, then one migration transaction per
            part, broadcast immediately.

            Privacy disclosure (ZIP 318): parts go out alongside each other and
            alongside synchronization, so the server can correlate them with this
            wallet's activity. The `migration` command spreads them across
            anchor-height buckets instead.

            Usage:
            migrate
        "},
    },
    wallet! {
        migration,
        short_help: "Drive the scheduled Orchard to Ironwood migration",
        help: indoc! {r"
            Drive the scheduled Orchard to Ironwood migration (ZIP 318).

            `plan` computes the plan (rounds, parts, fees, residual dust) from the
            wallet's spendable Orchard notes and prints its hash. Nothing is sent.
            `start` records consent to the plan with that hash and begins. --per-bucket
            caps how many parts share a broadcast window: lower is more private, higher
            is faster. Fails if the notes changed since planning.
            `continue` syncs, then drives one splitting step, broadcasting the next
            round of self-sends or, once every note is part-ready, binding the parts and
            scheduling them. Repeat, syncing between rounds, until it reports them
            scheduled.
            `cadence` resets parts-per-window and redraws the schedule. Usable until the
            first part is signed, so the choice can wait for splitting to end.
            `execute` syncs, then sends everything owed right now in one batch, the
            current window's due parts plus any missed windows', spaced by the given
            seconds (default 30). Reports each part's outcome. The manual counterpart
            to `auto`.
            `auto` syncs, then broadcasts whatever the current window has due. Run it
            periodically to drive the migration hands-off.
            `status` reports the Orchard confirmed-spendable balance, the phase, part
            counts and values, and the coming windows.
            `windows` lists each window's block range, whether the chain is inside it,
            and how many parts and how much value confirmed. The current window is
            reported even with no migration running.
            `reconcile` checks the persisted schedule against the chain and applies what
            is safe unattended. Run it after every sync.
            `catchup` sends overdue parts now, spaced by the given seconds (default 30).
            Disclosure (ZIP 318): sending at catch-up time correlates the broadcasts
            with this wallet's activity.
            `cancel` abandons the migration. Confirmed parts stand, pending ones are
            dropped and their notes released.

            Usage:
            migration plan
            migration start <plan_hash> [--per-bucket N]
            migration cadence <N>
            migration execute [spacing_seconds]
            migration catchup [spacing_seconds]
            migration continue | auto | status | windows | reconcile | cancel
        "},
    },
    wallet! {
        new_address,
        short_help: "Create a new unified address.",
        help: indoc! {r"
            Create a new unified address, with an orchard receiver, a sapling one, or
            both. No transparent receivers: use `new_taddress` for those.

            Usage:
            new_address o | z | oz
        "},
    },
    wallet! {
        new_taddress,
        short_help: "Create a new transparent address.",
        help: indoc! {r"
            Create a new transparent address.

            Usage:
            new_taddress
        "},
    },
    wallet! {
        new_taddress_allow_gap,
        short_help: "Create a new transparent address (even if the last one did not receive any funds).",
        help: indoc! {r#"
            Create a new transparent address even if the last one never received funds.

            This bypasses the no-gap rule, which exists because recovery from seed may
            not discover addresses beyond a gap. Funds sent to skipped addresses can go
            missing after a restore unless you rescan or raise the gap limit, so you are
            taking on tracking the unused ones yourself.

            Usage:
            new_taddress_allow_gap
        "#},
    },
    wallet! {
        notes,
        short_help: "Show the wallet's notes (shielded outputs).",
        help: indoc! {r"
            Show the wallet's notes (shielded outputs). `all` includes spent ones.

            Usage:
            notes [all]
        "},
    },
    wallet! {
        nym,
        short_help: "Control the Nym mixnet transport (on/off/status/probe/history).",
        help: indoc! {r"
            Control the Nym mixnet transport for send and price-fetch.

            With Mixnet Mode on, both route over the mixnet and fail closed while it
            bootstraps, never falling back to clearnet. Turning it off is a deliberate
            choice to transmit over clearnet.

            `status` reports off, bootstrapping or ready. `on` starts the nym-proxy
            child, taking the binary from the given path, else $ZINGO_NYM_PROXY, else
            one bundled beside this binary, else PATH. `off` reverts to clearnet.
            `probe` runs GetLightdInfo over both routes side by side to tell whether a
            failure is mixnet-specific, and its clearnet leg uses your real IP.
            `history` shows per-indexer attempts across sessions, and needs the
            nym-diary feature plus --indexer-diary.

            Usage:
            nym status | on [binary_path] | off | probe [uri] | history
        "},
    },
    standalone! {
        parse_address,
        short_help: "Parse an address",
        help: concat!(
            "Parse an address.\n",
            "\n",
            "Usage:\n",
            "parse_address <address>\n",
            "\n",
            "Example\n",
            "parse_address ",
            crate::examples::transparent_address!(),
            "\n",
        ),
    },
    standalone! {
        parse_viewkey,
        short_help: "Parse a view_key.",
        help: concat!(
            "Parse a viewing key.\n",
            "\n",
            "Usage:\n",
            "parse_viewkey <viewing_key>\n",
            "\n",
            "Example\n",
            "parse_viewkey ",
            crate::examples::unified_viewing_key!(),
            "\n",
        ),
    },
    wallet! {
        quicksend,
        short_help: "Send ZEC, fusing `send` and `confirm`.",
        help: concat!(
            "Send ZEC, fusing `send` and `confirm`. The fee comes out of your balance\n",
            "and you never see it before the transaction goes out.\n",
            "\n",
            "Usage:\n",
            "    quicksend <address> <zatoshis> \"<optional memo>\"\n",
            "    quicksend '[{\"address\":\"<address>\", \"amount\":<zatoshis>, \"memo\":\"<optional memo>\"}, ...]'\n",
            "Example:\n",
            "    quicksend ",
            crate::examples::sapling_address!(),
            " ",
            crate::examples::amount_zatoshis!(),
            " \"",
            crate::examples::memo!(),
            "\"\n",
        ),
    },
    wallet! {
        quickshield,
        short_help: "Shield transparent funds, fusing `shield` and `confirm`.",
        help: indoc! {r"
            Shield transparent funds into the ironwood pool, fusing `shield` and
            `confirm`. The fee comes out of your balance and you never see it before
            the transaction goes out.

            Usage:
                quickshield
        "},
    },
    wallet! {
        quit,
        short_help: "Quit the light client, saving state to disk.",
        help: indoc! {r"
            Quit the light client, saving state to disk.

            Usage:
            quit
        "},
    },
    wallet! {
        recovery_info,
        short_help: "Print the wallet's seed phrase, birthday and account count.",
        help: indoc! {r"
            Print the wallet's seed phrase, birthday and account count.

            The seed phrase recovers the whole wallet. Save it carefully, share it with
            nobody.

            Usage:
            recovery_info
        "},
    },
    wallet! {
        remove_transaction,
        short_help: "Remove a failed transaction from the wallet.",
        help: indoc! {r"
            Remove a failed transaction from the wallet. Manual on purpose, so a failed
            send keeps its memos until you decide to drop them.

            Usage:
            remove_transaction <txid>
        "},
    },
    wallet! {
        rescan,
        short_help: "Clear all chain-derived wallet data and sync again from the birthday.",
        help: indoc! {r"
            Clear all chain-derived wallet data and sync again from the birthday.

            Usage:
            rescan
        "},
    },
    wallet! {
        save,
        short_help: "Launch the save task. Not meant to be called by hand.",
        help: indoc! {r"
            Launch the task that persists the wallet as its state changes. Not meant to
            be called by hand.

            Usage:
            save run | check | shutdown
        "},
    },
    wallet! {
        send,
        short_help: "Propose a transfer of ZEC, for 'confirm' to broadcast.",
        help: concat!(
            "Propose a transfer of ZEC. Shows the fee, then 'confirm' broadcasts it.\n",
            "\n",
            "Usage:\n",
            "    send <address> <zatoshis> \"<optional memo>\"\n",
            "    send '[{\"address\":\"<address>\", \"amount\":<zatoshis>, \"memo\":\"<optional memo>\"}, ...]'\n",
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
    },
    wallet! {
        send_all,
        short_help: "Propose a transfer of all shielded ZEC to one address, for 'confirm' to broadcast.",
        help: concat!(
            "Propose a transfer of every shielded ZEC to one address. Shows the fee,\n",
            "then 'confirm' broadcasts it. `zennies_for_zingo` adds 1_000_000 ZAT to the\n",
            "zingolabs developer address per transaction.\n",
            "\n",
            "Skips transparent funds: shield those first, see `help shield`.\n",
            "\n",
            "Usage:\n",
            "    send_all <address> \"<optional memo>\"\n",
            "    send_all '{ \"address\": \"<address>\", \"memo\": \"<optional memo>\", \"zennies_for_zingo\": <true|false> }'\n",
            "Example:\n",
            "    send_all ",
            crate::examples::sapling_address!(),
            " \"",
            crate::examples::send_all_memo!(),
            "\"\n",
            "    confirm\n",
        ),
    },
    wallet! {
        sends_to_address,
        short_help: "Show by address number of sends for this seed.",
        help: indoc! {r"
            Print the number of sends, keyed by address.

            Usage:
            sends_to_address
        "},
    },
    CommandSpec {
        name: "servers",
        short_help: "Show ranked indexer servers and response times",
        help: "Show ranked indexer servers and their get_info() response times.\nUsage: servers",
        body: CommandBody::Repl,
    },
    wallet! {
        settings,
        short_help: "Show or set wallet settings.",
        help: indoc! {r"
            Show or set wallet settings. With no argument, prints them all. To set one,
            name it and give a value.

            performance        low | medium | high | maximum
            min_confirmations  1 or greater

            Usage:
            settings
            settings performance high
        "},
    },
    wallet! {
        shield,
        short_help: "Propose a shield of transparent funds, for 'confirm' to broadcast.",
        help: indoc! {r"
            Propose a shield of transparent funds into the ironwood pool. Shows the
            fee, then 'confirm' broadcasts it.

            Usage:
                shield
        "},
    },
    wallet! {
        spendable_balance,
        short_help: "Display the wallet's spendable balance.",
        help: indoc! {r"
            Print the wallet's spendable balance.

            Usage:
            spendable_balance
        "},
    },
    wallet! {
        split,
        short_help: "Split Orchard notes into ZIP 318 part sizes, one round per call.",
        help: indoc! {r"
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

            Usage:
            split plan | now
        "},
    },
    wallet! {
        sync,
        short_help: "Sync the wallet to the latest state of the blockchain.",
        help: indoc! {r"
            Sync the wallet to the chain tip.

            `run` starts or resumes. `pause` halts scanning. `stop` shuts sync down
            early. `status` reports progress. `poll` returns the result once complete,
            and is not meant to be called by hand.

            Usage:
            sync run | pause | stop | status | poll
        "},
    },
    wallet! {
        t_addresses,
        short_help: "List transparent addresses in the wallet.",
        help: indoc! {r"
            List the wallet's transparent addresses.

            Usage:
            t_addresses
        "},
    },
    wallet! {
        transactions,
        short_help: "List the wallet's transaction summaries by block height.",
        help: indoc! {r"
            List the wallet's transaction summaries by block height.

            Usage:
            transactions
        "},
    },
    wallet! {
        transmit,
        short_help: "Transmit calculated transactions to the Indexer.",
        help: concat!(
            "Transmit calculated transactions to the Indexer. With no arguments, sends\n",
            "every Calculated transaction in target-height order. Pass txids in the order\n",
            "'calculate' printed them to fix the order yourself, which multi-step proposals\n",
            "such as TEX sends require.\n",
            "\n",
            "Anything you leave untransmitted stays live value in flight until it expires or\n",
            "its inputs are spent.\n",
            "\n",
            "Usage:\n",
            "    transmit [txid ...]\n",
            "Example:\n",
            "    transmit\n",
        ),
    },
    wallet! {
        value_to_address,
        short_help: "Show by address value transfers for this seed.",
        help: indoc! {r"
            Print total value sent, keyed by address.

            Usage:
            value_to_address
        "},
    },
    wallet! {
        value_transfers,
        short_help: "List all value transfers for this wallet.",
        help: indoc! {r"
            List the wallet's value transfers, each one a transaction's notes to a
            single receiver.

            Usage:
            value_transfers
        "},
    },
    standalone! {
        version,
        short_help: "Get version of build code",
        help: indoc! {r"
            Print the build's git describe --dirty.
        "},
    },
    wallet! {
        wallet_kind,
        short_help: "Displays the kind of wallet currently loaded",
        help: indoc! {r"
            Print the loaded wallet's kind. For a UFVK, lists the supported pools.
            Spend-capable wallets always spend from all three.
            "},
    },
];

/// Renders the help listing (no arguments) or one command's help text,
/// straight from [`COMMANDS`].
pub fn format_help(args: &[&str]) -> String {
    match args.len() {
        0 => {
            let mut lines = Vec::new();
            lines.push("Standalone commands (no wallet required):".to_string());
            lines.extend(
                COMMANDS
                    .iter()
                    .filter(|spec| !matches!(spec.body, CommandBody::Wallet(_)))
                    .map(|spec| format!("  {} - {}", spec.name, spec.short_help)),
            );
            lines.push(String::new());
            lines.push("Wallet commands:".to_string());
            lines.extend(
                COMMANDS
                    .iter()
                    .filter(|spec| matches!(spec.body, CommandBody::Wallet(_)))
                    .map(|spec| format!("  {} - {}", spec.name, spec.short_help)),
            );
            lines.join("\n")
        }
        1 => match COMMANDS.iter().find(|spec| spec.name == args[0]) {
            Some(spec) => spec.help.to_string(),
            None => format!("Command {} not found", args[0]),
        },
        _ => "Usage: help [command_name]".to_string(),
    }
}

/// Dispatches a user command by name through [`COMMANDS`], exposing the
/// typed success/failure crossing. Unknown names and the REPL-owned
/// `servers` command return typed errors (ADR 0031).
pub async fn do_user_command_result(
    cmd: &str,
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    let lowered = cmd.to_ascii_lowercase();
    match COMMANDS.iter().find(|spec| spec.name == lowered) {
        Some(spec) => match spec.body {
            CommandBody::Standalone(run) => run(args),
            CommandBody::Wallet(run) => run(args, lightclient).await,
            CommandBody::Repl => Err(CommandError::ReplOnly(spec.name)),
        },
        None => Err(CommandError::UnknownCommand(cmd.to_string())),
    }
}

/// The crate's single sync-to-async dispatch seam (ADR 0030): string
/// frontends call this, and the one `block_on` below is the only place
/// command execution crosses from sync into async.
#[allow(clippy::disallowed_methods)]
pub fn do_user_command(
    cmd: &str,
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    RT.block_on(do_user_command_result(cmd, args, lightclient))
}
