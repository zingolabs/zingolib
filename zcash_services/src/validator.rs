//! Module for the structs that represent and manage the validator/full-node processes i.e. Zebrad.
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    process::Child,
};

use zcash_protocol::consensus::BlockHeight;

use getset::{CopyGetters, Getters};
use portpicker::Port;
use tempfile::TempDir;
use zebra_chain::{
    parameters::testnet::ConfiguredActivationHeights, serialization::ZcashSerialize as _,
};
use zebra_node_services::rpc_client::RpcRequestClient;
use zebra_rpc::{
    client::{BlockTemplateResponse, BlockTemplateTimeSource},
    proposal_block_from_template,
};

use crate::{
    config,
    error::LaunchError,
    launch, logs,
    network::{self, ActivationHeights, Network},
    Process,
};

/// Zebrad default miner address. Regtest/Testnet transparent address for [Abandon Abandon .. Art] seed (entropy all zeros)
pub const ZEBRAD_DEFAULT_MINER: &str = "tmBsTi2xWTjUdEXnuTceL7fecEQKeWaPDJd";

/// Zcashd configuration
///
/// Use `zcashd_bin` and `zcash_cli_bin` to specify the paths to the binaries.
/// If these binaries are in $PATH, `None` can be specified to run "zcashd" / "zcash-cli".
///
/// If `rpc_listen_port` is `None`, a port is picked at random between 15000-25000.
///
/// Use `activation_heights` to specify custom network upgrade activation heights.
///
/// Use `miner_address` to specify the target address for the block rewards when blocks are generated.
///
/// If `chain_cache` path is `None`, a new chain is launched.
pub struct ZcashdConfig {
    /// Zcashd binary location
    pub zcashd_bin: Option<PathBuf>,
    /// Zcash-cli binary location
    pub zcash_cli_bin: Option<PathBuf>,
    /// Zcashd RPC listen port
    pub rpc_listen_port: Option<Port>,
    /// Local network upgrade activation heights
    pub activation_heights: network::ActivationHeights,
    /// Miner address
    pub miner_address: Option<&'static str>,
    /// Chain cache path
    pub chain_cache: Option<PathBuf>,
}

/// Zebrad configuration
///
/// Use `zebrad_bin` to specify the binary location.
/// If the binary is in $PATH, `None` can be specified to run "zebrad".
///
/// If `rpc_listen_port` is `None`, a port is picked at random between 15000-25000.
///
/// Use `activation_heights` to specify custom network upgrade activation heights.
///
/// Use `miner_address` to specify the target address for the block rewards when blocks are generated.
///
/// If `chain_cache` path is `None`, a new chain is launched.
///
/// `network` can be used for testing against cached testnet / mainnet chains where large chains are needed.
/// `activation_heights` and `miner_address` will be ignored while not using regtest network.
pub struct ZebradConfig {
    /// Zebrad binary location
    pub zebrad_bin: Option<PathBuf>,
    /// Zebrad network listen port
    pub network_listen_port: Option<Port>,
    /// Zebrad JSON-RPC listen port
    pub rpc_listen_port: Option<Port>,
    /// Zebrad gRPC listen port
    pub indexer_listen_port: Option<Port>,
    /// Local network upgrade activation heights
    pub activation_heights: network::ActivationHeights,
    /// Miner address
    pub miner_address: &'static str,
    /// Chain cache path
    pub chain_cache: Option<PathBuf>,
    /// Network type
    pub network: Network,
}

impl Default for ZebradConfig {
    fn default() -> Self {
        Self {
            zebrad_bin: None,
            network_listen_port: None,
            rpc_listen_port: None,
            indexer_listen_port: None,
            activation_heights: network::ActivationHeights::default(),
            miner_address: ZEBRAD_DEFAULT_MINER,
            chain_cache: None,
            network: Network::Regtest,
        }
    }
}

/// Functionality for validator/full-node processes.
pub trait Validator: Sized {
    /// Config filename
    const CONFIG_FILENAME: &str;

    /// Validator config struct
    type Config;

    /// Return activation heights
    fn activation_heights(&self) -> ActivationHeights;

    /// Launch the process.
    fn launch(
        config: Self::Config,
    ) -> impl std::future::Future<Output = Result<Self, LaunchError>> + Send;

    /// Stop the process.
    fn stop(&mut self);

    /// Generate `n` blocks. This implementation should also call [`Self::poll_chain_height`] so the chain is at the
    /// correct height when this function returns.
    fn generate_blocks(
        &self,
        n: u32,
    ) -> impl std::future::Future<Output = std::io::Result<()>> + Send;

    /// Get chain height
    fn get_chain_height(&self) -> impl std::future::Future<Output = BlockHeight> + Send;

    /// Polls chain until it reaches target height
    fn poll_chain_height(
        &self,
        target_height: BlockHeight,
    ) -> impl std::future::Future<Output = ()> + Send;

    /// Get temporary config directory.
    fn config_dir(&self) -> &TempDir;

    /// Get temporary logs directory.
    fn logs_dir(&self) -> &TempDir;

    /// Get temporary data directory.
    fn data_dir(&self) -> &TempDir;

    /// Returns path to config file.
    fn config_path(&self) -> PathBuf {
        self.config_dir().path().join(Self::CONFIG_FILENAME)
    }

    /// Network type
    fn network(&self) -> Network;

    /// Caches chain. This stops the zcashd process.
    fn cache_chain(&mut self, chain_cache: PathBuf) -> std::process::Output {
        if chain_cache.exists() {
            panic!("chain cache already exists!");
        }

        self.stop();
        std::thread::sleep(std::time::Duration::from_secs(3));

        std::process::Command::new("cp")
            .arg("-r")
            .arg(self.data_dir().path())
            .arg(chain_cache)
            .output()
            .unwrap()
    }

    /// Checks `chain cache` is valid and loads into `validator_data_dir`.
    /// Returns the path to the loaded chain cache.
    ///
    /// If network is not `Regtest` variant, the chain cache will not be copied and the original cache path will be
    /// returned instead
    fn load_chain(
        chain_cache: PathBuf,
        validator_data_dir: PathBuf,
        validator_network: Network,
    ) -> PathBuf;

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

/// This struct is used to represent and manage the Zcashd process.
#[derive(Getters, CopyGetters)]
#[getset(get = "pub")]
pub struct Zcashd {
    /// Child process handle
    handle: Child,
    /// RPC port
    #[getset(skip)]
    #[getset(get_copy = "pub")]
    port: Port,
    /// Config directory
    config_dir: TempDir,
    /// Logs directory
    logs_dir: TempDir,
    /// Data directory
    data_dir: TempDir,
    /// Zcash cli binary location
    zcash_cli_bin: Option<PathBuf>,
    /// Network upgrade activation heights
    #[getset(skip)]
    activation_heights: network::ActivationHeights,
}

impl Zcashd {
    /// Runs a Zcash-cli command with the given `args`.
    ///
    /// Example usage for generating blocks in Zcashd local net:
    /// ```ignore (incomplete)
    /// self.zcash_cli_command(&["generate", "1"]);
    /// ```
    pub fn zcash_cli_command(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
        let mut command = match &self.zcash_cli_bin {
            Some(path) => std::process::Command::new(path),
            None => std::process::Command::new("zcash-cli"),
        };

        command.arg(format!("-conf={}", self.config_path().to_str().unwrap()));
        command.args(args).output()
    }
}

impl Validator for Zcashd {
    const CONFIG_FILENAME: &str = config::ZCASHD_FILENAME;

    type Config = ZcashdConfig;

    fn activation_heights(&self) -> ActivationHeights {
        self.activation_heights
    }

    async fn launch(config: Self::Config) -> Result<Self, LaunchError> {
        let logs_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();

        if let Some(cache) = config.chain_cache.clone() {
            Self::load_chain(cache, data_dir.path().to_path_buf(), Network::Regtest);
        }

        let port = network::pick_unused_port(config.rpc_listen_port);
        let config_dir = tempfile::tempdir().unwrap();
        let config_file_path = config::zcashd(
            config_dir.path(),
            port,
            &config.activation_heights,
            config.miner_address,
        )
        .unwrap();

        let mut command = match config.zcashd_bin {
            Some(path) => std::process::Command::new(path),
            None => std::process::Command::new("zcashd"),
        };
        command
            .args([
                "--printtoconsole",
                format!(
                    "--conf={}",
                    config_file_path.to_str().expect("should be valid UTF-8")
                )
                .as_str(),
                format!(
                    "--datadir={}",
                    data_dir.path().to_str().expect("should be valid UTF-8")
                )
                .as_str(),
                "-debug=1",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut handle = command.spawn().unwrap_or_else(|_| {
            panic!(
                "{} {}",
                command.get_program().to_string_lossy(),
                command
                    .get_args()
                    .map(|arg| arg.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        });

        logs::write_logs(&mut handle, &logs_dir);
        launch::wait(
            Process::Zcashd,
            &mut handle,
            &logs_dir,
            None,
            &["init message: Done loading"],
            &["Error:"],
            &[],
        )?;

        let zcashd = Zcashd {
            handle,
            port,
            config_dir,
            logs_dir,
            data_dir,
            zcash_cli_bin: config.zcash_cli_bin,
            activation_heights: config.activation_heights,
        };

        if config.chain_cache.is_none() {
            // generate genesis block
            zcashd.generate_blocks(1).await.unwrap();
        }

        Ok(zcashd)
    }

    fn stop(&mut self) {
        match self.zcash_cli_command(&["stop"]) {
            Ok(_) => {
                if let Err(e) = self.handle.wait() {
                    tracing::error!("zcashd cannot be awaited: {e}")
                } else {
                    tracing::info!("zcashd successfully shut down")
                };
            }
            Err(e) => {
                tracing::error!(
                    "Can't stop zcashd from zcash-cli: {e}\n\
                    Sending SIGKILL to zcashd process."
                );
                if let Err(e) = self.handle.kill() {
                    tracing::warn!("zcashd has already terminated: {e}")
                };
            }
        }
    }

    async fn generate_blocks(&self, n: u32) -> std::io::Result<()> {
        let chain_height = self.get_chain_height().await;
        self.zcash_cli_command(&["generate", &n.to_string()])?;
        self.poll_chain_height(chain_height + n).await;

        Ok(())
    }

    async fn get_chain_height(&self) -> BlockHeight {
        let output = self.zcash_cli_command(&["getchaintips"]).unwrap();
        let stdout_json = json::parse(&String::from_utf8_lossy(&output.stdout)).unwrap();
        BlockHeight::from_u32(stdout_json[0]["height"].as_u32().unwrap())
    }

    async fn poll_chain_height(&self, target_height: BlockHeight) {
        while self.get_chain_height().await < target_height {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }

    fn config_dir(&self) -> &TempDir {
        &self.config_dir
    }

    fn logs_dir(&self) -> &TempDir {
        &self.logs_dir
    }

    fn data_dir(&self) -> &TempDir {
        &self.data_dir
    }

    fn network(&self) -> Network {
        unimplemented!();
    }

    fn load_chain(
        chain_cache: PathBuf,
        validator_data_dir: PathBuf,
        _validator_network: Network,
    ) -> PathBuf {
        let regtest_dir = chain_cache.clone().join("regtest");
        if !regtest_dir.exists() {
            panic!("regtest directory not found!");
        }

        std::process::Command::new("cp")
            .arg("-r")
            .arg(regtest_dir)
            .arg(validator_data_dir)
            .output()
            .unwrap();
        chain_cache
    }
}

impl Drop for Zcashd {
    fn drop(&mut self) {
        self.stop();
    }
}

/// This struct is used to represent and manage the Zebrad process.
#[derive(Getters, CopyGetters)]
#[getset(get = "pub")]
pub struct Zebrad {
    /// Child process handle
    handle: Child,
    /// network listen port
    #[getset(skip)]
    #[getset(get_copy = "pub")]
    network_listen_port: Port,
    /// RPC listen port
    #[getset(skip)]
    #[getset(get_copy = "pub")]
    rpc_listen_port: Port,
    /// Config directory
    config_dir: TempDir,
    /// Logs directory
    logs_dir: TempDir,
    /// Data directory
    data_dir: TempDir,
    /// Network upgrade activation heights
    #[getset(skip)]
    activation_heights: network::ActivationHeights,
    /// RPC request client
    client: RpcRequestClient,
    /// Network type
    network: Network,
}

impl Validator for Zebrad {
    const CONFIG_FILENAME: &str = config::ZEBRAD_FILENAME;

    type Config = ZebradConfig;

    fn activation_heights(&self) -> ActivationHeights {
        self.activation_heights
    }

    async fn launch(config: Self::Config) -> Result<Self, LaunchError> {
        let logs_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();

        if !matches!(config.network, Network::Regtest) && config.chain_cache.is_none() {
            panic!("chain cache must be specified when not using a regtest network!")
        }

        let cache_dir = if let Some(cache) = config.chain_cache.clone() {
            Self::load_chain(cache.clone(), data_dir.path().to_path_buf(), config.network);
            cache
        } else {
            data_dir.path().to_path_buf()
        };

        let network_listen_port = network::pick_unused_port(config.network_listen_port);
        let rpc_listen_port = network::pick_unused_port(config.rpc_listen_port);
        let indexer_listen_port = network::pick_unused_port(config.indexer_listen_port);
        let config_dir = tempfile::tempdir().unwrap();
        let config_file_path = config::zebrad(
            config_dir.path().to_path_buf(),
            cache_dir,
            network_listen_port,
            rpc_listen_port,
            indexer_listen_port,
            &config.activation_heights,
            config.miner_address,
            config.network,
        )
        .unwrap();
        // create zcashd conf necessary for lightwalletd
        config::zcashd(
            config_dir.path(),
            rpc_listen_port,
            &config.activation_heights,
            None,
        )
        .unwrap();

        let mut command = match config.zebrad_bin {
            Some(path) => std::process::Command::new(path),
            None => std::process::Command::new("zebrad"),
        };
        command
            .args([
                "--config",
                config_file_path
                    .to_str()
                    .expect("should be valid UTF-8")
                    .to_string()
                    .as_str(),
                "start",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut handle = command.spawn().unwrap();

        logs::write_logs(&mut handle, &logs_dir);
        launch::wait(
            Process::Zebrad,
            &mut handle,
            &logs_dir,
            None,
            &[
                "zebra_rpc::server: Opened RPC endpoint at ",
                "zebra_rpc::indexer::server: Opened RPC endpoint at ",
                "spawned initial Zebra tasks",
            ],
            &[
                " panicked at",
                "ERROR ",
                "fatal",
                "failed to ",
                "unable to ",
                "Aborting",
                " backtrace:",
            ],
            &[
                // exclude benign noise that often shows up during bootstrap:
                "DNS error resolving peer IP addresses",
                "Seed peer DNS resolution failed",
                "warning: some trace filter directives would enable traces that are disabled statically",
            ],
        )?;
        std::thread::sleep(std::time::Duration::from_secs(5));

        let rpc_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), rpc_listen_port);
        let client = zebra_node_services::rpc_client::RpcRequestClient::new(rpc_address);

        let zebrad = Zebrad {
            handle,
            network_listen_port,
            rpc_listen_port,
            config_dir,
            logs_dir,
            data_dir,
            activation_heights: config.activation_heights,
            client,
            network: config.network,
        };

        if config.chain_cache.is_none() && matches!(config.network, Network::Regtest) {
            // generate genesis block
            zebrad.generate_blocks(1).await.unwrap();
        }
        std::thread::sleep(std::time::Duration::from_secs(5));

        Ok(zebrad)
    }

    fn stop(&mut self) {
        self.handle.kill().expect("zebrad couldn't be killed")
    }

    async fn generate_blocks(&self, n: u32) -> std::io::Result<()> {
        let chain_height = self.get_chain_height().await;

        for _ in 0..n {
            let block_template: BlockTemplateResponse = self
                .client
                .json_result_from_call("getblocktemplate", "[]".to_string())
                .await
                .expect("response should be success output with a serialized `GetBlockTemplate`");

            let network =
                zebra_chain::parameters::Network::new_regtest(ConfiguredActivationHeights {
                    before_overwinter: Some(1),
                    overwinter: Some(self.activation_heights.overwinter.into()),
                    sapling: Some(self.activation_heights.sapling.into()),
                    blossom: Some(self.activation_heights.blossom.into()),
                    heartwood: Some(self.activation_heights.heartwood.into()),
                    canopy: Some(self.activation_heights.canopy.into()),
                    nu5: Some(self.activation_heights.nu5.into()),
                    nu6: Some(self.activation_heights.nu6.into()),
                    nu6_1: None,
                    nu7: None,
                });

            let block_data = hex::encode(
                proposal_block_from_template(
                    &block_template,
                    BlockTemplateTimeSource::default(),
                    &network,
                )
                .unwrap()
                .zcash_serialize_to_vec()
                .unwrap(),
            );

            let submit_block_response = self
                .client
                .text_from_call("submitblock", format!(r#"["{block_data}"]"#))
                .await
                .unwrap();

            if !submit_block_response.contains(r#""result":null"#) {
                dbg!(&submit_block_response);
                panic!("failed to submit block!");
            }
        }
        self.poll_chain_height(chain_height + n).await;

        Ok(())
    }

    async fn get_chain_height(&self) -> BlockHeight {
        let response: serde_json::Value = self
            .client
            .json_result_from_call("getblockchaininfo", "[]".to_string())
            .await
            .unwrap();

        let chain_height: u32 = response
            .get("blocks")
            .and_then(|h| h.as_u64())
            .and_then(|h| u32::try_from(h).ok())
            .unwrap();

        BlockHeight::from_u32(chain_height)
    }

    async fn poll_chain_height(&self, target_height: BlockHeight) {
        while self.get_chain_height().await < target_height {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    fn config_dir(&self) -> &TempDir {
        &self.config_dir
    }

    fn logs_dir(&self) -> &TempDir {
        &self.logs_dir
    }

    fn data_dir(&self) -> &TempDir {
        &self.data_dir
    }

    fn network(&self) -> Network {
        self.network
    }

    fn load_chain(
        chain_cache: PathBuf,
        validator_data_dir: PathBuf,
        validator_network: Network,
    ) -> PathBuf {
        let state_dir = chain_cache.clone().join("state");
        if !state_dir.exists() {
            panic!("state directory not found!");
        }

        if matches!(validator_network, Network::Regtest) {
            std::process::Command::new("cp")
                .arg("-r")
                .arg(state_dir)
                .arg(validator_data_dir.clone())
                .output()
                .unwrap();
            validator_data_dir
        } else {
            chain_cache
        }
    }
}

impl Drop for Zebrad {
    fn drop(&mut self) {
        self.stop();
    }
}
