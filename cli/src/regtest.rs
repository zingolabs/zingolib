pub(crate) fn git_check() {
    use std::process::Command;
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
        .contains("bb5774128dd3b88a78e13a2129769d1aced7b884")
    {
        panic!("Zingo-cli's regtest mode must be run within its own git worktree");
    }
}

pub(crate) async fn launch() -> Result<(), Box<dyn std::error::Error>> {
    use std::ffi::OsString;
    use std::fs::File;
    use tokio::time::sleep;
    use tokio::time::timeout;
    use tokio::time::Duration;
    //use tokio::fs::File;
    //use std::io::BufWriter;
    //use std::io::Read;
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::{thread, time};
    use tokio::io::{copy, AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::process::Command;

    pub struct DaemonsReady {
        zcashd: bool,
        lightwalletd: bool,
    }

    let mut daemon_ready = DaemonsReady {
        zcashd: false,
        lightwalletd: false,
    };

    // get the top level directory for this repo worktree
    let revparse_raw = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output();
    //.expect("problem invoking git rev-parse");

    //let revparse = std::str::from_utf8(&revparse_raw.stdout).expect("revparse error");
    //let revparse = std::str::from_utf8(&(revparse_raw.await?).stdout).expect("revparse error");
    let revparse: String =
        String::from_utf8(revparse_raw.await?.stdout).expect("revparse into string failed");
    //let revparse = revparse_raw.await?;

    // Cross-platform OsString
    let mut worktree_home = OsString::new();
    worktree_home.push(&revparse.trim_end());

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
    //TODO try direct tokio fs
    let std_zcashd_logfile = File::create(&zcashd_stdout_log).expect("file::create Result error");
    let mut zcashd_logfile = tokio::fs::File::from_std(std_zcashd_logfile);

    let mut zd_child = Command::new(zcashd_bin)
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
        .expect("failed starting zcashd");
    //zcashd_command.stdout(Stdio::piped());
    //zcashd_command.stderr(Stdio::piped());
    //let child = zcashd_command.spawn().expect("failed to start zcashd");

    let zd_stdout = zd_child.stdout.take().expect("zd child stdout not taken!");
    // TODO stderr

    // TODO redundant?
    //let mut alt_zd_reader = BufReader::new(zd_stdout);
    let mut zd_reader = BufReader::new(zd_stdout).lines();

    // TODO launches... runtime forever?
    tokio::spawn(async move {
        let zd_status = zd_child.wait().await.expect("zd_status error");
        println!("child zd_status was: {}", zd_status);
    });

    while let Some(line) = zd_reader.next_line().await? {
        println!("Line: {}", line);
        // check for match = blocking
        // write to file, continuous.
        if line.contains("Error:") {
            panic!("zcashd reporting ERROR! exiting with panic. you may have to shut the daemon down manually.");
        } else if line.contains("init message: Done loading") {
            println!("MATCHED zcashd done loading");
            daemon_ready.zcashd = true;
            println!("zcashd daemonready: {}", &daemon_ready.zcashd);
            // need to create blocking check every check_interval
        }
        zcashd_logfile
            // TODO re-add new lines?
            .write_all(line.as_bytes())
            .await
            .expect("problem during zwd write_all: logging stdout to file");
    }
    println!("pre timeout");

    const CHECK_INTERVAL: Duration = time::Duration::from_millis(100);
    async fn zd_ready(dr: &DaemonsReady) {
        loop {
            if dr.zcashd == true {
                break;
            } else {
                println!("sleeping");
                sleep(CHECK_INTERVAL).await;
            }
        }
    }

    // 30 seconds maximum wait time
    // TODO set higher once confirmed working?
    println!("pre timeout");
    if let Err(_) = timeout(Duration::from_secs(30), zd_ready(&daemon_ready)).await {
        println!("TIMEOUT!!!!!");
    }
    println!("past timeout");

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
    let std_lwd_logfile = File::create(&lwd_stdout_log).expect("file::create Result error");
    let mut lwd_logfile = tokio::fs::File::from_std(std_lwd_logfile);

    let mut lwd_child = Command::new(lwd_bin)
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

    let lwd_stdout = lwd_child
        .stdout
        .take()
        .expect("lwd child stdout not taken!");
    // TODO stderr

    // TODO redundant?
    //let mut alt_zd_reader = BufReader::new(zd_stdout);
    let mut lwd_reader = BufReader::new(lwd_stdout).lines();

    // TODO launches... runtime forever?
    tokio::spawn(async move {
        let lwd_status = lwd_child.wait().await.expect("lwd_status error");
        println!("child zd_status was: {}", lwd_status);
    });

    println!("lightwalletd is now started in regtest mode, please standby...");
    while let Some(lwd_line) = lwd_reader.next_line().await? {
        println!("LWD Line: {}", lwd_line);
        // write to file, continuous.
        if lwd_line.contains("Lightwalletd died")
            || lwd_line.contains("FATAL")
            || lwd_line.contains("error")
        {
            panic!("lwd reporting ERROR! exiting with panic. you may have to shut the daemons down manually.");
        } else if lwd_line.contains("Starting insecure no-TLS (plaintext) server") {
            println!("MATCHED lwd done loading");
            daemon_ready.lightwalletd = true;
        }
        lwd_logfile
            .write_all(lwd_line.as_bytes())
            .await
            .expect("problem with lwd write_all: logging stdout to file");
    }
    async fn lwd_ready(dr: &DaemonsReady) {
        loop {
            if dr.lightwalletd == true {
                break;
            } else {
                println!("sleeping");
                sleep(CHECK_INTERVAL).await;
            }
        }
    }

    // 30 seconds maximum wait time
    // TODO set higher once confirmed working?
    if let Err(_) = timeout(Duration::from_secs(30), lwd_ready(&daemon_ready)).await {
        println!("TIMEOUT!!!!!");
    }
    println!("past lwd timeout");
    println!("lwd start section completed, lightwalletd should be running!");
    println!("Standby, Zingo-cli should be running in regtest mode momentarily...");
    Ok(())
}
