//! `ZingoCli` — a command-line interface for the Zingo Zcash light wallet.
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
mod most_up_indexer_uris;
mod server_select;

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use clap::{self, Arg};
use log::{error, info};

use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
use zingo_netutils::Indexer as _;
use zingolib::config::{ChainType, ClientConfig, DEFAULT_WALLET_NAME, WalletConfig};
use zingolib::data::PollReport;
use zingolib::lightclient::{DEFAULT_REQUEST_TIMEOUT, LightClient};
use zingolib::wallet::WalletSettings;

use crate::commands::{RT, ShortCircuitedCommand};

pub(crate) mod version;

/// Builds the clap `Command` definition for the CLI.
pub fn build_clap_app() -> clap::Command {
    clap::Command::new("Zingo CLI").version(version::VERSION)
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
                .help("Indexer server to connect to.")
                .value_parser(parse_uri)
                .default_value(zingolib::config::DEFAULT_INDEXER_URI))
            .arg(Arg::new("offline")
                .long("offline")
                .action(clap::ArgAction::SetTrue)
                .conflicts_with_all(["server", "waitsync"])
                .help("Run the session in Offline mode: no Indexer is ever configured. Local operations (addresses, balances, history, proposing) work; sync, transmission, and server commands are unavailable."))
            .arg(Arg::new("data-dir")
                .long("data-dir")
                .value_name("data-dir")
                .help("Absolute path to use as data directory"))
            .arg(Arg::new("log-file")
                .long("log-file")
                .value_name("PATH")
                .help("Path to the log file for interactive mode. Defaults to .zingo-cli/cli.log"))
            .arg(Arg::new("COMMAND")
                .help("Command to execute. If a command is not specified, zingo-cli will start in interactive mode.")
                .required(false)
                .index(1))
            .arg(Arg::new("extra_args")
                .help("Params to execute command with. Run the 'help' command to get usage help.")
                .required(false)
                .num_args(1..)
                .index(2)
                .action(clap::ArgAction::Append)
        )
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

/// Performs the per-prompt housekeeping on the command-loop thread, where
/// the [`LightClient`] lives: polls the sync task, reports any save-task
/// failure, and returns the sync indicator to embed in the interactive
/// prompt — `" [Syncing X / Y outputs]"` while sync is in progress,
/// `" [Synced X / X outputs]"` when fully synced, `" [Sync error]"` on
/// failure, or `" [Sync stopped at X / Y outputs]"` when no sync task is
/// running and the wallet is not fully synced.
///
/// Every outcome is classified from typed values ([`PollReport`],
/// [`pepper_sync::sync_status`], `check_save_error`), never by inspecting
/// a command's output string.
fn prompt_indicator(lightclient: &mut LightClient) -> String {
    let indicator = match lightclient.poll_sync() {
        PollReport::Ready(Err(e)) => {
            // The doubled "Sync error: Error:" is deliberate: it reproduces
            // the historical output byte for byte, where the polled command
            // string (itself prefixed "Error:") was interpolated after
            // "Sync error: ".
            eprintln!("Sync error: Error: {e}\nPlease restart sync with `sync run`.");
            " [Sync error]".to_string()
        }
        PollReport::Ready(Ok(sync_result)) => {
            println!("{sync_result}");
            synced_indicator(scan_progress(lightclient))
        }
        PollReport::NotReady => syncing_indicator(scan_progress(lightclient)),
        PollReport::NoHandle => idle_indicator(scan_progress(lightclient)),
    };
    if let Err(e) = RT.block_on(lightclient.check_save_error()) {
        eprintln!("Error: save failed. {e}\nRestarting save task...");
    }
    indicator
}

/// The wallet's scan progress: the exact integer ratio of outputs scanned,
/// and whether sync is complete. No floating-point representation appears
/// anywhere in the prompt's reporting.
struct ScanProgress {
    outputs_scanned: u64,
    total_outputs: u64,
    complete: bool,
}

/// Reads the wallet's scan progress, or `None` if sync status is
/// unavailable.
fn scan_progress(lightclient: &LightClient) -> Option<ScanProgress> {
    RT.block_on(async {
        pepper_sync::sync_status(&*lightclient.wallet().read().await)
            .await
            .ok()
            .map(|status| ScanProgress {
                outputs_scanned: status.total_outputs_scanned,
                total_outputs: status.total_outputs,
                complete: status.is_complete(),
            })
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

/// Formats the ranked server list for display by the `servers` command.
fn format_ranked_servers(cli_config: &ConfigTemplate) -> String {
    let Some(server) = &cli_config.server else {
        return "Offline mode: no server is configured this session.".to_string();
    };
    if cli_config.ranked_servers.is_empty() {
        return format!("Server was set explicitly: {server}\nNo other servers were probed.");
    }
    let mut out = String::from("Servers ranked by get_info() response time:\n");
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

    let send_request = |request: Request| -> String {
        let description = match &request {
            Request::Command(cmd, _) => cmd.clone(),
            Request::PromptIndicator => "prompt indicator".to_string(),
        };
        ch.transmitter.send(request).unwrap();
        match ch.receiver.recv() {
            Ok(s) => s,
            Err(e) => {
                let e = format!("Error executing command {description}: {e}");
                eprintln!("{e}");
                error!("{e}");
                String::new()
            }
        }
    };
    let send_command =
        |cmd: String, args: Vec<String>| -> String { send_request(Request::Command(cmd, args)) };

    let mut chain_name = String::new();

    loop {
        if chain_name.is_empty() {
            let info = send_command("info".to_string(), vec![]);
            chain_name = json::parse(&info)
                .map(|mut json_info| json_info.remove("chain_name"))
                .ok()
                .and_then(|name| name.as_str().map(ToString::to_string))
                .unwrap_or_default();
        }
        // Read the height first
        let height = json::parse(&send_command(
            "height".to_string(),
            vec!["false".to_string()],
        ))
        .unwrap()["height"]
            .as_i64()
            .unwrap();

        let sync_indicator = send_request(Request::PromptIndicator);

        let readline = rl.readline(&format!(
            "({chain_name}) Block:{height}{sync_indicator} >> "
        ));
        match readline {
            Ok(line) => {
                rl.add_history_entry(line.as_str())
                    .expect("Ability to add history entry");
                // Parse command line arguments
                let mut cmd_args = if let Ok(args) = shellwords::split(&line) {
                    args
                } else {
                    println!("Mismatched Quotes");
                    continue;
                };

                if cmd_args.is_empty() {
                    continue;
                }

                let cmd = cmd_args.remove(0);
                let args: Vec<String> = cmd_args;

                // CLI-only commands that don't need the LightClient.
                if cmd == "servers" {
                    println!("{}", format_ranked_servers(cli_config));
                    continue;
                }

                println!("{}", send_command(cmd, args));

                // Special check for Quit command.
                if line == "quit" || line == "exit" {
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
/// The variant — not the content of the response string — tells the
/// requester how to interpret the reply, so no consumer ever classifies
/// a response by sniffing its text (the in-band-error problem of issue
/// zingolabs/zingolib#2446).
enum Request {
    /// Execute a user command; the reply is the command's output.
    Command(String, Vec<String>),
    /// Perform the per-prompt housekeeping (sync poll, save check) via
    /// typed calls on the loop thread; the reply is the sync indicator
    /// to embed in the interactive prompt.
    PromptIndicator,
}

/// A paired request/response channel for communicating with the background command loop.
struct CommandChannel {
    transmitter: Sender<Request>,
    receiver: Receiver<String>,
}

/// Spawns a background thread that listens for `(command, args)` messages,
/// executes each command against the [`LightClient`], and sends the
/// string response back through the returned [`CommandChannel`].
///
/// The loop exits when it receives a `"quit"` or `"exit"` command.
pub(crate) fn command_loop(
    mut lightclient: LightClient,
    communication_mode: CommunicationMode,
) -> CommandChannel {
    let (command_transmitter, command_receiver) = channel::<Request>();
    let (resp_transmitter, resp_receiver) = channel::<String>();

    std::thread::spawn(move || {
        while let Ok(request) = command_receiver.recv() {
            let (cmd, args) = match request {
                Request::Command(cmd, args) => (cmd, args),
                Request::PromptIndicator => {
                    resp_transmitter
                        .send(prompt_indicator(&mut lightclient))
                        .unwrap();
                    continue;
                }
            };
            // The Offline-mode pin: this session never configures an Indexer.
            if communication_mode == CommunicationMode::Offline && cmd == "change_server" {
                resp_transmitter
                    .send(
                        "Error: this session is in Offline mode; no Indexer may be configured. \
                         Restart without --offline to change servers."
                            .to_string(),
                    )
                    .unwrap();
                continue;
            }
            let args: Vec<_> = args.iter().map(std::convert::AsRef::as_ref).collect();

            let cmd_response = commands::do_user_command(&cmd, &args[..], &mut lightclient);
            resp_transmitter.send(cmd_response).unwrap();

            if cmd == "quit" || cmd == "exit" {
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
    /// Execute a single command and exit.
    Command {
        /// The command name (e.g. "balance", "send").
        name: String,
        /// Additional positional arguments for the command.
        args: Vec<String>,
    },
}

/// Determines the mode of operation from parsed CLI arguments.
///
/// Returns [`ModeOfOperation::Command`] if a command is given, or
/// [`ModeOfOperation::Interactive`] when no command is given.
///
/// The `help` command is handled separately before this function is called,
/// so it will never appear as a [`ModeOfOperation::Command`].
fn get_mode_of_operation(matches: &clap::ArgMatches) -> ModeOfOperation {
    if let Some(cmd_name) = matches.get_one::<String>("COMMAND") {
        let args = matches
            .get_many::<String>("extra_args")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();
        ModeOfOperation::Command {
            name: cmd_name.clone(),
            args,
        }
    } else {
        ModeOfOperation::Interactive
    }
}

/// Whether the CLI communicates with a remote indexer or operates locally.
///
/// Selected at argument-parse time by the `--offline` flag and pinned for
/// the life of the session (Offline mode, issue #2286).
#[derive(Debug, Clone, Copy, PartialEq)]
enum CommunicationMode {
    /// Connected to a remote indexer for sync, send, etc.
    Online,
    /// The session never configures an Indexer: the client remains
    /// Indexerless, and only that state's capability set is available.
    Offline,
}

/// Determines the communication mode from parsed CLI arguments: the
/// `--offline` flag pins the session to Offline mode.
fn get_communication_mode(matches: &clap::ArgMatches) -> CommunicationMode {
    if matches.get_flag("offline") {
        CommunicationMode::Offline
    } else {
        CommunicationMode::Online
    }
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
    /// The Indexer to connect to; `None` exactly when the session is in
    /// Offline mode.
    server: Option<http::Uri>,
    /// All servers that responded to `get_info()` during dynamic selection,
    /// sorted fastest to slowest. Empty if `--server` was specified explicitly.
    /// Will be used for automatic failover when sync fails.
    #[allow(dead_code)]
    ranked_servers: Vec<server_select::RankedServer>,
    seed: Option<String>,
    ufvk: Option<String>,
    birthday: u64,
    data_dir: PathBuf,
    sync: bool,
    waitsync: bool,
    chaintype: ChainType,
}

impl ConfigTemplate {
    fn fill(
        mode: ModeOfOperation,
        communication_mode: CommunicationMode,
        matches: clap::ArgMatches,
    ) -> Result<Self, String> {
        let seed = matches.get_one::<String>("seed").cloned();
        let ufvk = matches.get_one::<String>("viewkey").cloned();
        if seed.is_some() && ufvk.is_some() {
            return Err("Cannot load a wallet from both seed phrase and viewkey!".to_string());
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
            return Err(
                "This should be the block height where the wallet was created.\
If you don't remember the block height, you can pass '--birthday 0' to scan from the start of the blockchain."
                    .to_string(),
            );
        }
        let birthday = match maybe_birthday.unwrap_or("0".to_string()).parse::<u64>() {
            Ok(b) => b,
            Err(e) => {
                return Err(format!(
                    "Couldn't parse birthday. This should be a block number. Error={e}"
                ));
            }
        };

        let data_dir = if let Some(dir) = matches.get_one::<String>("data-dir") {
            PathBuf::from(dir.clone())
        } else {
            PathBuf::from("wallets")
        };
        log::info!("data_dir: {}", &data_dir.to_str().unwrap());
        // Offline mode never resolves a server — resolution probes the
        // network, and the session's contract is that no Indexer is ever
        // configured.
        let (server, ranked_servers) = match communication_mode {
            CommunicationMode::Offline => (None, vec![]),
            CommunicationMode::Online => {
                let (server, ranked_servers) =
                    server_select::resolve_server(&matches).map_err(|e| e.to_string())?;
                // Test to make sure the server has all of scheme, host and port
                if server.scheme_str().is_none()
                    || server.host().is_none()
                    || server.port().is_none()
                {
                    return Err(format!(
                        "Please provide the --server parameter as [scheme]://[host]:[port].\nYou provided: {server}"
                    ));
                }
                (Some(server), ranked_servers)
            }
        };
        let chaintype = if let Some(chain) = matches.get_one::<String>("chain") {
            ChainType::try_from(chain.as_str()).map_err(|e| e.to_string())?
        } else {
            ChainType::Mainnet
        };

        let sync = !matches.get_flag("nosync") && communication_mode == CommunicationMode::Online;
        let waitsync = matches.get_flag("waitsync");
        Ok(Self {
            mode,
            communication_mode,
            server,
            ranked_servers,
            seed,
            ufvk,
            birthday,
            data_dir,
            sync,
            waitsync,
            chaintype,
        })
    }
}

/// Builds a `ClientConfig` from the filled config template.
///
/// This is a pure function — no I/O or side effects — and is the
/// first testable seam inside the startup sequence.
fn build_zingo_config(filled_template: &ConfigTemplate) -> std::io::Result<ClientConfig> {
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
        println!("Creating a new wallet");
        let chain_height = match filled_template.server.clone() {
            Some(server) => RT
                .block_on(async move {
                    zingo_netutils::GrpcIndexer::new(server)
                        .await
                        .map_err(|e| format!("{e:?}"))?
                        .get_latest_block(DEFAULT_REQUEST_TIMEOUT)
                        .await
                        .map(|block_id| block_id.height as u32)
                        .map_err(|e| format!("{e:?}"))
                })
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
    Ok(builder.build())
}

pub(crate) fn startup(filled_template: &ConfigTemplate) -> std::io::Result<CommandChannel> {
    let config = build_zingo_config(filled_template)?;

    let mut lightclient = RT.block_on(async move {
        LightClient::new(config, false)
            .await
            .map_err(|e| std::io::Error::other(format!("Failed to create lightclient. {e}")))
    })?;

    if matches!(filled_template.mode, ModeOfOperation::Interactive) {
        // Print startup Messages
        info!(""); // Blank line
        info!("Starting Zingo-CLI");
        match &filled_template.server {
            Some(server) => info!("Lightclient connecting to {server}"),
            None => info!("Offline mode: no Indexer will be configured this session"),
        }
    }

    if filled_template.sync {
        let update = commands::do_user_command("sync", &["run"], &mut lightclient);
        println!("{update}");
    }

    let update = commands::do_user_command("save", &["run"], &mut lightclient);
    println!("{update}");

    lightclient = RT.block_on(async move {
        if filled_template.sync
            && filled_template.waitsync
            && let Err(e) = lightclient.await_sync().await
        {
            eprintln!("error: {e}");
        }

        lightclient
    });

    // Start the command loop
    Ok(command_loop(
        lightclient,
        filled_template.communication_mode,
    ))
}

fn dispatch_command_or_start_interactive(cli_config: &ConfigTemplate) -> std::io::Result<()> {
    let ch = startup(cli_config)?;
    match &cli_config.mode {
        ModeOfOperation::Interactive => start_interactive(cli_config, ch),
        ModeOfOperation::Command { name, args } => {
            ch.transmitter
                .send(Request::Command(name.clone(), args.clone()))
                .unwrap();

            match ch.receiver.recv() {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    let e = format!("Error executing command {name}: {e}");
                    eprintln!("{e}");
                    error!("{e}");
                }
            }

            ch.transmitter
                .send(Request::Command("quit".to_string(), vec![]))
                .unwrap();
            match ch.receiver.recv() {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("{e}");
                }
            }
        }
    }
    Ok(())
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

/// Returns help text if the parsed arguments indicate the `help` command,
/// or `None` for all other modes. The caller is responsible for printing
/// the text and exiting the process.
pub fn help_output(matches: &clap::ArgMatches) -> Option<String> {
    if matches.get_one::<String>("COMMAND").map(String::as_str) == Some("help") {
        let args: Vec<String> = matches
            .get_many::<String>("extra_args")
            .map(|v| v.cloned().collect())
            .unwrap_or_default();
        Some(commands::HelpCommand::exec_without_lc(args))
    } else {
        None
    }
}

/// Runs the CLI from pre-parsed arguments.
///
/// This function never calls `std::process::exit` or reads `std::env::args`.
/// The caller (the binary entry point) is responsible for parsing arguments,
/// handling the help short-circuit, process-level setup, and error reporting.
pub fn run_cli(matches: clap::ArgMatches) -> std::io::Result<()> {
    let mode = get_mode_of_operation(&matches);
    let communication_mode = get_communication_mode(&matches);
    let cli_config =
        ConfigTemplate::fill(mode, communication_mode, matches).map_err(std::io::Error::other)?;
    dispatch_command_or_start_interactive(&cli_config)
}

#[cfg(test)]
mod tests;
