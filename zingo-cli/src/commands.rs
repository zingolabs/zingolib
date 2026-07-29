//! Command definitions and dispatch for zingo-cli.
//!
//! Each command implements the [`Command`] trait (or [`ShortCircuitedCommand`]
//! for commands that run without a wallet). All commands are registered in
//! [`get_commands`] and dispatched by [`do_user_command`].

mod error;
mod utils;

use std::collections::HashMap;
use std::convert::TryInto;
use std::num::NonZeroU32;
use std::str::FromStr;

use indoc::indoc;
use json::object;
use pepper_sync::config::PerformanceLevel;
use pepper_sync::keys::transparent;
use std::sync::LazyLock;
use tokio::runtime::Runtime;

use zcash_address::unified::{Container, Encoding, Ufvk};
use zcash_keys::address::Address;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_protocol::consensus::NetworkType;
use zcash_protocol::value::Zatoshis;

use pepper_sync::wallet::{IronwoodNote, KeyIdInterface, OrchardNote, SaplingNote, SyncMode};
use zingo_common_components::protocol::ActivationHeights;
use zingolib::data::{PollReport, proposal};
use zingolib::lightclient::LightClient;
use zingolib::lightclient::migrate::{
    ImmediateMigrationPhase, ImmediateMigrationStatus, PartSendResult, SplitOutcome, SplitPhase,
    SplitStatus, SplitStep,
};
use zingolib::utils::conversion::txid_from_hex_encoded_str;
use zingolib::wallet::keys::WalletAddressRef;
use zingolib::wallet::keys::unified::{ReceiverSelection, UnifiedKeyStore};
use zingolib::wallet::migration::{self, MigrationPhase};

pub static RT: LazyLock<Runtime> = LazyLock::new(|| tokio::runtime::Runtime::new().unwrap());

use zingolib::netutils::time::TRANSMIT_HEARTBEAT_INTERVAL;

/// Awaits `operation`, emitting a heartbeat every
/// [`TRANSMIT_HEARTBEAT_INTERVAL`]: the latest line from `latest` (the
/// transmit-progress side channel `operation` narrates into) plus the elapsed
/// seconds. `emit` is injected so tests capture the lines. Production prints
/// them to STDERR, never stdout, because command results own stdout and a scripted
/// `zingo-cli ... quicksend | jq` stays parseable however slow the send
/// (PR #2470 review, M5). Grab the progress handle *before* building
/// `operation`, which borrows the client mutably.
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

/// Typed failure of a CLI command. `do_user_command` remains the single
/// site that renders these to prose for string frontends. Typed
/// frontends consume them directly via `do_user_command_result`.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Migration(#[from] MigrationCommandError),
    #[error(transparent)]
    Nym(#[from] NymCommandError),
    /// Transitional quarantine for commands whose failure prose is not
    /// yet typed: the message is stored WITHOUT the "Error: " prefix
    /// (the renderer adds it). Every construction site is a candidate
    /// for a dedicated variant, and none may ever be string-matched.
    #[error("{0}")]
    NotYetTyped(String),
}

/// This command interface is used both by cli and also consumers.
pub trait Command {
    /// display command help (in cli)
    fn help(&self) -> &'static str;

    /// A one-line summary shown in the two-column command listing.
    fn short_help(&self) -> &'static str;

    /// in zingocli, the success string is printed to console
    /// consumers occasionally make assumptions about this
    /// e. expect it to be a json object
    ///
    /// Failure crosses the boundary structurally as a [`CommandError`].
    /// [`do_user_command`] renders it to prose for string frontends.
    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError>;
}

/// A command that can execute without an active [`LightClient`].
///
/// This is used for commands like `help` that must run before the wallet
/// is loaded, for example when the user passes `help` as the COMMAND
/// argument on the command line.
pub trait ShortCircuitedCommand {
    /// Execute the command without a [`LightClient`], returning the
    /// output string that will be printed to the console.
    fn exec_without_lc(args: Vec<String>) -> String;
}

struct GetVersionCommand {}
impl Command for GetVersionCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Print the build's git describe --dirty.
        "}
    }

    fn short_help(&self) -> &'static str {
        "Get version of build code"
    }

    fn exec(&self, _args: &[&str], _lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(zingolib::git_description().to_string())
    }
}

struct ChangeServerCommand {}
impl Command for ChangeServerCommand {
    fn help(&self) -> &'static str {
        concat!(
            "Change the indexer server.\n",
            "\n",
            "Usage:\n",
            "change_server <server_uri>\n",
            "\n",
            "Example:\n",
            "change_server ",
            crate::examples::server_uri!(),
            "\n",
        )
    }

    fn short_help(&self) -> &'static str {
        "Change indexer server"
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(RT.block_on(async move {
            match args.len() {
                0 => match lightclient.set_indexer_uri(http::Uri::default()).await {
                    Ok(()) => "server set".to_string(),
                    Err(e) => format!("failed to set server: {e}"),
                },
                1 => match http::Uri::from_str(args[0]) {
                    Ok(uri) => match lightclient.set_indexer_uri(uri).await {
                        Ok(()) => "server set".to_string(),
                        Err(e) => format!("failed to set server: {e}"),
                    },
                    Err(_) => match args[0] {
                        "" => match lightclient.set_indexer_uri(http::Uri::default()).await {
                            Ok(()) => "server set".to_string(),
                            Err(e) => format!("failed to set server: {e}"),
                        },
                        _ => "invalid server uri".to_string(),
                    },
                },
                _ => self.help().to_string(),
            }
        }))
    }
}

struct BirthdayCommand {}
impl Command for BirthdayCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Print the height the wallet was created at.

            Usage:
            birthday
        "}
    }

    fn short_help(&self) -> &'static str {
        "Returns block height wallet was created"
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(lightclient.birthday().to_string())
    }
}

struct WalletKindCommand {}
impl Command for WalletKindCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Print the loaded wallet's kind. For a UFVK, lists the supported pools.
            Spend-capable wallets always spend from all three.
            "}
    }

    fn short_help(&self) -> &'static str {
        "Displays the kind of wallet currently loaded"
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(RT.block_on(async move {
            if lightclient.mnemonic_phrase().is_some() {
                object! {"kind" => "Loaded from mnemonic (seed or phrase)",
                        "transparent" => true,
                        "sapling" => true,
                        "orchard" => true,
                }
                .pretty(4)
            } else {
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
                }
            }
        }))
    }
}

struct ParseAddressCommand {}
impl Command for ParseAddressCommand {
    fn help(&self) -> &'static str {
        concat!(
            "Parse an address.\n",
            "\n",
            "Usage:\n",
            "parse_address <address>\n",
            "\n",
            "Example\n",
            "parse_address ",
            crate::examples::transparent_address!(),
            "\n",
        )
    }

    fn short_help(&self) -> &'static str {
        "Parse an address"
    }

    fn exec(&self, args: &[&str], _lightclient: &mut LightClient) -> Result<String, CommandError> {
        if args.len() > 1 || args.is_empty() {
            return Ok(self.help().to_string());
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
}

struct ParseViewKeyCommand {}
impl Command for ParseViewKeyCommand {
    fn help(&self) -> &'static str {
        concat!(
            "Parse a viewing key.\n",
            "\n",
            "Usage:\n",
            "parse_viewkey <viewing_key>\n",
            "\n",
            "Example\n",
            "parse_viewkey ",
            crate::examples::unified_viewing_key!(),
            "\n",
        )
    }

    fn short_help(&self) -> &'static str {
        "Parse a view_key."
    }

    fn exec(&self, args: &[&str], _lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(match args.len() {
            1 => json::stringify_pretty(
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
            ),
            _ => self.help().to_string(),
        })
    }
}

struct SyncCommand {}
impl Command for SyncCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Sync the wallet to the chain tip.

            `run` starts or resumes. `pause` halts scanning. `stop` shuts sync down
            early. `status` reports progress. `poll` returns the result once complete,
            and is not meant to be called by hand.

            Usage:
            sync run | pause | stop | status | poll
        "}
    }

    fn short_help(&self) -> &'static str {
        "Sync the wallet to the latest state of the blockchain."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if args.len() != 1 {
            return Err(CommandError::NotYetTyped(
                "sync command expects 1 argument. Type \"help sync\" for usage.".to_string(),
            ));
        }

        match args[0] {
            "run" => {
                if lightclient.sync_mode() == SyncMode::Paused {
                    lightclient.resume_sync().expect("sync should be paused");
                    Ok("Resuming sync task...".to_string())
                } else {
                    RT.block_on(async move {
                        match lightclient.sync().await {
                            Ok(()) => Ok("Launching sync task...".to_string()),
                            Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
                        }
                    })
                }
            }
            "pause" => match lightclient.pause_sync() {
                Ok(()) => Ok("Pausing sync task...".to_string()),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            },
            "stop" => match lightclient.stop_sync() {
                Ok(()) => Ok("Stopping sync task...".to_string()),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            },
            "status" => RT.block_on(async move {
                match pepper_sync::sync_status(&*lightclient.wallet().read().await).await {
                    Ok(status) => Ok(json::JsonValue::from(status).pretty(2)),
                    Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
                }
            }),
            "poll" => match lightclient.poll_sync() {
                PollReport::NoHandle => Ok("Sync task has not been launched.".to_string()),
                PollReport::NotReady => Ok("Sync task is not complete.".to_string()),
                PollReport::Ready(result) => match result {
                    Ok(sync_result) => Ok(sync_result.to_string()),
                    Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
                },
            },
            _ => Err(CommandError::NotYetTyped(
                "invalid sub-command. Type \"help sync\" for usage.".to_string(),
            )),
        }
    }
}

struct RescanCommand {}
impl Command for RescanCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Clear all chain-derived wallet data and sync again from the birthday.

            Usage:
            rescan
        "}
    }

    fn short_help(&self) -> &'static str {
        "Clear all chain-derived wallet data and sync again from the birthday."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if !args.is_empty() {
            return Err(CommandError::NotYetTyped(
                "rescan command expects no arguments. Type \"rescan help\" for usage.".to_string(),
            ));
        }

        RT.block_on(async move {
            match lightclient.rescan().await {
                Ok(()) => Ok("Launching rescan...".to_string()),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            }
        })
    }
}

struct ClearCommand {}
impl Command for ClearCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Drop every note, coin and transaction, leaving the wallet to sync from scratch.

            Usage:
            clear
        "}
    }

    fn short_help(&self) -> &'static str {
        "Clear the wallet state, rolling back the wallet to an empty state."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(RT.block_on(async move {
            lightclient.wallet().write().await.clear_all();

            let result = object! { "result" => "success" };
            result.pretty(2)
        }))
    }
}

/// Lists all available commands or shows detailed help for a specific command.
pub struct HelpCommand {}
impl Command for HelpCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            List every command, or show one command's help.

            Usage:
            help [command]
        "}
    }

    fn short_help(&self) -> &'static str {
        "Lists all available commands"
    }

    fn exec(&self, args: &[&str], _: &mut LightClient) -> Result<String, CommandError> {
        Ok(format_help(args))
    }
}

impl ShortCircuitedCommand for HelpCommand {
    fn exec_without_lc(args: Vec<String>) -> String {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        format_help(&refs)
    }
}

fn format_help(args: &[&str]) -> String {
    match args.len() {
        0 => {
            let mut lines = Vec::new();

            lines.push("Standalone commands (no wallet required):".to_string());
            let standalone = get_standalone_commands();
            let mut standalone_lines: Vec<_> = standalone
                .iter()
                .map(|(cmd, obj)| format!("  {} - {}", cmd, obj.short_help()))
                .collect();
            // Also include `servers` which is handled by the REPL directly.
            standalone_lines
                .push("  servers - Show ranked indexer servers and response times".to_string());
            standalone_lines.sort();
            lines.extend(standalone_lines);

            lines.push(String::new());
            lines.push("Wallet commands:".to_string());
            let wallet = get_wallet_commands();
            let mut wallet_lines: Vec<_> = wallet
                .iter()
                .map(|(cmd, obj)| format!("  {} - {}", cmd, obj.short_help()))
                .collect();
            wallet_lines.sort();
            lines.extend(wallet_lines);

            lines.join("\n")
        }
        1 => {
            if args[0] == "servers" {
                return "Show ranked indexer servers and their get_info() response times.\nUsage: servers".to_string();
            }
            match get_commands().get(args[0]) {
                Some(cmd) => cmd.help().to_string(),
                None => format!("Command {} not found", args[0]),
            }
        }
        _ => "Usage: help [command_name]".to_string(),
    }
}

struct InfoCommand {}
impl Command for InfoCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Print the connected indexer's info.

            Usage:
            info
        "}
    }

    fn short_help(&self) -> &'static str {
        "Get the indexer server's info"
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        // The presentation boundary: typed data becomes rendered JSON and
        // typed failure becomes display text here, and nowhere earlier.
        Ok(RT.block_on(async move {
            match lightclient.info().await {
                Ok(info) => json::JsonValue::from(info).pretty(2),
                Err(e) => e.to_string(),
            }
        }))
    }
}

struct CurrentPriceCommand {}
impl Command for CurrentPriceCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
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
        "}
    }

    fn short_help(&self) -> &'static str {
        "Updates and returns current price of ZEC."
    }

    #[cfg(feature = "nym")]
    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(RT.block_on(async move {
            match lightclient.update_current_price().await {
                Ok(fetch) => format!(
                    "current price: {} USD (source: {}, rtt: {} ms, fetched over the mixnet via {})",
                    fetch.usd,
                    fetch.source.name(),
                    fetch.round_trip.as_millis(),
                    fetch.via_socks5
                ),
                Err(e) => format!("error: {e}"),
            }
        }))
    }

    #[cfg(not(feature = "nym"))]
    fn exec(&self, _args: &[&str], _lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(
            "This build has no price fetch: price travels only over the Nym mixnet (ADR 0011). \
             Rebuild zingo-cli with `--features nym`."
                .to_string(),
        )
    }
}

struct NymCommand {}
impl Command for NymCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
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
        "}
    }

    fn short_help(&self) -> &'static str {
        "Control the Nym mixnet transport (on/off/status/probe/history)."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(nym_command(args, lightclient)?)
    }
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
#[derive(Debug, thiserror::Error)]
pub enum NymCommandError {
    #[cfg(feature = "nym")]
    #[error(
        "unknown nym subcommand '{0}'. Use: nym status | nym on [path] | nym off | \
         nym probe [uri] | nym history"
    )]
    UnknownSubCommand(String),
    #[cfg(feature = "nym")]
    #[error("'{0}' is not a valid indexer uri to probe")]
    InvalidProbeTarget(String),
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

/// A parsed `nym` command. Arguments parse completely into this
/// enum before any wallet access.
#[cfg(feature = "nym")]
#[derive(Debug, PartialEq, Eq)]
enum NymSubCommand {
    Status,
    On { path: Option<String> },
    Off,
    Probe { target: Option<http::Uri> },
    History,
}

#[cfg(feature = "nym")]
fn parse_nym_args(args: &[&str]) -> Result<NymSubCommand, NymCommandError> {
    match args.first().copied() {
        None | Some("status") => Ok(NymSubCommand::Status),
        Some("on") => Ok(NymSubCommand::On {
            path: args.get(1).map(|path| path.to_string()),
        }),
        Some("off") => Ok(NymSubCommand::Off),
        Some("probe") => {
            let target = args
                .get(1)
                .map(|raw| {
                    let uri = raw
                        .parse::<http::Uri>()
                        .map_err(|_| NymCommandError::InvalidProbeTarget((*raw).to_string()))?;
                    // https-only: the mixnet leg refuses a plaintext target at
                    // dial time, so reject it up front with a clear message.
                    if uri.scheme_str() != Some("https") {
                        return Err(NymCommandError::InvalidProbeTarget(format!(
                            "{raw} (indexers must be https)"
                        )));
                    }
                    Ok(uri)
                })
                .transpose()?;
            Ok(NymSubCommand::Probe { target })
        }
        Some("history") => Ok(NymSubCommand::History),
        Some(other) => Err(NymCommandError::UnknownSubCommand(other.to_string())),
    }
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
fn nym_command(args: &[&str], lightclient: &mut LightClient) -> Result<String, NymCommandError> {
    let subcommand = parse_nym_args(args)?;
    RT.block_on(async move {
        match subcommand {
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
    })
}

/// The body of the `nym` command when the mixnet transport is not compiled in.
#[cfg(not(feature = "nym"))]
fn nym_command(_args: &[&str], _lightclient: &mut LightClient) -> Result<String, NymCommandError> {
    Err(NymCommandError::FeatureAbsent)
}

struct BalanceCommand {}
impl Command for BalanceCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Return the wallet ZEC balance for each pool (account 0).
        "}
    }

    fn short_help(&self) -> &'static str {
        "Return the wallet ZEC balance for each pool (account 0)."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        RT.block_on(async move {
            match lightclient.account_balance(zip32::AccountId::ZERO).await {
                Ok(bal) => Ok(bal.to_string()),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            }
        })
    }
}

struct SpendableBalanceCommand {}
impl Command for SpendableBalanceCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Print the wallet's spendable balance.

            Usage:
            spendable_balance
        "}
    }

    fn short_help(&self) -> &'static str {
        "Display the wallet's spendable balance."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        RT.block_on(async move {
            let wallet = lightclient.wallet().read().await;
            let spendable_balance =
                match wallet.shielded_spendable_balance(zip32::AccountId::ZERO, false) {
                    Ok(bal) => bal,
                    Err(e) => return Err(CommandError::NotYetTyped(e.to_string())),
                };
            Ok(object! {
                "spendable_balance" => spendable_balance.into_u64(),
            }
            .pretty(2))
        })
    }
}

struct MaxSendValueCommand {}
impl Command for MaxSendValueCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Print the most the wallet can send to an address: shielded spendable
            balance less the fee. Mid-sync this can trail the confirmed balance.
            `zennies_for_zingo` also budgets 1_000_000 ZAT to the ZingoLabs developer
            address.

            Usage:
            max_send_value <address>
            max_send_value { "address": "<address>", "zennies_for_zingo": <true|false> }
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Print the most the wallet can send to a given address."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        let (address, zennies_for_zingo) = match utils::parse_max_send_value_args(args) {
            Ok(address_and_zennies) => address_and_zennies,
            Err(e) => {
                return Err(CommandError::NotYetTyped(format!(
                    "{e}\nTry 'help max_send_value' for correct usage and examples."
                )));
            }
        };
        Ok(RT.block_on(async move {
            match lightclient
                .max_send_value(address, zennies_for_zingo, zip32::AccountId::ZERO)
                .await
            {
                Ok(bal) => {
                    object! {
                        "max_send_value" => bal.into_u64(),
                    }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        }))
    }
}

struct NewUnifiedAddressCommand {}
impl Command for NewUnifiedAddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Create a new unified address, with an orchard receiver, a sapling one, or
            both. No transparent receivers: use `new_taddress` for those.

            Usage:
            new_address o | z | oz
        "}
    }

    fn short_help(&self) -> &'static str {
        "Create a new unified address."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if args.len() != 1 {
            return Ok(format!("No address type specified\n{}", self.help()));
        }
        if !args[0].contains('o') && !args[0].contains('z') {
            return Ok(format!("No address type specified\n{}", self.help()));
        }

        Ok(RT.block_on(async move {
            let chain_type = lightclient.chain_type();
            let mut wallet = lightclient.wallet().write().await;
            let receivers = ReceiverSelection {
                orchard: args[0].contains('o'),
                sapling: args[0].contains('z'),
            };
            match wallet.generate_unified_address(receivers, zip32::AccountId::ZERO) {
                Ok((id, unified_address)) => {
                    json::object! {
                        "account" => u32::from(zip32::AccountId::ZERO), // used concrete type instead of u32 to simplify upgrading CLI to multi-account
                        "address_index" => id.address_index,
                        "has_orchard" => unified_address.has_orchard(),
                        "has_sapling" => unified_address.has_sapling(),
                        "has_transparent" => unified_address.has_transparent(),
                        "encoded_address" => unified_address.encode(&chain_type),
                    }
                }
                Err(e) => object! { "error" => e.to_string() },
            }
            .pretty(2)
        }))
    }
}

struct NewTransparentAddressCommand {}
impl Command for NewTransparentAddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Create a new transparent address.

            Usage:
            new_taddress
        "}
    }

    fn short_help(&self) -> &'static str {
        "Create a new transparent address."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(RT.block_on(async move {
            let chain_type = lightclient.chain_type();
            let mut wallet = lightclient.wallet().write().await;
            match wallet.generate_transparent_address(zip32::AccountId::ZERO, true) {
                Ok((id, transparent_address)) => {
                    json::object! {
                        "account" => u32::from(id.account_id()),
                        "address_index" => id.address_index().index(),
                        "scope" => id.scope().to_string(),
                        "encoded_address" => transparent::encode_address(&chain_type,  transparent_address),
                    }
                }
                Err(e) => object! { "error" => e.to_string() },
            }
            .pretty(2)
        }))
    }
}

struct NewTransparentAddressAllowGapCommand {}
impl Command for NewTransparentAddressAllowGapCommand {
    fn help(&self) -> &'static str {
        indoc! {r#"
            Create a new transparent address even if the last one never received funds.

            This bypasses the no-gap rule, which exists because recovery from seed may
            not discover addresses beyond a gap. Funds sent to skipped addresses can go
            missing after a restore unless you rescan or raise the gap limit, so you are
            taking on tracking the unused ones yourself.

            Usage:
            new_taddress_allow_gap
        "#}
    }

    fn short_help(&self) -> &'static str {
        "Create a new transparent address (even if the last one did not receive any funds)."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(RT.block_on(async move {
            // Generate without enforcing the no-gap constraint
            let chain_type= lightclient.chain_type();
            let mut wallet = lightclient.wallet().write().await;

            match wallet.generate_transparent_address(zip32::AccountId::ZERO, false) {
                Ok((id, transparent_address)) => {
                    json::object! {
                        "account" => u32::from(id.account_id()),
                        "address_index" => id.address_index().index(),
                        "scope" => id.scope().to_string(),
                        "encoded_address" => transparent::encode_address(&chain_type, transparent_address),
                    }
                }
                Err(e) => object! { "error" => e.to_string() },
            }
            .pretty(2)
        }))
    }
}

struct UnifiedAddressesCommand {}
impl Command for UnifiedAddressesCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            List the wallet's unified addresses.

            Usage:
            addresses
        "}
    }

    fn short_help(&self) -> &'static str {
        "List unified addresses in the wallet."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(RT.block_on(async move { lightclient.unified_addresses_json().await.pretty(2) }))
    }
}

struct TransparentAddressesCommand {}
impl Command for TransparentAddressesCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            List the wallet's transparent addresses.

            Usage:
            t_addresses
        "}
    }

    fn short_help(&self) -> &'static str {
        "List transparent addresses in the wallet."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(RT.block_on(async move { lightclient.transparent_addresses_json().await.pretty(2) }))
    }
}

struct CheckAddressCommand {}
impl Command for CheckAddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Check whether an encoded address derives from the wallet's keys.

            Usage:
            check_address <encoded_address>
        "}
    }

    fn short_help(&self) -> &'static str {
        "Checks if the given encoded address is derived by the wallet's keys."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if args.len() != 1 {
            return Ok(json::object! { "error" => "no address specified. try 'help check_address' for correct usage and examples."
                .to_string() }.pretty(2))            ;
        }
        Ok(RT.block_on(async move {
            match lightclient
                .wallet()
                .read()
                .await
                .is_address_derived_by_keys(args[0])
            {
                Ok(address_ref) => address_ref.map_or(
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
                ),
                Err(e) => json::object! { "error" => e.to_string() },
            }
            .pretty(2)
        }))
    }
}

struct ExportUfvkCommand {}
impl Command for ExportUfvkCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Export the wallet's unified full viewing key. To back up spend capability,
            use `recovery_info` instead.

            Usage:
            export_ufvk
        "}
    }

    fn short_help(&self) -> &'static str {
        "Export unified full viewing key for the wallet."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(RT.block_on(async move {
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
                Err(e) => return e.to_string(),
            };
            object! {
                "ufvk" => ufvk.encode(&lightclient.chain_type()),
                "birthday" => lightclient.birthday()
            }
            .pretty(2)
        }))
    }
}

struct SendCommand {}
impl Command for SendCommand {
    fn help(&self) -> &'static str {
        concat!(
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
        )
    }

    fn short_help(&self) -> &'static str {
        "Propose a transfer of ZEC, for 'confirm' to broadcast."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        let receivers = match utils::parse_send_args(args) {
            Ok(receivers) => receivers,
            Err(e) => {
                return Err(CommandError::NotYetTyped(format!(
                    "{e}\nTry 'help send' for correct usage and examples."
                )));
            }
        };
        let request = match zingolib::data::receivers::transaction_request_from_receivers(receivers)
        {
            Ok(request) => request,
            Err(e) => {
                return Err(CommandError::NotYetTyped(format!(
                    "{e}\nTry 'help send' for correct usage and examples."
                )));
            }
        };
        Ok(RT.block_on(async move {
            match lightclient
                .propose_send(request, zip32::AccountId::ZERO)
                .await
            {
                Ok(proposal) => {
                    let fee = match zingolib::data::proposal::total_fee(&proposal) {
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
    }
}

struct SendAllCommand {}
impl Command for SendAllCommand {
    fn help(&self) -> &'static str {
        concat!(
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
        )
    }

    fn short_help(&self) -> &'static str {
        "Propose a transfer of all shielded ZEC to one address, for 'confirm' to broadcast."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        let (address, zennies_for_zingo, memo) = match utils::parse_send_all_args(args) {
            Ok(parse_results) => parse_results,
            Err(e) => {
                return Err(CommandError::NotYetTyped(format!(
                    "{e}\nTry 'help sendall' for correct usage and examples."
                )));
            }
        };
        Ok(RT.block_on(async move {
            match lightclient
                .propose_send_all(address, zennies_for_zingo, memo, zip32::AccountId::ZERO)
                .await
            {
                Ok(proposal) => {
                    let amount = match proposal::total_payment_amount(&proposal) {
                        Ok(amount) => amount,
                        Err(e) => return object! { "error" => e.to_string() }.pretty(2),
                    };
                    let fee = match proposal::total_fee(&proposal) {
                        Ok(fee) => fee,
                        Err(e) => return object! { "error" => e.to_string() }.pretty(2),
                    };
                    object! {
                        "amount" => amount.into_u64(),
                        "fee" => fee.into_u64(),
                    }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        }))
    }
}

struct QuickSendCommand {}
impl Command for QuickSendCommand {
    fn help(&self) -> &'static str {
        concat!(
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
        )
    }

    fn short_help(&self) -> &'static str {
        "Send ZEC, fusing `send` and `confirm`."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        let receivers = match utils::parse_send_args(args) {
            Ok(receivers) => receivers,
            Err(e) => {
                return Err(CommandError::NotYetTyped(format!(
                    "{e}\nTry 'help quicksend' for correct usage and examples."
                )));
            }
        };
        let request = match zingolib::data::receivers::transaction_request_from_receivers(receivers)
        {
            Ok(request) => request,
            Err(e) => {
                return Err(CommandError::NotYetTyped(format!(
                    "{e}\nTry 'help quicksend' for correct usage and examples."
                )));
            }
        };
        Ok(RT.block_on(async move {
            let progress = lightclient.transmit_progress_handle();
            match with_transmit_heartbeat(
                "quicksend",
                move || progress.latest(),
                |line| eprintln!("{line}"),
                lightclient.quick_send_reported(request, zip32::AccountId::ZERO, true),
            )
            .await
            {
                Ok(reports) => {
                    object! {
                        "txids" => reports.iter().map(|report| report.txid.to_string()).collect::<Vec<_>>(),
                        "transmissions" => reports.iter().map(render_transmit_report).collect::<Vec<_>>(),
                    }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        }))
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

struct ShieldCommand {}
impl Command for ShieldCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Propose a shield of transparent funds into the ironwood pool. Shows the
            fee, then 'confirm' broadcasts it.

            Usage:
                shield
        "}
    }

    fn short_help(&self) -> &'static str {
        "Propose a shield of transparent funds, for 'confirm' to broadcast."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if !args.is_empty() {
            return Err(CommandError::NotYetTyped(format!(
                "{}\nTry 'help shield' for correct usage and examples.",
                error::CommandError::InvalidArguments
            )));
        }

        Ok(RT.block_on(async move {
            match lightclient.propose_shield(zip32::AccountId::ZERO).await {
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
    }
}

struct QuickShieldCommand {}
impl Command for QuickShieldCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Shield transparent funds into the ironwood pool, fusing `shield` and
            `confirm`. The fee comes out of your balance and you never see it before
            the transaction goes out.

            Usage:
                quickshield
        "}
    }

    fn short_help(&self) -> &'static str {
        "Shield transparent funds, fusing `shield` and `confirm`."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if !args.is_empty() {
            return Err(CommandError::NotYetTyped(format!(
                "{}\nTry 'help shield' for correct usage and examples.",
                error::CommandError::InvalidArguments
            )));
        }

        Ok(RT.block_on(async move {
            let progress = lightclient.transmit_progress_handle();
            match with_transmit_heartbeat(
                "quickshield",
                move || progress.latest(),
                |line| eprintln!("{line}"),
                lightclient.quick_shield(zip32::AccountId::ZERO),
            )
            .await
            {
                Ok(txids) => {
                    object! { "txids" => txids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>() }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        }))
    }
}

struct ConfirmCommand {}
impl Command for ConfirmCommand {
    fn help(&self) -> &'static str {
        concat!(
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
        )
    }

    fn short_help(&self) -> &'static str {
        "Build and transmit the latest proposal, then resume sync."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if !args.is_empty() {
            return Err(CommandError::NotYetTyped(format!(
                "{}\nTry 'help confirm' for correct usage and examples.",
                error::CommandError::InvalidArguments
            )));
        }

        Ok(RT.block_on(async move {
            let progress = lightclient.transmit_progress_handle();
            match with_transmit_heartbeat(
                "confirm",
                move || progress.latest(),
                |line| eprintln!("{line}"),
                lightclient.send_stored_proposal(true),
            )
            .await
            {
                Ok(txids) => {
                    object! { "txids" => txids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>() }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        }))
    }
}

struct CalculateCommand {}
impl Command for CalculateCommand {
    fn help(&self) -> &'static str {
        concat!(
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
        )
    }

    fn short_help(&self) -> &'static str {
        "Sign the latest proposal without transmitting it."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if !args.is_empty() {
            return Err(CommandError::NotYetTyped(format!(
                "{}\nTry 'help calculate' for correct usage and examples.",
                error::CommandError::InvalidArguments
            )));
        }

        Ok(RT.block_on(async move {
            match lightclient.calculate_stored_proposal().await {
                Ok(txids) => {
                    object! {
                        "txids" => txids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>(),
                    }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        }))
    }
}

struct TransmitCommand {}
impl Command for TransmitCommand {
    fn help(&self) -> &'static str {
        concat!(
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
        )
    }

    fn short_help(&self) -> &'static str {
        "Transmit calculated transactions to the Indexer."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        RT.block_on(async move {
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
                match args
                    .iter()
                    .map(|arg| zingolib::utils::conversion::txid_from_hex_encoded_str(arg))
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(txids) => txids,
                    Err(e) => {
                        return Err(CommandError::NotYetTyped(format!(
                            "{e}\nTry 'help transmit' for correct usage and examples."
                        )));
                    }
                }
            };

            let Some(txids) = nonempty::NonEmpty::from_vec(txids) else {
                return Ok(
                    object! { "error" => "no calculated transactions to transmit" }.pretty(2),
                );
            };

            let progress = lightclient.transmit_progress_handle();
            Ok(match with_transmit_heartbeat(
                "transmit",
                move || progress.latest(),
                |line| eprintln!("{line}"),
                lightclient.transmit_calculated(txids),
            )
            .await
            {
                Ok(txids) => {
                    object! { "txids" => txids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>() }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2))
        })
    }
}

// TODO: add a decline command which deletes latest proposal?

struct DeleteCommand {}
impl Command for DeleteCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Delete the wallet file from disk.

            Usage:
            delete
        "}
    }

    fn short_help(&self) -> &'static str {
        "Delete wallet file from disk"
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(RT.block_on(async move {
            match lightclient.delete_wallet_file().await {
                Ok(()) => {
                    let r = object! { "result" => "success",
                    "wallet_path" => lightclient.wallet_path().to_str().expect("should be valid UTF-8") };
                    r.pretty(2)
                }
                Err(e) => {
                    let r = object! {
                        "result" => "error",
                        "error" => e.to_string()
                    };
                    r.pretty(2)
                }
            }
        }))
    }
}

struct RecoveryInfoCommand {}
impl Command for RecoveryInfoCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Print the wallet's seed phrase, birthday and account count.

            The seed phrase recovers the whole wallet. Save it carefully, share it with
            nobody.

            Usage:
            recovery_info
        "}
    }

    fn short_help(&self) -> &'static str {
        "Print the wallet's seed phrase, birthday and account count."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(RT.block_on(async move {
            match lightclient.wallet().read().await.recovery_info() {
                Some(backup_info) => backup_info.to_string(),
                None => "error: no mnemonic found. wallet loaded from key.".to_string(),
            }
        }))
    }
}

struct ValueTransfersCommand {}
impl Command for ValueTransfersCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            List the wallet's value transfers, each one a transaction's notes to a
            single receiver.

            Usage:
            value_transfers
        "}
    }

    fn short_help(&self) -> &'static str {
        "List all value transfers for this wallet."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        RT.block_on(async move {
            match lightclient.value_transfers(false).await {
                Ok(value_transfers) => Ok(value_transfers.to_string()),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            }
        })
    }
}

struct MessagesFilterCommand {}
impl Command for MessagesFilterCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            List the wallet's memo-bearing value transfers. An address filters to that
            correspondent, any other string filters to memos containing it, and no
            argument shows every memo. Received messages are matched on the memo's
            reply-to address.

            Usage:
            messages [address | string]
        "}
    }

    fn short_help(&self) -> &'static str {
        "List memos for this wallet."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if args.len() > 1 {
            return Err(CommandError::NotYetTyped(
                "invalid arguments\nTry 'help messages' for correct usage and examples".to_string(),
            ));
        }

        RT.block_on(async move {
            match lightclient.messages_containing(args.first().copied()).await {
                Ok(value_transfers) => Ok(json::JsonValue::from(value_transfers).pretty(2)),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            }
        })
    }
}

struct TransactionsCommand {}
impl Command for TransactionsCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            List the wallet's transaction summaries by block height.

            Usage:
            transactions
        "}
    }

    fn short_help(&self) -> &'static str {
        "List the wallet's transaction summaries by block height."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if !args.is_empty() {
            return Err(CommandError::NotYetTyped(
                "invalid arguments\nTry 'help transactions' for correct usage and examples"
                    .to_string(),
            ));
        }
        RT.block_on(async move {
            match lightclient.transaction_summaries(false).await {
                Ok(transactions) => Ok(transactions.to_string()),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            }
        })
    }
}

struct MemoBytesToAddressCommand {}
impl Command for MemoBytesToAddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Print total memo bytes sent, keyed by address.

            Usage:
            memobytes_to_address
        "}
    }

    fn short_help(&self) -> &'static str {
        "Show by address memo_bytes transfers for this seed."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if args.len() > 1 {
            return Ok(format!("didn't understand arguments\n{}", self.help()));
        }

        RT.block_on(async move {
            match lightclient.do_total_memobytes_to_address().await {
                Ok(total_memo_bytes) => Ok(json::JsonValue::from(total_memo_bytes).pretty(2)),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            }
        })
    }
}

struct ValueToAddressCommand {}
impl Command for ValueToAddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Print total value sent, keyed by address.

            Usage:
            value_to_address
        "}
    }

    fn short_help(&self) -> &'static str {
        "Show by address value transfers for this seed."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if args.len() > 1 {
            return Ok(format!("didn't understand arguments\n{}", self.help()));
        }

        RT.block_on(async move {
            match lightclient.do_total_value_to_address().await {
                Ok(total_values) => Ok(json::JsonValue::from(total_values).pretty(2)),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            }
        })
    }
}

struct SendsToAddressCommand {}
impl Command for SendsToAddressCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Print the number of sends, keyed by address.

            Usage:
            sends_to_address
        "}
    }

    fn short_help(&self) -> &'static str {
        "Show by address number of sends for this seed."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if args.len() > 1 {
            return Ok(format!("didn't understand arguments\n{}", self.help()));
        }

        RT.block_on(async move {
            match lightclient.do_total_spends_to_address().await {
                Ok(total_spends) => Ok(json::JsonValue::from(total_spends).pretty(2)),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            }
        })
    }
}

struct SettingsCommand {}
impl Command for SettingsCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Show or set wallet settings. With no argument, prints them all. To set one,
            name it and give a value.

            performance        low | medium | high | maximum
            min_confirmations  1 or greater

            Usage:
            settings
            settings performance high
        "}
    }

    fn short_help(&self) -> &'static str {
        "Show or set wallet settings."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        RT.block_on(async move {
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

            match args[0] {
                "performance" => match args[1] {
                    "low" => wallet.wallet_settings.sync_config.performance_level = PerformanceLevel::Low,
                    "medium" => wallet.wallet_settings.sync_config.performance_level = PerformanceLevel::Medium,
                    "high" => wallet.wallet_settings.sync_config.performance_level = PerformanceLevel::High,
                    "maximum" => wallet.wallet_settings.sync_config.performance_level = PerformanceLevel::Maximum,
                    _ => {
                return Err(CommandError::NotYetTyped(
                    "invalid arguments\nTry 'help settings' for correct usage and examples"
                        .to_string(),
                ));}
                    },
                "min_confirmations" => {
                    let min_confirmations = match args[1].parse::<u32>() {
                        Ok(m) => match NonZeroU32::try_from(m) {
                            Ok(m) => m,
                            Err(_) => {
                                return Err(CommandError::NotYetTyped(
                                    "invalid arguments\nTry 'help settings' for correct usage and examples"
                                        .to_string(),
                                ));
                            }
                        },
                        Err(_) => {
                            return Err(CommandError::NotYetTyped(
                                "invalid arguments\nTry 'help settings' for correct usage and examples"
                                    .to_string(),
                            ));
                        }
                    };
                    wallet.wallet_settings.min_confirmations = min_confirmations;
                }
                _ => {
            return Err(CommandError::NotYetTyped(
                "invalid arguments\nTry 'help settings' for correct usage and examples"
                    .to_string(),
            ));}
            }

            wallet.mark_dirty();

            Ok("Successfully updated settings.".to_string())
        })
    }
}

struct HeightCommand {}
impl Command for HeightCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Print the chain height as of the wallet's last request to the server.

            Usage:
            height
        "}
    }

    fn short_help(&self) -> &'static str {
        "Print the chain height as of the wallet's last request to the server."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(RT.block_on(async move {
            object! { "height" => json::JsonValue::from(lightclient.wallet().read().await.sync_state.last_known_chain_height().map_or(0, u32::from))}.pretty(2)
        }))
    }
}

struct NotesCommand {}
impl Command for NotesCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Show the wallet's notes (shielded outputs). `all` includes spent ones.

            Usage:
            notes [all]
        "}
    }

    fn short_help(&self) -> &'static str {
        "Show the wallet's notes (shielded outputs)."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        // Parse the args.
        if args.len() > 1 {
            return Ok(self.short_help().to_string());
        }

        // Make sure we can parse the amount
        let all_notes = if args.len() == 1 {
            match args[0] {
                "all" => true,
                a => {
                    return Ok(format!(
                        "Invalid argument \"{a}\". Specify 'all' to include spent notes"
                    ));
                }
            }
        } else {
            false
        };

        Ok(RT.block_on(async move {
            let wallet = lightclient.wallet().read().await;

            json::object! {
                "ironwood_notes" => json::JsonValue::from(wallet.note_summaries::<IronwoodNote>(all_notes)),
                "orchard_notes" => json::JsonValue::from(wallet.note_summaries::<OrchardNote>(all_notes)),
                "sapling_notes" => json::JsonValue::from(wallet.note_summaries::<SaplingNote>(all_notes)),
            }
            .pretty(2)
        }))
    }
}

struct CoinsCommand {}
impl Command for CoinsCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Show the wallet's coins (transparent outputs). `all` includes spent ones.

            Usage:
            coins [all]
        "}
    }

    fn short_help(&self) -> &'static str {
        "Show the wallet's coins (transparent outputs)."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        // Parse the args.
        if args.len() > 1 {
            return Ok(self.short_help().to_string());
        }

        // Make sure we can parse the amount
        let all_coins = if args.len() == 1 {
            match args[0] {
                "all" => true,
                a => {
                    return Ok(format!(
                        "Invalid argument \"{a}\". Specify 'all' to include spent coins"
                    ));
                }
            }
        } else {
            false
        };

        Ok(RT.block_on(async move {
            json::object! {
                "transparent_coins" => json::JsonValue::from(lightclient.wallet().read().await.coin_summaries(all_coins)),
            }
            .pretty(2)
        }))
    }
}

struct RemoveTransactionCommand {}
impl Command for RemoveTransactionCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Remove a failed transaction from the wallet. Manual on purpose, so a failed
            send keeps its memos until you decide to drop them.

            Usage:
            remove_transaction <txid>
        "}
    }

    fn short_help(&self) -> &'static str {
        "Remove a failed transaction from the wallet."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if args.len() != 1 {
            return Err(CommandError::NotYetTyped(
                "remove command expects 1 argument. Type \"help remove\" for usage.".to_string(),
            ));
        }

        let txid = match txid_from_hex_encoded_str(args[0]) {
            Ok(txid) => txid,
            Err(e) => return Err(CommandError::NotYetTyped(e.to_string())),
        };

        RT.block_on(async move {
            match lightclient
                .wallet()
                .write()
                .await
                .remove_failed_transaction(txid)
            {
                Ok(()) => Ok("Successfully removed failed transaction.".to_string()),
                Err(e) => Err(CommandError::NotYetTyped(e.to_string())),
            }
        })
    }
}

struct SaveCommand {}
impl Command for SaveCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Launch the task that persists the wallet as its state changes. Not meant to
            be called by hand.

            Usage:
            save run | check | shutdown
        "}
    }

    fn short_help(&self) -> &'static str {
        "Launch the save task. Not meant to be called by hand."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        if args.len() != 1 {
            return Err(CommandError::NotYetTyped(
                "save command expects 1 argument. Type \"help save\" for usage.".to_string(),
            ));
        }

        match args[0] {
            "run" => {
                RT.block_on(async move { lightclient.save_task().await });
                Ok("Launching save task...".to_string())
            }
            "check" => match RT.block_on(async move { lightclient.check_save_error().await }) {
                Ok(()) => Ok(String::new()),
                Err(e) => Err(CommandError::NotYetTyped(format!(
                    "save failed. {e}\nRestarting save task..."
                ))),
            },
            "shutdown" => {
                match RT.block_on(async move { lightclient.shutdown_save_task().await }) {
                    Ok(()) => Ok("Save task shutdown successfully.".to_string()),
                    Err(e) => Err(CommandError::NotYetTyped(format!("save failed. {e}"))),
                }
            }
            _ => Err(CommandError::NotYetTyped(
                "invalid sub-command. Type \"help save\" for usage.".to_string(),
            )),
        }
    }
}

struct QuitCommand {}
impl Command for QuitCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
            Quit the light client, saving state to disk.

            Usage:
            quit
        "}
    }

    fn short_help(&self) -> &'static str {
        "Quit the light client, saving state to disk."
    }

    fn exec(&self, _args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        let save_shutdown = do_user_command("save", &["shutdown"], lightclient);

        Ok(format!("{save_shutdown}\nZingo CLI quit successfully."))
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
/// pattern PR #2464 established: the discriminant lives in the type,
/// argument parsing happens before any wallet access, and prose is
/// produced at exactly one rendering site per command. Each Display
/// message is byte-identical to the in-band string it replaced, so no
/// frontend observes the change.
#[derive(Debug, thiserror::Error)]
pub enum MigrationCommandError {
    #[error("migrate command expects no arguments. Type \"help migrate\" for usage.")]
    UnexpectedArguments,
    #[error("migration command expects a sub-command. Type \"help migration\" for usage.")]
    MissingSubCommand,
    #[error("invalid sub-command. Type \"help migration\" for usage.")]
    InvalidSubCommand,
    #[error("migration start expects the plan hash printed by \"migration plan\".")]
    MissingPlanHash,
    #[error("the plan hash must be 64 hex characters.")]
    MalformedPlanHash,
    #[error("--per-bucket expects a positive integer.")]
    MalformedPerBucket,
    #[error("cadence expects the number of parts per broadcast window.")]
    MalformedCadence,
    #[error("spacing must be a number of seconds.")]
    MalformedSpacing,
    #[error("drain expects a sub-command: plan | now. Type \"help drain\" for usage.")]
    DrainUsage,
    #[error("split expects a sub-command: plan | now. Type \"help split\" for usage.")]
    SplitUsage,
    #[error("sync failed: {0}")]
    Sync(zingolib::lightclient::error::LightClientError),
    #[error("{0}")]
    Client(#[from] zingolib::lightclient::error::LightClientError),
}

/// A parsed migration command. Arguments parse completely
/// into this enum before any wallet access.
#[derive(Debug, PartialEq, Eq)]
enum MigrationSubCommand {
    Plan,
    Start {
        plan_hash: [u8; 32],
        per_bucket: Option<u32>,
    },
    Continue,
    Cadence {
        per_bucket: u32,
    },
    Execute {
        spacing: std::time::Duration,
    },
    Auto,
    Status,
    Windows,
    Reconcile,
    Catchup {
        spacing: std::time::Duration,
    },
    Cancel,
}

/// Pure parser for the migration command family's arguments.
fn parse_migration_args(args: &[&str]) -> Result<MigrationSubCommand, MigrationCommandError> {
    let Some(sub_command) = args.first() else {
        return Err(MigrationCommandError::MissingSubCommand);
    };
    match *sub_command {
        "plan" => Ok(MigrationSubCommand::Plan),
        "start" => {
            let hash_hex = args.get(1).ok_or(MigrationCommandError::MissingPlanHash)?;
            let plan_hash: [u8; 32] = hex::decode(hash_hex)
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or(MigrationCommandError::MalformedPlanHash)?;
            let mut per_bucket = None;
            let mut remaining = args[2..].iter();
            while let Some(arg) = remaining.next() {
                if *arg == "--per-bucket" {
                    per_bucket = Some(
                        remaining
                            .next()
                            .and_then(|value| value.parse::<u32>().ok())
                            .ok_or(MigrationCommandError::MalformedPerBucket)?,
                    );
                }
            }
            Ok(MigrationSubCommand::Start {
                plan_hash,
                per_bucket,
            })
        }
        "continue" => Ok(MigrationSubCommand::Continue),
        "cadence" => {
            let per_bucket = args
                .get(1)
                .and_then(|value| value.parse::<u32>().ok())
                .ok_or(MigrationCommandError::MalformedCadence)?;
            Ok(MigrationSubCommand::Cadence { per_bucket })
        }
        "execute" => {
            let spacing = match args.get(1) {
                Some(seconds) => std::time::Duration::from_secs(
                    seconds
                        .parse::<u64>()
                        .map_err(|_| MigrationCommandError::MalformedSpacing)?,
                ),
                None => std::time::Duration::from_secs(30),
            };
            Ok(MigrationSubCommand::Execute { spacing })
        }
        "auto" => Ok(MigrationSubCommand::Auto),
        "status" => Ok(MigrationSubCommand::Status),
        "windows" => Ok(MigrationSubCommand::Windows),
        "reconcile" => Ok(MigrationSubCommand::Reconcile),
        "catchup" => {
            let spacing = match args.get(1) {
                Some(seconds) => std::time::Duration::from_secs(
                    seconds
                        .parse::<u64>()
                        .map_err(|_| MigrationCommandError::MalformedSpacing)?,
                ),
                None => std::time::Duration::from_secs(30),
            };
            Ok(MigrationSubCommand::Catchup { spacing })
        }
        "cancel" => Ok(MigrationSubCommand::Cancel),
        _ => Err(MigrationCommandError::InvalidSubCommand),
    }
}

/// Renders displayable ids as a JSON array of strings.
fn txids_json<T: ToString>(txids: &[T]) -> json::JsonValue {
    txids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .into()
}

/// Runs the `migrate` command. Its errors cross `exec` as
/// [`CommandError::Migration`] and render at `do_user_command`.
fn run_migrate(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, MigrationCommandError> {
    if !args.is_empty() {
        return Err(MigrationCommandError::UnexpectedArguments);
    }
    let progress = lightclient.transmit_progress_handle();
    let summary = RT.block_on(with_transmit_heartbeat(
        "migrate",
        move || progress.latest(),
        |line| eprintln!("{line}"),
        lightclient.migrate_to_ironwood(zip32::AccountId::ZERO),
    ))?;
    Ok(object! {
        "split_txids" => txids_json(&summary.split_txids),
        "part_txids" => txids_json(&summary.part_txids),
        "residual" => summary.residual,
    }
    .pretty(2))
}

/// Runs one `migration` sub-command.
fn run_migration(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, MigrationCommandError> {
    Ok(match parse_migration_args(args)? {
        MigrationSubCommand::Plan => {
            let plan = RT.block_on(lightclient.plan_ironwood_migration(zip32::AccountId::ZERO))?;
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
            let progress = lightclient.transmit_progress_handle();
            RT.block_on(with_transmit_heartbeat(
                "migration start",
                move || progress.latest(),
                |line| eprintln!("{line}"),
                lightclient.start_ironwood_migration(
                    zip32::AccountId::ZERO,
                    migration::SigningStrategy::LazyAtBoundary,
                    plan_hash,
                    per_bucket,
                ),
            ))?;
            "Migration started.".to_string()
        }
        MigrationSubCommand::Continue => {
            RT.block_on(lightclient.sync_and_await())
                .map_err(MigrationCommandError::Sync)?;
            match RT.block_on(lightclient.continue_note_splitting())? {
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
            RT.block_on(lightclient.reschedule_parts(per_bucket))?;
            format!("Cadence set to {per_bucket} per window; the schedule was re-drawn.")
        }
        MigrationSubCommand::Execute { spacing } => {
            RT.block_on(lightclient.sync_and_await())
                .map_err(MigrationCommandError::Sync)?;
            let progress = lightclient.batch_progress_handle();
            let report = RT.block_on(with_transmit_heartbeat(
                "migration execute",
                move || progress.status().as_ref().map(batch_progress_line),
                |line| eprintln!("{line}"),
                lightclient.execute_due_parts(spacing),
            ))?;
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
            RT.block_on(lightclient.sync_and_await())
                .map_err(MigrationCommandError::Sync)?;
            let txids = RT.block_on(lightclient.auto_broadcast_if_due())?;
            if txids.is_empty() {
                "No parts due yet.".to_string()
            } else {
                object! { "broadcast" => txids_json(&txids) }.pretty(2)
            }
        }
        MigrationSubCommand::Status => {
            let status = RT.block_on(lightclient.migration_status())?;
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
            let timeline = RT.block_on(lightclient.window_timeline())?;
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
            let report = RT.block_on(lightclient.reconcile_migration())?;
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
            let progress = lightclient.transmit_progress_handle();
            let txids = RT.block_on(with_transmit_heartbeat(
                "migration catchup",
                move || progress.latest(),
                |line| eprintln!("{line}"),
                lightclient.catch_up_migration(spacing),
            ))?;
            if txids.is_empty() {
                "No overdue parts.".to_string()
            } else {
                object! { "part_txids" => txids_json(&txids) }.pretty(2)
            }
        }
        MigrationSubCommand::Cancel => {
            RT.block_on(lightclient.cancel_ironwood_migration())?;
            "Migration canceled.".to_string()
        }
    })
}

/// A parsed `drain` sub-command.
#[derive(Debug, PartialEq, Eq)]
enum DrainSubCommand {
    Plan,
    Now,
}

/// Pure parser for the drain command's arguments.
fn parse_drain_args(args: &[&str]) -> Result<DrainSubCommand, MigrationCommandError> {
    match args {
        ["plan"] => Ok(DrainSubCommand::Plan),
        ["now"] => Ok(DrainSubCommand::Now),
        _ => Err(MigrationCommandError::DrainUsage),
    }
}

/// A parsed `split` sub-command.
#[derive(Debug, PartialEq, Eq)]
enum SplitSubCommand {
    Plan,
    Now,
}

/// Pure parser for the split command's arguments.
fn parse_split_args(args: &[&str]) -> Result<SplitSubCommand, MigrationCommandError> {
    match args {
        ["plan"] => Ok(SplitSubCommand::Plan),
        ["now"] => Ok(SplitSubCommand::Now),
        _ => Err(MigrationCommandError::SplitUsage),
    }
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
fn run_drain(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, MigrationCommandError> {
    Ok(match parse_drain_args(args)? {
        DrainSubCommand::Plan => {
            let plan = RT.block_on(lightclient.plan_immediate_migration(zip32::AccountId::ZERO))?;
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
            let summary = RT.block_on(with_transmit_heartbeat(
                "drain",
                move || progress.status().as_ref().map(drain_progress_line),
                |line| eprintln!("{line}"),
                lightclient.quick_immediate_migration(zip32::AccountId::ZERO, true),
            ))?;
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
fn run_split(
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, MigrationCommandError> {
    Ok(match parse_split_args(args)? {
        SplitSubCommand::Plan => {
            let plan = RT.block_on(lightclient.plan_note_split(zip32::AccountId::ZERO))?;
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
            match RT.block_on(with_transmit_heartbeat(
                "split",
                move || progress.status().as_ref().map(split_progress_line),
                |line| eprintln!("{line}"),
                lightclient.quick_split(zip32::AccountId::ZERO, true),
            ))? {
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

struct DrainCommand {}
impl Command for DrainCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
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
        "}
    }

    fn short_help(&self) -> &'static str {
        "Send all Orchard funds into the Ironwood pool now (immediate ZIP 318 path)."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(run_drain(args, lightclient)?)
    }
}

struct SplitCommand {}
impl Command for SplitCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
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
        "}
    }

    fn short_help(&self) -> &'static str {
        "Split Orchard notes into ZIP 318 part sizes, one round per call."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(run_split(args, lightclient)?)
    }
}

struct MigrateCommand {}
impl Command for MigrateCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
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
        "}
    }

    fn short_help(&self) -> &'static str {
        "Migrate all Orchard funds to the Ironwood pool in one interactive run."
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(run_migrate(args, lightclient)?)
    }
}

struct MigrationCommand {}
impl Command for MigrationCommand {
    fn help(&self) -> &'static str {
        indoc! {r"
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
        "}
    }

    fn short_help(&self) -> &'static str {
        "Drive the scheduled Orchard to Ironwood migration"
    }

    fn exec(&self, args: &[&str], lightclient: &mut LightClient) -> Result<String, CommandError> {
        Ok(run_migration(args, lightclient)?)
    }
}

/// Commands that do not require a wallet connection.
pub fn get_standalone_commands() -> HashMap<&'static str, Box<dyn Command>> {
    vec![
        ("help", Box::new(HelpCommand {}) as Box<dyn Command>),
        ("parse_address", Box::new(ParseAddressCommand {})),
        ("parse_viewkey", Box::new(ParseViewKeyCommand {})),
        ("version", Box::new(GetVersionCommand {})),
    ]
    .into_iter()
    .collect()
}

/// Commands that require a wallet connection.
pub fn get_wallet_commands() -> HashMap<&'static str, Box<dyn Command>> {
    vec![
        (
            "addresses",
            Box::new(UnifiedAddressesCommand {}) as Box<dyn Command>,
        ),
        ("balance", Box::new(BalanceCommand {})),
        ("birthday", Box::new(BirthdayCommand {})),
        ("change_server", Box::new(ChangeServerCommand {})),
        ("check_address", Box::new(CheckAddressCommand {})),
        ("clear", Box::new(ClearCommand {})),
        ("coins", Box::new(CoinsCommand {})),
        ("calculate", Box::new(CalculateCommand {})),
        ("confirm", Box::new(ConfirmCommand {})),
        ("transmit", Box::new(TransmitCommand {})),
        ("current_price", Box::new(CurrentPriceCommand {})),
        ("delete", Box::new(DeleteCommand {})),
        ("drain", Box::new(DrainCommand {})),
        ("export_ufvk", Box::new(ExportUfvkCommand {})),
        ("height", Box::new(HeightCommand {})),
        ("info", Box::new(InfoCommand {})),
        ("max_send_value", Box::new(MaxSendValueCommand {})),
        (
            "memobytes_to_address",
            Box::new(MemoBytesToAddressCommand {}),
        ),
        ("messages", Box::new(MessagesFilterCommand {})),
        ("migrate", Box::new(MigrateCommand {})),
        ("migration", Box::new(MigrationCommand {})),
        ("new_address", Box::new(NewUnifiedAddressCommand {})),
        ("new_taddress", Box::new(NewTransparentAddressCommand {})),
        ("nym", Box::new(NymCommand {})),
        (
            "new_taddress_allow_gap",
            Box::new(NewTransparentAddressAllowGapCommand {}),
        ),
        ("notes", Box::new(NotesCommand {})),
        ("quicksend", Box::new(QuickSendCommand {})),
        ("quickshield", Box::new(QuickShieldCommand {})),
        ("quit", Box::new(QuitCommand {})),
        ("recovery_info", Box::new(RecoveryInfoCommand {})),
        ("remove_transaction", Box::new(RemoveTransactionCommand {})),
        ("rescan", Box::new(RescanCommand {})),
        ("save", Box::new(SaveCommand {})),
        ("send", Box::new(SendCommand {})),
        ("send_all", Box::new(SendAllCommand {})),
        ("sends_to_address", Box::new(SendsToAddressCommand {})),
        ("settings", Box::new(SettingsCommand {})),
        ("shield", Box::new(ShieldCommand {})),
        ("spendable_balance", Box::new(SpendableBalanceCommand {})),
        ("split", Box::new(SplitCommand {})),
        ("sync", Box::new(SyncCommand {})),
        ("t_addresses", Box::new(TransparentAddressesCommand {})),
        ("transactions", Box::new(TransactionsCommand {})),
        ("value_to_address", Box::new(ValueToAddressCommand {})),
        ("value_transfers", Box::new(ValueTransfersCommand {})),
        ("wallet_kind", Box::new(WalletKindCommand {})),
    ]
    .into_iter()
    .collect()
}

/// All commands (standalone + wallet). Used for dispatch and `help <command>`.
pub fn get_commands() -> HashMap<&'static str, Box<dyn Command>> {
    let mut all = get_standalone_commands();
    all.extend(get_wallet_commands());
    all
}

/// Dispatches a user command by name to the appropriate [`Command`] implementation,
/// exposing the typed success/failure crossing.
///
/// An unknown command returns its "Unknown command" prose via `Ok`, mirroring
/// the string entry point's historical behavior of not treating it as an error.
pub fn do_user_command_result(
    cmd: &str,
    args: &[&str],
    lightclient: &mut LightClient,
) -> Result<String, CommandError> {
    match get_commands().get(cmd.to_ascii_lowercase().as_str()) {
        Some(cmd) => cmd.exec(args, lightclient),
        None => Ok(format!(
            "Unknown command : {cmd}. Type 'help' for a list of commands"
        )),
    }
}

/// Dispatches a user command by name to the appropriate [`Command`] implementation.
///
/// Returns the command's output string, or an "Unknown command" message
/// if no command with the given name exists. This is the single site that
/// renders [`CommandError`] to prose for string frontends.
pub fn do_user_command(cmd: &str, args: &[&str], lightclient: &mut LightClient) -> String {
    match do_user_command_result(cmd, args, lightclient) {
        Ok(output) => output,
        Err(e) => format!("Error: {e}"),
    }
}

#[cfg(test)]
mod transmit_heartbeat {
    //! Paused-clock falsifiers for the transmit heartbeat's contract: silence
    //! for fast transmissions, a narrated line on the ratified 20-40s cadence
    //! for slow ones, always carrying the side channel's latest detail.

    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;

    /// HYPOTHESIS: a transmission finishing before the first tick emits
    /// nothing, because the heartbeat must not add noise to a normal fast send.
    #[tokio::test(start_paused = true)]
    async fn a_fast_transmission_stays_silent() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = lines.clone();
        let out = with_transmit_heartbeat(
            "confirm",
            || Some("submitting".to_string()),
            move |line| sink.lock().expect("line sink poisoned").push(line),
            tokio::time::sleep(zingo_netutils::time::test::SIMULATED_TRANSMIT),
        )
        .await;
        let () = out;
        assert!(
            lines.lock().expect("line sink poisoned").is_empty(),
            "no heartbeat before the first interval"
        );
    }

    /// HYPOTHESIS: a slow transmission is narrated on the interval cadence,
    /// each line carrying the label, the side channel's latest detail, and
    /// the elapsed seconds. Falsified if the wait stays silent or drops the
    /// detail.
    #[tokio::test(start_paused = true)]
    async fn a_slow_transmission_heartbeats_the_latest_detail() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = lines.clone();
        with_transmit_heartbeat(
            "confirm",
            || Some("witness zec.rocks: submitting".to_string()),
            move |line| sink.lock().expect("line sink poisoned").push(line),
            tokio::time::sleep(Duration::from_secs(95)),
        )
        .await;
        let lines = lines.lock().expect("line sink poisoned").clone();
        assert_eq!(
            lines,
            vec![
                "confirm: witness zec.rocks: submitting (30s elapsed)".to_string(),
                "confirm: witness zec.rocks: submitting (60s elapsed)".to_string(),
                "confirm: witness zec.rocks: submitting (90s elapsed)".to_string(),
            ]
        );
    }

    /// An empty side channel still heartbeats, falling back to a generic
    /// line rather than skipping the tick.
    #[tokio::test(start_paused = true)]
    async fn an_empty_side_channel_still_heartbeats() {
        let lines: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = lines.clone();
        with_transmit_heartbeat(
            "transmit",
            || None,
            move |line| sink.lock().expect("line sink poisoned").push(line),
            tokio::time::sleep(Duration::from_secs(35)),
        )
        .await;
        assert_eq!(
            lines.lock().expect("line sink poisoned").clone(),
            vec!["transmit: transmitting (30s elapsed)".to_string()]
        );
    }
}

#[cfg(test)]
mod migration_command_parsing {
    //! Pins the pure argument parser and the byte-identity of the typed
    //! errors' rendering with the in-band strings they replaced.

    use super::*;

    #[test]
    fn start_parses_hash_and_per_bucket() {
        let hash_hex = "11".repeat(32);
        let parsed = parse_migration_args(&["start", &hash_hex, "--per-bucket", "3"])
            .expect("well-formed arguments parse");
        assert_eq!(
            parsed,
            MigrationSubCommand::Start {
                plan_hash: [0x11; 32],
                per_bucket: Some(3),
            }
        );
    }

    #[test]
    fn malformed_plan_hash_is_typed() {
        assert!(matches!(
            parse_migration_args(&["start", "abc"]),
            Err(MigrationCommandError::MalformedPlanHash)
        ));
    }

    #[test]
    fn continue_parses_bare() {
        assert_eq!(
            parse_migration_args(&["continue"]).expect("bare continue parses"),
            MigrationSubCommand::Continue
        );
    }

    #[test]
    fn windows_parses_bare() {
        assert_eq!(
            parse_migration_args(&["windows"]).expect("bare windows parses"),
            MigrationSubCommand::Windows
        );
    }

    #[test]
    fn cadence_requires_a_count() {
        assert_eq!(
            parse_migration_args(&["cadence", "4"]).expect("well-formed cadence parses"),
            MigrationSubCommand::Cadence { per_bucket: 4 }
        );
        assert!(matches!(
            parse_migration_args(&["cadence"]),
            Err(MigrationCommandError::MalformedCadence)
        ));
    }

    #[test]
    fn execute_defaults_spacing_to_thirty_seconds() {
        assert_eq!(
            parse_migration_args(&["execute"]).expect("bare execute parses"),
            MigrationSubCommand::Execute {
                spacing: std::time::Duration::from_secs(30),
            }
        );
        assert_eq!(
            parse_migration_args(&["execute", "5"]).expect("spaced execute parses"),
            MigrationSubCommand::Execute {
                spacing: std::time::Duration::from_secs(5),
            }
        );
    }

    #[test]
    fn catchup_defaults_spacing_to_thirty_seconds() {
        assert_eq!(
            parse_migration_args(&["catchup"]).expect("bare catchup parses"),
            MigrationSubCommand::Catchup {
                spacing: std::time::Duration::from_secs(30),
            }
        );
    }

    #[test]
    fn errors_render_byte_identically_to_the_replaced_strings() {
        assert_eq!(
            format!("Error: {}", MigrationCommandError::MissingSubCommand),
            "Error: migration command expects a sub-command. Type \"help migration\" for usage."
        );
        assert_eq!(
            format!("Error: {}", MigrationCommandError::MalformedPerBucket),
            "Error: --per-bucket expects a positive integer."
        );
    }
}

#[cfg(test)]
mod drain_and_split_command_parsing {
    //! Pins the pure argument parsers of the mobile-parity migration
    //! commands: `drain` and `split` accept exactly `plan` or `now`.

    use super::*;

    #[test]
    fn drain_accepts_exactly_plan_or_now() {
        assert_eq!(
            parse_drain_args(&["plan"]).expect("drain plan parses"),
            DrainSubCommand::Plan
        );
        assert_eq!(
            parse_drain_args(&["now"]).expect("drain now parses"),
            DrainSubCommand::Now
        );
        for junk in [&[][..], &["bogus"][..], &["now", "extra"][..]] {
            assert!(matches!(
                parse_drain_args(junk),
                Err(MigrationCommandError::DrainUsage)
            ));
        }
    }

    #[test]
    fn split_accepts_exactly_plan_or_now() {
        assert_eq!(
            parse_split_args(&["plan"]).expect("split plan parses"),
            SplitSubCommand::Plan
        );
        assert_eq!(
            parse_split_args(&["now"]).expect("split now parses"),
            SplitSubCommand::Now
        );
        for junk in [&[][..], &["bogus"][..], &["plan", "extra"][..]] {
            assert!(matches!(
                parse_split_args(junk),
                Err(MigrationCommandError::SplitUsage)
            ));
        }
    }

    #[test]
    fn usage_errors_render_with_help_pointers() {
        assert_eq!(
            format!("Error: {}", MigrationCommandError::DrainUsage),
            "Error: drain expects a sub-command: plan | now. Type \"help drain\" for usage."
        );
        assert_eq!(
            format!("Error: {}", MigrationCommandError::SplitUsage),
            "Error: split expects a sub-command: plan | now. Type \"help split\" for usage."
        );
    }
}

#[cfg(test)]
mod nym_command_parsing {
    //! Pins the pure argument parser and the byte-identity of the typed
    //! errors' rendering with the in-band strings they replaced.

    use super::*;

    #[cfg(feature = "nym")]
    #[test]
    fn bare_and_status_both_parse_to_status() {
        assert_eq!(
            parse_nym_args(&[]).expect("a bare nym parses"),
            NymSubCommand::Status
        );
        assert_eq!(
            parse_nym_args(&["status"]).expect("nym status parses"),
            NymSubCommand::Status
        );
    }

    #[cfg(feature = "nym")]
    #[test]
    fn on_captures_the_optional_path() {
        assert_eq!(
            parse_nym_args(&["on"]).expect("bare nym on parses"),
            NymSubCommand::On { path: None }
        );
        assert_eq!(
            parse_nym_args(&["on", "/opt/nym-proxy"]).expect("nym on with a path parses"),
            NymSubCommand::On {
                path: Some("/opt/nym-proxy".to_string()),
            }
        );
    }

    #[cfg(feature = "nym")]
    #[test]
    fn unknown_subcommand_renders_byte_identically_to_the_replaced_string() {
        assert_eq!(
            parse_nym_args(&["bogus"])
                .expect_err("an unknown subcommand is typed")
                .to_string(),
            "unknown nym subcommand 'bogus'. Use: nym status | nym on [path] | nym off | \
             nym probe [uri] | nym history"
        );
    }

    #[cfg(feature = "nym")]
    #[test]
    fn probe_parses_its_optional_target_and_rejects_junk() {
        assert_eq!(
            parse_nym_args(&["probe"]).expect("bare probe parses"),
            NymSubCommand::Probe { target: None }
        );
        assert_eq!(
            parse_nym_args(&["probe", "https://zec.rocks:443"]).expect("probe with a uri parses"),
            NymSubCommand::Probe {
                target: Some("https://zec.rocks:443".parse().expect("static uri")),
            }
        );
        assert!(matches!(
            parse_nym_args(&["probe", "not a uri"]),
            Err(NymCommandError::InvalidProbeTarget(_))
        ));
        assert!(
            matches!(
                parse_nym_args(&["probe", "http://zec.rocks:9067"]),
                Err(NymCommandError::InvalidProbeTarget(_))
            ),
            "a plaintext http target is refused: mixnet transmission is https-only"
        );
        assert_eq!(
            parse_nym_args(&["history"]).expect("history parses"),
            NymSubCommand::History
        );
    }

    /// HYPOTHESIS: the paired-probe rendering makes a mixnet-specific failure
    /// legible at a glance: clearnet ok beside mixnet FAILED. Falsified if
    /// either leg's outcome, timing, or the not-ready skip is dropped.
    #[cfg(feature = "nym")]
    #[test]
    fn paired_probe_renders_both_legs_side_by_side() {
        use zingo_net_diag::{NetOpFailure, NetOpStage};
        use zingolib::nym::probe::{PairedProbe, ProbeLeg, ProbeSuccess};

        let tip = ProbeSuccess {
            chain: "main".to_string(),
            height: 3_420_400,
        };
        let mixnet_specific = PairedProbe {
            host: "carover0.xyz".to_string(),
            clearnet: ProbeLeg {
                outcome: Ok(tip.clone()),
                millis: 210,
            },
            mixnet: Some(ProbeLeg {
                outcome: Err(NetOpFailure {
                    stage: NetOpStage::SocksHandshake,
                    target: "carover0.xyz".to_string(),
                    cause_chain: vec![
                        "the mixnet exit could not reach carover0.xyz:9067 (timed out after 20.0s)"
                            .to_string(),
                    ],
                }),
                millis: 20_000,
            }),
        };
        assert_eq!(
            render_paired_probe(&mixnet_specific),
            "carover0.xyz\n  clearnet: ok in 210ms: chain main, height 3420400\n  mixnet:   FAILED after 20000ms: failed at socks-handshake to carover0.xyz: the mixnet exit could not reach carover0.xyz:9067 (timed out after 20.0s)"
        );

        let proxy_not_ready = PairedProbe {
            host: "zec.rocks".to_string(),
            clearnet: ProbeLeg {
                outcome: Ok(tip),
                millis: 180,
            },
            mixnet: None,
        };
        assert_eq!(
            render_paired_probe(&proxy_not_ready),
            "zec.rocks\n  clearnet: ok in 180ms: chain main, height 3420400\n  mixnet:   skipped (mixnet proxy not ready)"
        );
    }

    /// HYPOTHESIS: the history rendering aggregates per host and route with
    /// the most recent outcome and its age. Falsified if counts mix routes
    /// or the last outcome reflects file order rather than timestamps.
    #[cfg(all(feature = "nym", feature = "nym-diary"))]
    #[test]
    fn history_aggregates_per_host_and_route() {
        use zingolib::lightclient::indexer_history::{
            AttemptKind, AttemptRoute, FailureKind, IndexerAttempt,
        };

        let attempt = |host: &str, route, unix_secs, outcome| IndexerAttempt {
            unix_secs,
            host: host.to_string(),
            route,
            kind: AttemptKind::Send,
            millis: 10,
            outcome,
        };
        let tunnel = Err(FailureKind::Unreachable);
        let attempts = vec![
            attempt("zec.rocks", AttemptRoute::Mixnet, 1_000, tunnel),
            attempt("zec.rocks", AttemptRoute::Mixnet, 2_000, Ok(())),
            attempt("zec.rocks", AttemptRoute::Clearnet, 1_500, Ok(())),
            attempt("carover0.xyz", AttemptRoute::Mixnet, 1_800, tunnel),
        ];

        assert_eq!(
            render_history(&attempts, 2_060),
            "Indexer history (all sessions):\n  \
             carover0.xyz: mixnet 0/1 ok, last failed 4m ago\n  \
             zec.rocks: clearnet 1/1 ok, last ok 9m ago; mixnet 1/2 ok, last ok 1m ago"
        );
        assert_eq!(render_history(&[], 0), "No indexer history recorded yet.");
    }

    #[cfg(not(feature = "nym"))]
    #[test]
    fn feature_absent_renders_byte_identically_to_the_replaced_string() {
        assert_eq!(
            NymCommandError::FeatureAbsent.to_string(),
            "This build has no Nym mixnet support. Rebuild zingo-cli with `--features nym`."
        );
    }

    /// Pins the `nym status` mode strings via the pure renderer.
    #[cfg(feature = "nym")]
    #[test]
    fn status_lines_render_byte_identically_to_the_replaced_strings() {
        use zingolib::nym::MixnetMode;

        assert_eq!(
            render_status(MixnetMode::Unattached, None, None),
            "Mixnet Mode: unattached. The mixnet has not been enabled, and no consent to \
             clearnet has been given: send and price-fetch refuse. Run `nym on` to enable \
             the mixnet, or `nym off` to use clearnet.",
            "absence is not consent: unattached names refusal, never clearnet"
        );
        assert_eq!(
            render_status(MixnetMode::SwitchedOff, None, None),
            "Mixnet Mode: switched off (send and price-fetch use clearnet)"
        );
        assert_eq!(
            render_status(MixnetMode::Bootstrapping, None, None),
            "Mixnet Mode: bootstrapping (send and price-fetch are unavailable until ready)"
        );
        assert_eq!(
            render_status(MixnetMode::Ready, Some("127.0.0.1:43210"), None),
            "Mixnet Mode: ready (SOCKS5 127.0.0.1:43210)"
        );
        assert_eq!(
            render_status(MixnetMode::Ready, None, None),
            "Mixnet Mode: ready",
            "ready with no address yet still renders (the route resolver, \
             not the renderer, refuses that state)"
        );
        assert_eq!(
            render_status(MixnetMode::Died, None, None),
            "Mixnet Mode: died. The proxy exited unexpectedly. Send and price-fetch \
             refuse and will not fall back to clearnet. Run `nym on` to restart the proxy.",
            "a died proxy is reported distinctly from switched off, and tells the user how to \
             recover"
        );
    }

    /// HYPOTHESIS: live bootstrap progress reaches the `nym status` line, so
    /// the connect race is narrated rather than an opaque wait. Falsified if
    /// the detail is dropped by the renderer. The detail is shown only while
    /// bootstrapping: a ready proxy has no bootstrap left to narrate.
    #[cfg(feature = "nym")]
    #[test]
    fn bootstrap_detail_reaches_the_status_line_only_while_bootstrapping() {
        use zingolib::nym::MixnetMode;

        assert_eq!(
            render_status(
                MixnetMode::Bootstrapping,
                None,
                Some("attempt 2/10: 2 in flight, 0 failed")
            ),
            "Mixnet Mode: bootstrapping, attempt 2/10: 2 in flight, 0 failed \
             (send and price-fetch are unavailable until ready)"
        );
        assert_eq!(
            render_status(MixnetMode::Ready, Some("127.0.0.1:1"), Some("stale")),
            "Mixnet Mode: ready (SOCKS5 127.0.0.1:1)",
            "a stale detail must not leak into the ready line"
        );
    }

    /// HYPOTHESIS: `nym status` always carries the IP-correlation disclaimer in
    /// every mode, so a "ready" mixnet is never mistaken for end-to-end IP
    /// protection while synchronization stays on clearnet (ZIP-0318). The mode
    /// line is preserved verbatim as the first line. Falsified if the
    /// disclaimer is dropped in any mode, no longer leads with the mode line,
    /// or omits the sync/IP/indexer/balance risk it must name.
    #[cfg(feature = "nym")]
    #[test]
    fn status_always_carries_the_ip_correlation_disclaimer() {
        use zingolib::nym::MixnetMode;

        for mode in [
            MixnetMode::Unattached,
            MixnetMode::SwitchedOff,
            MixnetMode::Bootstrapping,
            MixnetMode::Ready,
            MixnetMode::Died,
        ] {
            let addr = Some("127.0.0.1:43210");
            let out = render_status_with_disclaimer(mode, addr, None);
            assert!(
                out.starts_with(&render_status(mode, addr, None)),
                "the mode line must lead the status output: {out}"
            );
            for phrase in [
                "IP-correlation risk",
                "synchronization",
                "sync indexer",
                "total balance",
                "ZIP-0318",
            ] {
                assert!(
                    out.contains(phrase),
                    "the disclaimer must name {phrase:?} in mode {mode:?}: {out}"
                );
            }
        }
    }
}

#[cfg(test)]
mod offline_contract {
    //! The Offline-mode contract at the command surface (issue #2286,
    //! ADR 0006, ADR 0025). Every client here is genuinely Indexerless —
    //! built by [`LightClient::new_for_test`] from a synthetic wallet, with
    //! no server configured and no network object constructed — so these
    //! tests run with zero traffic and prove two halves of one contract:
    //!
    //! - every wallet-local command completes offline, meaning connectivity
    //!   is never the obstacle (a domain failure such as "nothing to
    //!   shield" is legitimate; the Offline refusal is not); and
    //! - every connectivity-requiring command refuses offline with the one
    //!   typed refusal, [`zingolib::lightclient::error::LightClientError::Offline`],
    //!   rendered through the command's own error channel — never a hang, a
    //!   panic, or a silent clearnet fallback.
    //!
    //! The `change_server` pin lives at the REPL dispatch, not here: see
    //! `offline_mode_refusal` and its tests in `crate::tests`.
    //!
    //! Deliberately untested, with the reasoning on record: `drain now`,
    //! `split now`, and `migration catchup` refuse at the transmit stage,
    //! whose pre-flight `transmit` and `quicksend` pin below (each extra
    //! case would buy another proving run, not another guarantee); `nym on`
    //! and `nym probe` currently carry NO offline gate (they would emit
    //! traffic from an Offline session — a known gap tracked for the ADR
    //! 0024 session driver), and the REPL-owned `servers` command likewise
    //! probes the network unguarded.

    use zingolib::lightclient::LightClient;
    use zingolib::testutils::synthetic_wallet::SyntheticWalletBuilder;

    use super::{CommandError, RT, get_commands};

    /// The Display of `LightClientError::Offline`: the single refusal every
    /// connectivity-requiring command must surface, and the string no
    /// offline-capable command may ever emit.
    const OFFLINE_REFUSAL: &str =
        "Offline: no indexer configured. Call set_indexer_uri() to connect.";

    /// An Indexerless client over an empty synthetic wallet.
    fn offline_client() -> LightClient {
        RT.block_on(LightClient::new_for_test(
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build(),
        ))
    }

    /// An Indexerless client whose wallet holds a spendable orchard note
    /// and a transparent coin, so proposing, planning, and shielding have
    /// material to work with.
    fn funded_offline_client() -> LightClient {
        RT.block_on(LightClient::new_for_test(
            SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
                .orchard_note(100_000_000)
                .transparent_coin(50_000_000)
                .build(),
        ))
    }

    fn exec(
        client: &mut LightClient,
        command: &str,
        args: &[&str],
    ) -> Result<String, CommandError> {
        get_commands()
            .get(command)
            .unwrap_or_else(|| panic!("command `{command}` is registered"))
            .exec(args, client)
    }

    /// Asserts `command` succeeds offline and returns its output.
    fn assert_works_offline(client: &mut LightClient, command: &str, args: &[&str]) -> String {
        let output = exec(client, command, args)
            .unwrap_or_else(|error| panic!("`{command}` must work offline: {error}"));
        assert!(
            !output.contains(OFFLINE_REFUSAL),
            "`{command}` must not surface the Offline refusal: {output}"
        );
        output
    }

    /// Asserts connectivity is never `command`'s obstacle: it may succeed
    /// or fail on domain grounds, but must not surface the Offline refusal.
    fn assert_unblocked_offline(client: &mut LightClient, command: &str, args: &[&str]) -> String {
        let rendered = match exec(client, command, args) {
            Ok(output) => output,
            Err(error) => error.to_string(),
        };
        assert!(
            !rendered.contains(OFFLINE_REFUSAL),
            "`{command}` must not be blocked by Offline mode: {rendered}"
        );
        rendered
    }

    /// Asserts `command` refuses offline through its `Err` channel with the
    /// typed Offline refusal.
    fn assert_refuses_offline_via_err(client: &mut LightClient, command: &str, args: &[&str]) {
        let error = exec(client, command, args).expect_err(command);
        assert!(
            error.to_string().contains(OFFLINE_REFUSAL),
            "`{command}` must refuse with the typed Offline error: {error}"
        );
    }

    /// Asserts `command` refuses offline through its rendered `error` field
    /// (the send family reports failure inside its JSON success string).
    fn assert_refuses_offline_in_output(client: &mut LightClient, command: &str, args: &[&str]) {
        let output = exec(client, command, args)
            .unwrap_or_else(|error| panic!("`{command}` renders refusals into output: {error}"));
        assert!(
            output.contains(OFFLINE_REFUSAL),
            "`{command}` must report the typed Offline refusal: {output}"
        );
    }

    /// A fresh unified address from the wallet itself, so send-family tests
    /// stay on the wallet's own chain.
    fn own_unified_address(client: &mut LightClient) -> String {
        let output = assert_works_offline(client, "new_address", &["o"]);
        let parsed = json::parse(&output).expect("new_address returns JSON");
        parsed["encoded_address"]
            .as_str()
            .expect("encoded_address is a string")
            .to_string()
    }

    mod works_offline {
        //! The offline-capable surface: every command here must complete
        //! against an Indexerless client.

        use super::*;

        #[test]
        fn version() {
            let output = assert_works_offline(&mut offline_client(), "version", &[]);
            assert!(!output.is_empty());
        }

        #[test]
        fn help() {
            let output = assert_works_offline(&mut offline_client(), "help", &[]);
            assert!(output.contains("Wallet commands"), "{output}");
        }

        #[test]
        fn parse_address_of_the_wallets_own() {
            let mut client = offline_client();
            let address = own_unified_address(&mut client);
            let output = assert_works_offline(&mut client, "parse_address", &[&address]);
            assert!(output.contains("success"), "{output}");
        }

        #[test]
        fn parse_viewkey_of_the_exported_ufvk() {
            let mut client = offline_client();
            let exported = assert_works_offline(&mut client, "export_ufvk", &[]);
            let ufvk = json::parse(&exported).expect("export_ufvk returns JSON")["ufvk"]
                .as_str()
                .expect("ufvk is a string")
                .to_string();
            let output = assert_works_offline(&mut client, "parse_viewkey", &[&ufvk]);
            assert!(output.contains("success"), "{output}");
        }

        #[test]
        fn addresses() {
            let output = assert_works_offline(&mut offline_client(), "addresses", &[]);
            json::parse(&output).expect("addresses returns JSON");
        }

        #[test]
        fn t_addresses() {
            let output = assert_works_offline(&mut offline_client(), "t_addresses", &[]);
            json::parse(&output).expect("t_addresses returns JSON");
        }

        #[test]
        fn new_address() {
            let output = assert_works_offline(&mut offline_client(), "new_address", &["oz"]);
            assert!(output.contains("encoded_address"), "{output}");
        }

        /// Funded, because the address-gap rule (a new transparent address
        /// only after the latest one received funds) is a domain rule that
        /// applies offline exactly as it does online.
        #[test]
        fn new_taddress() {
            let output = assert_works_offline(&mut funded_offline_client(), "new_taddress", &[]);
            assert!(output.contains("encoded_address"), "{output}");
        }

        #[test]
        fn balance() {
            let output = assert_works_offline(&mut funded_offline_client(), "balance", &[]);
            assert!(!output.is_empty());
        }

        #[test]
        fn spendable_balance() {
            let output =
                assert_works_offline(&mut funded_offline_client(), "spendable_balance", &[]);
            assert!(output.contains("spendable_balance"), "{output}");
        }

        #[test]
        fn max_send_value() {
            let mut client = funded_offline_client();
            let address = own_unified_address(&mut client);
            let output = assert_works_offline(&mut client, "max_send_value", &[&address]);
            assert!(output.contains("max_send_value"), "{output}");
        }

        #[test]
        fn birthday() {
            let output = assert_works_offline(&mut offline_client(), "birthday", &[]);
            assert!(!output.is_empty());
        }

        #[test]
        fn height_reports_the_wallets_own_view() {
            let output = assert_works_offline(&mut offline_client(), "height", &[]);
            assert!(output.contains("20"), "the synthetic tip is 20: {output}");
        }

        #[test]
        fn notes() {
            assert_works_offline(&mut funded_offline_client(), "notes", &[]);
        }

        #[test]
        fn coins() {
            assert_works_offline(&mut funded_offline_client(), "coins", &[]);
        }

        #[test]
        fn transactions() {
            assert_works_offline(&mut offline_client(), "transactions", &[]);
        }

        #[test]
        fn value_transfers() {
            assert_works_offline(&mut offline_client(), "value_transfers", &[]);
        }

        #[test]
        fn messages() {
            assert_works_offline(&mut offline_client(), "messages", &[]);
        }

        #[test]
        fn sends_to_address() {
            assert_works_offline(&mut offline_client(), "sends_to_address", &[]);
        }

        #[test]
        fn value_to_address() {
            assert_works_offline(&mut offline_client(), "value_to_address", &[]);
        }

        #[test]
        fn memobytes_to_address() {
            assert_works_offline(&mut offline_client(), "memobytes_to_address", &[]);
        }

        #[test]
        fn wallet_kind() {
            let output = assert_works_offline(&mut offline_client(), "wallet_kind", &[]);
            assert!(output.contains("mnemonic"), "{output}");
        }

        #[test]
        fn settings() {
            assert_unblocked_offline(&mut offline_client(), "settings", &[]);
        }

        #[test]
        fn recovery_info() {
            let output = assert_works_offline(&mut offline_client(), "recovery_info", &[]);
            assert!(!output.is_empty());
        }

        #[test]
        fn export_ufvk() {
            let output = assert_works_offline(&mut offline_client(), "export_ufvk", &[]);
            assert!(output.contains("ufvk"), "{output}");
        }

        #[test]
        fn check_address_recognizes_the_wallets_own() {
            let mut client = offline_client();
            let address = own_unified_address(&mut client);
            let output = assert_works_offline(&mut client, "check_address", &[&address]);
            assert!(output.contains("is_wallet_address"), "{output}");
        }

        #[test]
        fn clear() {
            let output = assert_works_offline(&mut offline_client(), "clear", &[]);
            assert!(output.contains("success"), "{output}");
        }

        /// `sync status` reads wallet state only; on a synthetic wallet it
        /// reports a wallet-shape error (no wallet blocks are fabricated),
        /// which is a domain outcome — connectivity is never the obstacle.
        #[test]
        fn sync_status() {
            assert_unblocked_offline(&mut offline_client(), "sync", &["status"]);
        }

        #[test]
        fn sync_poll() {
            let output = assert_works_offline(&mut offline_client(), "sync", &["poll"]);
            assert_eq!(output, "Sync task has not been launched.");
        }

        #[test]
        fn quit() {
            let output = assert_works_offline(&mut offline_client(), "quit", &[]);
            assert!(output.contains("quit successfully"), "{output}");
        }

        /// Proposing is an Indexerless capability (ADR 0006): `send` shows
        /// the fee offline, for a later `calculate`/`transmit`.
        #[test]
        fn send_proposes_offline() {
            let mut client = funded_offline_client();
            let address = own_unified_address(&mut client);
            let output = assert_works_offline(&mut client, "send", &[&address, "50000"]);
            assert!(output.contains("fee"), "{output}");
        }

        #[test]
        fn send_all_proposes_offline() {
            let mut client = funded_offline_client();
            let address = own_unified_address(&mut client);
            let output = assert_works_offline(&mut client, "send_all", &[&address]);
            assert!(output.contains("fee"), "{output}");
        }

        #[test]
        fn shield_proposes_offline() {
            let output = assert_works_offline(&mut funded_offline_client(), "shield", &[]);
            assert!(output.contains("fee"), "{output}");
        }

        /// Offline signing (ADR 0006): `calculate` signs the stored
        /// proposal with no Indexer, leaving Calculated transactions for a
        /// connected `transmit`.
        #[test]
        fn calculate_signs_offline() {
            let mut client = funded_offline_client();
            let address = own_unified_address(&mut client);
            assert_works_offline(&mut client, "send", &[&address, "50000"]);
            let output = assert_works_offline(&mut client, "calculate", &[]);
            assert!(output.contains("txids"), "{output}");
        }

        #[test]
        fn drain_plan() {
            let output = assert_works_offline(&mut funded_offline_client(), "drain", &["plan"]);
            assert!(output.contains("transactions"), "{output}");
        }

        #[test]
        fn split_plan() {
            let output = assert_works_offline(&mut funded_offline_client(), "split", &["plan"]);
            assert!(output.contains("split_rounds"), "{output}");
        }

        #[test]
        fn migration_plan() {
            let output = assert_works_offline(&mut funded_offline_client(), "migration", &["plan"]);
            assert!(output.contains("plan_hash"), "{output}");
        }

        #[test]
        fn migration_status() {
            let output =
                assert_works_offline(&mut funded_offline_client(), "migration", &["status"]);
            assert!(output.contains("phase"), "{output}");
        }

        #[test]
        fn migration_windows() {
            assert_unblocked_offline(&mut funded_offline_client(), "migration", &["windows"]);
        }

        /// `nym status` reads the wallet's mode: an offline session never
        /// bootstraps the mixnet, so a fresh client reports unattached.
        #[cfg(feature = "nym")]
        #[test]
        fn nym_status_reports_unattached() {
            let output = assert_works_offline(&mut offline_client(), "nym", &["status"]);
            assert!(output.contains("unattached"), "{output}");
        }

        #[test]
        fn remove_transaction_fails_on_the_txid_never_on_connectivity() {
            let unknown_txid = "ab".repeat(32);
            assert_unblocked_offline(
                &mut offline_client(),
                "remove_transaction",
                &[&unknown_txid],
            );
        }

        #[test]
        fn delete_fails_on_the_file_never_on_connectivity() {
            assert_unblocked_offline(&mut offline_client(), "delete", &[]);
        }
    }

    mod refuses_offline {
        //! The connectivity-requiring surface: every command here must
        //! refuse offline with the typed Offline error, through its own
        //! error channel.

        use super::*;

        #[test]
        fn sync_run() {
            assert_refuses_offline_via_err(&mut offline_client(), "sync", &["run"]);
        }

        #[test]
        fn rescan() {
            assert_refuses_offline_via_err(&mut offline_client(), "rescan", &[]);
        }

        /// `info` renders the typed failure at its presentation boundary:
        /// the output IS the refusal, byte for byte.
        #[test]
        fn info() {
            let output = exec(&mut offline_client(), "info", &[]).expect("info renders errors");
            assert_eq!(output, OFFLINE_REFUSAL);
        }

        /// `confirm` pre-flights the Indexer before touching the stored
        /// proposal, so the refusal needs no proposal and costs no proving.
        #[test]
        fn confirm() {
            assert_refuses_offline_in_output(&mut offline_client(), "confirm", &[]);
        }

        /// `transmit` pre-flights the Indexer before resolving txids, so
        /// even an unknown txid refuses on connectivity first.
        #[test]
        fn transmit() {
            let txid = "ab".repeat(32);
            assert_refuses_offline_in_output(&mut offline_client(), "transmit", &[&txid]);
        }

        /// `quicksend` proposes and signs offline (both Indexerless
        /// capabilities), then refuses at the transmit stage: the wallet
        /// does the work it can and leaks nothing.
        #[test]
        fn quicksend() {
            let mut client = funded_offline_client();
            let address = own_unified_address(&mut client);
            assert_refuses_offline_in_output(&mut client, "quicksend", &[&address, "50000"]);
        }

        /// `quickshield` mirrors `quicksend`: the shield proposal and its
        /// signing succeed offline, and the transmit stage refuses.
        #[test]
        fn quickshield() {
            assert_refuses_offline_in_output(&mut funded_offline_client(), "quickshield", &[]);
        }

        /// `migrate` syncs before building anything, so the refusal
        /// arrives from the sync pre-flight with no proving spent.
        #[test]
        fn migrate() {
            assert_refuses_offline_via_err(&mut funded_offline_client(), "migrate", &[]);
        }

        #[test]
        fn migration_continue() {
            assert_refuses_offline_via_err(
                &mut funded_offline_client(),
                "migration",
                &["continue"],
            );
        }

        #[test]
        fn migration_execute() {
            assert_refuses_offline_via_err(&mut funded_offline_client(), "migration", &["execute"]);
        }

        #[test]
        fn migration_auto() {
            assert_refuses_offline_via_err(&mut funded_offline_client(), "migration", &["auto"]);
        }

        /// `current_price` is mixnet-only (ADR 0011): an offline session
        /// never bootstraps the mixnet, so the fetch refuses from the
        /// unattached mode — a typed refusal, zero traffic.
        #[cfg(feature = "nym")]
        #[test]
        fn current_price_refuses_from_the_unattached_mixnet() {
            let output = exec(&mut offline_client(), "current_price", &[])
                .expect("current_price renders refusals");
            assert!(
                output.contains("error: the Nym mixnet is not enabled"),
                "{output}"
            );
        }

        /// Without the nym feature there is no price fetch at all: the
        /// command says so instead of touching the network.
        #[cfg(not(feature = "nym"))]
        #[test]
        fn current_price_has_no_fetch_to_offer() {
            let output = exec(&mut offline_client(), "current_price", &[])
                .expect("current_price explains the absent fetch");
            assert!(output.contains("no price fetch"), "{output}");
        }
    }
}
