//! `ZingoCli`, a command-line interface for the Zingo Zcash light wallet.
//!
//! This crate provides the library half of `zingo-cli`. It owns argument
//! parsing ([`build_clap_app`]), configuration assembly, wallet startup,
//! the interactive REPL, and single-command dispatch.
//!
//! The binary entry point (`main.rs`) is intentionally thin: it decides
//! when process-global state is installed, calling [`init_tracing`] and
//! the crypto provider before delegating to [`run_cli`], which builds a
//! [`LightClient`] and runs the command loop, and it reports the errors
//! that come back.

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
use std::sync;
use std::sync::mpsc::{Receiver, Sender, channel};

use clap::{self, Arg};
use log::{error, info};
// The mixnet narration task is the only debug/warn logger; the imports
// follow the feature.
#[cfg(feature = "nym")]
use log::{debug, warn};

use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
#[cfg(feature = "nym")]
use pepper_sync::error::{SyncError, SyncRecoveryObservables};
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
                .help("Create a new wallet with the given 24-word seed phrase. Will fail if wallet already exists. \
A seed passed here is visible in this host's process list and shell history; export ZINGO_SEED instead to \
keep it to this process and its child."))
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

/// The environment variable a seed phrase may arrive in, so a caller need
/// not put it where the process list and the shell history can read it.
const SEED_ENV: &str = "ZINGO_SEED";

/// The seed a session starts from: the flag when given, otherwise the
/// environment, and neither when what arrives is blank.
fn resolve_seed(flag: Option<String>, from_env: Option<String>) -> Option<String> {
    flag.or(from_env).filter(|phrase| !phrase.trim().is_empty())
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
/// any save-task failure, and returns the prompt's status segment,
/// `Block:{height}` followed by the sync indicator:
/// `" [Syncing X / Y outputs]"` while sync is in
/// progress, `" [Synced X / X outputs]"` when fully synced,
/// `" [Sync error]"` on failure, or `" [Sync stopped at X / Y outputs]"`
/// when no sync task is running and the wallet is not fully synced.
///
/// Every outcome is classified from typed values ([`PollReport`],
/// [`pepper_sync::sync_status`], `check_save_error`), never by inspecting
/// a command's output string. Every line this function prints is
/// narration, so all of it goes to stderr (ADR 0031).
async fn prompt_indicator(
    lightclient: &mut LightClient,
    sync_recovery: &mut SyncRecovery,
) -> String {
    let height = prompt_height(lightclient).await;
    let indicator = match lightclient.poll_sync() {
        PollReport::Ready(Err(e)) => {
            // The doubled "Sync error: Error:" is deliberate: it reproduces
            // the historical output byte for byte, where the polled command
            // string (itself prefixed "Error:") was interpolated after
            // "Sync error: ".
            eprintln!("Sync error: Error: {}", commands::render_error_chain(&e));
            #[cfg(feature = "nym")]
            let recovered = attempt_sync_recovery(lightclient, sync_recovery, &e).await;
            #[cfg(not(feature = "nym"))]
            let recovered = {
                let _ = &sync_recovery;
                false
            };
            if recovered {
                syncing_indicator(scan_progress(lightclient).await)
            } else {
                eprintln!("Please restart sync with `sync run`.");
                " [Sync error]".to_string()
            }
        }
        PollReport::Ready(Ok(sync_result)) => {
            #[cfg(feature = "nym")]
            {
                sync_recovery.attempts_left = SYNC_RECOVERY_ATTEMPT_BUDGET;
            }
            eprintln!("{sync_result}");
            synced_indicator(scan_progress(lightclient).await)
        }
        PollReport::NotReady => syncing_indicator(scan_progress(lightclient).await),
        PollReport::NoHandle => idle_indicator(scan_progress(lightclient).await),
    };
    if let Err(e) = lightclient.check_save_error().await {
        eprintln!("Error: save failed. {e}\nRestarting save task...");
    }
    format!("Block:{height}{indicator}")
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

/// Reads the prompt's chain height from the sync engine's progress channel while sync runs, from the wallet otherwise, or zero when neither knows a height yet.
async fn prompt_height(lightclient: &LightClient) -> u32 {
    if lightclient.sync_mode() == pepper_sync::wallet::SyncMode::NotRunning {
        lightclient
            .wallet()
            .read()
            .await
            .sync_state
            .last_known_chain_height()
            .map_or(0, u32::from)
    } else {
        lightclient
            .latest_sync_status()
            .as_ref()
            .and_then(|status| status.scan_ranges.last())
            .map_or(0, |range| u32::from(range.block_range().end - 1))
    }
}

/// Reads the scan progress from the sync engine's progress channel while sync runs, from the wallet otherwise, or `None` if sync status is unavailable.
async fn scan_progress(lightclient: &LightClient) -> Option<ScanProgress> {
    let status = if lightclient.sync_mode() == pepper_sync::wallet::SyncMode::NotRunning {
        pepper_sync::sync_status(&*lightclient.wallet().read().await)
            .await
            .ok()?
    } else {
        lightclient.latest_sync_status()?
    };
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
fn format_ranked_servers(cli_config: &CliConfigTemplate) -> String {
    match &cli_config.server {
        Some(server) => format!("Configured indexer: {server}. Nothing was probed."),
        None => "Configured indexer: none. This session is offline and probes nothing.".to_string(),
    }
}

/// Formats the ranked server list for display by the `servers` command.
#[cfg(feature = "clearnet-test-mode")]
fn format_ranked_servers(cli_config: &CliConfigTemplate) -> String {
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

/// Executes one command against the loop, drains the reply, closes the
/// session, and returns the exit code the command earned (ADR 0031).
fn start_noninteractive(command: &commands::CliCommand, ch: CommandChannel) -> ExitCode {
    let description = command.name();
    let mut succeeded = if ch
        .transmitter
        .send(Request::Command(command.clone()))
        .is_err()
    {
        let e = format!("Error executing command {description}: the command loop has exited");
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

    if succeeded {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
/// Runs the interactive prompt until it closes, returning the exit code the
/// session earned: success when the user ended it, failure when the terminal
/// did (ADR 0031).
fn start_interactive(cli_config: &CliConfigTemplate, ch: CommandChannel) -> ExitCode {
    // `()` can be used when no completer is required
    let mut rl = match rustyline::DefaultEditor::new() {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("Error: the interactive prompt could not start: {e}");
            error!("the interactive prompt could not start: {e}");
            return ExitCode::FAILURE;
        }
    };

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
        let prompt_status = send_request(Request::PromptIndicator).unwrap_or_default();

        let readline = rl.readline(&format!("({chain_name}) {prompt_status} >> "));
        match readline {
            Ok(line) => {
                if let Err(e) = rl.add_history_entry(line.as_str()) {
                    // A history that will not grow costs the user recall, never
                    // the session: the command they typed still runs.
                    log::warn!("the prompt's history did not record the line: {e}");
                }
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
                    break ExitCode::SUCCESS;
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                println!("CTRL-C");
                info!("CTRL-C");
                break ExitCode::SUCCESS;
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("CTRL-D");
                info!("CTRL-D");
                break ExitCode::SUCCESS;
            }
            Err(err) => {
                // The terminal ended the session, not the user, so the shell
                // hears failure rather than a clean close.
                eprintln!("Error: the interactive prompt failed: {err}");
                error!("the interactive prompt failed: {err}");
                break ExitCode::FAILURE;
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
    /// typed calls on the loop thread. The reply is the prompt's status
    /// segment: the chain height and the sync indicator, read without
    /// contending with a running sync for the wallet lock.
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

/// How many automatic sync recoveries may run back-to-back before the
/// session parks on the reported error and waits for `sync run`.
#[cfg(feature = "nym")]
const SYNC_RECOVERY_ATTEMPT_BUDGET: usize = 3;

/// What the command loop needs to recover a dead sync without the user.
#[cfg(feature = "nym")]
pub(crate) struct SyncRecovery {
    /// The census chain a redraw surveys; `None` withholds the redraw
    /// (a pinned, offline, or regtest session), leaving only same-server
    /// relaunches.
    redraw_chain: Option<zingolib::indexers::IndexerChain>,
    /// The `nym-proxy` binary a redraw's sweep spawns.
    proxy_path: String,
    /// The automatic recoveries left before the session parks.
    attempts_left: usize,
}

/// What the command loop needs to recover a dead sync without the user.
#[cfg(not(feature = "nym"))]
pub(crate) struct SyncRecovery;

/// The move one automatic sync-recovery attempt makes.
#[cfg(feature = "nym")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryAction {
    /// Report the error and wait for the user's `sync run`.
    Park,
    /// Launch a fresh sync task against the still-bound indexer.
    Relaunch,
    /// Redraw the sync indexer with a Server-Selection Sweep, then launch
    /// a fresh sync task against the winner.
    Redraw,
}

/// Plans one automatic recovery from the error's typed recommendation, the
/// attempts left, and whether this session may redraw its indexer.
#[cfg(feature = "nym")]
fn plan_recovery(
    observable: SyncRecoveryObservables,
    attempts_left: usize,
    redraw_available: bool,
) -> RecoveryAction {
    if attempts_left == 0 {
        return RecoveryAction::Park;
    }
    match observable {
        SyncRecoveryObservables::Abort => RecoveryAction::Park,
        SyncRecoveryObservables::MaybeRecoverableServer => RecoveryAction::Relaunch,
        SyncRecoveryObservables::ServerUnavailable if redraw_available => RecoveryAction::Redraw,
        SyncRecoveryObservables::ServerUnavailable => RecoveryAction::Park,
    }
}

/// Spawns a background thread that executes each parsed-command message
/// against the [`LightClient`] and replies through the returned
/// [`CommandChannel`], exiting on [`commands::CliCommand::Quit`].
#[allow(clippy::disallowed_methods)]
pub(crate) fn command_loop(
    mut lightclient: LightClient,
    communications: Communications,
    mut sync_recovery: SyncRecovery,
) -> CommandChannel {
    let (command_transmitter, command_receiver) = channel::<Request>();
    let (resp_transmitter, resp_receiver) = channel::<Result<String, String>>();

    std::thread::spawn(move || {
        while let Ok(request) = command_receiver.recv() {
            let command = match request {
                Request::Command(command) => command,
                Request::PromptIndicator => {
                    resp_transmitter
                        .send(Ok(RT.block_on(prompt_indicator(
                            &mut lightclient,
                            &mut sync_recovery,
                        ))))
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
            let live_mode = match communications {
                Communications::DeliberateOffline => Communications::DeliberateOffline,
                _ if lightclient.indexer_uri().is_some() => Communications::Online,
                #[cfg(feature = "nym")]
                _ if lightclient.read_mixnet_indicator()
                    != zingolib::mixnet::Indicator::Unattached =>
                {
                    Communications::Online
                }
                _ => Communications::UnconsentedOffline,
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
enum Operations {
    /// Start the interactive REPL.
    Interactive,
    /// Execute a single command and exit, the command arriving already
    /// parsed from the same grammar the REPL uses.
    NonInteractive {
        /// The parsed command to execute.
        command: commands::CliCommand,
    },
}

/// Determines the mode of operation from parsed CLI arguments:
/// [`ModeOfOperation::NonInteractive`] when a command is given, or
/// [`ModeOfOperation::Interactive`] when none is.
fn get_mode_of_operation(matches: &clap::ArgMatches) -> Operations {
    use clap::FromArgMatches as _;

    commands::CliCommand::from_arg_matches(matches).map_or(Operations::Interactive, |command| {
        Operations::NonInteractive { command }
    })
}

/// Whether the CLI communicates with a remote indexer or operates locally.
///
/// Selected at argument-parse time by the `--offline` flag and pinned for
/// the life of the session (Offline mode, issue #2286).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Communications {
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
    communications: Communications,
    command: &commands::CliCommand,
) -> Option<String> {
    command
        .suppressed(communications)
        .then(|| offline_refusal_text(communications, &command.name()))
}

/// Renders the minted refusal for a suppressed command, naming the live
/// remedy the posture leaves open.
#[cfg(feature = "nym")]
fn offline_refusal_text(communications: Communications, name: &str) -> String {
    match communications {
        Communications::DeliberateOffline => format!(
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
fn offline_refusal_text(_communications: Communications, name: &str) -> String {
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
fn get_communications(matches: &clap::ArgMatches) -> std::io::Result<Communications> {
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
            Communications::DeliberateOffline
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
            Communications::Online
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
            Communications::UnconsentedOffline
        }
    })
}

/// The communication mode without the mixnet capability: Offline Mode is
/// the only mode such a build can be in (ADR 0026). The online consent
/// acts refuse loudly rather than silently degrade, and a stored standing
/// consent is reported as inert. `--forget-online` still works, so an
/// opt-out build can retire a stored consent.
#[cfg(not(feature = "nym"))]
fn get_communications(matches: &clap::ArgMatches) -> std::io::Result<Communications> {
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
        Communications::DeliberateOffline
    } else {
        Communications::UnconsentedOffline
    })
}

/// All CLI-derived configuration needed to create a [`LightClient`] and
/// start the command loop.
///
/// Built by [`ConfigTemplate::fill`] from parsed [`clap::ArgMatches`],
/// then consumed by [`build_zingo_config`] and [`dispatch_command_or_start_interactive`].
#[derive(Debug)]
pub(crate) struct CliConfigTemplate {
    mode: Operations,
    communications: Communications,
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

impl CliConfigTemplate {
    fn fill(
        mode: Operations,
        communications: Communications,
        matches: clap::ArgMatches,
    ) -> Result<Self, ConfigTemplateError> {
        let seed = resolve_seed(
            matches.get_one::<String>("seed").cloned(),
            std::env::var(SEED_ENV).ok(),
        );
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
            && let Operations::NonInteractive { command } = &mode
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
        let (server, ranked_servers) = match communications {
            Communications::DeliberateOffline | Communications::UnconsentedOffline => {
                (None, vec![])
            }
            Communications::Online => {
                let (server, ranked_servers) = server_select_clearnet::resolve_server(&matches)?;
                (Some(server), ranked_servers)
            }
        };
        // Without the quarantined sweep, resolution is pure: the typed
        // `--server` pin or nothing, never a probe and never a default.
        #[cfg(not(feature = "clearnet-test-mode"))]
        let server = match communications {
            Communications::DeliberateOffline | Communications::UnconsentedOffline => None,
            Communications::Online => matches
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
        let sync = !matches.get_flag("nosync") && communications == Communications::Online;
        let waitsync = matches.get_flag("waitsync");
        let nym_proxy_path = matches.get_one::<String>("nym-proxy").cloned();
        Ok(Self {
            mode,
            communications,
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
        })
    }
}

/// Builds a `ClientConfig` from the filled config template.
///
/// This is the first testable seam inside the startup sequence. Its only
/// I/O is the chain-tip fetch that dates a brand-new wallet, which an
/// Offline-mode session never performs.
async fn build_zingo_config(filled_template: &CliConfigTemplate) -> std::io::Result<ClientConfig> {
    let wallet_path = filled_template.data_dir.clone().join(DEFAULT_WALLET_NAME);
    let no_of_accounts = NonZeroU32::try_from(1).expect("hard-coded integer");
    let wallet_settings = WalletSettings {
        sync_config: SyncConfig {
            transparent_address_discovery: TransparentAddressDiscovery::minimal(),
            performance_level: PerformanceLevel::High,
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
pub(crate) fn startup(filled_template: &CliConfigTemplate) -> std::io::Result<CommandChannel> {
    let lightclient = RT.block_on(startup_async(filled_template))?;
    #[cfg(feature = "nym")]
    let sync_recovery = SyncRecovery {
        redraw_chain: (matches!(filled_template.communications, Communications::Online)
            && !filled_template.server_pinned)
            .then(|| census_chain(&filled_template.chaintype))
            .flatten(),
        proxy_path: commands::resolve_proxy_path(filled_template.nym_proxy_path.as_deref()),
        attempts_left: SYNC_RECOVERY_ATTEMPT_BUDGET,
    };
    #[cfg(not(feature = "nym"))]
    let sync_recovery = SyncRecovery;
    Ok(command_loop(
        lightclient,
        filled_template.communications,
        sync_recovery,
    ))
}

async fn startup_async(filled_template: &CliConfigTemplate) -> std::io::Result<LightClient> {
    let config = build_zingo_config(filled_template).await?;

    let mut lightclient = LightClient::new(config, false)
        .await
        .map_err(|e| std::io::Error::other(format!("Failed to create lightclient. {e}")))?;

    if matches!(filled_template.mode, Operations::Interactive) {
        // Print startup Messages
        info!(""); // Blank line
        info!("Starting Zingo-CLI");
        match &filled_template.server {
            Some(server) => info!("Lightclient connecting to {server}"),
            None if filled_template.communications == Communications::Online => {
                info!("No pinned Indexer; the Server-Selection Sweep selects the sync indexer")
            }
            None => info!("Offline mode: no Indexer will be configured this session"),
        }
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
    if filled_template.communications == Communications::Online {
        use zingolib::mixnet::{MixnetStartPolicy, ProvisionStrategy};
        info!(
            "Mixnet Mode enabling; the nym proxy is bootstrapping. Send and price-fetch \
             become available once it is ready (see `network status`)."
        );
        // Narrate Mixnet Mode transitions from the session's status
        // subscription (push, not poll): mode changes at info/warn through
        // the standard log path, bootstrap progress at debug. Rendering is
        // this consumer's own (ADR 0024, decision 8) — variant matching,
        // never prose matching. The subscription precedes the blocking
        // enable so the startup birth itself is narrated, and the seed is
        // Bootstrapping because the line above already announced it.
        let mut status_rx = lightclient.subscribe_mixnet_status();
        tokio::spawn(async move {
            let mut last_mode = zingolib::mixnet::Indicator::Bootstrapping;
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
                    zingolib::mixnet::Indicator::Ready
                    | zingolib::mixnet::Indicator::PreviouslyProvenThisEpoch => info!(
                        "Mixnet Mode ready; send and price-fetch route over the mixnet \
                         (see `network status`).{}",
                        commands::render_exit_nodes(&status.exits)
                    ),
                    zingolib::mixnet::Indicator::Died => {
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
    }

    // The sweep binds the session's indexer, so it runs for every online
    // session, not only a syncing one: a --nosync session still accepts
    // interactive sync and send, which need an indexer bound. A pinned
    // server is bound at config time and surveyed here; an unpinned online
    // session has no indexer until the sweep selects one.
    #[cfg(feature = "nym")]
    let indexer_ready = if filled_template.communications == Communications::Online {
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

    if std::env::var_os("ZINGO_DISABLE_SAVER").is_none() {
        let save_run = commands::CliCommand::Save {
            sub: commands::SaveSubCommand::Run,
        };
        match commands::dispatch_parsed(save_run, &mut lightclient).await {
            Ok(update) => eprintln!("{update}"),
            Err(e) => eprintln!("Error: {}", commands::render_error_chain(&e)),
        }
    } else {
        eprintln!(
            "ZINGO_DISABLE_SAVER is set: the save task will not run, so nothing \
             persists this session."
        );
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
/// binds a sync indexer; returns whether this Sync Session may open. A
/// pinned server rides the survey and is chosen when it answers; a pin the
/// survey found unresponsive is reported with the sweep's verdict offered
/// as the alternative, and a sweep whose own transport failed never counts
/// against the pin.
#[cfg(feature = "nym")]
async fn sweep_select_sync_indexer(
    lightclient: &mut LightClient,
    filled_template: &CliConfigTemplate,
) -> bool {
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
            narrate_sweep_progress,
        )
        .await;
    match judge_sweep_outcome(&selection, pin.as_ref()) {
        SweepVerdict::PinStands { notice } => {
            eprintln!("{notice}");
            true
        }
        SweepVerdict::Refuse { notice } => {
            eprintln!("{notice}");
            false
        }
        SweepVerdict::BindWinner { chosen, notice } => {
            match lightclient.set_indexer_uri(chosen.clone()).await {
                Ok(()) => {
                    eprintln!("{notice}");
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
        SweepVerdict::RecommendFallback {
            alternative,
            notice,
        } => {
            eprintln!("{notice}");
            if !consent_to_fallback(&filled_template.mode, &alternative) {
                eprintln!(
                    "Server-Selection Sweep: fallback declined. This Sync Session does not \
                     open; run `changeserver {alternative}` at the prompt, or relaunch \
                     pinning a live server."
                );
                return false;
            }
            match lightclient.set_indexer_uri(alternative.clone()).await {
                Ok(()) => {
                    eprintln!(
                        "Server-Selection Sweep: sync attaches to {alternative} by your \
                         explicit consent."
                    );
                    true
                }
                Err(e) => {
                    eprintln!(
                        "Server-Selection Sweep: consented to {alternative}, but binding it \
                         failed: {}. This Sync Session does not open.",
                        commands::render_error_chain(&e),
                    );
                    false
                }
            }
        }
    }
}

/// Narrates one Server-Selection Sweep phase transition on stderr.
#[cfg(feature = "nym")]
fn narrate_sweep_progress(phase: zingolib::lightclient::select::SweepProgress) {
    use zingolib::lightclient::select::SweepProgress;
    match phase {
        SweepProgress::TransportBootstrapping => eprintln!(
            "Server-Selection Sweep: bootstrapping a dedicated sweep transport \
             (its Exit Node is recycled when the sweep completes)..."
        ),
        SweepProgress::Surveying { candidates } => eprintln!(
            "Server-Selection Sweep: surveying {candidates} candidates over the mixnet..."
        ),
        SweepProgress::ExitAbandoned { draw } => eprintln!(
            "Server-Selection Sweep: the Exit Node of draw {draw} carries nothing — \
             a reliable address stayed silent through it; abandoning that exit and \
             surveying again from the start..."
        ),
        SweepProgress::Judging { answered, surveyed } => eprintln!(
            "Server-Selection Sweep: {answered} of {surveyed} candidates answered; \
             judging the live cohort..."
        ),
    }
}

/// Reports whether two URIs name the same indexer endpoint by scheme, host,
/// and port, so a bound URI's path rendering never defeats the comparison.
#[cfg(feature = "nym")]
fn same_endpoint(a: &http::Uri, b: &http::Uri) -> bool {
    a.scheme_str() == b.scheme_str() && a.host() == b.host() && a.port_u16() == b.port_u16()
}

/// Attempts one automatic recovery of a dead sync, returning whether a new
/// sync task is running.
#[cfg(feature = "nym")]
async fn attempt_sync_recovery<E: std::fmt::Debug + std::fmt::Display>(
    lightclient: &mut LightClient,
    recovery: &mut SyncRecovery,
    error: &SyncError<E>,
) -> bool {
    let Some(bound) = lightclient.indexer_uri() else {
        return false;
    };
    let action = plan_recovery(
        error.recovery_recommendation(),
        recovery.attempts_left,
        recovery.redraw_chain.is_some(),
    );
    let attempt = SYNC_RECOVERY_ATTEMPT_BUDGET - recovery.attempts_left + 1;
    match action {
        RecoveryAction::Park => return false,
        RecoveryAction::Relaunch => {
            recovery.attempts_left -= 1;
            eprintln!(
                "Sync relaunches against {bound}: the error class says the same server may \
                 recover (automatic recovery {attempt} of {SYNC_RECOVERY_ATTEMPT_BUDGET})."
            );
        }
        RecoveryAction::Redraw => {
            recovery.attempts_left -= 1;
            eprintln!(
                "Sync's indexer {bound} failed and the error class condemns the server; \
                 abandoning it and redrawing \
                 (automatic recovery {attempt} of {SYNC_RECOVERY_ATTEMPT_BUDGET})..."
            );
            let chain = recovery
                .redraw_chain
                .expect("plan_recovery only redraws when a census chain is present");
            if !redraw_sync_indexer(lightclient, chain, &recovery.proxy_path, &bound).await {
                return false;
            }
        }
    }
    let sync_run = commands::CliCommand::Sync {
        sub: commands::SyncSubCommand::Run,
    };
    match commands::dispatch_parsed(sync_run, lightclient).await {
        Ok(update) => {
            eprintln!("{update}");
            true
        }
        Err(e) => {
            eprintln!("Error: {}", commands::render_error_chain(&e));
            false
        }
    }
}

/// Runs a redraw Server-Selection Sweep that demotes the failed indexer,
/// surveying it again only when it is the sole candidate, and returns
/// whether a fresh winner is bound.
#[cfg(feature = "nym")]
async fn redraw_sync_indexer(
    lightclient: &mut LightClient,
    chain: zingolib::indexers::IndexerChain,
    proxy_path: &str,
    failed: &http::Uri,
) -> bool {
    let mut candidates: Vec<http::Uri> = zingolib::indexers::mixnet_eligible(chain)
        .map(|indexer| indexer.uri.parse().expect("census URIs parse"))
        .collect();
    if candidates.len() > 1 {
        candidates.retain(|candidate| !same_endpoint(candidate, failed));
    }
    let selection = lightclient
        .run_server_selection_sweep(
            std::path::Path::new(proxy_path),
            &candidates,
            None,
            narrate_sweep_progress,
        )
        .await;
    match judge_sweep_outcome(&selection, None) {
        SweepVerdict::BindWinner { chosen, notice } => {
            match lightclient.set_indexer_uri(chosen.clone()).await {
                Ok(()) => {
                    eprintln!("{notice}");
                    true
                }
                Err(e) => {
                    eprintln!(
                        "Server-Selection Sweep: selected {chosen}, but binding it failed: {}.",
                        commands::render_error_chain(&e),
                    );
                    false
                }
            }
        }
        SweepVerdict::Refuse { notice }
        | SweepVerdict::PinStands { notice }
        | SweepVerdict::RecommendFallback { notice, .. } => {
            eprintln!("{notice}");
            false
        }
    }
}

/// The decision a finished Server-Selection Sweep leaves the Sync Session
/// with.
#[cfg(feature = "nym")]
#[derive(Debug)]
enum SweepVerdict {
    /// The pinned, already-bound server stands and the Sync Session opens.
    PinStands {
        /// The narration this decision prints.
        notice: String,
    },
    /// The sweep's winner binds and the Sync Session opens on it.
    BindWinner {
        /// The indexer to bind.
        chosen: http::Uri,
        /// The narration a successful bind prints.
        notice: String,
    },
    /// The Sync Session does not open.
    Refuse {
        /// The narration this refusal prints.
        notice: String,
    },
    /// The survey refused the pin but holds a healthy alternative, which
    /// binds only by the user's explicit consent, refusing by default.
    RecommendFallback {
        /// The healthy indexer the sweep drew.
        alternative: http::Uri,
        /// The narration recommending the fallback before the consent ask.
        notice: String,
    },
}

/// Asks explicit consent to open the Sync Session on `alternative` instead
/// of the refused pin, defaulting to No wherever no interactive terminal
/// can answer.
#[cfg(feature = "nym")]
fn consent_to_fallback(mode: &Operations, alternative: &http::Uri) -> bool {
    use std::io::IsTerminal as _;
    if !matches!(mode, Operations::Interactive) || !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("Server-Selection Sweep: use {alternative} for this Sync Session instead? [y/N] ");
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim(), "y" | "Y" | "yes" | "Yes")
}

/// Judges a finished sweep against the pin, deciding whether the Sync
/// Session opens and on which indexer.
#[cfg(feature = "nym")]
fn judge_sweep_outcome(
    selection: &Result<
        zingolib::mixnet::sweep::Selection,
        zingolib::lightclient::select::ServerSelectionError,
    >,
    pin: Option<&http::Uri>,
) -> SweepVerdict {
    match selection {
        Ok(selection) => {
            if let Some(pinned) = pin {
                if selection.cohort.iter().any(|live| &live.uri == pinned) {
                    // The user's selection answered the survey: it is chosen,
                    // and it is already bound from config.
                    return SweepVerdict::PinStands {
                        notice: format!(
                            "Server-Selection Sweep: sync attaches to the pinned server \
                             {pinned} (live cohort of {}).",
                            selection.cohort.len(),
                        ),
                    };
                }
                // The ruled default is No: the recommendation binds the
                // alternative only through the caller's explicit consent.
                return SweepVerdict::RecommendFallback {
                    alternative: selection.sync_indexer.clone(),
                    notice: format!(
                        "Server-Selection Sweep: the pinned server {pinned} did not answer the \
                         survey or lags the live cohort of {}. The sweep drew a healthy \
                         alternative: {}.",
                        selection.cohort.len(),
                        selection.sync_indexer,
                    ),
                };
            }
            SweepVerdict::BindWinner {
                chosen: selection.sync_indexer.clone(),
                notice: format!(
                    "Server-Selection Sweep: sync attaches to {} (live cohort of {}, \
                     {} transmit candidates exclude its operator).",
                    selection.sync_indexer,
                    selection.cohort.len(),
                    selection.transmit_candidates.len(),
                ),
            }
        }
        Err(e) => {
            let judged_the_pin = matches!(
                e,
                zingolib::lightclient::select::ServerSelectionError::Selection(_)
            );
            match pin {
                // A Selection error means the survey ran through a proven
                // exit and the pinned candidate was judged with the rest:
                // opening against it would trust a server just refused.
                Some(pinned) if judged_the_pin => SweepVerdict::Refuse {
                    notice: format!(
                        "Server-Selection Sweep: the survey ran and selected nothing — the \
                         pinned server {pinned} was judged and refused: {}. This Sync Session \
                         does not open; run `changeserver` at the prompt, or relaunch pinning \
                         a live server.",
                        commands::render_error_chain(e),
                    ),
                },
                Some(pinned) => SweepVerdict::PinStands {
                    notice: format!(
                        "Server-Selection Sweep: no verdict: {}. The survey never judged the \
                         pinned server {pinned}; it stands, and this Sync Session opens \
                         against it.",
                        commands::render_error_chain(e),
                    ),
                },
                None => SweepVerdict::Refuse {
                    notice: sweep_refusal_notice(e),
                },
            }
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
    cli_config: &CliConfigTemplate,
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

fn dispatch_command_or_start_interactive(
    cli_config: &CliConfigTemplate,
) -> std::io::Result<ExitCode> {
    let ch = match startup(cli_config) {
        Ok(ch) => ch,
        Err(startup_error) => {
            if matches!(
                &cli_config.mode,
                Operations::NonInteractive {
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
        Operations::Interactive => Ok(start_interactive(cli_config, ch)),
        Operations::NonInteractive { command } => Ok(start_noninteractive(command, ch)),
    }
}

/// Whether the CLI will start the interactive REPL, which it does when no
/// command was given.
fn is_interactive(matches: &clap::ArgMatches) -> bool {
    matches!(get_mode_of_operation(matches), Operations::Interactive)
}

/// Default log file directory.
const LOG_DIR: &str = ".zingo-cli";
/// Default log file name within the log directory.
const LOG_FILE: &str = "cli.log";

/// Returns the log file path from `--log-file` or the default `.zingo-cli/cli.log`.
fn log_file_path(matches: &clap::ArgMatches) -> PathBuf {
    if let Some(path) = matches.get_one::<String>("log-file") {
        PathBuf::from(path)
    } else {
        PathBuf::from(LOG_DIR).join(LOG_FILE)
    }
}

/// Installs the process-global tracing subscriber, writing to the log file
/// in interactive mode so error-level output never pollutes the REPL, and
/// to stderr in every other case.
pub fn init_tracing(matches: &clap::ArgMatches) {
    let env_filter = tracing_subscriber::EnvFilter::from_default_env();

    if is_interactive(matches) {
        let log_path = log_file_path(matches);
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Ok(file) => {
                tracing_subscriber::fmt()
                    .with_env_filter(env_filter)
                    .with_writer(sync::Mutex::new(file))
                    .with_ansi(false)
                    .init();
                return;
            }
            Err(e) => {
                eprintln!(
                    "Warning: could not open log file {}: {e}. Logging to stderr.",
                    log_path.display()
                );
            }
        }
    }

    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}

/// Reads the posture the parsed arguments imply without performing any
/// consent act, for surfaces that render before startup.
fn posture_preview(matches: &clap::ArgMatches) -> Communications {
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
            ConnectivityDecision::DeliberateOffline => Communications::DeliberateOffline,
            ConnectivityDecision::Online { .. } => Communications::Online,
            ConnectivityDecision::UnconsentedOffline => Communications::UnconsentedOffline,
        }
    }
    #[cfg(not(feature = "nym"))]
    {
        if matches.get_flag("offline") {
            Communications::DeliberateOffline
        } else {
            Communications::UnconsentedOffline
        }
    }
}

/// Returns help text if the parsed arguments indicate the `help` command,
/// or `None` for all other modes. The caller is responsible for printing
/// the text and exiting the process.
pub fn help_output(matches: &clap::ArgMatches) -> Option<String> {
    match get_mode_of_operation(matches) {
        Operations::NonInteractive {
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
    if let Operations::NonInteractive { command } = &mode
        && let Err(refusal) = command.validate_deferred_grammar()
    {
        eprintln!("{refusal}");
        return Ok(ExitCode::from(2));
    }
    let communications = get_communications(&matches)?;
    let cli_config =
        CliConfigTemplate::fill(mode, communications, matches).map_err(std::io::Error::other)?;
    dispatch_command_or_start_interactive(&cli_config)
}

#[cfg(test)]
mod tests;
