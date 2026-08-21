//! `ZingoCli`, a command-line interface for the Zingo Zcash light wallet.
//!
//! This crate provides the library half of `zingo-cli`. It owns argument
//! parsing ([`build_clap_app`]), configuration assembly, wallet startup,
//! the interactive REPL, and single-command dispatch.
//!
//! The binary entry point (`main.rs`) is intentionally thin: it handles
//! process-level concerns (tracing, crypto-provider installation, error
//! reporting) and delegates to [`run_cli`], which builds a
//! [`LightClient`] and runs the command loop.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod commands;
mod examples;

// The retired clearnet server-selection sweep. Never compiled by default:
// the feature is the explicit review act (2026-08-06 ruling) that
// re-incorporates any of it.
#[cfg(feature = "clearnet-test-mode")]
mod server_select_clearnet;

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, Sender, channel};

use clap::{self, Arg};
use log::{error, info};
// The mixnet narration task is the only debug/warn logger; the imports
// follow the feature.
#[cfg(feature = "nym")]
use log::{debug, warn};

use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
use zingolib::config::{ChainType, ClientConfig, DEFAULT_WALLET_NAME, WalletConfig};
use zingolib::data::PollReport;
use zingolib::lightclient::{DEFAULT_REQUEST_TIMEOUT, LightClient};
use zingolib::netutils::Indexer as _;
use zingolib::wallet::WalletSettings;

use crate::commands::RT;

pub(crate) mod version;

/// Builds the clap `Command` definition for the CLI: the session's options
/// followed by every dispatchable command, so a one-shot command parses
/// into its typed form here, before any wallet work begins.
pub fn build_clap_app() -> clap::Command {
    use clap::Subcommand as _;

    let session_options = clap::Command::new("Zingo CLI").version(version::VERSION)
            .disable_help_subcommand(true)
            .arg(Arg::new("nosync")
                .help("By default, zingo-cli will sync the wallet at startup. Pass --nosync to prevent the automatic sync at startup.")
                .long("nosync")
                .short('n')
                .action(clap::ArgAction::SetTrue))
            .arg(Arg::new("waitsync")
                .help("Block execution of the specified command until the background sync completes. Has no effect if --nosync is set.")
                .long("waitsync")
                .action(clap::ArgAction::SetTrue))
            .arg(Arg::new("chain")
                .long("chain").short('c')
                .value_name("CHAIN")
                .help(
                    r#"What chain to expect. One of "mainnet", "testnet", or "regtest". Defaults to "mainnet""#
                ))
            .arg(Arg::new("seed")
                .short('s')
                .long("seed")
                .value_name("SEED PHRASE")
                .value_parser(parse_seed)
                .help("Create a new wallet with the given 24-word seed phrase. Will fail if wallet already exists"))
            .arg(Arg::new("viewkey")
                .long("viewkey")
                .value_name("UFVK")
                .value_parser(parse_ufvk)
                .help("Create a new wallet with the given encoded unified full viewing key. Will fail if wallet already exists"))
            .arg(Arg::new("birthday")
                .long("birthday")
                .value_name("birthday")
                .value_parser(clap::value_parser!(u32))
                .help("Specify wallet birthday when restoring from seed. This is the earliest block height where the wallet has a transaction. \
For a NEW wallet created in Offline mode it is instead an optional override of the library's built-in birthday floor."))
            .arg(Arg::new("server")
                .long("server")
                .value_name("server")
                .help("Pin a specific Indexer server. Without it, an online session's \
Server-Selection Sweep selects the sync indexer.")
                .value_parser(parse_uri))
            .arg(Arg::new("offline")
                .long("offline")
                .action(clap::ArgAction::SetTrue)
                .conflicts_with_all(["server", "waitsync"])
                .help("Run the session in Offline mode: no Indexer is ever configured. Local operations (addresses, balances, history, proposing) work; sync, transmission, and server commands are unavailable."))
            .arg(Arg::new("online")
                .long("online")
                .action(clap::ArgAction::SetTrue)
                .conflicts_with("offline")
                .help("Consent to go online this session (Connectivity Consent). First boot is offline by design; this flag, --remember-online, an explicit --server, or a stored standing consent takes a session online. The choice is not persisted."))
            .arg(Arg::new("remember-online")
                .long("remember-online")
                .action(clap::ArgAction::SetTrue)
                .conflicts_with_all(["offline", "forget-online"])
                .help("Consent to go online this session AND store the choice beside the wallet, so future sessions attach to the network automatically. Undo with --forget-online."))
            .arg(Arg::new("forget-online")
                .long("forget-online")
                .action(clap::ArgAction::SetTrue)
                .help("Remove the stored standing Connectivity Consent before deciding this session's connectivity. Without another consent act the session then runs offline."))
            .arg(Arg::new("nym-proxy")
                .long("nym-proxy")
                .value_name("PATH")
                .help("Path to the nym-proxy binary spawned for Mixnet Mode. Without it: $ZINGO_NYM_PROXY, then a nym-proxy bundled beside this binary, then `nym-proxy` on PATH. Used only with the `nym` build feature."))
            .arg(Arg::new("indexer-diary")
                .long("indexer-diary")
                .action(clap::ArgAction::SetTrue)
                .help("Record per-indexer send and probe outcomes for this session to indexer-history.tsv beside the wallet (view with `network history`). The diary stores hosts, timings, and a failure category, never server text, and is capped. Requires the `nym-diary` build feature. The choice is never persisted."))
            .arg(Arg::new("data-dir")
                .long("data-dir")
                .value_name("data-dir")
                .help("Absolute path to use as data directory"))
            .arg(Arg::new("log-file")
                .long("log-file")
                .value_name("PATH")
                .help("Path to the log file for interactive mode. Defaults to .zingo-cli/cli.log"));
    commands::CliCommand::augment_subcommands(session_options)
        .about(
            "A command-line light wallet for Zcash. Runs the given command and exits, or \
             starts the interactive prompt when given none.",
        )
        .long_about(None)
}

/// A session option placed after the command, and the corrected invocation.
/// Session options configure the whole session, so clap accepts them only
/// before the command; placed after, clap rejects them with an opaque
/// "unexpected argument". This detector names the misplacement and the fix
/// instead, deriving both the option set and the command set from
/// [`build_clap_app`] so neither list drifts.
///
/// Returns `None` when `args` (the process arguments including `argv[0]`) put
/// every session option ahead of the command, or name no command at all.
pub fn misplaced_session_option(args: &[String]) -> Option<String> {
    let app = build_clap_app();
    let commands: std::collections::HashSet<String> = app
        .get_subcommands()
        .flat_map(|c| std::iter::once(c.get_name().to_string()))
        .collect();
    let long_options: std::collections::HashSet<String> = app
        .get_arguments()
        .filter_map(|a| a.get_long().map(str::to_string))
        .collect();
    let short_options: std::collections::HashSet<char> = app
        .get_arguments()
        .filter_map(clap::Arg::get_short)
        .collect();

    // Everything after the command token is the command's own; scan it for a
    // token that names a session option. The `--` marker ends option parsing,
    // so nothing after it is a misplaced option.
    let mut after_command = args
        .iter()
        .skip(1)
        .skip_while(|token| !commands.contains(token.as_str()))
        .skip(1)
        .take_while(|token| token.as_str() != "--");

    let corrected = |option: &str| {
        format!(
            "`{option}` is a session option and must come before the command.\n       \
             try:  zingo-cli {option} {rest}",
            rest = args
                .iter()
                .skip(1)
                .filter(|t| t.as_str() != option)
                .cloned()
                .collect::<Vec<_>>()
                .join(" ")
        )
    };

    after_command.find_map(|token| {
        if let Some(long) = token.strip_prefix("--") {
            // A `--flag=value` form carries the name before the `=`.
            let name = long.split('=').next().unwrap_or(long);
            long_options
                .contains(name)
                .then(|| corrected(&format!("--{name}")))
        } else if let Some(shorts) = token.strip_prefix('-').filter(|s| !s.is_empty()) {
            // A single short session flag; clustered shorts are not session
            // options, so only the lone form is diagnosed.
            let mut chars = shorts.chars();
            let first = chars.next().expect("non-empty after the dash");
            (chars.next().is_none() && short_options.contains(&first))
                .then(|| corrected(&format!("-{first}")))
        } else {
            None
        }
    })
}

/// Custom function to parse a string into an `http::Uri`
fn parse_uri(s: &str) -> Result<http::Uri, String> {
    s.parse::<http::Uri>().map_err(|e| e.to_string())
}
/// Custom function to parse a string into a compliant ZIP32/BIP39 mnemonic phrase
/// currently this is just a whitespace delimited string of 24 words.  I am
/// poking around to use the actual BIP39 parser (presumably from librustzcash).
fn parse_seed(s: &str) -> Result<String, String> {
    match s.parse::<String>() {
        Ok(s) => {
            let count = s.split_whitespace().count();
            if [12, 15, 18, 21, 24].contains(&count) {
                Ok(s)
            } else {
                Err(format!(
                    "Expected 12/15/18/21/24 words, but received: {count}."
                ))
            }
        }
        Err(_) => Err("Unexpected failure to parse String!!".to_string()),
    }
}
/// Parse encoded UFVK to String and check for whitespaces
fn parse_ufvk(s: &str) -> Result<String, String> {
    match s.parse::<String>() {
        Ok(s) => {
            let count = s.split_whitespace().count();
            if count == 1 {
                Ok(s)
            } else {
                Err("Encoded UFVK should not contain whitespace!".to_string())
            }
        }
        Err(_) => Err("Unexpected failure to parse String!!".to_string()),
    }
}

/// Performs the per-prompt housekeeping, awaited on the command-loop
/// thread where the [`LightClient`] lives: polls the sync task, reports
/// any save-task failure, and returns the sync indicator to embed in the
/// interactive prompt: `" [Syncing X / Y outputs]"` while sync is in
/// progress, `" [Synced X / X outputs]"` when fully synced,
/// `" [Sync error]"` on failure, or `" [Sync stopped at X / Y outputs]"`
/// when no sync task is running and the wallet is not fully synced.
///
/// Every outcome is classified from typed values ([`PollReport`],
/// [`pepper_sync::sync_status`], `check_save_error`), never by inspecting
/// a command's output string. Every line this function prints is
/// narration, so all of it goes to stderr (ADR 0031).
async fn prompt_indicator(lightclient: &mut LightClient) -> String {
    let indicator = match lightclient.poll_sync() {
        PollReport::Ready(Err(e)) => {
            // The doubled "Sync error: Error:" is deliberate: it reproduces
            // the historical output byte for byte, where the polled command
            // string (itself prefixed "Error:") was interpolated after
            // "Sync error: ".
            eprintln!(
                "Sync error: Error: {}\nPlease restart sync with `sync run`.",
                commands::render_error_chain(&e)
            );
            " [Sync error]".to_string()
        }
        PollReport::Ready(Ok(sync_result)) => {
            eprintln!("{sync_result}");
            synced_indicator(scan_progress(lightclient).await)
        }
        PollReport::NotReady => syncing_indicator(scan_progress(lightclient).await),
        PollReport::NoHandle => idle_indicator(scan_progress(lightclient).await),
    };
    if let Err(e) = lightclient.check_save_error().await {
        eprintln!("Error: save failed. {e}\nRestarting save task...");
    }
    indicator
}

/// Waits on the loop thread for the launched sync task to finish, narrating
/// scan progress on stderr at the standard heartbeat cadence, and returns
/// the sync result's rendering.
async fn await_sync_narrated(lightclient: &mut LightClient) -> Result<String, String> {
    let mut interval = tokio::time::interval(zingolib::netutils::time::PROGRESS_HEARTBEAT_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        match lightclient.poll_sync() {
            PollReport::NoHandle => {
                return Err("Error: no sync task is running to wait for".to_string());
            }
            PollReport::NotReady => {
                eprintln!(
                    "sync:{}",
                    syncing_indicator(scan_progress(lightclient).await)
                );
            }
            PollReport::Ready(result) => {
                return result
                    .map(|sync_result| sync_result.to_string())
                    .map_err(|e| format!("Error: {}", commands::render_error_chain(&e)));
            }
        }
    }
}

/// The wallet's scan progress: the exact integer ratio of outputs scanned,
/// and whether sync is complete. No floating-point representation appears
/// anywhere in the prompt's reporting.
struct ScanProgress {
    outputs_scanned: u64,
    total_outputs: u64,
    complete: bool,
}

/// Reads the scan progress from the sync engine's progress channel while sync runs, from the wallet otherwise, or `None` if sync status is unavailable.
async fn scan_progress(lightclient: &LightClient) -> Option<ScanProgress> {
    // FIXME: handle sync status error and remove unwrap
    let status = lightclient.sync_status()?;

    Some(ScanProgress {
        outputs_scanned: status.total_outputs_scanned,
        total_outputs: status.total_outputs,
        complete: status.is_complete(),
    })
}

/// Formats a prompt indicator: `" [{labeled} X / Y outputs]"` when the
/// output ratio is known, `" [{bare}]"` otherwise (status unavailable, or
/// an output-free scan range where the ratio is vacuously 0 / 0).
fn ratio_indicator(labeled: &str, bare: &str, progress: Option<ScanProgress>) -> String {
    match progress {
        Some(progress) if progress.total_outputs > 0 => format!(
            " [{labeled} {} / {} outputs]",
            progress.outputs_scanned, progress.total_outputs
        ),
        _ => format!(" [{bare}]"),
    }
}

/// The prompt indicator while a sync task is running.
fn syncing_indicator(progress: Option<ScanProgress>) -> String {
    ratio_indicator("Syncing", "Syncing", progress)
}

/// The prompt indicator when no sync task is running.
fn idle_indicator(progress: Option<ScanProgress>) -> String {
    match progress {
        Some(progress) if progress.complete => synced_indicator(Some(progress)),
        _ => ratio_indicator("Sync stopped at", "Sync stopped", progress),
    }
}

/// The prompt indicator when sync is complete, reporting the full ratio.
fn synced_indicator(progress: Option<ScanProgress>) -> String {
    ratio_indicator("Synced", "Synced", progress)
}

/// Formats the configured indexer for the `servers` command; the session
/// probes nothing to answer it.
#[cfg(not(feature = "clearnet-test-mode"))]
fn format_ranked_servers(cli_config: &ConfigTemplate) -> String {
    match &cli_config.server {
        Some(server) => format!("Configured indexer: {server}. Nothing was probed."),
        None => "Configured indexer: none. This session is offline and probes nothing.".to_string(),
    }
}

/// Formats the ranked server list for display by the `servers` command.
#[cfg(feature = "clearnet-test-mode")]
fn format_ranked_servers(cli_config: &ConfigTemplate) -> String {
    let Some(server) = &cli_config.server else {
        return "Last Known servers: none. This session is offline and probes nothing.".to_string();
    };
    if cli_config.ranked_servers.is_empty() {
        return format!("Server was set explicitly: {server}\nNo other servers were probed.");
    }
    let mut out = String::from("Last Known server ranking, from this session's launch probe:\n");
    for (i, r) in cli_config.ranked_servers.iter().enumerate() {
        let marker = if r.uri == *server { " (active)" } else { "" };
        out.push_str(&format!(
            "  {:>2}. {} {:>8.1}ms{}\n",
            i + 1,
            r.uri,
            r.latency.as_secs_f64() * 1000.0,
            marker,
        ));
    }
    out
}

fn start_interactive(cli_config: &ConfigTemplate, ch: CommandChannel) {
    // `()` can be used when no completer is required
    let mut rl = rustyline::DefaultEditor::new().expect("Default rustyline Editor not creatable!");

    log::debug!("Ready!");

    let send_request = |request: Request| -> Result<String, String> {
        let description = match &request {
            Request::Command(command) => command.name(),
            Request::PromptIndicator => "prompt indicator".to_string(),
            Request::AwaitSync => "await sync".to_string(),
        };
        if ch.transmitter.send(request).is_err() {
            let e = format!("Error executing command {description}: the command loop has exited");
            eprintln!("{e}");
            error!("{e}");
            return Err(e);
        }
        match ch.receiver.recv() {
            Ok(response) => response,
            Err(e) => {
                let e = format!("Error executing command {description}: {e}");
                eprintln!("{e}");
                error!("{e}");
                Err(e)
            }
        }
    };
    // The prompt's chain label comes from local config, not the server. An
    // `info` round trip here blocked the first prompt behind the cold mixnet
    // tunnel (up to MIXNET_ROUND_TRIP_BOUND), and an offline session got a
    // refusal instead of a name, leaving the prompt's parens empty.
    let chain_name = match cli_config.chaintype {
        ChainType::Mainnet => "main",
        ChainType::Testnet => "test",
        ChainType::Regtest(_) => "regtest",
    };

    loop {
        // Read the height first
        let height = send_request(Request::Command(commands::CliCommand::Height))
            .ok()
            .and_then(|s| json::parse(&s).ok())
            .and_then(|v| v["height"].as_i64())
            .unwrap_or(0);

        let sync_indicator = send_request(Request::PromptIndicator).unwrap_or_default();

        let readline = rl.readline(&format!(
            "({chain_name}) Block:{height}{sync_indicator} >> "
        ));
        match readline {
            Ok(line) => {
                rl.add_history_entry(line.as_str())
                    .expect("Ability to add history entry");
                // Parse command line arguments
                let Ok(tokens) = shellwords::split(&line) else {
                    println!("Mismatched Quotes");
                    continue;
                };

                if tokens.is_empty() {
                    continue;
                }

                let command = match commands::parse_command_tokens(&tokens) {
                    Ok(command) => command,
                    Err(rendered) => {
                        eprintln!("{rendered}");
                        continue;
                    }
                };

                // CLI-only commands that don't need the LightClient.
                if matches!(command, commands::CliCommand::Servers) {
                    println!("{}", format_ranked_servers(cli_config));
                    continue;
                }

                // Special check for Quit command.
                let is_quit = matches!(command, commands::CliCommand::Quit);

                match send_request(Request::Command(command)) {
                    Ok(output) => println!("{output}"),
                    Err(rendered) => eprintln!("{rendered}"),
                }

                if is_quit {
                    break;
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                println!("CTRL-C");
                info!("CTRL-C");
                break;
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("CTRL-D");
                info!("CTRL-D");
                break;
            }
            Err(err) => {
                println!("Error: {err:?}");
                break;
            }
        }
    }
}

/// A request to the background command loop.
///
/// The variant, rather than the content of the response string, tells the
/// requester how to interpret the reply, so no consumer ever classifies
/// a response by sniffing its text (the in-band-error problem of issue
/// zingolabs/zingolib#2446).
enum Request {
    /// Execute a user command, already parsed by the sender, replying `Ok`
    /// with the command's output or `Err` with the rendered error line.
    Command(commands::CliCommand),
    /// Perform the per-prompt housekeeping (sync poll, save check) via
    /// typed calls on the loop thread. The reply is the sync indicator
    /// to embed in the interactive prompt.
    PromptIndicator,
    /// Block on the loop thread until the launched sync task finishes,
    /// narrating scan progress on stderr, and reply with the sync result.
    /// Sent by the one-shot path so `sync run` means sync to completion.
    AwaitSync,
}

/// A paired request/response channel for communicating with the background
/// command loop.
///
/// A response is `Ok` with the command's result, or `Err` with a fully
/// rendered error line that already begins with `Error: `. The requester
/// therefore learns of a failure from the variant, and never by reading
/// the text (ADR 0031).
struct CommandChannel {
    transmitter: Sender<Request>,
    receiver: Receiver<Result<String, String>>,
}

/// Spawns a background thread that executes each parsed-command message
/// against the [`LightClient`] and replies through the returned
/// [`CommandChannel`], exiting on [`commands::CliCommand::Quit`].
#[allow(clippy::disallowed_methods)]
pub(crate) fn command_loop(
    mut lightclient: LightClient,
    communication_mode: CommunicationMode,
) -> CommandChannel {
    let (command_transmitter, command_receiver) = channel::<Request>();
    let (resp_transmitter, resp_receiver) = channel::<Result<String, String>>();

    std::thread::spawn(move || {
        while let Ok(request) = command_receiver.recv() {
            let command = match request {
                Request::Command(command) => command,
                Request::PromptIndicator => {
                    resp_transmitter
                        .send(Ok(RT.block_on(prompt_indicator(&mut lightclient))))
                        .unwrap();
                    continue;
                }
                Request::AwaitSync => {
                    resp_transmitter
                        .send(RT.block_on(await_sync_narrated(&mut lightclient)))
                        .unwrap();
                    continue;
                }
            };
            // The Offline-mode pin reads the live client, not the launch
            // snapshot: `network on` may have granted consent mid-session,
            // and `network off` may have torn the session down to the
            // unconsented posture. A deliberate `--offline` never lifts.
            // Consent shows as a configured Indexer or a non-Unattached
            // mixnet slot, since a mixnet-only session binds no Indexer.
            let live_mode = match communication_mode {
                CommunicationMode::DeliberateOffline => CommunicationMode::DeliberateOffline,
                _ if lightclient.indexer_uri().is_some() => CommunicationMode::Online,
                #[cfg(feature = "nym")]
                _ if lightclient.mixnet_mode() != zingolib::mixnet::MixnetMode::Unattached => {
                    CommunicationMode::Online
                }
                _ => CommunicationMode::UnconsentedOffline,
            };
            // `help` renders the live posture's surface: what a suppressed
            // session does not offer, its help does not list.
            if let commands::CliCommand::Help { command } = &command {
                resp_transmitter
                    .send(Ok(commands::format_help(live_mode, command.as_deref())))
                    .unwrap();
                continue;
            }
            if let Some(refusal) = offline_mode_refusal(live_mode, &command) {
                resp_transmitter.send(Err(refusal)).unwrap();
                continue;
            }
            let is_quit = matches!(command, commands::CliCommand::Quit);

            let cmd_response = RT
                .block_on(commands::dispatch_parsed(command, &mut lightclient))
                .map_err(|e| format!("Error: {}", commands::render_error_chain(&e)));
            resp_transmitter.send(cmd_response).unwrap();

            if is_quit {
                info!("Quit");
                break;
            }
        }
    });

    CommandChannel {
        transmitter: command_transmitter,
        receiver: resp_receiver,
    }
}

/// The CLI operates in one of two mutually exclusive modes,
/// determined at the earliest possible moment from the parsed CLI arguments.
#[derive(Debug, PartialEq)]
enum ModeOfOperation {
    /// Start the interactive REPL.
    Interactive,
    /// Execute a single command and exit, the command arriving already
    /// parsed from the same grammar the REPL uses.
    Command {
        /// The parsed command to execute.
        command: commands::CliCommand,
    },
}

/// Determines the mode of operation from parsed CLI arguments:
/// [`ModeOfOperation::Command`] when a command is given, or
/// [`ModeOfOperation::Interactive`] when none is.
fn get_mode_of_operation(matches: &clap::ArgMatches) -> ModeOfOperation {
    use clap::FromArgMatches as _;

    commands::CliCommand::from_arg_matches(matches)
        .map_or(ModeOfOperation::Interactive, |command| {
            ModeOfOperation::Command { command }
        })
}

/// Whether the CLI communicates with a remote indexer or operates locally.
///
/// Selected at argument-parse time by the `--offline` flag and pinned for
/// the life of the session (Offline mode, issue #2286).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CommunicationMode {
    /// Connected to a remote indexer for sync, send, etc.
    Online,
    /// `--offline`: the deliberate zero-traffic session that no in-session
    /// act can lift; the whole network-requiring surface is suppressed.
    DeliberateOffline,
    /// No Connectivity Consent exists anywhere: the session runs offline
    /// and `network on` remains its in-session consent act.
    UnconsentedOffline,
}

/// The minted launch notice for a deliberate `--offline` session, naming
/// the only exit.
const DELIBERATE_OFFLINE_NOTICE: &str = "This session is deliberately offline (--offline): network-requiring \
     commands are unavailable, and nothing touches the network. The only \
     exit is to relaunch without --offline.";

/// The Offline-mode gate at the dispatch boundary: returns the fully
/// rendered refusal to send in place of executing a suppressed `command`,
/// naming the live remedy, or `None` when the command may proceed.
fn offline_mode_refusal(
    communication_mode: CommunicationMode,
    command: &commands::CliCommand,
) -> Option<String> {
    command
        .suppressed(communication_mode)
        .then(|| offline_refusal_text(communication_mode, &command.name()))
}

/// Renders the minted refusal for a suppressed command, naming the live
/// remedy the posture leaves open.
#[cfg(feature = "nym")]
fn offline_refusal_text(communication_mode: CommunicationMode, name: &str) -> String {
    match communication_mode {
        CommunicationMode::DeliberateOffline => format!(
            "Error: `{name}` requires network access, and --offline pins this session \
             offline. The only exit is to relaunch without --offline."
        ),
        _ => format!(
            "Error: `{name}` requires network access, and this session runs offline \
             without Connectivity Consent. Grant consent for this session with \
             `network on`, or relaunch with --online."
        ),
    }
}

/// Renders the minted refusal without the mixnet capability: Offline Mode
/// is the only mode such a build can be in, so the remedy is a rebuild.
#[cfg(not(feature = "nym"))]
fn offline_refusal_text(_communication_mode: CommunicationMode, name: &str) -> String {
    format!(
        "Error: `{name}` requires network access, and this build has no mixnet \
         capability, so Offline Mode is its only mode. Rebuild with default \
         features (plain `cargo build`, or `makers run-cli`) to go online."
    )
}

/// One session's connectivity verdict (ADR 0025): how the launch acts and
/// the stored standing choice combine. Pure, so the precedence is pinned
/// by unit tests without touching a filesystem. Exists only with the
/// mixnet capability: without it there is no verdict to reach, because
/// Offline Mode is the only mode (ADR 0026).
#[cfg(feature = "nym")]
#[derive(Debug, Clone, Copy, PartialEq)]
enum ConnectivityDecision {
    /// `--offline`: the deliberate zero-traffic session contract.
    DeliberateOffline,
    /// A consent act — this launch's or the stored standing choice —
    /// authorizes the connection; `store` additionally records the
    /// standing choice for future sessions.
    Online { store: bool },
    /// No consent exists anywhere: the session runs offline, and the
    /// first-boot notice names the acts that would take it online.
    UnconsentedOffline,
}

/// Combines the launch acts with the stored choice (ADR 0025): the
/// deliberate `--offline` wins over everything, a launch act (`--online`,
/// `--remember-online`, an explicit `--server`) over the store, the store
/// alone sustains the connection, and nothing else goes online.
#[cfg(feature = "nym")]
fn decide_connectivity(
    offline: bool,
    online: bool,
    remember_online: bool,
    explicit_server: bool,
    stored: zingolib::connectivity::ConnectivityConsent,
) -> ConnectivityDecision {
    if offline {
        return ConnectivityDecision::DeliberateOffline;
    }
    if remember_online {
        return ConnectivityDecision::Online { store: true };
    }
    if online || explicit_server {
        return ConnectivityDecision::Online { store: false };
    }
    match stored {
        zingolib::connectivity::ConnectivityConsent::StandingOnline => {
            ConnectivityDecision::Online { store: false }
        }
        zingolib::connectivity::ConnectivityConsent::Unrecorded => {
            ConnectivityDecision::UnconsentedOffline
        }
    }
}

/// The session's data directory: `--data-dir`, or the `wallets` directory
/// under the working directory. Shared by the Connectivity Consent record
/// and the wallet path, which must agree on where "beside the wallet" is.
fn data_dir_from(matches: &clap::ArgMatches) -> PathBuf {
    matches
        .get_one::<String>("data-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("wallets"))
}

/// Determines the communication mode from the parsed arguments and the
/// stored Connectivity Consent (ADR 0025), performing the acts' effects:
/// `--forget-online` removes the record before the decision,
/// `--remember-online` stores it, and a session with no consent anywhere
/// runs offline behind a notice naming the ways online.
#[cfg(feature = "nym")]
fn get_communication_mode(matches: &clap::ArgMatches) -> std::io::Result<CommunicationMode> {
    let data_dir = data_dir_from(matches);
    if matches.get_flag("forget-online") {
        zingolib::connectivity::forget_connectivity_consent(&data_dir)?;
        eprintln!("Standing Connectivity Consent forgotten; future sessions start offline again.");
    }
    let explicit_server =
        matches.value_source("server") == Some(clap::parser::ValueSource::CommandLine);
    let decision = decide_connectivity(
        matches.get_flag("offline"),
        matches.get_flag("online"),
        matches.get_flag("remember-online"),
        explicit_server,
        zingolib::connectivity::load_connectivity_consent(&data_dir),
    );
    Ok(match decision {
        ConnectivityDecision::DeliberateOffline => {
            eprintln!("{DELIBERATE_OFFLINE_NOTICE}");
            CommunicationMode::DeliberateOffline
        }
        ConnectivityDecision::Online { store } => {
            if store {
                zingolib::connectivity::store_standing_online(&data_dir)?;
                eprintln!(
                    "Standing Connectivity Consent stored in '{}'; future sessions attach \
                     to the network automatically. Undo with --forget-online.",
                    data_dir
                        .join(zingolib::connectivity::CONNECTIVITY_CONSENT_FILE)
                        .display()
                );
            }
            CommunicationMode::Online
        }
        ConnectivityDecision::UnconsentedOffline => {
            eprintln!(
                "No Connectivity Consent is recorded, so this session runs offline: local \
                 operations work and nothing touches the network. To go online, pass --online \
                 (this session only), --remember-online (store the choice for future \
                 sessions), or --server <uri>; in the session, `network on` also grants \
                 consent and switches to ONLINE MODE. Pass --offline to run offline \
                 deliberately and silence this notice."
            );
            CommunicationMode::UnconsentedOffline
        }
    })
}

/// The communication mode without the mixnet capability: Offline Mode is
/// the only mode such a build can be in (ADR 0026). The online consent
/// acts refuse loudly rather than silently degrade, and a stored standing
/// consent is reported as inert. `--forget-online` still works, so an
/// opt-out build can retire a stored consent.
#[cfg(not(feature = "nym"))]
fn get_communication_mode(matches: &clap::ArgMatches) -> std::io::Result<CommunicationMode> {
    let data_dir = data_dir_from(matches);
    if matches.get_flag("forget-online") {
        zingolib::connectivity::forget_connectivity_consent(&data_dir)?;
        eprintln!("Standing Connectivity Consent forgotten; future sessions start offline again.");
    }
    let explicit_server =
        matches.value_source("server") == Some(clap::parser::ValueSource::CommandLine);
    if matches.get_flag("online") || matches.get_flag("remember-online") || explicit_server {
        return Err(std::io::Error::other(
            "this build has no mixnet capability, so Offline Mode is its only mode; \
             going online is not possible. Rebuild with default features (plain \
             `cargo build`, or `makers run-cli`) to go online.",
        ));
    }
    if matches!(
        zingolib::connectivity::load_connectivity_consent(&data_dir),
        zingolib::connectivity::ConnectivityConsent::StandingOnline
    ) {
        eprintln!(
            "A standing Connectivity Consent is recorded, but this build has no mixnet \
             capability: Offline Mode is its only mode, so the session runs offline. \
             Rebuild with default features to honor the standing consent."
        );
    }
    Ok(if matches.get_flag("offline") {
        eprintln!("{DELIBERATE_OFFLINE_NOTICE}");
        CommunicationMode::DeliberateOffline
    } else {
        CommunicationMode::UnconsentedOffline
    })
}

/// All CLI-derived configuration needed to create a [`LightClient`] and
/// start the command loop.
///
/// Built by [`ConfigTemplate::fill`] from parsed [`clap::ArgMatches`],
/// then consumed by [`build_zingo_config`] and [`dispatch_command_or_start_interactive`].
#[derive(Debug)]
pub(crate) struct ConfigTemplate {
    mode: ModeOfOperation,
    communication_mode: CommunicationMode,
    /// The pinned Indexer: `None` when the session is Offline, and equally
    /// when it is Online unpinned, where the Server-Selection Sweep selects.
    server: Option<http::Uri>,
    /// True when `--server` was typed on the command line: the pin the
    /// Server-Selection Sweep surveys and never substitutes.
    #[cfg_attr(not(feature = "nym"), allow(dead_code))]
    server_pinned: bool,
    /// All servers that responded to `get_info()` during dynamic selection,
    /// sorted fastest to slowest. Empty if `--server` was specified explicitly.
    /// Will be used for automatic failover when sync fails.
    #[cfg(feature = "clearnet-test-mode")]
    #[allow(dead_code)]
    ranked_servers: Vec<server_select_clearnet::RankedServer>,
    seed: Option<String>,
    ufvk: Option<String>,
    birthday: u64,
    data_dir: PathBuf,
    sync: bool,
    waitsync: bool,
    chaintype: ChainType,
    /// `--nym-proxy`: an explicit path to the nym-proxy binary. Read only by
    /// the forced-on policy, which the `nym` feature gates.
    #[cfg_attr(not(feature = "nym"), allow(dead_code))]
    nym_proxy_path: Option<String>,
    /// `--indexer-diary`: opt this session in to recording the indexer diary.
    /// Effective only with the `nym-diary` build feature. Other builds warn.
    indexer_diary: bool,
}

/// A refusal to fill the launch configuration template.
#[derive(Debug, thiserror::Error)]
enum ConfigTemplateError {
    /// Both key sources arrived, and a wallet loads from exactly one.
    #[error("Cannot load a wallet from both seed phrase and viewkey!")]
    BothSeedAndViewkey,
    /// A key-provided load arrived without the wallet birthday.
    #[error(
        "This should be the block height where the wallet was created.\
If you don't remember the block height, you can pass '--birthday 0' to scan from the start of the blockchain."
    )]
    BirthdayRequired,
    /// The birthday token is not a block number.
    #[error("Couldn't parse birthday. This should be a block number. Error={0}")]
    BirthdayUnparseable(std::num::ParseIntError),
    /// `--online` grants a connection the named command never uses.
    #[error(
        "`{command}` needs no network, so `--online` grants a connection it never \
         uses. Drop `--online`, or run it at the interactive prompt."
    )]
    OnlineGrantUnused {
        /// The offline-capable command that was launched with `--online`.
        command: String,
    },
    /// The clearnet sweep resolved no server.
    #[cfg(feature = "clearnet-test-mode")]
    #[error(transparent)]
    ResolveServer(#[from] server_select_clearnet::ResolveServerError),
    /// The pinned `--server` is not a valid indexer URI.
    #[cfg(not(feature = "clearnet-test-mode"))]
    #[error("invalid --server URI.")]
    IndexerUri(#[from] http::uri::InvalidUri),
    /// The pinned server misses its scheme, host, or port.
    #[error(
        "Please provide the --server parameter as [scheme]://[host]:[port].\nYou provided: {server}"
    )]
    ServerShape {
        /// The under-specified server URI.
        server: http::Uri,
    },
    /// The chain name is not a known chain.
    #[error(transparent)]
    Chain(#[from] zingolib::config::InvalidChainType),
}

impl ConfigTemplate {
    fn fill(
        mode: ModeOfOperation,
        communication_mode: CommunicationMode,
        matches: clap::ArgMatches,
    ) -> Result<Self, ConfigTemplateError> {
        let seed = matches.get_one::<String>("seed").cloned();
        let ufvk = matches.get_one::<String>("viewkey").cloned();
        if seed.is_some() && ufvk.is_some() {
            return Err(ConfigTemplateError::BothSeedAndViewkey);
        }
        let maybe_birthday = matches
            .get_one::<u32>("birthday")
            .map(std::string::ToString::to_string);
        let from_provided = seed.is_some() || ufvk.is_some();
        if from_provided && maybe_birthday.is_none() {
            eprintln!("ERROR!");
            eprintln!(
                "Please specify the wallet birthday (eg. '--birthday 600000') to restore a wallet. (If you want to load the entire blockchain instead, you can use birthday 0. /this would require extensive time and computational resources)"
            );
            return Err(ConfigTemplateError::BirthdayRequired);
        }
        let birthday = match maybe_birthday.unwrap_or("0".to_string()).parse::<u64>() {
            Ok(b) => b,
            Err(e) => {
                return Err(ConfigTemplateError::BirthdayUnparseable(e));
            }
        };

        // A one-shot `--online <command>` grants a connection for that single
        // command, so the command must be one that uses it. An offline-capable
        // command after `--online` is a contradiction the launch refuses
        // early, before any network or wallet work.
        if matches.get_flag("online")
            && let ModeOfOperation::Command { command } = &mode
            && !command.requires_online()
        {
            return Err(ConfigTemplateError::OnlineGrantUnused {
                command: command.name().to_string(),
            });
        }

        let data_dir = data_dir_from(&matches);
        log::info!("data_dir: {}", data_dir.to_str().unwrap());
        // Offline mode never resolves a server: the session's contract is
        // that no Indexer is ever configured.
        #[cfg(feature = "clearnet-test-mode")]
        let (server, ranked_servers) = match communication_mode {
            CommunicationMode::DeliberateOffline | CommunicationMode::UnconsentedOffline => {
                (None, vec![])
            }
            CommunicationMode::Online => {
                let (server, ranked_servers) = server_select_clearnet::resolve_server(&matches)?;
                (Some(server), ranked_servers)
            }
        };
        // Without the quarantined sweep, resolution is pure: the typed
        // `--server` pin or nothing, never a probe and never a default.
        #[cfg(not(feature = "clearnet-test-mode"))]
        let server = match communication_mode {
            CommunicationMode::DeliberateOffline | CommunicationMode::UnconsentedOffline => None,
            CommunicationMode::Online => matches
                .get_one::<http::Uri>("server")
                .map(|server| {
                    zingolib::config::construct_indexer_uri(server.to_string())
                        .map_err(ConfigTemplateError::from)
                })
                .transpose()?,
        };
        if let Some(server) = &server {
            // Test to make sure the server has all of scheme, host and port
            if server.scheme_str().is_none() || server.host().is_none() || server.port().is_none() {
                return Err(ConfigTemplateError::ServerShape {
                    server: server.clone(),
                });
            }
        }
        let chaintype = if let Some(chain) = matches.get_one::<String>("chain") {
            ChainType::try_from(chain.as_str()).map_err(ConfigTemplateError::from)?
        } else {
            ChainType::Mainnet
        };

        let server_pinned =
            matches.value_source("server") == Some(clap::parser::ValueSource::CommandLine);
        let sync = !matches.get_flag("nosync") && communication_mode == CommunicationMode::Online;
        let waitsync = matches.get_flag("waitsync");
        let nym_proxy_path = matches.get_one::<String>("nym-proxy").cloned();
        let indexer_diary = matches.get_flag("indexer-diary");
        Ok(Self {
            mode,
            communication_mode,
            server,
            server_pinned,
            #[cfg(feature = "clearnet-test-mode")]
            ranked_servers,
            seed,
            ufvk,
            birthday,
            data_dir,
            sync,
            waitsync,
            chaintype,
            nym_proxy_path,
            indexer_diary,
        })
    }
}

/// Builds a `ClientConfig` from the filled config template.
///
/// This is the first testable seam inside the startup sequence. Its only
/// I/O is the chain-tip fetch that dates a brand-new wallet, which an
/// Offline-mode session never performs.
async fn build_zingo_config(filled_template: &ConfigTemplate) -> std::io::Result<ClientConfig> {
    let wallet_path = filled_template.data_dir.clone().join(DEFAULT_WALLET_NAME);
    let no_of_accounts = NonZeroU32::try_from(1).expect("hard-coded integer");
    let wallet_settings = WalletSettings {
        sync_config: SyncConfig {
            transparent_address_discovery: TransparentAddressDiscovery::minimal(),
            performance_level: PerformanceLevel::High,
            shutdown_on_completion: false,
        },
        min_confirmations: NonZeroU32::try_from(3).unwrap(),
    };

    let wallet_config = if let Some(seed_phrase) = filled_template.seed.clone() {
        // Create client from seed phrase
        WalletConfig::MnemonicPhrase {
            mnemonic_phrase: seed_phrase,
            no_of_accounts,
            birthday: filled_template.birthday as u32,
            wallet_settings,
        }
    } else if let Some(ufvk) = filled_template.ufvk.clone() {
        // Create client from UFVK
        WalletConfig::Ufvk {
            ufvk,
            birthday: filled_template.birthday as u32,
            wallet_settings,
        }
    } else if wallet_path.exists() {
        // Create client from wallet file
        WalletConfig::Read
    } else {
        // Create client from a new wallet
        eprintln!("Creating a new wallet");
        let chain_height = match filled_template.server.clone() {
            Some(server) => async move {
                zingolib::netutils::GrpcIndexer::new(server)
                    .await
                    .map_err(|e| format!("{e:?}"))?
                    .get_latest_block(DEFAULT_REQUEST_TIMEOUT)
                    .await
                    .map(|block_id| block_id.height as u32)
                    .map_err(|e| format!("{e:?}"))
            }
            .await
            .map_err(|e| std::io::Error::other(format!("Failed to create lightclient. {e}")))?,
            // Offline mode has no Indexer to ask for the chain tip; a
            // user-supplied birthday stands in for it, and absent that the
            // Library Birthday is a safe floor: a new seed cannot predate
            // the library that generated it.
            None => match u32::try_from(filled_template.birthday) {
                Ok(birthday) if birthday > 0 => birthday,
                _ => zingolib::config::lib_birthday(filled_template.chaintype),
            },
        };

        WalletConfig::NewSeed {
            no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
            chain_height,
            wallet_settings,
        }
    };

    let builder = ClientConfig::builder()
        .set_chain_type(filled_template.chaintype)
        .set_wallet_dir(filled_template.data_dir.clone())
        .set_wallet_config(wallet_config);
    // In Offline mode no Indexer URI is configured: the client starts (and
    // stays) Indexerless.
    let builder = match filled_template.server.clone() {
        Some(server) => builder.set_indexer_uri(server),
        None => builder,
    };

    builder
        .build()
        .map_err(|e| std::io::Error::other(e.to_string()))
}

#[allow(clippy::disallowed_methods)]
pub(crate) fn startup(filled_template: &ConfigTemplate) -> std::io::Result<CommandChannel> {
    let lightclient = RT.block_on(startup_async(filled_template))?;
    Ok(command_loop(
        lightclient,
        filled_template.communication_mode,
    ))
}

async fn startup_async(filled_template: &ConfigTemplate) -> std::io::Result<LightClient> {
    let config = build_zingo_config(filled_template).await?;

    let mut lightclient = LightClient::new(config, false)
        .await
        .map_err(|e| std::io::Error::other(format!("Failed to create lightclient. {e}")))?;

    if matches!(filled_template.mode, ModeOfOperation::Interactive) {
        // Print startup Messages
        info!(""); // Blank line
        info!("Starting Zingo-CLI");
        match &filled_template.server {
            Some(server) => info!("Lightclient connecting to {server}"),
            None if filled_template.communication_mode == CommunicationMode::Online => {
                info!("No pinned Indexer; the Server-Selection Sweep selects the sync indexer")
            }
            None => info!("Offline mode: no Indexer will be configured this session"),
        }
    }

    // The indexer diary is a per-session runtime opt-in on top of its build
    // gate: recording starts only when the user passes --indexer-diary, and
    // the choice is never persisted. A build without the feature warns loudly
    // instead of silently not recording, because failing safe is not recording.
    #[cfg(feature = "nym-diary")]
    if filled_template.indexer_diary {
        lightclient.set_indexer_diary(true);
        info!("Indexer diary: recording send and probe outcomes this session (`network history`).");
    }
    #[cfg(not(feature = "nym-diary"))]
    if filled_template.indexer_diary {
        eprintln!(
            "--indexer-diary has no effect: this build has no indexer diary support. \
             Rebuild zingo-cli with `--features nym-diary`."
        );
    }

    // The session driver call at the go-online moment (ADR 0024, decision
    // 2): zingolib owns the forced-on policy and the provisioning
    // precedence; this consumer supplies only its platform hints. The
    // mixnet is unconditional for a connected session — clearnet carries
    // sync alone (2026-08-06 ruling) — so a provisioning failure fails
    // closed: the session aborts rather than quietly transmitting over
    // clearnet. Offline sessions never transmit and skip the driver
    // entirely.
    #[cfg(feature = "nym")]
    if filled_template.communication_mode == CommunicationMode::Online {
        use zingolib::mixnet::{MixnetStartPolicy, ProvisionStrategy};
        lightclient
            .start_mixnet_session(
                ProvisionStrategy::Spawn(commands::spawn_hints(
                    filled_template.nym_proxy_path.as_deref(),
                )),
                MixnetStartPolicy::ForcedOn,
            )
            .await
            .map_err(|e| {
                std::io::Error::other(format!(
                    "Failed to start the Nym mixnet proxy: {}. Mixnet Mode is required for a \
                     connected session; install the nym-proxy binary, pass --nym-proxy <path>, \
                     or set $ZINGO_NYM_PROXY.",
                    commands::render_error_chain(&e),
                ))
            })?;
        info!(
            "Mixnet Mode enabling; the nym proxy is bootstrapping. Send and price-fetch \
             become available once it is ready (see `network status`)."
        );
        // Narrate Mixnet Mode transitions from the session's status
        // subscription (push, not poll): mode changes at info/warn through
        // the standard log path, bootstrap progress at debug. Rendering is
        // this consumer's own (ADR 0024, decision 8) — variant matching,
        // never prose matching.
        let mut status_rx = lightclient.subscribe_mixnet_status();
        tokio::spawn(async move {
            let mut last_mode = status_rx.borrow().mode;
            while status_rx.changed().await.is_ok() {
                let status = status_rx.borrow_and_update().clone();
                if status.mode == last_mode {
                    if let Some(detail) = &status.bootstrap_detail {
                        debug!("nym bootstrap: {detail}");
                    }
                    continue;
                }
                last_mode = status.mode;
                match status.mode {
                    zingolib::mixnet::MixnetMode::Ready => info!(
                        "Mixnet Mode ready; send and price-fetch route over the mixnet \
                         (see `network status`).{}",
                        commands::render_exit_nodes(&status.exits)
                    ),
                    zingolib::mixnet::MixnetMode::Died => {
                        let cause = status
                            .death
                            .as_ref()
                            .and_then(|death| death.detail.as_ref())
                            .map(|detail| format!(": {detail}"))
                            .unwrap_or_default();
                        warn!(
                            "The mixnet transport died{cause}. Send and price-fetch refuse \
                             until you re-enable it with `network on`."
                        );
                    }
                    other => info!("Mixnet Mode is now {other}."),
                }
            }
        });
    }

    // The sweep binds the session's indexer, so it runs for every online
    // session, not only a syncing one: a --nosync session still accepts
    // interactive sync and send, which need an indexer bound. A pinned
    // server is bound at config time and surveyed here; an unpinned online
    // session has no indexer until the sweep selects one.
    #[cfg(feature = "nym")]
    let indexer_ready = if filled_template.communication_mode == CommunicationMode::Online {
        sweep_select_sync_indexer(&mut lightclient, filled_template).await
    } else {
        true
    };
    #[cfg(not(feature = "nym"))]
    let indexer_ready = true;

    if filled_template.sync && indexer_ready {
        let sync_run = commands::CliCommand::Sync {
            sub: commands::SyncSubCommand::Run,
        };
        match commands::dispatch_parsed(sync_run, &mut lightclient).await {
            Ok(update) => eprintln!("{update}"),
            Err(e) => eprintln!("Error: {}", commands::render_error_chain(&e)),
        }
    }

    let save_run = commands::CliCommand::Save {
        sub: commands::SaveSubCommand::Run,
    };
    match commands::dispatch_parsed(save_run, &mut lightclient).await {
        Ok(update) => eprintln!("{update}"),
        Err(e) => eprintln!("Error: {}", commands::render_error_chain(&e)),
    }

    if filled_template.sync
        && indexer_ready
        && filled_template.waitsync
        && let Err(e) = lightclient.await_sync().await
    {
        eprintln!("error: {}", commands::render_error_chain(&e));
    }

    Ok(lightclient)
}

/// Maps the session chain to its census chain; `None` for regtest, whose
/// indexers the census does not carry.
#[cfg(feature = "nym")]
fn census_chain(chain: &ChainType) -> Option<zingolib::indexers::IndexerChain> {
    match chain {
        ChainType::Mainnet => Some(zingolib::indexers::IndexerChain::Main),
        ChainType::Testnet => Some(zingolib::indexers::IndexerChain::Test),
        ChainType::Regtest(_) => None,
    }
}

/// Runs the Server-Selection Sweep (ADR 0034), narrating each phase, and
/// binds its verdict as the sync indexer; returns whether this Sync Session
/// may open.
#[cfg(feature = "nym")]
async fn sweep_select_sync_indexer(
    lightclient: &mut LightClient,
    filled_template: &ConfigTemplate,
) -> bool {
    use zingolib::lightclient::select::SweepProgress;

    let Some(chain) = census_chain(&filled_template.chaintype) else {
        return true;
    };
    let pin = filled_template
        .server_pinned
        .then(|| filled_template.server.clone())
        .flatten();
    if let Some(pinned) = &pin
        && !zingolib::mixnet::probe::probe_eligible(pinned)
    {
        eprintln!(
            "Server-Selection Sweep: pinned server {pinned} is outside the mixnet exit policy \
             (https on port 443), so no sweep can survey it; binding it directly."
        );
        return true;
    }
    let mut candidates: Vec<http::Uri> = zingolib::indexers::mixnet_eligible(chain)
        .map(|indexer| indexer.uri.parse().expect("census URIs parse"))
        .collect();
    if let Some(pinned) = &pin
        && !candidates.contains(pinned)
    {
        candidates.push(pinned.clone());
    }
    let proxy_path = commands::resolve_proxy_path(filled_template.nym_proxy_path.as_deref());
    let selection = lightclient
        .run_server_selection_sweep(
            std::path::Path::new(&proxy_path),
            &candidates,
            pin.as_ref(),
            |phase| match phase {
                SweepProgress::TransportBootstrapping => eprintln!(
                    "Server-Selection Sweep: bootstrapping a dedicated sweep transport \
                     (its Exit Node is recycled when the sweep completes)..."
                ),
                SweepProgress::Surveying { candidates } => eprintln!(
                    "Server-Selection Sweep: surveying {candidates} candidates over the mixnet..."
                ),
                SweepProgress::Judging { answered, surveyed } => eprintln!(
                    "Server-Selection Sweep: {answered} of {surveyed} candidates answered; \
                     judging the live cohort..."
                ),
            },
        )
        .await;
    match selection {
        Ok(selection) => {
            let chosen = selection.sync_indexer.clone();
            match lightclient.set_indexer_uri(chosen.clone()).await {
                Ok(()) => {
                    eprintln!(
                        "Server-Selection Sweep: sync attaches to {chosen} (live cohort of {}, \
                         {} transmit candidates exclude its operator).",
                        selection.cohort.len(),
                        selection.transmit_candidates.len(),
                    );
                    true
                }
                Err(e) => {
                    eprintln!(
                        "Server-Selection Sweep: selected {chosen}, but binding it failed: {}. \
                         This Sync Session does not open.",
                        commands::render_error_chain(&e),
                    );
                    false
                }
            }
        }
        Err(e) => {
            eprintln!("{}", sweep_refusal_notice(&e));
            false
        }
    }
}

/// Composes the notice a refused Server-Selection Sweep prints, stating the
/// whole source chain of `error`.
#[cfg(feature = "nym")]
fn sweep_refusal_notice(error: &zingolib::lightclient::select::ServerSelectionError) -> String {
    format!(
        "Server-Selection Sweep: no sync indexer selected: {}. This Sync Session does \
         not open; the mixnet posture stands, and send and price-fetch continue.",
        commands::render_error_chain(error)
    )
}

/// Falls back to the prefix-only salvage reader when the user asked for
/// `recovery_info` but the wallet file cannot be fully parsed. The whole
/// point of that command is to escape a wallet no current build can read.
fn print_salvaged_recovery_info(
    cli_config: &ConfigTemplate,
    startup_error: &std::io::Error,
) -> std::io::Result<()> {
    let wallet_path = cli_config.data_dir.clone().join(DEFAULT_WALLET_NAME);
    let wallet_file = std::fs::File::open(&wallet_path)?;
    let recovery_info =
        zingolib::wallet::LightWallet::read_recovery_info(std::io::BufReader::new(wallet_file))
            .map_err(|salvage_error| {
                std::io::Error::other(format!(
                    "failed to load the wallet ({startup_error}), \
                     and salvaging recovery info from the file prefix also failed: \
                     {salvage_error}"
                ))
            })?;
    eprintln!(
        "The wallet file could not be fully loaded ({startup_error}); \
         showing recovery info salvaged from its prefix."
    );
    println!("{recovery_info}");
    Ok(())
}

fn dispatch_command_or_start_interactive(cli_config: &ConfigTemplate) -> std::io::Result<ExitCode> {
    let ch = match startup(cli_config) {
        Ok(ch) => ch,
        Err(startup_error) => {
            if matches!(
                &cli_config.mode,
                ModeOfOperation::Command {
                    command: commands::CliCommand::RecoveryInfo
                }
            ) {
                return print_salvaged_recovery_info(cli_config, &startup_error)
                    .map(|()| ExitCode::SUCCESS);
            }
            return Err(startup_error);
        }
    };
    match &cli_config.mode {
        ModeOfOperation::Interactive => {
            start_interactive(cli_config, ch);
            Ok(ExitCode::SUCCESS)
        }
        ModeOfOperation::Command { command } => {
            let description = command.name();
            let mut succeeded = if ch
                .transmitter
                .send(Request::Command(command.clone()))
                .is_err()
            {
                let e =
                    format!("Error executing command {description}: the command loop has exited");
                eprintln!("{e}");
                error!("{e}");
                false
            } else {
                match ch.receiver.recv() {
                    Ok(Ok(output)) => {
                        println!("{output}");
                        true
                    }
                    Ok(Err(rendered)) => {
                        eprintln!("{rendered}");
                        false
                    }
                    Err(e) => {
                        let e = format!("Error executing command {description}: {e}");
                        eprintln!("{e}");
                        error!("{e}");
                        false
                    }
                }
            };

            // A one-shot `sync run` means sync to completion: the session
            // holds open, narrating progress, until the sync task reports
            // its result, which becomes the command's outcome.
            if succeeded
                && matches!(
                    &command,
                    commands::CliCommand::Sync {
                        sub: commands::SyncSubCommand::Run
                    }
                )
            {
                succeeded = if ch.transmitter.send(Request::AwaitSync).is_err() {
                    eprintln!("Error awaiting sync: the command loop has exited");
                    false
                } else {
                    match ch.receiver.recv() {
                        Ok(Ok(output)) => {
                            println!("{output}");
                            true
                        }
                        Ok(Err(rendered)) => {
                            eprintln!("{rendered}");
                            false
                        }
                        Err(e) => {
                            eprintln!("Error awaiting sync: {e}");
                            false
                        }
                    }
                };
            }
            let exit_code = if succeeded {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };

            if ch
                .transmitter
                .send(Request::Command(commands::CliCommand::Quit))
                .is_ok()
            {
                match ch.receiver.recv() {
                    Ok(Ok(trailer)) => eprintln!("{trailer}"),
                    Ok(Err(rendered)) => eprintln!("{rendered}"),
                    Err(e) => eprintln!("{e}"),
                }
            }

            Ok(exit_code)
        }
    }
}

/// Returns `true` if the CLI will start the interactive REPL
/// (i.e. no COMMAND was given).
///
/// This is a thin wrapper around `ModeOfOperation` so that the binary
/// entry point can query the mode without exposing the enum publicly.
pub fn is_interactive(matches: &clap::ArgMatches) -> bool {
    matches!(get_mode_of_operation(matches), ModeOfOperation::Interactive)
}

/// Default log file directory.
const LOG_DIR: &str = ".zingo-cli";
/// Default log file name within the log directory.
const LOG_FILE: &str = "cli.log";

/// Returns the log file path from `--log-file` or the default `.zingo-cli/cli.log`.
pub fn log_file_path(matches: &clap::ArgMatches) -> PathBuf {
    if let Some(path) = matches.get_one::<String>("log-file") {
        PathBuf::from(path)
    } else {
        PathBuf::from(LOG_DIR).join(LOG_FILE)
    }
}

/// Reads the posture the parsed arguments imply without performing any
/// consent act, for surfaces that render before startup.
fn posture_preview(matches: &clap::ArgMatches) -> CommunicationMode {
    #[cfg(feature = "nym")]
    {
        let explicit_server =
            matches.value_source("server") == Some(clap::parser::ValueSource::CommandLine);
        match decide_connectivity(
            matches.get_flag("offline"),
            matches.get_flag("online"),
            matches.get_flag("remember-online"),
            explicit_server,
            zingolib::connectivity::load_connectivity_consent(&data_dir_from(matches)),
        ) {
            ConnectivityDecision::DeliberateOffline => CommunicationMode::DeliberateOffline,
            ConnectivityDecision::Online { .. } => CommunicationMode::Online,
            ConnectivityDecision::UnconsentedOffline => CommunicationMode::UnconsentedOffline,
        }
    }
    #[cfg(not(feature = "nym"))]
    {
        if matches.get_flag("offline") {
            CommunicationMode::DeliberateOffline
        } else {
            CommunicationMode::UnconsentedOffline
        }
    }
}

/// Returns help text if the parsed arguments indicate the `help` command,
/// or `None` for all other modes. The caller is responsible for printing
/// the text and exiting the process.
pub fn help_output(matches: &clap::ArgMatches) -> Option<String> {
    match get_mode_of_operation(matches) {
        ModeOfOperation::Command {
            command: commands::CliCommand::Help { command: named },
        } => Some(commands::format_help(
            posture_preview(matches),
            named.as_deref(),
        )),
        _ => None,
    }
}

/// Runs the CLI from pre-parsed arguments, returning the process exit code
/// the session earned: `SUCCESS`, or `FAILURE` when a one-shot command
/// fails (ADR 0031).
///
/// This function never calls `std::process::exit` or reads `std::env::args`.
/// The caller (the binary entry point) is responsible for parsing arguments,
/// handling the help short-circuit, process-level setup, and error reporting.
pub fn run_cli(matches: clap::ArgMatches) -> std::io::Result<ExitCode> {
    let mode = get_mode_of_operation(&matches);
    if let ModeOfOperation::Command { command } = &mode
        && let Err(refusal) = command.validate_deferred_grammar()
    {
        eprintln!("{refusal}");
        return Ok(ExitCode::from(2));
    }
    let communication_mode = get_communication_mode(&matches)?;
    let cli_config =
        ConfigTemplate::fill(mode, communication_mode, matches).map_err(std::io::Error::other)?;
    dispatch_command_or_start_interactive(&cli_config)
}

#[cfg(test)]
mod tests;
