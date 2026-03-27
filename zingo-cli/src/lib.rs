//! `ZingoCli`
//! TODO: Add Crate Description Here!

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod commands;
mod examples;

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use clap::{self, Arg};
use log::{error, info};

use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
use zingo_netutils::Indexer as _;
use zingolib::config::{ChainType, ClientConfig, DEFAULT_WALLET_NAME, WalletConfig};
use zingolib::lightclient::LightClient;
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
                .help("Specify wallet birthday when restoring from seed. This is the earliest block height where the wallet has a transaction."))
            .arg(Arg::new("server")
                .long("server")
                .value_name("server")
                .help("Lightwalletd server to connect to.")
                .value_parser(parse_uri)
                .default_value(zingolib::config::DEFAULT_INDEXER_URI))
            .arg(Arg::new("data-dir")
                .long("data-dir")
                .value_name("data-dir")
                .help("Absolute path to use as data directory"))
            .arg(Arg::new("tor")
                .long("tor")
                .help("Enable tor for price fetching")
                .action(clap::ArgAction::SetTrue) )
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
#[cfg(target_os = "linux")]
/// This function is only tested against Linux.
fn report_permission_error() {
    let user = std::env::var("USER").expect("Unexpected error reading value of $USER!");
    let home = std::env::var("HOME").expect("Unexpected error reading value of $HOME!");
    let current_executable =
        std::env::current_exe().expect("Unexpected error reporting executable path!");
    eprintln!("USER: {user}");
    eprintln!("HOME: {home}");
    eprintln!("Executable: {}", current_executable.display());
    if home == "/" {
        eprintln!("User {user} must have permission to write to '{home}.zcash/' .");
    } else {
        eprintln!("User {user} must have permission to write to '{home}/.zcash/' .");
    }
}

/// Polls the sync task and returns a string to embed in the interactive prompt.
///
/// Returns `" [Syncing X.X%]"` while sync is in progress, `" [Synced]"` when
/// fully synced, `" [Sync error]"` on failure, or `" [Not syncing X.X%]"` when
/// no sync task is running and the wallet is not fully synced.
fn poll_sync_for_prompt_indicator(send_command: &impl Fn(String, Vec<String>) -> String) -> String {
    let poll = send_command("sync".to_string(), vec!["poll".to_string()]);
    if poll.starts_with("Error:") {
        eprintln!("Sync error: {poll}\nPlease restart sync with `sync run`.");
        " [Sync error]".to_string()
    } else if poll.starts_with("Sync completed succesfully:") {
        println!("{poll}");
        " [Synced]".to_string()
    } else if poll == "Sync task is not complete." {
        let status = send_command("sync".to_string(), vec!["status".to_string()]);
        if let Ok(parsed) = json::parse(&status) {
            let pct = parsed["percentage_total_outputs_scanned"]
                .as_f32()
                .unwrap_or(0.0);
            format!(" [Syncing {pct:.1}% complete]")
        } else {
            " [Syncing]".to_string()
        }
    } else {
        sync_indicator_from_status(send_command)
    }
}

/// Checks sync status when no sync task is running.
///
/// Returns `" [Synced]"` if outputs are 100% scanned, otherwise
/// `" [Not syncing X.X%]"` to indicate incomplete sync without an active task.
fn sync_indicator_from_status(send_command: &impl Fn(String, Vec<String>) -> String) -> String {
    let status = send_command("sync".to_string(), vec!["status".to_string()]);
    if let Ok(parsed) = json::parse(&status) {
        let pct = parsed["percentage_total_outputs_scanned"]
            .as_f32()
            .unwrap_or(0.0);
        if pct >= 100.0 {
            " [Synced]".to_string()
        } else {
            format!(" [Not syncing {pct:.1}% complete]")
        }
    } else {
        " [Not syncing]".to_string()
    }
}

/// TODO: `start_interactive` does not explicitly reference a wallet, do we need
/// to expose new/more/higher-layer abstractions to facilitate wallet reuse from
/// the CLI?
fn start_interactive(ch: CommandChannel) {
    // `()` can be used when no completer is required
    let mut rl = rustyline::DefaultEditor::new().expect("Default rustyline Editor not creatable!");

    log::debug!("Ready!");

    let send_command = |cmd: String, args: Vec<String>| -> String {
        ch.transmitter.send((cmd.clone(), args)).unwrap();
        match ch.receiver.recv() {
            Ok(s) => s,
            Err(e) => {
                let e = format!("Error executing command {cmd}: {e}");
                eprintln!("{e}");
                error!("{e}");
                String::new()
            }
        }
    };

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

        let sync_indicator = poll_sync_for_prompt_indicator(&send_command);

        match send_command("save".to_string(), vec!["check".to_string()]) {
            check if check.starts_with("Error:") => eprintln!("{check}"),
            _ => (),
        }

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

/// A paired command/response channel for communicating with the background command loop.
struct CommandChannel {
    transmitter: Sender<(String, Vec<String>)>,
    receiver: Receiver<String>,
}

/// TODO: Add Doc Comment Here!
pub(crate) fn command_loop(mut lightclient: LightClient) -> CommandChannel {
    let (command_transmitter, command_receiver) = channel::<(String, Vec<String>)>();
    let (resp_transmitter, resp_receiver) = channel::<String>();

    std::thread::spawn(move || {
        while let Ok((cmd, args)) = command_receiver.recv() {
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

/// TODO: Add Doc Comment Here!
#[derive(Debug)]
pub(crate) struct ConfigTemplate {
    mode: ModeOfOperation,
    server: http::Uri,
    seed: Option<String>,
    ufvk: Option<String>,
    birthday: u64,
    data_dir: PathBuf,
    sync: bool,
    waitsync: bool,
    chaintype: ChainType,
    tor_enabled: bool,
}

impl ConfigTemplate {
    fn fill(mode: ModeOfOperation, matches: clap::ArgMatches) -> Result<Self, String> {
        let tor_enabled = matches.get_flag("tor");
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
        let server = matches
            .get_one::<http::Uri>("server")
            .map(ToString::to_string);
        log::info!("data_dir: {}", &data_dir.to_str().unwrap());
        let server =
            zingolib::config::construct_lightwalletd_uri(server).map_err(|e| e.to_string())?;
        let chaintype = if let Some(chain) = matches.get_one::<String>("chain") {
            ChainType::try_from(chain.as_str()).map_err(|e| e.to_string())?
        } else {
            ChainType::Mainnet
        };

        // Test to make sure the server has all of scheme, host and port
        if server.scheme_str().is_none() || server.host().is_none() || server.port().is_none() {
            return Err(format!(
                "Please provide the --server parameter as [scheme]://[host]:[port].\nYou provided: {server}"
            ));
        }

        let sync = !matches.get_flag("nosync");
        let waitsync = matches.get_flag("waitsync");
        Ok(Self {
            mode,
            server,
            seed,
            ufvk,
            birthday,
            data_dir,
            sync,
            waitsync,
            chaintype,
            tor_enabled,
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
        let chain_height = RT
            .block_on(async move {
                zingo_netutils::GrpcIndexer::new(filled_template.server.clone())
                    .map_err(|e| format!("{e:?}"))?
                    .get_latest_block()
                    .await
                    .map(|block_id| block_id.height as u32)
                    .map_err(|e| format!("{e:?}"))
            })
            .map_err(|e| std::io::Error::other(format!("Failed to create lightclient. {e}")))?;

        WalletConfig::NewSeed {
            no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
            chain_height,
            wallet_settings,
        }
    };

    Ok(ClientConfig::builder()
        .set_indexer_uri(filled_template.server.clone())
        .set_chain_type(filled_template.chaintype)
        .set_wallet_dir(filled_template.data_dir.clone())
        .set_wallet_config(wallet_config)
        .build())
}

pub(crate) fn startup(filled_template: &ConfigTemplate) -> std::io::Result<CommandChannel> {
    let config = build_zingo_config(filled_template)?;

    let mut lightclient = LightClient::new(config, false)
        .map_err(|e| std::io::Error::other(format!("Failed to create lightclient. {e}")))?;

    if matches!(filled_template.mode, ModeOfOperation::Interactive) {
        // Print startup Messages
        info!(""); // Blank line
        info!("Starting Zingo-CLI");
        info!("Lightclient connecting to {}", filled_template.server);
    }

    if filled_template.sync {
        let update = commands::do_user_command("sync", &["run"], &mut lightclient);
        println!("{update}");
    }

    let update = commands::do_user_command("save", &["run"], &mut lightclient);
    println!("{update}");

    lightclient = RT.block_on(async move {
        if filled_template.tor_enabled {
            info!("Creating tor client");
            if let Err(e) = lightclient.create_tor_client(None).await {
                eprintln!("error: failed to create tor client. price updates disabled. {e}");
            }
        }

        if filled_template.sync
            && filled_template.waitsync
            && let Err(e) = lightclient.await_sync().await
        {
            eprintln!("error: {e}");
        }

        lightclient
    });

    // Start the command loop
    Ok(command_loop(lightclient))
}

fn start_cli_service(cli_config: &ConfigTemplate) -> CommandChannel {
    match startup(cli_config) {
        Ok(ch) => ch,
        Err(e) => {
            let emsg = format!("Error during startup:\n{e}\n");
            eprintln!("{emsg}");
            error!("{emsg}");
            #[cfg(target_os = "linux")]
            // TODO: Test report_permission_error() for macos and change to target_family = "unix"
            if let Some(13) = e.raw_os_error() {
                report_permission_error();
            }
            panic!();
        }
    }
}

fn dispatch_command_or_start_interactive(cli_config: &ConfigTemplate) {
    let ch = start_cli_service(cli_config);
    match &cli_config.mode {
        ModeOfOperation::Interactive => {
            start_interactive(ch);
        }
        ModeOfOperation::Command { name, args } => {
            ch.transmitter.send((name.clone(), args.clone())).unwrap();

            match ch.receiver.recv() {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    let e = format!("Error executing command {name}: {e}");
                    eprintln!("{e}");
                    error!("{e}");
                }
            }

            ch.transmitter.send(("quit".to_string(), vec![])).unwrap();
            match ch.receiver.recv() {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("{e}");
                }
            }
        }
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
/// handling the help short-circuit, and process-level setup.
pub fn run_cli(matches: clap::ArgMatches) {
    match ConfigTemplate::fill(get_mode_of_operation(&matches), matches) {
        Ok(cli_config) => dispatch_command_or_start_interactive(&cli_config),
        Err(e) => eprintln!("Error filling config template: {e:?}"),
    }
}

#[cfg(test)]
mod tests;
