fn git_selfcheck() {
    use std::process::Command;
    let git_check = Command::new("git")
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
    let git_revlist = Command::new("git")
        .args(["rev-list", "--max-parents=0", "HEAD"])
        .output()
        .expect("problem invoking git rev-list");

    if !std::str::from_utf8(&git_revlist.stdout)
        .expect("git revlist error")
        .contains("27e5eedc6b35759f463d43ea341ce66714aa9e01")
    {
        panic!("I am not Jack's commit descendant");
    }

    let git_log = Command::new("git")
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

fn get_top_level_dir() {
    //TODO
}

pub(crate) fn launch() {
    use std::ffi::OsString;
    use std::fs::File;
    use std::io::Read;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::{thread, time};

    //check for git itself and that we are working within a zingolib repo
    git_selfcheck();

    let _ = get_top_level_dir();
    // confirm we are in a git worktree

    // get the top level directory for this repo worktree
    let revparse_raw = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .expect("problem invoking git rev-parse");

    let revparse = std::str::from_utf8(&revparse_raw.stdout).expect("revparse error");

    // Cross-platform OsString
    let mut worktree_home = OsString::new();
    worktree_home.push(revparse.trim_end());

    let bin_location: PathBuf = [
        worktree_home.clone(),
        OsString::from("regtest"),
        OsString::from("bin"),
    ]
    .iter()
    .collect();

    let mut zcashd_bin = bin_location.to_owned();
    zcashd_bin.push("zcashd");

    let mut lwd_bin = bin_location.to_owned();
    lwd_bin.push("lightwalletd");

    let zcash_confs: PathBuf = [
        worktree_home.clone(),
        OsString::from("regtest"),
        OsString::from("conf"),
        OsString::from(""),
    ]
    .iter()
    .collect();

    let mut flagged_zcashd_conf: String = "--conf=".to_string();
    flagged_zcashd_conf.push_str(zcash_confs.to_str().expect("error making zcash_datadir"));
    flagged_zcashd_conf.push_str("zcash.conf");

    let zcash_datadir: PathBuf = [
        worktree_home.clone(),
        OsString::from("regtest"),
        OsString::from("datadir"),
        OsString::from("zcash"),
        OsString::from(""),
    ]
    .iter()
    .collect();

    let mut flagged_datadir: String = "--datadir=".to_string();
    flagged_datadir.push_str(zcash_datadir.to_str().expect("error making zcash_datadir"));

    let zcash_logs: PathBuf = [
        worktree_home.clone(),
        OsString::from("regtest"),
        OsString::from("logs"),
        OsString::from(""),
    ]
    .iter()
    .collect();

    println!("zcashd datadir: {}", &flagged_datadir);
    println!("zcashd conf file: {}", &flagged_zcashd_conf);

    let mut zcashd_stdout_log: String = String::new();
    zcashd_stdout_log.push_str(zcash_logs.to_str().expect("error converting to str"));
    zcashd_stdout_log.push_str("zcashd_stdout.log");
    let mut zcashd_logfile = File::create(&zcashd_stdout_log).expect("file::create Result error");

    let mut zcashd_command = Command::new(zcashd_bin)
        .args([
            "--printtoconsole",
            &flagged_zcashd_conf,
            &flagged_datadir,
            // Right now I can't get zcashd to write to debug.log with this flag
            //"-debuglogfile=.../zingolib/regtest/logs/debug.log",
            //debug=1 will at least print to stdout
            "-debug=1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start zcashd");

    if let Some(mut zcashd_log) = zcashd_command.stdout.take() {
        std::thread::spawn(move || {
            std::io::copy(&mut zcashd_log, &mut zcashd_logfile)
                .expect("io::copy error writing zcashd_stdout.log");
        });
    }

    println!("zcashd is starting in regtest mode, please standby...");
    let check_interval = time::Duration::from_millis(100);

    let mut zcashd_log_open = File::open(&zcashd_stdout_log).expect("can't open zcashd log");
    let mut zcashd_logfile_state = String::new();
    //now enter loop to find string that indicates daemon is ready for next step
    loop {
        zcashd_log_open
            .read_to_string(&mut zcashd_logfile_state)
            .expect("problem reading zcashd_logfile into rust string"); // returns result
        if zcashd_logfile_state.contains("Error:") {
            panic!("zcashd reporting ERROR! exiting with panic. you may have to shut the daemon down manually.");
        } else if zcashd_logfile_state.contains("init message: Done loading") {
            break;
        } else {
            thread::sleep(check_interval);
        }
    }

    println!("zcashd start section completed, zcashd should be running.");
    println!("lightwalletd is about to start. This should only take a moment.");

    let lwd_confs: PathBuf = [
        worktree_home.clone(),
        OsString::from("regtest"),
        OsString::from("conf"),
        OsString::from(""),
    ]
    .iter()
    .collect();

    let mut unflagged_lwd_conf: String = String::new();
    unflagged_lwd_conf.push_str(lwd_confs.to_str().expect("trouble making flagged_lwd_conf"));
    unflagged_lwd_conf.push_str("lightwalletdconf.yml");

    // for lwd config
    let mut unflagged_zcashd_conf: String = String::new();
    unflagged_zcashd_conf.push_str(
        zcash_confs
            .to_str()
            .expect("error making unflagged zcash conf"),
    );
    unflagged_zcashd_conf.push_str("zcash.conf");

    let lwd_datadir: PathBuf = [
        worktree_home.clone(),
        OsString::from("regtest"),
        OsString::from("datadir"),
        OsString::from("lightwalletd"),
        OsString::from(""),
    ]
    .iter()
    .collect();

    let mut unflagged_lwd_datadir: String = String::new();
    unflagged_lwd_datadir.push_str(lwd_datadir.to_str().expect("error making lwd_datadir"));

    let lwd_logs: PathBuf = [
        worktree_home.clone(),
        OsString::from("regtest"),
        OsString::from("logs"),
        OsString::from(""),
    ]
    .iter()
    .collect();

    let mut lwd_stdout_log: String = String::new();
    lwd_stdout_log.push_str(lwd_logs.to_str().expect("error making lwd_datadir"));
    lwd_stdout_log.push_str("lwd_stdout.log");
    let mut lwd_logfile = File::create(&lwd_stdout_log).expect("file::create Result error");

    let mut lwd_command = Command::new(lwd_bin)
        .args([
            "--no-tls-very-insecure",
            "--zcash-conf-path",
            &unflagged_zcashd_conf,
            "--config",
            &unflagged_lwd_conf,
            "--data-dir",
            &unflagged_lwd_datadir,
            "--log-file",
            &lwd_stdout_log,
        ])
        // this currently prints stdout of lwd process' output also to the zingo-cli stdout
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start lwd");

    if let Some(mut lwd_log) = lwd_command.stdout.take() {
        std::thread::spawn(move || {
            std::io::copy(&mut lwd_log, &mut lwd_logfile)
                .expect("io::copy error writing lwd_stdout.log");
        });
    }

    println!("lightwalletd is now started in regtest mode, please standby...");

    let mut lwd_log_opened = File::open(&lwd_stdout_log).expect("can't open lwd log");
    let mut lwd_logfile_state = String::new();
    //now enter loop to find string that indicates daemon is ready for next step
    loop {
        lwd_log_opened
            .read_to_string(&mut lwd_logfile_state)
            .expect("problem reading lwd_logfile into rust string");
        if lwd_logfile_state.contains("Starting insecure no-TLS (plaintext) server") {
            println!("lwd start section completed, lightwalletd should be running!");
            println!("Standby, Zingo-cli should be running in regtest mode momentarily...");
            // we need to sleep because even after the last message is detected, lwd needs a moment to become ready for regtest mode
            thread::sleep(check_interval);
            break;
        } else {
            thread::sleep(check_interval);
        }
    }
}
