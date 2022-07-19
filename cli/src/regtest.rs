pub(crate) fn launch() {
    use std::ffi::OsString;
    use std::fs::File;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::{thread, time};

    let pid: u32 = std::process::id();
    println!("starting PID: {}", pid);

    // confirm we are in a git worktree
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
            let mut zcashd_stdout_log: String = String::new();
            zcashd_stdout_log.push_str(zcash_logs.to_str().expect("error converting to str"));
            zcashd_stdout_log.push_str("zcashd_stdout.log");

            let mut zcashd_logfile =
                File::create(zcashd_stdout_log).expect("file::create Result error");
            std::io::copy(&mut zcashd_log, &mut zcashd_logfile)
                .expect("io::copy error writing zcashd_studout.log");
        });
    }
    println!("zcashd is starting in regtest mode, please standby about 10 seconds...");
    // wait 10 seconds for zcashd to fire up
    // very generous, plan to tune down
    let ten_seconds = time::Duration::from_millis(10_000);
    thread::sleep(ten_seconds);

    println!("zcashd start section completed, zcashd should be running.");
    println!("Standby, lightwalletd is about to start. This should only take a moment.");

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
    let mut unflagged_lwd_log: String = String::new();
    unflagged_lwd_log.push_str(lwd_logs.to_str().expect("error making lwd_datadir"));
    unflagged_lwd_log.push_str("lwd.log");

    Command::new(lwd_bin)
        .args([
            "--no-tls-very-insecure",
            "--zcash-conf-path",
            &unflagged_zcashd_conf,
            "--config",
            &unflagged_lwd_conf,
            "--data-dir",
            &unflagged_lwd_datadir,
            "--log-file",
            &unflagged_lwd_log,
        ])
        // this currently prints stdout of lwd process' output also to the zingo-cli stdout
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

    println!("lwd start section completed, lightwalletd should be running!");
    println!("Standby, Zingo-cli should be running in regtest mode momentarily...");
}
