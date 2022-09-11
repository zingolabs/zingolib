use std::sync::mpsc::{Receiver, Sender};

use log::error;
use zingo_cli::{
    attempt_recover_seed, configure_app, regtest, report_permission_error, start_interactive,
    startup,
};
use zingoconfig::ZingoConfig;
struct Matches {
    params: Vec<String>,
    password: Option<String>,
    recover: bool,
    server: http::Uri,
    seed: Option<String>,
    birthday: u64,
    maybe_data_dir: Option<String>,
    sync: bool,
    command: Option<String>,
    regtest_mode_enabled: bool,
}
impl Matches {
    fn new() -> Self {
        let configured_app = configure_app();
        let matches = configured_app.get_matches();
        let recover = matches.is_present("recover");
        let seed = matches.value_of("seed").map(|s| s.to_string());
        let maybe_birthday = matches.value_of("birthday");
        if seed.is_some() && maybe_birthday.is_none() {
            eprintln!("ERROR!");
            eprintln!(
            "Please specify the wallet birthday (eg. '--birthday 600000') to restore from seed."
        );
            panic!("This should be the block height where the wallet was created. If you don't remember the block height, you can pass '--birthday 0' to scan from the start of the blockchain.");
        }
        let birthday = match maybe_birthday.unwrap_or("0").parse::<u64>() {
            Ok(b) => b,
            Err(e) => {
                panic!(
                    "Couldn't parse birthday. This should be a block number. Error={}",
                    e
                );
            }
        };

        let command = matches.value_of("COMMAND");
        let params = matches
            .values_of("PARAMS")
            .map(|v| v.collect())
            .or(Some(vec![]))
            .unwrap();

        let params = params.iter().map(|s| s.to_string()).collect();
        let maybe_server = matches.value_of("server").map(|s| s.to_string());

        let maybe_data_dir = matches.value_of("data-dir").map(|s| s.to_string());

        let regtest_mode_enabled = matches.is_present("regtest");
        let clean_regtest_data = !matches.is_present("no-clean");
        let server = if regtest_mode_enabled {
            regtest::launch(clean_regtest_data);
            ZingoConfig::get_server_or_default(Some("http://127.0.0.1".to_string()))
            // do the regtest
        } else {
            ZingoConfig::get_server_or_default(maybe_server)
        };

        // Test to make sure the server has all of scheme, host and port
        if server.scheme_str().is_none() || server.host().is_none() || server.port().is_none() {
            panic!(
            "Please provide the --server parameter as [scheme]://[host]:[port].\nYou provided: {}",
            server
        );
        }

        let sync = !matches.is_present("nosync");
        let password = matches.value_of("password").map(|s| s.to_string());
        Self {
            params,
            password,
            recover,
            server,
            seed,
            birthday,
            maybe_data_dir,
            sync,
            command: Some(command.unwrap().to_string()),
            regtest_mode_enabled,
        }
    }
    fn check_recover(&self) {
        if self.recover {
            // Create a Light Client Config in an attempt to recover the file.
            attempt_recover_seed(self.password.clone());
            std::process::exit(0x0100);
        }
    }
    fn start_cli_service(&self) -> (Sender<(String, Vec<String>)>, Receiver<String>) {
        let startup_chan = startup(
            self.server.clone(),
            self.seed.clone(),
            self.birthday,
            self.maybe_data_dir.clone(),
            self.sync,
            self.command.is_none(),
            self.regtest_mode_enabled,
        );
        match startup_chan {
            Ok(c) => c,
            Err(e) => {
                let emsg = format!("Error during startup:{}\nIf you repeatedly run into this issue, you might have to restore your wallet from your seed phrase.", e);
                eprintln!("{}", emsg);
                error!("{}", emsg);
                if cfg!(target_os = "unix") {
                    match e.raw_os_error() {
                        Some(13) => report_permission_error(),
                        _ => {}
                    }
                };
                panic!();
            }
        }
    }
    fn dispatch_command_or_start_interactive(&self) {
        let (command_transmitter, resp_receiver) = self.start_cli_service();
        if self.command.is_none() {
            start_interactive(command_transmitter, resp_receiver);
        } else {
            command_transmitter
                .send((
                    self.command.clone().unwrap().to_string(),
                    self.params
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
                        self.command.clone().unwrap(),
                        e
                    );
                    eprintln!("{}", e);
                    error!("{}", e);
                }
            }

            // Save before exit
            command_transmitter
                .send(("save".to_string(), vec![]))
                .unwrap();
            resp_receiver.recv().unwrap();
        }
    }
}
pub fn main() {}
