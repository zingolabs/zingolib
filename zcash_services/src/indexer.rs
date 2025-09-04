//! Module for the structs that represent and manage the indexer processes i.e. Zainod.
//!
//! Processes which are not strictly indexers but have a similar role in serving light-clients/light-wallets
//! (i.e. Lightwalletd) are also included in this category and are referred to as "light-nodes".

use std::{fs::File, path::PathBuf, process::Child};

use getset::{CopyGetters, Getters};
use portpicker::Port;
use tempfile::TempDir;

use crate::{
    config,
    error::LaunchError,
    launch, logs,
    network::{self, Network},
    Process,
};

/// Zainod configuration
///
/// If `listen_port` is `None`, a port is picked at random between 15000-25000.
///
/// The `validator_port` must be specified and the validator process must be running before launching Zainod.
///
/// `network` must match the configured network of the validator.
pub struct ZainodConfig {
    /// Zainod binary location
    pub zainod_bin: Option<PathBuf>,
    /// Listen RPC port
    pub listen_port: Option<Port>,
    /// Validator RPC port
    pub validator_port: Port,
    /// Chain cache path
    pub chain_cache: Option<PathBuf>,
    /// Network type.
    pub network: Network,
}

/// Lightwalletd configuration
///
/// If `listen_port` is `None`, a port is picked at random between 15000-25000.
///
/// The `zcash_conf` path must be specified and the validator process must be running before launching Lightwalletd.
/// When running a validator that is not Zcashd (i.e. Zebrad), a zcash config file must still be created to specify the
/// validator port. This is automatically handled by [`crate::LocalNet::launch`] when using [`crate::LocalNet`].
pub struct LightwalletdConfig {
    /// Lightwalletd binary location
    pub lightwalletd_bin: Option<PathBuf>,
    /// Listen RPC port
    pub listen_port: Option<Port>,
    /// Zcashd configuration file location. Required even when running non-Zcashd validators.
    pub zcashd_conf: PathBuf,
    /// Enables darkside
    pub darkside: bool,
}

/// Empty configuration
///
/// For use when not launching an Indexer with [`crate::LocalNet::launch`].
pub struct EmptyConfig {}

/// Functionality for indexer/light-node processes.
pub trait Indexer: Sized {
    /// Config filename
    const CONFIG_FILENAME: &str;

    /// Indexer config struct
    type Config;

    /// Indexer listen port
    fn listen_port(&self) -> Port;

    /// Launch the process.
    fn launch(config: Self::Config) -> Result<Self, LaunchError>;

    /// Stop the process.
    fn stop(&mut self);

    /// Get temporary config directory.
    fn config_dir(&self) -> &TempDir;

    /// Get temporary logs directory.
    fn logs_dir(&self) -> &TempDir;

    /// Returns path to config file.
    fn config_path(&self) -> PathBuf {
        self.config_dir().path().join(Self::CONFIG_FILENAME)
    }

    /// Prints the stdout log.
    fn print_stdout(&self) {
        let stdout_log_path = self.logs_dir().path().join(logs::STDOUT_LOG);
        logs::print_log(stdout_log_path);
    }

    /// Prints the stdout log.
    fn print_stderr(&self) {
        let stdout_log_path = self.logs_dir().path().join(logs::STDERR_LOG);
        logs::print_log(stdout_log_path);
    }
}

/// This struct is used to represent and manage the Zainod process.
#[derive(Getters, CopyGetters)]
#[getset(get = "pub")]
pub struct Zainod {
    /// Child process handle
    handle: Child,
    /// RPC port
    #[getset(skip)]
    #[getset(get_copy = "pub")]
    port: Port,
    /// Logs directory
    logs_dir: TempDir,
    /// Config directory
    config_dir: TempDir,
}

impl Indexer for Zainod {
    const CONFIG_FILENAME: &str = config::ZAINOD_FILENAME;

    type Config = ZainodConfig;

    fn listen_port(&self) -> Port {
        self.port
    }

    fn launch(config: Self::Config) -> Result<Self, LaunchError> {
        let logs_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();

        let port = network::pick_unused_port(config.listen_port);
        let config_dir = tempfile::tempdir().unwrap();

        let cache_dir = if let Some(cache) = config.chain_cache.clone() {
            cache
        } else {
            data_dir.path().to_path_buf()
        };

        let config_file_path = config::zainod(
            config_dir.path(),
            cache_dir,
            port,
            config.validator_port,
            config.network,
        )
        .unwrap();

        let mut command = match config.zainod_bin {
            Some(path) => std::process::Command::new(path),
            None => std::process::Command::new("zainod"),
        };
        command
            .args([
                "--config",
                config_file_path.to_str().expect("should be valid UTF-8"),
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut handle = command.spawn().unwrap();

        logs::write_logs(&mut handle, &logs_dir);
        launch::wait(
            Process::Zainod,
            &mut handle,
            &logs_dir,
            None,
            &["Zaino Indexer started successfully."],
            &["Error:"],
            &[],
        )?;

        Ok(Zainod {
            handle,
            port,
            logs_dir,
            config_dir,
        })
    }

    fn stop(&mut self) {
        self.handle.kill().expect("zainod couldn't be killed")
    }

    fn config_dir(&self) -> &TempDir {
        &self.config_dir
    }

    fn logs_dir(&self) -> &TempDir {
        &self.logs_dir
    }
}

impl Drop for Zainod {
    fn drop(&mut self) {
        self.stop();
    }
}

/// This struct is used to represent and manage the Lightwalletd process.
#[derive(Getters, CopyGetters)]
#[getset(get = "pub")]
pub struct Lightwalletd {
    /// Child process handle
    handle: Child,
    /// RPC Port
    #[getset(skip)]
    #[getset(get_copy = "pub")]
    port: Port,
    /// Data directory
    _data_dir: TempDir,
    /// Logs directory
    logs_dir: TempDir,
    /// Config directory
    config_dir: TempDir,
}

impl Lightwalletd {
    /// Prints the stdout log.
    pub fn print_lwd_log(&self) {
        let stdout_log_path = self.logs_dir.path().join(logs::LIGHTWALLETD_LOG);
        logs::print_log(stdout_log_path);
    }
}

impl Indexer for Lightwalletd {
    const CONFIG_FILENAME: &str = config::LIGHTWALLETD_FILENAME;

    type Config = LightwalletdConfig;

    fn listen_port(&self) -> Port {
        self.port
    }

    fn launch(config: Self::Config) -> Result<Self, LaunchError> {
        let logs_dir = tempfile::tempdir().unwrap();
        let lwd_log_file_path = logs_dir.path().join(logs::LIGHTWALLETD_LOG);
        let _lwd_log_file = File::create(&lwd_log_file_path).unwrap();

        let data_dir = tempfile::tempdir().unwrap();

        let port = network::pick_unused_port(config.listen_port);
        let config_dir = tempfile::tempdir().unwrap();
        let config_file_path = config::lightwalletd(
            config_dir.path(),
            port,
            lwd_log_file_path.clone(),
            config.zcashd_conf.clone(),
        )
        .unwrap();

        let mut command = match config.lightwalletd_bin {
            Some(path) => std::process::Command::new(path),
            None => std::process::Command::new("lightwalletd"),
        };
        let mut args = vec![
            "--no-tls-very-insecure",
            "--data-dir",
            data_dir.path().to_str().unwrap(),
            "--log-file",
            lwd_log_file_path.to_str().unwrap(),
            "--zcash-conf-path",
            config.zcashd_conf.to_str().unwrap(),
            "--config",
            config_file_path.to_str().unwrap(),
        ];
        if config.darkside {
            args.push("--darkside-very-insecure");
        }

        command
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut handle = command.spawn().unwrap();

        logs::write_logs(&mut handle, &logs_dir);
        launch::wait(
            Process::Lightwalletd,
            &mut handle,
            &logs_dir,
            Some(lwd_log_file_path),
            &["Starting insecure no-TLS (plaintext) server"],
            &["error"],
            &[],
        )?;

        Ok(Lightwalletd {
            handle,
            port,
            _data_dir: data_dir,
            logs_dir,
            config_dir,
        })
    }

    fn stop(&mut self) {
        self.handle.kill().expect("lightwalletd couldn't be killed")
    }

    fn config_dir(&self) -> &TempDir {
        &self.config_dir
    }

    fn logs_dir(&self) -> &TempDir {
        &self.logs_dir
    }
}

impl Drop for Lightwalletd {
    fn drop(&mut self) {
        self.stop();
    }
}

/// This struct is used to represent and manage an empty Indexer process.
///
/// Dirs are created for integration.
#[derive(Getters, CopyGetters)]
#[getset(get = "pub")]
pub struct Empty {
    /// Logs directory
    logs_dir: TempDir,
    /// Config directory
    config_dir: TempDir,
}

impl Indexer for Empty {
    const CONFIG_FILENAME: &str = "";

    type Config = EmptyConfig;

    fn listen_port(&self) -> Port {
        0
    }

    fn launch(_config: Self::Config) -> Result<Self, LaunchError> {
        let logs_dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();

        Ok(Empty {
            logs_dir,
            config_dir,
        })
    }

    fn stop(&mut self) {}

    fn config_dir(&self) -> &TempDir {
        &self.config_dir
    }

    fn logs_dir(&self) -> &TempDir {
        &self.logs_dir
    }
}

impl Drop for Empty {
    fn drop(&mut self) {
        self.stop();
    }
}
