use std::fs::{self, File};
///  Enforce strict expectations for tool use with current zingolib.  Relaxing these restrictions will facilitate
///  use in other projects.  For example, this version of regtest will only run within a git repo that is historically
///  descended from 27e5eedc6b35759f463d43ea341ce66714aa9e01 (ie, I am Jack's commit descendant.)
fn git_selfcheck() {
    let git_check = std::process::Command::new("git")
        .arg("--help")
        .output()
        .expect("no git!? time to quit.");

    if !std::str::from_utf8(&git_check.stdout)
        .expect("git --help error")
        .contains("See 'git help git' for an overview of the system.")
    {
        panic!("git check failed!");
    }

    // confirm this worktree is a zingolib repo
    let git_revlist = std::process::Command::new("git")
        .args(["rev-list", "--max-parents=0", "HEAD"])
        .output()
        .expect("problem invoking git rev-list");

    if !std::str::from_utf8(&git_revlist.stdout)
        .expect("git revlist error")
        .contains("27e5eedc6b35759f463d43ea341ce66714aa9e01")
    {
        panic!("I am not Jack's commit descendant");
    }

    let git_log = std::process::Command::new("git")
        .args(["--no-pager", "log"])
        .output()
        .expect("git log error");

    if !std::str::from_utf8(&git_log.stdout)
        .expect("git log stdout error")
        .contains("e8677475da2676fcfec57615de6330a7cb542cc1")
    {
        panic!("Zingo-cli's regtest mode must be run within its own git worktree");
    }
}

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

fn config_zcashd_for_launch(
    bin_loc: &PathBuf,
    zcashd_logs: &PathBuf,
    zcashd_config: &PathBuf,
    zcashd_datadir: &PathBuf,
) -> (std::process::Child, File, PathBuf) {
    let mut zcashd_bin = bin_loc.clone();
    zcashd_bin.push("zcashd");
    let zcashd_stdout_log = zcashd_logs.join("stdout.log");
    let mut command = std::process::Command::new(zcashd_bin);
    println!("canonicalize: {:?}", fs::canonicalize(&zcashd_datadir));
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
            // debug=1 will at least print to stdout
            "-debug=1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    /*
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
    */
    println!("command {:?}", &command);
    let child = command.spawn().expect("failed to start zcashd");
    (
        child,
        File::create(&zcashd_stdout_log).expect("file::create Result error"),
        zcashd_stdout_log,
    )
}

pub(crate) fn launch() {
    use std::io::Read;
    use std::{thread, time};

    //check for git itself and that we are working within a zingolib repo
    git_selfcheck();

    let regtest_dir = get_regtest_dir();
    let confs_dir = regtest_dir.join("conf");
    let bin_location = regtest_dir.join("bin");
    let logs = regtest_dir.join("logs");
    let data_dir = regtest_dir.join("data");
    let zcashd_datadir = data_dir.clone(); //.join("zcashd");
    let zcashd_logs = logs.join("zcashd");
    let zcashd_config = confs_dir.join("zcash.conf");
    let lightwalletd_config = confs_dir.join("lightwalletd.yaml");
    let lightwalletd_logs = logs.join("lightwalletd");
    let lightwalletd_stdout_log = lightwalletd_logs.join("stdout.log");
    let lightwalletd_stderr_log = lightwalletd_logs.join("stderr.log");
    let lightwalletd_datadir = data_dir.clone(); // .join("lightwalletd");

    /*    assert!(&zcashd_config
        .to_str()
        .unwrap()
        .ends_with("/regtest/conf/zcash.conf"));
    assert!(&zcashd_datadir
        .to_str()
        .unwrap()
        .ends_with("/regtest/data/zcashd"));
    */
    println!(
        "zcashd bin: {:?} logs:{:?} config:{:?} datadir:{:?}",
        &bin_location, &zcashd_logs, &zcashd_config, &zcashd_datadir
    );
    let (mut zcashd_command, mut zcashd_logfile, zcashd_stdout_log) =
        config_zcashd_for_launch(&bin_location, &zcashd_logs, &zcashd_config, &zcashd_datadir);

    if let Some(mut zcashd_stdout_data) = zcashd_command.stdout.take() {
        println!("zcashd_command stdout taken");
        std::thread::spawn(move || {
            std::io::copy(&mut zcashd_stdout_data, &mut zcashd_logfile)
                .expect("io::copy error writing zcashd_stdout.log");
        });
    }

    println!("zcashd is starting in regtest mode, please standby...");
    let check_interval = time::Duration::from_millis(1000);
    // adding sleep to test timing
    thread::sleep(check_interval);

    let mut zcashd_log_open = File::open(&zcashd_stdout_log).expect("can't open zcashd log");
    let mut zcashd_logfile_state = String::new();

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
    println!("lightwalletd is about to start. This should only take a moment.");

    let mut lwd_logfile =
        File::create(&lightwalletd_stdout_log).expect("file::create Result error");
    let mut lwd_err_logfile =
        File::create(&lightwalletd_stderr_log).expect("file::create Result error");

    let mut lwd_bin = bin_location.to_owned();
    lwd_bin.push("lightwalletd");

    println!("lwd_bin: {:?}", &lwd_bin);
    println!("zcashd_config: {:?}", &zcashd_config);
    println!("lightwalletd_config: {:?}", &lightwalletd_config);
    println!("lightwalletd_datadir: {:?}", &lightwalletd_datadir);
    println!("lightwalletd_stdout_log: {:?}", &lightwalletd_stdout_log);
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
        .expect("failed to start lwd");

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
}
