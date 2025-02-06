use std::fs::File;

use std::io::Read;
///  Simple helper to succinctly reference the project root dir.
use std::path::PathBuf;
use std::process::Child;

///  To manage the state associated a "regtest" run this type:
///   * sets up paths to config and log directories
///   * optionally receives parameters that allow configs to be set in nonstandard
///     locations.  We use this to input configs for the libtonode_tests
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct RegtestManager {
    regtest_dir: PathBuf,
    confs_dir: PathBuf,
    bin_dir: PathBuf,
    cli_bin: PathBuf,
    logs_dir: PathBuf,
    data_dir: PathBuf,
    /// TODO: Add Doc Comment Here!
    pub zcashd_data_dir: PathBuf,
    /// TODO: Add Doc Comment Here!
    pub zcashd_logs_dir: PathBuf,
    /// TODO: Add Doc Comment Here!
    pub zcashd_stdout_log: PathBuf,
    /// TODO: Add Doc Comment Here!
    pub zcashd_config: PathBuf,
    /// TODO: Add Doc Comment Here!
    pub lightwalletd_config: PathBuf,
    /// TODO: Add Doc Comment Here!
    pub lightwalletd_logs_dir: PathBuf,
    /// TODO: Add Doc Comment Here!
    pub lightwalletd_log: PathBuf,
    lightwalletd_stdout_log: PathBuf,
    lightwalletd_stderr_log: PathBuf,
    lightwalletd_data_dir: PathBuf,
    /// TODO: Add Doc Comment Here!
    pub zingo_datadir: PathBuf,
}

///  We use the `ChildProcessHandler` to handle the children of generated in scenario testing
pub struct ChildProcessHandler {
    /// TODO: Add Doc Comment Here!
    pub zcashd: Child,
    /// TODO: Add Doc Comment Here!
    pub lightwalletd: Child,
    /// TODO: Add Doc Comment Here!
    pub zcash_cli_command: std::process::Command,
}

impl Drop for ChildProcessHandler {
    fn drop(&mut self) {
        match self.zcash_cli_command.arg("stop").output() {
            Ok(_) => {
                if let Err(e) = self.zcashd.wait() {
                    log::warn!("zcashd cannot be awaited: {e}")
                } else {
                    log::debug!("zcashd successfully shut down")
                };
                if let Err(e) = self.lightwalletd.kill() {
                    log::warn!("lightwalletd cannot be awaited: {e}")
                } else {
                    log::debug!("lightwalletd successfully shut down")
                };
            }
            Err(e) => {
                log::error!(
                    "Can't stop zcashd from zcash-cli: {e}\n\
                    Sending SIGKILL to zcashd process."
                );
                if let Err(e) = self.zcashd.kill() {
                    log::warn!("zcashd has already terminated: {e}")
                };

                if let Err(e) = self.lightwalletd.kill() {
                    log::warn!("lightwalletd has already terminated: {e}")
                }
            }
        }
    }
}

/// TODO: Add Doc Comment Here!
#[derive(Debug)]
pub enum LaunchChildProcessError {
    /// TODO: Add Doc Comment Here!
    ZcashdState {
        /// TODO: Add Doc Comment Here!
        errorcode: std::process::ExitStatus,
        /// TODO: Add Doc Comment Here!
        stdout: String,
        /// TODO: Add Doc Comment Here!
        stderr: String,
    },
}

impl From<LaunchChildProcessError> for String {
    fn from(underlyingerror: LaunchChildProcessError) -> Self {
        format!("LaunchChildProcessError from {:?}", underlyingerror)
    }
}

/// TODO: Add Doc Comment Here!
pub fn launch_lightwalletd(
    logsdir: PathBuf,
    confsdir: PathBuf,
    datadir: PathBuf,
    bindir: PathBuf,
    check_interval: std::time::Duration,
    darkside: Option<String>,
) -> Child {
    let bin = bindir.join("lightwalletd");
    let lightwalletd_config = confsdir.join("lightwalletd.yml");
    let lightwalletd_logs_dir = logsdir.join("lightwalletd");
    std::fs::create_dir_all(&lightwalletd_logs_dir).expect("Couldn't create dir.");
    let lightwalletd_log = lightwalletd_logs_dir.join("lwd.log");
    let lightwalletd_stdout_log = lightwalletd_logs_dir.join("stdout.log");
    let lightwalletd_stderr_log = lightwalletd_logs_dir.join("stderr.log");
    let lightwalletd_data_dir = datadir.join("lightwalletd");
    std::fs::create_dir_all(&lightwalletd_data_dir).expect("Couldn't create dir.");
    let zcashd_config_tmp = confsdir.join("zcash.conf");
    let zcashd_config = zcashd_config_tmp
        .to_str()
        .expect("zcashd_config PathBuf to str fail!");
    log::debug!("lightwalletd is about to start. This should only take a moment.");

    File::create(&lightwalletd_log).expect("file::create Result error");
    let mut lightwalletd_stdout_logfile =
        File::create(lightwalletd_stdout_log).expect("file::create Result error");
    let mut lightwalletd_stderr_logfile =
        File::create(lightwalletd_stderr_log).expect("file::create Result error");

    let mut args = vec![
        "--no-tls-very-insecure".to_string(),
        "--data-dir".to_string(),
        lightwalletd_data_dir.to_string_lossy().to_string(),
        "--log-file".to_string(),
        lightwalletd_log.to_string_lossy().to_string(),
    ];
    args.extend_from_slice(&if let Some(grpc_bind_addr) = darkside {
        vec![
            "--darkside-very-insecure".to_string(),
            "--grpc-bind-addr".to_string(),
            grpc_bind_addr,
        ]
    } else {
        vec![
            "--zcash-conf-path".to_string(),
            zcashd_config.to_string(),
            "--config".to_string(),
            lightwalletd_config.to_string_lossy().to_string(),
        ]
    });
    let prepped_args = args.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut lightwalletd_child = std::process::Command::new(bin.clone())
        .args(prepped_args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|_| {
            panic!(
                "{}",
                format!(
                    "failed to start lightwalletd at {}. see docs/libtonode-tests.txt",
                    bin.display()
                )
                .to_owned()
            )
        });

    if let Some(mut lwd_stdout_data) = lightwalletd_child.stdout.take() {
        std::thread::spawn(move || {
            std::io::copy(&mut lwd_stdout_data, &mut lightwalletd_stdout_logfile)
                .expect("io::copy error writing lwd_stdout.log");
        });
    }

    if let Some(mut lwd_stderr_data) = lightwalletd_child.stderr.take() {
        std::thread::spawn(move || {
            std::io::copy(&mut lwd_stderr_data, &mut lightwalletd_stderr_logfile)
                .expect("io::copy error writing lwd_stderr.log");
        });
    }

    log::debug!("lightwalletd is now started in regtest mode, please standby...");

    let mut lwd_log_opened = File::open(&lightwalletd_log).expect("can't open lwd log");
    let mut lwd_logfile_state = String::new();

    //now enter loop to find string that indicates daemon is ready for next step
    loop {
        std::io::Read::read_to_string(&mut lwd_log_opened, &mut lwd_logfile_state)
            .expect("problem reading lwd_logfile into rust string");
        if lwd_logfile_state.contains("Starting insecure no-TLS (plaintext) server") {
            log::debug!("lwd start section completed, lightwalletd should be running!");
            log::debug!("Standby, Zingo-cli should be running in regtest mode momentarily...");
            break;
        }
        // we need to sleep because even after the last message is detected, lwd needs a moment to become ready for regtest mode
        std::thread::sleep(check_interval);
    }
    lightwalletd_child
}

#[cfg(not(feature = "zaino-test"))]
fn write_zcash_conf(location: &PathBuf) {
    // This is the only data we need to supply *to* the zcashd, the other files are created by zcashd and lightwalletd
    use std::io::Write;
    let conf_bytes: &'static [u8] = include_bytes!("regtest/conf/zcash.conf");
    File::create(location)
        .unwrap()
        .write_all(conf_bytes)
        .unwrap();
}

impl RegtestManager {
    /// TODO: Add Doc Comment Here!
    pub fn new(rootpathname: PathBuf) -> Self {
        let regtest_dir = rootpathname;
        let confs_dir = regtest_dir.join("conf");
        let zcashd_config = confs_dir.join("zcash.conf");
        std::fs::create_dir_all(&confs_dir).expect("Couldn't create dir.");
        #[cfg(not(feature = "zaino-test"))]
        {
            write_zcash_conf(&zcashd_config);
        }
        let bin_dir = super::paths::get_bin_dir();
        std::fs::create_dir_all(&bin_dir).expect("Couldn't create dir.");
        let cli_bin = bin_dir.join("zcash-cli");
        let logs_dir = regtest_dir.join("logs");
        let data_dir = regtest_dir.join("data");
        let zcashd_data_dir = data_dir.join("zcashd");
        std::fs::create_dir_all(&zcashd_data_dir).expect("Couldn't create dir.");
        let zcashd_logs_dir = logs_dir.join("zcashd");
        std::fs::create_dir_all(&zcashd_logs_dir).expect("Couldn't create dir.");
        let zcashd_stdout_log = zcashd_logs_dir.join("stdout.log");
        let lightwalletd_config = confs_dir.join("lightwalletd.yml");
        let lightwalletd_logs_dir = logs_dir.join("lightwalletd");
        std::fs::create_dir_all(&lightwalletd_logs_dir).expect("Couldn't create dir.");
        let lightwalletd_log = lightwalletd_logs_dir.join("lwd.log");
        let lightwalletd_stdout_log = lightwalletd_logs_dir.join("stdout.log");
        let lightwalletd_stderr_log = lightwalletd_logs_dir.join("stderr.log");
        let lightwalletd_data_dir = data_dir.join("lightwalletd");
        std::fs::create_dir_all(&lightwalletd_data_dir).expect("Couldn't create dir.");
        let zingo_data_dir = data_dir.join("zingo");
        std::fs::create_dir_all(&zingo_data_dir).expect("Couldn't create dir.");
        RegtestManager {
            regtest_dir,
            confs_dir,
            bin_dir,
            cli_bin,
            logs_dir,
            data_dir,
            zcashd_data_dir,
            zcashd_logs_dir,
            zcashd_stdout_log,
            zcashd_config,
            lightwalletd_config,
            lightwalletd_logs_dir,
            lightwalletd_log,
            lightwalletd_stdout_log,
            lightwalletd_stderr_log,
            lightwalletd_data_dir,
            zingo_datadir: zingo_data_dir,
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_cli_handle(&self) -> std::process::Command {
        let config_str = &self
            .zcashd_config
            .to_str()
            .expect("Path to string failure!");

        let mut command = std::process::Command::new(&self.cli_bin);
        command.arg(format!("-conf={config_str}"));
        command
    }

    /// TODO: Add Doc Comment Here!
    pub fn generate_n_blocks(
        &self,
        num_blocks: u32,
    ) -> Result<std::process::Output, std::io::Error> {
        self.get_cli_handle()
            .args(["generate".to_string(), num_blocks.to_string()])
            .output()
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_chain_tip(&self) -> Result<std::process::Output, std::io::Error> {
        self.get_cli_handle().arg("getchaintips").output()
    }

    fn clean_regtest_data(&self) {
        // remove contents of existing data directories
        let zcd_subdir = &self.zcashd_data_dir.join("regtest");

        std::process::Command::new("rm")
            .arg("-r")
            .arg(zcd_subdir)
            .output()
            .expect("problem with rm zcd subdir");

        let lwd_subdir = &self.lightwalletd_data_dir.join("db");

        std::process::Command::new("rm")
            .arg("-r")
            .arg(lwd_subdir)
            .output()
            .expect("problem with rm lwd subdir");

        let zingo_file_one = &self.zingo_datadir.join("zingo-wallet.dat");
        let zingo_file_two = &self.zingo_datadir.join("zingo-wallet.debug.log");
        std::process::Command::new("rm")
            .arg(zingo_file_one)
            .output()
            .expect("problem with rm zingofile");
        std::process::Command::new("rm")
            .arg(zingo_file_two)
            .output()
            .expect("problem with rm zingofile");

        // copy contents from regtestvector directory to working zcashd data directory
        let destination_subdir = &self.zcashd_data_dir.join("regtest").join("*");

        std::process::Command::new("rm")
            .arg("-r")
            .arg(destination_subdir)
            .output()
            .expect("problem with rm -r contents of regtest dir");
    }

    fn zcashd_launch(&self) -> (std::process::Child, File) {
        use std::ffi::OsStr;
        let zcashd_bin = &mut self.bin_dir.clone();
        zcashd_bin.push("zcashd");
        let mut command = std::process::Command::new(zcashd_bin);
        command
            .args([
                "--printtoconsole",
                format!(
                    "--conf={}",
                    &self.zcashd_config.to_str().expect("Unexpected string!")
                )
                .as_str(),
                format!(
                    "--datadir={}",
                    &self.zcashd_data_dir.to_str().expect("Unexpected string!")
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
            &command.get_args().collect::<Vec<&OsStr>>()[0]
                .to_str()
                .unwrap(),
            &"--printtoconsole"
        );
        assert!(&command.get_args().collect::<Vec<&OsStr>>()[1]
            .to_str()
            .unwrap()
            .starts_with("--conf="));
        assert!(&command.get_args().collect::<Vec<&OsStr>>()[2]
            .to_str()
            .unwrap()
            .starts_with("--datadir="));
        assert_eq!(
            &command.get_args().collect::<Vec<&OsStr>>()[3]
                .to_str()
                .unwrap(),
            &"-debug=1"
        );
        log::info!("{:?}", &command.get_current_dir());
        log::info!("{:?}", &command.get_args());
        log::info!("{:?}", &command.get_envs());
        log::info!("{:?}", &command.get_program());

        let child = command.spawn().unwrap_or_else(|_| {
            panic!(
                "{}",
                format!(
                    "failed to start zcashd at {}. see docs/libtonode-tests.txt",
                    self.bin_dir.clone().display()
                )
                .to_owned()
            )
        });

        log::debug!("zcashd is starting in regtest mode, please standby...");

        (
            child,
            File::create(&self.zcashd_stdout_log).expect("file::create Result error"),
        )
    }

    /// Once the expected filesystem setup is complete attempt to launch the children
    /// lightwalletd and zcashd.  Child processes are killed during drop of their
    /// "ChildProcessHandler" container
    pub fn launch(
        &self,
        clean_regtest_data: bool,
    ) -> Result<ChildProcessHandler, LaunchChildProcessError> {
        if clean_regtest_data {
            self.clean_regtest_data();
        }

        let (mut zcashd_handle, mut zcashd_logfile) = self.zcashd_launch();

        if let Some(mut zcashd_stdout_data) = zcashd_handle.stdout.take() {
            std::thread::spawn(move || {
                std::io::copy(&mut zcashd_stdout_data, &mut zcashd_logfile)
                    .expect("io::copy error writing zcashd_stdout.log");
            });
        }

        let mut zcashd_log_open =
            File::open(&self.zcashd_stdout_log).expect("can't open zcashd log");
        log::debug!(
            "Opened zcashd_stdout_log: {}",
            &self.zcashd_stdout_log.to_string_lossy().to_string()
        );
        let mut zcashd_logfile_state = String::new();

        let check_interval = std::time::Duration::from_millis(500);

        //now enter loop to find string that indicates daemon is ready for next step
        loop {
            match zcashd_handle.try_wait() {
                Ok(Some(exit_status)) => {
                    let mut stdout = String::new();
                    zcashd_log_open.read_to_string(&mut stdout).unwrap();
                    let mut stderr = String::new();
                    zcashd_handle
                        .stderr
                        .as_mut()
                        .unwrap()
                        .read_to_string(&mut stderr)
                        .unwrap();
                    return Err(LaunchChildProcessError::ZcashdState {
                        errorcode: exit_status, //"zcashd exited with code: {exitcode:?}".to_string(),
                        stdout,
                        stderr,
                    });
                }
                Ok(None) => (),
                Err(e) => {
                    panic!("Unexpected Error: {e}")
                }
            };
            std::io::Read::read_to_string(&mut zcashd_log_open, &mut zcashd_logfile_state)
                .expect("problem reading zcashd_logfile into rust string");
            if zcashd_logfile_state.contains("Error:") {
                panic!("zcashd reporting ERROR! exiting with panic. you may have to shut the daemon down manually.\n\
                    See logfile for more details: {}", self.zcashd_stdout_log.to_string_lossy());
            } else if zcashd_logfile_state.contains("init message: Done loading") {
                break;
            }
            std::thread::sleep(check_interval);
        }

        log::debug!("zcashd start section completed, zcashd reports it is done loading.");

        if clean_regtest_data {
            log::debug!("generating initial block[s]");
            //std::thread::sleep(std::time::Duration::new(15, 0));
            let generate_output = &self.generate_n_blocks(1);

            match generate_output {
                Ok(output) => log::debug!(
                    "generated block {}",
                    std::str::from_utf8(&output.stdout).unwrap()
                ),
                Err(e) => {
                    log::debug!("generating initial block returned error {e}");
                    log::debug!("exiting!");
                    zcashd_handle
                        .kill()
                        .expect("Stop! Stop! Zcash is already dead!");
                    panic!("")
                }
            }
        } else {
            log::debug!("Keeping old regtest data")
        }
        let lightwalletd_child = launch_lightwalletd(
            self.lightwalletd_logs_dir.clone(),
            self.confs_dir.clone(),
            self.data_dir.clone(),
            self.bin_dir.clone(),
            check_interval,
            None,
        );
        Ok(ChildProcessHandler {
            zcashd: zcashd_handle,
            lightwalletd: lightwalletd_child,
            zcash_cli_command: self.get_cli_handle(),
        })
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_current_height(&self) -> Result<u32, String> {
        let wut = self
            .get_cli_handle()
            .arg("getblockchaininfo")
            .output()
            .unwrap()
            .stdout;
        Ok(
            json::parse(std::str::from_utf8(&wut).unwrap()).unwrap()["blocks"]
                .as_u32()
                .unwrap(),
        )
    }
}
