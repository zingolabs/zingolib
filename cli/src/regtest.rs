use std::fs::File;

///  Simple helper to succinctly reference the project root dir.
use std::path::{Path, PathBuf};
fn get_top_level_dir() -> PathBuf {
    let revparse_raw = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .expect("problem invoking git rev-parse");
    Path::new(
        std::str::from_utf8(&revparse_raw.stdout)
            .expect("revparse error")
            .trim(),
    )
    .to_path_buf()
}
fn get_regtest_dir() -> PathBuf {
    get_top_level_dir().join("regtest")
}

fn prepare_working_directories(
    zcd_datadir: &PathBuf,
    lwd_datadir: &PathBuf,
    zingo_datadir: &PathBuf,
) {
    // remove contents of existing data directories
    let zcd_subdir = zcd_datadir.join("regtest");

    assert!(&zcd_subdir
        .to_str()
        .unwrap()
        .ends_with("/regtest/data/zcashd/regtest"));

    std::process::Command::new("rm")
        .arg("-r")
        .arg(zcd_subdir)
        .output()
        .expect("problem with rm zcd subdir");

    let lwd_subdir = lwd_datadir.join("db");

    assert!(&lwd_subdir
        .to_str()
        .unwrap()
        .ends_with("/regtest/data/lightwalletd/db"));

    std::process::Command::new("rm")
        .arg("-r")
        .arg(lwd_subdir)
        .output()
        .expect("problem with rm lwd subdir");

    let zingo_file_one = zingo_datadir.join("zingo-wallet.dat");
    let zingo_file_two = zingo_datadir.join("zingo-wallet.debug.log");

    assert!(&zingo_file_one
        .to_str()
        .unwrap()
        .ends_with("/regtest/data/zingo/zingo-wallet.dat"));
    assert!(&zingo_file_two
        .to_str()
        .unwrap()
        .ends_with("/regtest/data/zingo/zingo-wallet.debug.log"));

    std::process::Command::new("rm")
        .arg(zingo_file_one)
        .output()
        .expect("problem with rm zingofile");
    std::process::Command::new("rm")
        .arg(zingo_file_two)
        .output()
        .expect("problem with rm zingofile");

    // copy contents from regtestvector directory to working zcashd data directory
    let destination_subdir = zcd_datadir.join("regtest").join("*");

    std::process::Command::new("rm")
        .arg("-r")
        .arg(destination_subdir)
        .output()
        .expect("problem with rm -r contents of regtest dir");
}

fn zcashd_launch(
    bin_loc: &PathBuf,
    zcashd_logs: &PathBuf,
    zcashd_config: &PathBuf,
    zcashd_datadir: &PathBuf,
) -> (std::process::Child, File, PathBuf) {
    use std::ffi::OsStr;
    let mut zcashd_bin = bin_loc.clone();
    zcashd_bin.push("zcashd");
    let zcashd_stdout_log = zcashd_logs.join("stdout.log");
    let mut command = std::process::Command::new(zcashd_bin);
    command
        .args([
            "--printtoconsole",
            format!(
                "--conf={}",
                zcashd_config.to_str().expect("Unexpected string!")
            )
            .as_str(),
            format!(
                "--datadir={}",
                zcashd_datadir.to_str().expect("Unexpected string!")
            )
            .as_str(),
            // Currently zcashd will not write to debug.log with the following flag
            // "-debuglogfile=.../zingolib/regtest/logs/debug.log",
            // debug=1 will print verbose debugging information to stdout
            // to then be logged to a file
            "-debug=1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    assert_eq!(command.get_args().len(), 4usize);
    assert_eq!(
        &command.get_args().into_iter().collect::<Vec<&OsStr>>()[0]
            .to_str()
            .unwrap(),
        &"--printtoconsole"
    );
    assert!(&command.get_args().into_iter().collect::<Vec<&OsStr>>()[1]
        .to_str()
        .unwrap()
        .starts_with("--conf="));
    assert!(&command.get_args().into_iter().collect::<Vec<&OsStr>>()[2]
        .to_str()
        .unwrap()
        .starts_with("--datadir="));
    assert_eq!(
        &command.get_args().into_iter().collect::<Vec<&OsStr>>()[3]
            .to_str()
            .unwrap(),
        &"-debug=1"
    );

    let child = command.spawn()
        .expect("failed to start zcashd. It's possible the zcashd binary is not in the /zingolib/regtest/bin/ directory, see /regtest/README.md");
    println!("zcashd is starting in regtest mode, please standby...");

    (
        child,
        File::create(&zcashd_stdout_log).expect("file::create Result error"),
        zcashd_stdout_log,
    )
}

fn generate_initial_block(
    bin_loc: &PathBuf,
    zcashd_config: &PathBuf,
) -> Result<std::process::Output, std::io::Error> {
    let cli_bin = bin_loc.join("zcash-cli");
    let config_str = zcashd_config.to_str().expect("Path to string failure!");
    std::process::Command::new(cli_bin)
        .args([
            format!("-conf={config_str}"),
            "generate".to_string(),
            "1".to_string(),
        ])
        .output()
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct RegtestManager {
    regtest_dir: PathBuf,
    confs_dir: PathBuf,
    bin_location: PathBuf,
    logs: PathBuf,
    data_dir: PathBuf,
    zcashd_datadir: PathBuf,
    zcashd_logs: PathBuf,
    zcashd_config: PathBuf,
    lightwalletd_config: PathBuf,
    lightwalletd_logs: PathBuf,
    lightwalletd_stdout_log: PathBuf,
    lightwalletd_stderr_log: PathBuf,
    lightwalletd_datadir: PathBuf,
    zingo_datadir: PathBuf,
    zcashd_pid: u32,
}
impl RegtestManager {
    pub fn launch(clean_regtest_data: bool) -> Self {
        let regtest_dir = get_regtest_dir();
        let confs_dir = regtest_dir.join("conf");
        let bin_location = regtest_dir.join("bin");
        let logs = regtest_dir.join("logs");
        let data_dir = regtest_dir.join("data");
        let zcashd_datadir = data_dir.join("zcashd");
        let zcashd_logs = logs.join("zcashd");
        let zcashd_config = confs_dir.join("zcash.conf");
        let lightwalletd_config = confs_dir.join("lightwalletd.yaml");
        let lightwalletd_logs = logs.join("lightwalletd");
        let lightwalletd_stdout_log = lightwalletd_logs.join("stdout.log");
        let lightwalletd_stderr_log = lightwalletd_logs.join("stderr.log");
        let lightwalletd_datadir = data_dir.join("lightwalletd");
        let zingo_datadir = data_dir.join("zingo");

        use std::io::Read;
        use std::{thread, time};
        assert!(&zcashd_config
            .to_str()
            .unwrap()
            .ends_with("/regtest/conf/zcash.conf"));
        assert!(&zcashd_datadir
            .to_str()
            .unwrap()
            .ends_with("/regtest/data/zcashd"));

        if clean_regtest_data {
            prepare_working_directories(&zcashd_datadir, &lightwalletd_datadir, &zingo_datadir);
        }

        let (mut zcashd_command, mut zcashd_logfile, zcashd_stdout_log) =
            zcashd_launch(&bin_location, &zcashd_logs, &zcashd_config, &zcashd_datadir);

        let zcashd_pid = zcashd_command.id();
        if let Some(mut zcashd_stdout_data) = zcashd_command.stdout.take() {
            std::thread::spawn(move || {
                std::io::copy(&mut zcashd_stdout_data, &mut zcashd_logfile)
                    .expect("io::copy error writing zcashd_stdout.log");
            });
        }

        let mut zcashd_log_open = File::open(&zcashd_stdout_log).expect("can't open zcashd log");
        let mut zcashd_logfile_state = String::new();

        let check_interval = time::Duration::from_millis(500);

        //now enter loop to find string that indicates daemon is ready for next step
        loop {
            zcashd_log_open
                .read_to_string(&mut zcashd_logfile_state)
                .expect("problem reading zcashd_logfile into rust string");
            if zcashd_logfile_state.contains("Error:") {
                panic!("zcashd reporting ERROR! exiting with panic. you may have to shut the daemon down manually.");
            } else if zcashd_logfile_state.contains("init message: Done loading") {
                break;
            }
            thread::sleep(check_interval);
        }

        println!("zcashd start section completed, zcashd reports it is done loading.");

        if clean_regtest_data {
            println!("Generating initial block");
            let generate_output = generate_initial_block(&bin_location, &zcashd_config);

            match generate_output {
                Ok(output) => println!(
                    "generated block {}",
                    std::str::from_utf8(&output.stdout).unwrap()
                ),
                Err(e) => {
                    println!("generating initial block returned error {e}");
                    println!("exiting!");
                    zcashd_command
                        .kill()
                        .expect("Stop! Stop! Zcash is already dead!");
                    panic!("")
                }
            }
        } else {
            println!("Keeping old regtest data")
        }
        println!("lightwalletd is about to start. This should only take a moment.");

        let mut lwd_logfile =
            File::create(&lightwalletd_stdout_log).expect("file::create Result error");
        let mut lwd_err_logfile =
            File::create(&lightwalletd_stderr_log).expect("file::create Result error");

        let mut lwd_bin = bin_location.to_owned();
        lwd_bin.push("lightwalletd");

        let mut lwd_command = std::process::Command::new(lwd_bin)
        .args([
            "--no-tls-very-insecure",
            "--zcash-conf-path",
            &zcashd_config
                .to_str()
                .expect("zcashd_config PathBuf to str fail!"),
            "--config",
            &lightwalletd_config
                .to_str()
                .expect("lightwalletd_config PathBuf to str fail!"),
            "--data-dir",
            &lightwalletd_datadir
                .to_str()
                .expect("lightwalletd_datadir PathBuf to str fail!"),
            "--log-file",
            &lightwalletd_stdout_log
                .to_str()
                .expect("lightwalletd_stdout_log PathBuf to str fail!"),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to start lightwalletd. It's possible the lightwalletd binary is not in the /zingolib/regtest/bin/ directory, see /regtest/README.md");

        if let Some(mut lwd_log) = lwd_command.stdout.take() {
            std::thread::spawn(move || {
                std::io::copy(&mut lwd_log, &mut lwd_logfile)
                    .expect("io::copy error writing lwd_stdout.log");
            });
        }

        if let Some(mut lwd_err_log) = lwd_command.stderr.take() {
            std::thread::spawn(move || {
                std::io::copy(&mut lwd_err_log, &mut lwd_err_logfile)
                    .expect("io::copy error writing lwd_stderr.log");
            });
        }

        println!("lightwalletd is now started in regtest mode, please standby...");

        let mut lwd_log_opened = File::open(&lightwalletd_stdout_log).expect("can't open lwd log");
        let mut lwd_logfile_state = String::new();

        //now enter loop to find string that indicates daemon is ready for next step
        loop {
            lwd_log_opened
                .read_to_string(&mut lwd_logfile_state)
                .expect("problem reading lwd_logfile into rust string");
            if lwd_logfile_state.contains("Starting insecure no-TLS (plaintext) server") {
                println!("lwd start section completed, lightwalletd should be running!");
                println!("Standby, Zingo-cli should be running in regtest mode momentarily...");
                break;
            }
            // we need to sleep because even after the last message is detected, lwd needs a moment to become ready for regtest mode
            thread::sleep(check_interval);
        }
        RegtestManager {
            regtest_dir,
            confs_dir,
            bin_location,
            logs,
            data_dir,
            zcashd_datadir,
            zcashd_logs,
            zcashd_config,
            lightwalletd_config,
            lightwalletd_logs,
            lightwalletd_stdout_log,
            lightwalletd_stderr_log,
            lightwalletd_datadir,
            zingo_datadir,
            zcashd_pid,
        }
    }
}
