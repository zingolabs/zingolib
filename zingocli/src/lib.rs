#![forbid(unsafe_code)]
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use log::{error, info};

use clap::{self, Arg};
use zingo_testutils::regtest;
use zingoconfig::ChainType;
use zingolib::wallet::keys::unified::WalletCapability;
use zingolib::wallet::WalletBase;
use zingolib::{commands, lightclient::LightClient};

pub mod version;

fn ufvk_to_string_helper(raw_ufvk: &str) -> Result<String, zingolib::error::ZingoLibError> {
    match WalletCapability::parse_string_encoded_ufvk(raw_ufvk) {
        Ok((_, _)) => Ok(raw_ufvk.to_string()),
        Err(_) => Err(zingolib::error::ZingoLibError::CouldNotParseUfvkString(
            raw_ufvk.to_string(),
        )),
    }
}
#[test]
fn helper_handles_raw_strings() {
    let res = ufvk_to_string_helper("f");
    assert!(res.is_err());
    let res2 = ufvk_to_string_helper(zingolib::testvectors::MAINNET_ALPHA_VIEWKEY);
    assert!(res2.is_ok());
    let last = res2.as_ref().unwrap().len() - 1;
    let dropped_char = &res2.unwrap()[0..last];
    let res3 = ufvk_to_string_helper(dropped_char);
    assert!(res3.is_err())
}
pub fn build_clap_app() -> clap::ArgMatches {
    clap::Command::new("Zingo CLI").version(version::VERSION)
            .arg(Arg::new("nosync")
                .help("By default, zingo-cli will sync the wallet at startup. Pass --nosync to prevent the automatic sync at startup.")
                .long("nosync").short('n')
                .action(clap::ArgAction::SetTrue))
            .arg(Arg::new("regtest")
                .long("regtest")
                .help("Regtest mode")
                .action(clap::ArgAction::SetTrue) )
            .arg(Arg::new("no-clean")
                .long("no-clean")
                .help("Don't clean regtest state before running. Regtest mode only")
                .action(clap::ArgAction::SetTrue))
            .arg(Arg::new("chain")
                .long("chain").short('c')
                .help(r#"What chain to expect, if it's not inferrable from the server URI. One of "mainnet", "testnet", or "regtest""#))
            .arg(Arg::new("seed-phrase")
                .requires("birthday")
                .long("seed-phrase")
                .env("SEED_PHRASE")
                .hide_env(true)
                .value_parser(WalletBase::parse_input_to_phrase)
                .help("Create a new wallet with the given seed phrase. Cannot be used with --view-key, or --load-wallet.
                     \nNOTE: This value may be read from the SEED_PHRASE environment variable.\n"))
            .arg(Arg::new("view-key")
                .requires("birthday")
                .long("view-key")
                .env("VIEW_KEY")
                .hide_env(true)
                .value_parser(ufvk_to_string_helper)
                .help("Create a new wallet with the given viewkey. Cannot be used with --seed-phrase, or --load-wallet.
                     \nNOTE: This value may be read from the VIEW_KEY environment variable.\n"))
            .arg(Arg::new("birthday")
                .requires("fresh_sources")
                .conflicts_with("load-wallet")
                .long("birthday")
                .value_name("birthday")
                .value_parser(clap::value_parser!(u32))
                .help("Specify wallet birthday when restoring from seed. This is the earlist block height where the wallet has a transaction."))
            .arg(Arg::new("load-wallet")
                .conflicts_with("birthday")
                .long("load-wallet")
                .value_name("load-wallet")
                .help("Launch with an existing wallet specified by the value which is the absolute path to the wallet. Cannot be used with --seed-phrase, or --view-key."))
            .arg(Arg::new("output-dir")
                .conflicts_with("load-wallet")
                .requires("fresh_sources") // each fresh source directly requires birthday
                .long("output-dir")
                .value_name("output-dir")
                .help("Specify where the newly created wallet will be located. Cannot be used *without* --seed-phrase, or --view-key."))
            .arg(Arg::new("server")
                .long("server")
                .value_name("server")
                .help("Lightwalletd server to connect to.")
                .value_parser(parse_uri)
                .default_value(zingoconfig::DEFAULT_LIGHTWALLETD_SERVER))
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
        .group(clap::ArgGroup::new("all_sources").args(["load-wallet", "seed-phrase", "view-key"]).multiple(false))
        .group(clap::ArgGroup::new("fresh_sources").args(["seed-phrase", "view-key"]))
        .get_matches()
}

/// Custom function to parse a string into an http::Uri
fn parse_uri(s: &str) -> Result<http::Uri, String> {
    s.parse::<http::Uri>().map_err(|e| e.to_string())
}
#[cfg(target_os = "linux")]
/// This function is only tested against Linux.
fn report_permission_error() {
    let user = std::env::var("USER").expect("Unexpected error reading value of $USER!");
    let home = std::env::var("HOME").expect("Unexpected error reading value of $HOME!");
    let current_executable =
        std::env::current_exe().expect("Unexpected error reporting executable path!");
    eprintln!("USER: {}", user);
    eprintln!("HOME: {}", home);
    eprintln!("Executable: {}", current_executable.display());
    if home == "/" {
        eprintln!(
            "User {} must have permission to write to '{}.zcash/' .",
            user, home
        );
    } else {
        eprintln!(
            "User {} must have permission to write to '{}/.zcash/' .",
            user, home
        );
    }
}

/// If the regtest flag was passed but a non regtest network is selected
/// exit immediately and vice versa.
fn regtest_config_check(regtest_manager: &Option<regtest::RegtestManager>, chain: &ChainType) {
    match (regtest_manager.is_some(), chain) {
        (true, ChainType::Regtest(_)) => println!("regtest detected and network set correctly!"),
        (true, _) => panic!("Regtest flag detected, but unexpected network set! Exiting."),
        (false, ChainType::Regtest(_)) => {
            println!("WARNING! regtest network in use but no regtest flag recognized!")
        }
        _ => {}
    }
}

/// TODO: start_interactive does not explicitly reference a wallet, do we need
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
                let e = format!("Error executing command {}: {}", cmd, e);
                eprintln!("{}", e);
                error!("{}", e);
                "".to_string()
            }
        }
    };

    let mut chain_name = "".to_string();

    loop {
        if chain_name.is_empty() {
            let info = send_command("info".to_string(), vec![]);
            chain_name = json::parse(&info)
                .map(|mut json_info| json_info.remove("chain_name"))
                .ok()
                .and_then(|name| name.as_str().map(ToString::to_string))
                .unwrap_or("".to_string());
        }
        // Read the height first
        let height = json::parse(&send_command(
            "height".to_string(),
            vec!["false".to_string()],
        ))
        .unwrap()["height"]
            .as_i64()
            .unwrap();

        let readline = rl.readline(&format!(
            "({}) Block:{} (type 'help') >> ",
            chain_name, height
        ));
        match readline {
            Ok(line) => {
                rl.add_history_entry(line.as_str())
                    .expect("Ability to add history entry");
                // Parse command line arguments
                let mut cmd_args = match shellwords::split(&line) {
                    Ok(args) => args,
                    Err(_) => {
                        println!("Mismatched Quotes");
                        continue;
                    }
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
                println!("Error: {:?}", err);
                break;
            }
        }
    }
}

pub fn command_loop(
    lightclient: Arc<LightClient>,
) -> (Sender<(String, Vec<String>)>, Receiver<String>) {
    let (command_transmitter, command_receiver) = channel::<(String, Vec<String>)>();
    let (resp_transmitter, resp_receiver) = channel::<String>();

    std::thread::spawn(move || {
        LightClient::start_mempool_monitor(lightclient.clone());

        while let Ok((cmd, args)) = command_receiver.recv() {
            let args: Vec<_> = args.iter().map(|s| s.as_ref()).collect();

            let cmd_response = commands::do_user_command(&cmd, &args[..], lightclient.as_ref());
            resp_transmitter.send(cmd_response).unwrap();

            if cmd == "quit" {
                info!("Quit");
                break;
            }
        }
    });

    (command_transmitter, resp_receiver)
}

pub struct ConfigTemplate {
    params: Vec<String>,
    server: http::Uri,
    from: Option<String>,
    birthday: u64,
    data_dir: PathBuf,
    sync: bool,
    command: Option<String>,
    regtest_manager: Option<regtest::RegtestManager>,
    #[allow(dead_code)] // This field is defined so that it can be used in Drop::drop
    child_process_handler: Option<regtest::ChildProcessHandler>,
    chaintype: ChainType,
}
use commands::ShortCircuitedCommand;
fn short_circuit_on_help(params: Vec<String>) {
    for h in commands::HelpCommand::exec_without_lc(params).lines() {
        println!("{}", h);
    }
    std::process::exit(0x0100);
}
use std::string::String;
#[derive(Debug)]
enum TemplateFillError {
    _BirthdaylessSeed(String),
    InvalidBirthday(String),
    MalformedServerURL(String),
    ChildLaunchError(regtest::LaunchChildProcessError),
    InvalidChain(String),
    RegtestAndChainSpecified(String),
}

impl From<regtest::LaunchChildProcessError> for TemplateFillError {
    fn from(underlyingerror: regtest::LaunchChildProcessError) -> Self {
        Self::ChildLaunchError(underlyingerror)
    }
}
fn get_from_and_set_to(matches: &clap::ArgMatches, is_regtest: bool) -> (Option<String>, PathBuf) {
    (
        if matches.get_one::<String>("seed-phrase").is_some() {
            matches.get_one::<String>("seed-phrase").cloned()
        } else if matches.get_one::<String>("view-key").is_some() {
            matches.get_one::<String>("view-key").cloned()
        } else {
            None
        },
        if let Some(dir) = matches.get_one::<String>("load-wallet") {
            PathBuf::from(dir.clone())
        } else if is_regtest {
            // Begin short_circuit section
            regtest::get_regtest_dir()
        } else {
            PathBuf::from("wallets")
        },
    )
}
fn get_birthday(matches: &clap::ArgMatches) -> Result<u64, TemplateFillError> {
    match matches
        .get_one::<u32>("birthday")
        .map(|bday| bday.to_string())
        .unwrap_or("0".to_string())
        .parse::<u64>()
    {
        Ok(b) => Ok(b),
        Err(e) => Err(TemplateFillError::InvalidBirthday(format!(
            "Couldn't parse birthday. This should be a block number. Error={}",
            e
        ))),
    }
}
/// This type manages setup of the zingo-cli utility among its responsibilities:
///  * parse arguments with standard clap: <https://crates.io/crates/clap>
///  * behave correctly as a function of each parameter that may have been passed
///      * add details of above here
///  * handle parameters as efficiently as possible.
///      * If a ShortCircuitCommand
///    is specified, then the system should execute only logic necessary to support that command,
///    in other words "help" the ShortCircuitCommand _MUST_ not launch either zcashd or lightwalletd
impl ConfigTemplate {
    fn fill(matches: clap::ArgMatches) -> Result<Self, TemplateFillError> {
        let is_regtest = matches.get_flag("regtest"); // Begin short_circuit section
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
        if matches.contains_id("chain") && is_regtest {
            return Err(TemplateFillError::RegtestAndChainSpecified(
                "regtest mode incompatible with custom chain selection".to_string(),
            ));
        }

        let clean_regtest_data = !matches.get_flag("no-clean");
        let (from, data_dir) = get_from_and_set_to(&matches, is_regtest);
        log::info!("data_dir: {}", &data_dir.to_str().unwrap());
        let mut server = matches
            .get_one::<http::Uri>("server")
            .map(|server| server.to_string());
        let mut child_process_handler = None;
        // Regtest specific launch:
        //   * spawn zcashd in regtest mode
        //   * spawn lighwalletd and connect it to zcashd
        let regtest_manager = if is_regtest {
            let regtest_manager = regtest::RegtestManager::new(data_dir.clone());
            child_process_handler = Some(regtest_manager.launch(clean_regtest_data)?);
            server = Some("http://127.0.0.1".to_string());
            Some(regtest_manager)
        } else {
            None
        };
        let server = zingoconfig::construct_lightwalletd_uri(server);
        let chaintype = if let Some(chain) = matches.get_one::<String>("chain") {
            match chain.as_str() {
                "mainnet" => ChainType::Mainnet,
                "testnet" => ChainType::Testnet,
                "regtest" => ChainType::Regtest(zingoconfig::RegtestNetwork::all_upgrades_active()),
                _ => return Err(TemplateFillError::InvalidChain(chain.clone())),
            }
        } else if is_regtest {
            ChainType::Regtest(zingoconfig::RegtestNetwork::all_upgrades_active())
        } else {
            ChainType::Mainnet
        };

        // Test to make sure the server has all of scheme, host and port
        if server.scheme_str().is_none() || server.host().is_none() || server.port().is_none() {
            return Err(TemplateFillError::MalformedServerURL(format!(
                "Please provide the --server parameter as [scheme]://[host]:[port].\nYou provided: {}",
                server )));
        }

        let sync = !matches.get_flag("nosync");
        Ok(Self {
            params,
            server,
            from,
            birthday: get_birthday(&matches)?,
            data_dir,
            sync,
            command,
            regtest_manager,
            child_process_handler,
            chaintype,
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
    // Try to get the configuration
    let data_dir = if let Some(regtest_manager) = filled_template.regtest_manager.clone() {
        regtest_manager.zingo_datadir
    } else {
        filled_template.data_dir.clone()
    };
    let config = zingoconfig::load_clientconfig(
        filled_template.server.clone(),
        Some(data_dir),
        filled_template.chaintype,
        true,
    )
    .unwrap();
    regtest_config_check(&filled_template.regtest_manager, &config.chain);

    let lightclient = match filled_template.from.clone() {
        Some(phrase) => Arc::new(LightClient::create_from_wallet_base(
            WalletBase::from_string(phrase),
            &config,
            filled_template.birthday,
            false,
        )?),
        None => {
            if config.wallet_path_exists() {
                Arc::new(LightClient::read_wallet_from_disk(&config)?)
            } else {
                println!("Creating a new wallet");
                // Call the lightwalletd server to get the current block-height
                // Do a getinfo first, before opening the wallet
                let server_uri = config.get_lightwalletd_uri();
                let block_height = zingolib::get_latest_block_height(server_uri);
                // Create a wallet with height - 100, to protect against reorgs
                Arc::new(LightClient::new(
                    &config,
                    block_height?.saturating_sub(100),
                )?)
            }
        }
    };

    if filled_template.command.is_none() {
        // Print startup Messages
        info!(""); // Blank line
        info!("Starting Zingo-CLI");
        info!("Light Client config {:?}", config);

        info!(
            "Lightclient connecting to {}",
            config.get_lightwalletd_uri()
        );
    }

    // At startup, run a sync.
    if filled_template.sync {
        let update = commands::do_user_command("sync", &[], lightclient.as_ref());
        println!("{}", update);
    }

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
            let emsg = format!("Error during startup:\n{}\n", e);
            eprintln!("{}", emsg);
            error!("{}", emsg);
            #[cfg(target_os = "linux")]
            // TODO: Test report_permission_error() for macos and change to target_family = "unix"
            if let Some(13) = e.raw_os_error() {
                report_permission_error()
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
        command_transmitter
            .send((
                cli_config.command.clone().unwrap(),
                cli_config
                    .params
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>(),
            ))
            .unwrap();

        match resp_receiver.recv() {
            Ok(s) => println!("{}", s),
            Err(e) => {
                let e = format!(
                    "Error executing command {}: {}",
                    cli_config.command.clone().unwrap(),
                    e
                );
                eprintln!("{}", e);
                error!("{}", e);
            }
        }
    }
}
pub fn run_cli() {
    // Initialize logging
    if let Err(e) = LightClient::init_logging() {
        eprintln!("Could not initialize logging: {e}")
    };
    let cli_config = ConfigTemplate::fill(build_clap_app()).unwrap();
    dispatch_command_or_start_interactive(&cli_config);
}
