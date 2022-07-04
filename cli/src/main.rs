use log::error;
use zingo_cli::{
    attempt_recover_seed, configure_clapapp, report_permission_error, start_interactive, startup,
    version::VERSION,
};
use zingoconfig::ZingoConfig;

pub fn main() {
    // Get command line arguments
    use clap::{App, Arg};
    let fresh_app = App::new("Zingo CLI");
    let configured_app = configure_clapapp!(fresh_app);
    let matches = configured_app.get_matches();

    if matches.is_present("recover") {
        // Create a Light Client Config in an attempt to recover the file.
        attempt_recover_seed(matches.value_of("password").map(|s| s.to_string()));
        return;
    }

    let command = matches.value_of("COMMAND");
    let params = matches
        .values_of("PARAMS")
        .map(|v| v.collect())
        .or(Some(vec![]))
        .unwrap();

    let maybe_server = matches.value_of("server").map(|s| s.to_string());

    let maybe_data_dir = matches.value_of("data-dir").map(|s| s.to_string());

    let seed = matches.value_of("seed").map(|s| s.to_string());
    let maybe_birthday = matches.value_of("birthday");

    if seed.is_some() && maybe_birthday.is_none() {
        eprintln!("ERROR!");
        eprintln!(
            "Please specify the wallet birthday (eg. '--birthday 600000') to restore from seed."
        );
        eprintln!("This should be the block height where the wallet was created. If you don't remember the block height, you can pass '--birthday 0' to scan from the start of the blockchain.");
        return;
    }

    let birthday = match maybe_birthday.unwrap_or("0").parse::<u64>() {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "Couldn't parse birthday. This should be a block number. Error={}",
                e
            );
            return;
        }
    };

    let regtest = matches.is_present("regtest");
    if regtest {
        println!("matches has detected regtest, moving into custom logic");
        use std::fs::File;
        use std::io;
        use std::process::{Command, Stdio};
        use std::{thread, time};
        // TODO need to sniff and build directories
        let zcashd_command = Command::new("./regtest/bin/zcashd")
            .args([
                "--printtoconsole",
                "-conf=/zingolib/regtest/conf/zcash.conf",
                "--datadir=/zingolib/regtest/datadir/zcash/",
                // Right now I can't get zcashd to write to debug.log with this flag
                //"-debuglogfile=/zingolib/regtest/logs/debug.log",
                //Debug=1 will at least print to stdout
                "-debug=1",
            ])
            // piping stdout off...
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to start zcashd");

        // ...to ... nowhere for now.
        //let mut zcashd_logfile =
        //File::create("/zingolib/regtest/logs/ping.log").unwrap();

        // TODO the next line is halting process.. so the actual rust won't run after it, but it works.
        // looks like BufReader etc is the way...
        //io::copy(&mut zcashd_command.stdout.unwrap(), &mut zcashd_logfile).unwrap();

        // wait 10 seconds for zcashd to fire up
        // very generous, plan to tune down
        let ten_seconds = time::Duration::from_millis(10_000);
        thread::sleep(ten_seconds);

        // this process does not shut down when rust client shuts down!
        // TODO Needs a cleanup function, or something.
        println!("zcashd start section completed");

        let lwd_command = Command::new("./regtest/bin/lightwalletd")
            .args([
                "--no-tls-very-insecure",
                "--zcash-conf-path",
                "/zingolib/regtest/conf/zcash.conf",
                "--config",
                "/zingolib/regtest/conf/lightwalletdconf.yml",
                "--data-dir",
                "/zingolib/regtest/datadir/lightwalletd/",
                "--log-file",
                "/zingolib/regtest/logs/lwd.log",
            ])
            // this will print stdout of lwd process' output also to the zingo-cli stdout
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("failed to start lwd");

        // this client's stdout is less verbose so logging it may not be needed along with the working lwd.log file.
        //let mut lwd_stdout_logfile =
        //File::create("/zingolib/regtest/logs/lwd-ping.log").unwrap();
        // the next line is a halting process..
        //io::copy(&mut lwd_command.stdout.unwrap(), &mut lwd_stdout_logfile).unwrap();

        // wait 5 seconds for lwd to fire up
        // very generous, plan to tune down
        let five_seconds = time::Duration::from_millis(5_000);
        thread::sleep(five_seconds);

        // this process does not shut down when rust client shuts down!
        // TODO Needs a cleanup function, or something.
        println!("lwd start section completed");
    }

    let server = ZingoConfig::get_server_or_default(maybe_server);

    // Test to make sure the server has all of scheme, host and port
    if server.scheme_str().is_none() || server.host().is_none() || server.port().is_none() {
        eprintln!(
            "Please provide the --server parameter as [scheme]://[host]:[port].\nYou provided: {}",
            server
        );
        return;
    }

    let nosync = matches.is_present("nosync");

    let startup_chan = startup(
        server,
        seed,
        birthday,
        maybe_data_dir,
        !nosync,
        command.is_none(),
        regtest,
    );
    let (command_transmitter, resp_receiver) = match startup_chan {
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
            return;
        }
    };

    if command.is_none() {
        start_interactive(command_transmitter, resp_receiver);
    } else {
        command_transmitter
            .send((
                command.unwrap().to_string(),
                params
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>(),
            ))
            .unwrap();

        match resp_receiver.recv() {
            Ok(s) => println!("{}", s),
            Err(e) => {
                let e = format!("Error executing command {}: {}", command.unwrap(), e);
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
