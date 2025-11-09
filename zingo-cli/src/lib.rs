//! `ZingoCli`
//! TODO: Add Crate Description Here!

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod commands;

#[cfg(feature = "regtest")]
mod regtest;

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use bip0039::Mnemonic;
use clap::{self, Arg};
use log::{error, info};

use zcash_protocol::consensus::BlockHeight;

use commands::ShortCircuitedCommand;
use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
use zingolib::config::ChainType;
use zingolib::lightclient::LightClient;

use zingolib::wallet::{LightWallet, WalletBase, WalletSettings};

use crate::commands::RT;

pub mod version;

/// TODO: Add Doc Comment Here!
pub fn build_clap_app() -> clap::ArgMatches {
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
                .help(if cfg!(feature = "regtest") {
                    r#"What chain to expect. One of "mainnet", "testnet", or "regtest". Defaults to "mainnet""#
                } else {
                    r#"What chain to expect. One of "mainnet" or "testnet". Defaults to "mainnet""#
                }))
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
                .default_value(zingolib::config::DEFAULT_LIGHTWALLETD_SERVER))
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
        ).get_matches()
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

/// TODO: `start_interactive` does not explicitly reference a wallet, do we need
/// to expose new/more/higher-layer abstractions to facilitate wallet reuse from
/// the CLI?
fn start_interactive(
    command_transmitter: Sender<(String, Vec<String>)>,
    resp_receiver: Receiver<String>,
) {
    // `()` can be used when no completer is required
    let mut rl = rustyline::DefaultEditor::new().expect("Default rustyline Editor not creatable!");

    log::debug!("Ready!");

    let send_command = |cmd: String, args: Vec<String>| -> String {
        command_transmitter.send((cmd.clone(), args)).unwrap();
        match resp_receiver.recv() {
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

        match send_command("sync".to_string(), vec!["poll".to_string()]) {
            poll if poll.starts_with("Error:") => {
                eprintln!("Sync error: {poll}\nPlease restart sync with `sync run`.");
            }
            poll if poll.starts_with("Sync completed succesfully:") => println!("{poll}"),
            _ => (),
        }

        match send_command("save".to_string(), vec!["check".to_string()]) {
            check if check.starts_with("Error:") => eprintln!("{check}"),
            _ => (),
        }

        let readline = rl.readline(&format!("({chain_name}) Block:{height} (type 'help') >> "));
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
                if line == "quit" {
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

/// TODO: Add Doc Comment Here!
pub fn command_loop(
    mut lightclient: LightClient,
) -> (Sender<(String, Vec<String>)>, Receiver<String>) {
    let (command_transmitter, command_receiver) = channel::<(String, Vec<String>)>();
    let (resp_transmitter, resp_receiver) = channel::<String>();

    std::thread::spawn(move || {
        while let Ok((cmd, args)) = command_receiver.recv() {
            let args: Vec<_> = args.iter().map(std::convert::AsRef::as_ref).collect();

            let cmd_response = commands::do_user_command(&cmd, &args[..], &mut lightclient);
            resp_transmitter.send(cmd_response).unwrap();

            if cmd == "quit" {
                info!("Quit");
                break;
            }
        }
    });

    (command_transmitter, resp_receiver)
}

/// TODO: Add Doc Comment Here!
pub struct ConfigTemplate {
    params: Vec<String>,
    server: http::Uri,
    seed: Option<String>,
    ufvk: Option<String>,
    birthday: u64,
    data_dir: PathBuf,
    sync: bool,
    waitsync: bool,
    command: Option<String>,
    chaintype: ChainType,
    tor_enabled: bool,
}

impl ConfigTemplate {
    fn fill(matches: clap::ArgMatches) -> Result<Self, String> {
        let tor_enabled = matches.get_flag("tor");
        let params = if let Some(vals) = matches.get_many::<String>("extra_args") {
            vals.cloned().collect()
        } else {
            vec![]
        };
        let command = if let Some(refstr) = matches.get_one::<String>("COMMAND") {
            if refstr == &"help".to_string() {
                short_circuit_on_help(params.clone());
            }
            Some(refstr.to_string())
        } else {
            None
        };
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
        let server = zingolib::config::construct_lightwalletd_uri(server);
        let chaintype = if let Some(chain) = matches.get_one::<String>("chain") {
            zingolib::config::chain_from_str(chain).map_err(|e| e.to_string())?
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
            params,
            server,
            seed,
            ufvk,
            birthday,
            data_dir,
            sync,
            waitsync,
            command,
            chaintype,
            tor_enabled,
        })
    }
}

/// A (command, args) request
pub type CommandRequest = (String, Vec<String>);

/// Command responses are strings
pub type CommandResponse = String;

/// Used by the zingocli crate, and the zingo-mobile application:
/// <https://github.com/zingolabs/zingolib/tree/dev/cli>
/// <https://github.com/zingolabs/zingo-mobile>
pub fn startup(
    filled_template: &ConfigTemplate,
) -> std::io::Result<(Sender<CommandRequest>, Receiver<CommandResponse>)> {
    let config = zingolib::config::load_clientconfig(
        filled_template.server.clone(),
        Some(filled_template.data_dir.clone()),
        filled_template.chaintype,
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                performance_level: PerformanceLevel::High,
            },
            min_confirmations: NonZeroU32::try_from(3).unwrap(),
        },
        1.try_into().unwrap(),
        "".to_string(),
    )
    .unwrap();

    let mut lightclient = if let Some(seed_phrase) = filled_template.seed.clone() {
        LightClient::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::Mnemonic {
                    mnemonic: Mnemonic::from_phrase(seed_phrase).map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!("Invalid seed phrase. {e}"),
                        )
                    })?,
                    no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
                },
                (filled_template.birthday as u32).into(),
                config.wallet_settings.clone(),
            )
            .map_err(|e| std::io::Error::other(format!("Failed to create wallet. {e}")))?,
            config.clone(),
            false,
        )
        .map_err(|e| std::io::Error::other(format!("Failed to create lightclient. {e}")))?
    } else if let Some(ufvk) = filled_template.ufvk.clone() {
        // Create client from UFVK
        LightClient::create_from_wallet(
            LightWallet::new(
                config.chain,
                WalletBase::Ufvk(ufvk),
                (filled_template.birthday as u32).into(),
                config.wallet_settings.clone(),
            )
            .map_err(|e| std::io::Error::other(format!("Failed to create wallet. {e}")))?,
            config.clone(),
            false,
        )
        .map_err(|e| std::io::Error::other(format!("Failed to create lightclient. {e}")))?
    } else if config.wallet_path_exists() {
        // Open existing wallet from path
        LightClient::create_from_wallet_path(config.clone())
            .map_err(|e| std::io::Error::other(format!("Failed to create lightclient. {e}")))?
    } else {
        // Fresh wallet: query chain tip and initialize at tip-100 to guard against reorgs
        println!("Creating a new wallet");
        // Call the lightwalletd server to get the current block-height
        // Do a getinfo first, before opening the wallet
        let server_uri = config.get_lightwalletd_uri();

        let chain_height = RT
            .block_on(async move {
                zingolib::grpc_connector::get_latest_block(server_uri)
                    .await
                    .map(|block_id| BlockHeight::from_u32(block_id.height as u32))
            })
            .map_err(|e| std::io::Error::other(format!("Failed to create lightclient. {e}")))?;

        LightClient::new(config.clone(), chain_height - 100, false)
            .map_err(|e| std::io::Error::other(format!("Failed to create lightclient. {e}")))?
    };

    if filled_template.command.is_none() {
        // Print startup Messages
        info!(""); // Blank line
        info!("Starting Zingo-CLI");
        info!("Light Client config {config:?}");

        info!(
            "Lightclient connecting to {}",
            config.get_lightwalletd_uri()
        );
    }

    if filled_template.tor_enabled {
        info!("Creating tor client");
        lightclient = RT.block_on(async move {
            if let Err(e) = lightclient.create_tor_client(None).await {
                eprintln!("error: failed to create tor client. price updates disabled. {e}");
            }
            lightclient
        });
    }

    // At startup, run a sync.
    if filled_template.sync {
        let update = commands::do_user_command("sync", &["run"], &mut lightclient);
        println!("{update}");
    }

    let update = commands::do_user_command("save", &["run"], &mut lightclient);
    println!("{update}");

    // Start the command loop
    let (command_transmitter, resp_receiver) = command_loop(lightclient);

    Ok((command_transmitter, resp_receiver))
}
fn start_cli_service(
    cli_config: &ConfigTemplate,
) -> (Sender<(String, Vec<String>)>, Receiver<String>) {
    match startup(cli_config) {
        Ok(c) => c,
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
    let (command_transmitter, resp_receiver) = start_cli_service(cli_config);
    if cli_config.command.is_none() {
        start_interactive(command_transmitter, resp_receiver);
    } else {
        // Optionally wait for background sync to finish before executing command
        if cli_config.sync && cli_config.waitsync {
            use std::{thread, time::Duration};
            loop {
                // Poll sync task status
                command_transmitter
                    .send(("sync".to_string(), vec!["poll".to_string()]))
                    .unwrap();
                match resp_receiver.recv() {
                    Ok(resp) => {
                        if resp.starts_with("Error:") {
                            eprintln!(
                                "Sync error while waiting: {resp}\nProceeding to execute the command."
                            );
                            break;
                        } else if resp.starts_with("Sync completed succesfully:") {
                            // Sync finished; proceed
                            break;
                        } else if resp == "Sync task has not been launched." {
                            // Try to launch sync and continue waiting
                            command_transmitter
                                .send(("sync".to_string(), vec!["run".to_string()]))
                                .unwrap();
                            let _ = resp_receiver.recv();
                            thread::sleep(Duration::from_millis(500));
                        } else {
                            // Not ready yet
                            thread::sleep(Duration::from_millis(500));
                        }
                    }
                    Err(_) => break,
                }
            }
        }

        command_transmitter
            .send((
                cli_config.command.clone().unwrap(),
                cli_config
                    .params
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<String>>(),
            ))
            .unwrap();

        match resp_receiver.recv() {
            Ok(s) => println!("{s}"),
            Err(e) => {
                let e = format!(
                    "Error executing command {}: {}",
                    cli_config.command.clone().unwrap(),
                    e
                );
                eprintln!("{e}");
                error!("{e}");
            }
        }

        command_transmitter
            .send(("quit".to_string(), vec![]))
            .unwrap();
        match resp_receiver.recv() {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("{e}");
            }
        }
    }
}

/// TODO: Add Doc Comment Here!
pub fn run_cli() {
    // Initialize logging
    match ConfigTemplate::fill(build_clap_app()) {
        Ok(cli_config) => dispatch_command_or_start_interactive(&cli_config),
        Err(e) => eprintln!("Error filling config template: {e:?}"),
    }
}

/// Run CLI in regtest mode
/// This function is only available when the regtest feature is enabled
/// It bypasses clap entirely and directly sets up a regtest environment
#[cfg(feature = "regtest")]
pub fn run_regtest_cli() {
    use crate::commands::RT;

    println!("Launching local regtest network...");

    // Launch the local network first
    let local_net = RT.block_on(regtest::launch_local_net());

    // Get the lightwalletd port from the launched network
    let lightwalletd_port = local_net.indexer().port();

    println!("Local network launched on port {lightwalletd_port}");

    // Create a regtest-specific config directly
    let data_dir = regtest::get_regtest_dir();

    // Ensure the regtest directory exists
    std::fs::create_dir_all(&data_dir).expect("Failed to create regtest directory");

    // Use a temporary directory for wallet data in regtest
    let wallet_dir = zingolib::testutils::tempfile::tempdir().expect("Failed to create temp dir");
    let wallet_data_dir = wallet_dir.path().to_path_buf();

    let cli_config = ConfigTemplate {
        params: vec![],
        server: zingolib::config::construct_lightwalletd_uri(Some(format!(
            "http://127.0.0.1:{lightwalletd_port}"
        ))),
        seed: None,
        ufvk: None,
        birthday: 0,
        data_dir: wallet_data_dir,
        sync: false, // Don't auto-sync in regtest
        waitsync: false,
        command: None,
        chaintype: ChainType::Regtest(
            zingo_common_components::protocol::activation_heights::for_test::all_height_one_nus(),
        ),
        tor_enabled: false,
    };

    // Start the CLI in interactive mode
    dispatch_command_or_start_interactive(&cli_config);
}

fn short_circuit_on_help(params: Vec<String>) {
    for h in commands::HelpCommand::exec_without_lc(params).lines() {
        println!("{h}");
    }
    std::process::exit(0x0100);
}
