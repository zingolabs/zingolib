//! `zcash_local_net` Wallet implementation driving the `zingo-cli` binary.
//!
//! [`ZingoCli`] drives the shipped `zingo-cli` executable as a managed
//! subprocess: every wallet operation is a run-to-completion one-shot
//! invocation (`zingo-cli … <command>`) against a persistent `--data-dir`,
//! mirroring the harness's in-tree zcash-devtool implementation. The
//! binary is located through the `TEST_BINARIES_DIR` environment
//! variable, falling back to `PATH` — the same discovery every other
//! harness-managed binary uses.
//!
//! Sync policy is strict: every operation passes `--nosync` except
//! [`Wallet::sync`], which lets the startup sync run and blocks on it
//! with `--waitsync`. `balance` and `address` are therefore pure local
//! reads, and callers sequence `act → mine → sync → assert` explicitly,
//! as the trait contract prescribes.
//!
//! Regtest activation heights reach the binary through the
//! `--activation-heights` TOML (devtool-compatible schema; an upgrade
//! whose key is omitted never activates). The heights themselves can
//! only come from a running Validator via
//! [`WalletNetwork::from_validator`] (ADR 0003), and the file is passed
//! on *every* invocation so the loaded wallet always interprets the
//! chain under the schedule it was created with.
//!
//! Output-shape contract: the parsers in [`parse`] mirror what the
//! zingo-cli binary prints today (`{"txids": […]}` from
//! quicksend/quickshield, the bracketed `balance` listing with
//! underscore-grouped zatoshis, `{"height": N}`, and the `info` JSON).
//! The unit tests pin the parsers against recorded shapes; the
//! proof-scenario integration test pins the contract against the real
//! binary.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::path::PathBuf;

use tempfile::TempDir;

use zcash_local_net::error::WalletError;
use zcash_local_net::indexer::Indexer;
use zcash_local_net::wallet::{
    AddressReceiver, GetInfo, Wallet, WalletBalance, WalletConfig, WalletNetwork,
};
use zingo_test_vectors::seeds::{ABANDON_ART_SEED, HOSPITAL_MUSEUM_SEED};

mod parse;

const EXECUTABLE_NAME: &str = "zingo-cli";

/// Filename of the activation-heights TOML written into the wallet
/// directory for regtest wallets.
const HEIGHTS_FILENAME: &str = "activation-heights.toml";

/// zingo-cli wallet configuration.
///
/// The wallet is restored from `mnemonic` at `birthday` and synced
/// against a lightwalletd-protocol (gRPC) server at
/// `127.0.0.1:indexer_port` — wire it to a running indexer with
/// [`WalletConfig::setup_indexer_connection`] or set the port directly.
#[derive(Clone, Debug)]
pub struct ZingoCliConfig {
    /// BIP-39 mnemonic phrase the wallet is restored from.
    pub mnemonic: String,
    /// Wallet birthday height.
    pub birthday: u32,
    /// gRPC port (on 127.0.0.1) of the indexer serving this wallet.
    pub indexer_port: u16,
    /// Network the wallet is launched against. The regtest variant is
    /// only constructible through [`WalletNetwork::from_validator`],
    /// so the heights this crate writes to zingo-cli's
    /// `--activation-heights` TOML are always the running Validator's
    /// own schedule (ADR 0003).
    pub network: WalletNetwork,
}

impl ZingoCliConfig {
    /// The standard faucet wallet: restored from the shared
    /// "abandon … art" mnemonic at birthday 0. Validators launched by
    /// the harness mine to addresses derived from the same seed, so
    /// this wallet sees the miner rewards.
    pub fn faucet(network: WalletNetwork) -> Self {
        Self {
            mnemonic: ABANDON_ART_SEED.to_string(),
            birthday: 0,
            indexer_port: 0,
            network,
        }
    }

    /// The standard recipient wallet: restored from the
    /// `HOSPITAL_MUSEUM` mnemonic at birthday 0. zingo-cli restores
    /// ZIP-32 account index 0; obtain addresses from
    /// [`Wallet::default_address`], not from constants recorded
    /// against other account indices.
    pub fn recipient(network: WalletNetwork) -> Self {
        Self {
            mnemonic: HOSPITAL_MUSEUM_SEED.to_string(),
            birthday: 0,
            indexer_port: 0,
            network,
        }
    }

    /// The `--chain` value for this config's network.
    fn chain_name(&self) -> &'static str {
        match self.network {
            WalletNetwork::Mainnet => "mainnet",
            WalletNetwork::Testnet => "testnet",
            WalletNetwork::Regtest(_) => "regtest",
        }
    }
}

impl WalletConfig for ZingoCliConfig {
    fn setup_indexer_connection<I: Indexer>(&mut self, indexer: &I) {
        self.indexer_port = indexer.listen_port();
    }
}

/// This struct represents and manages zingo-cli wallet invocations.
///
/// There is no resident child process: every operation spawns the
/// binary, waits for it to exit, and appends its output to the logs
/// directory. Dropping the struct removes the wallet directory (and
/// with it the wallet database and the activation-heights file).
#[derive(Debug)]
pub struct ZingoCli {
    /// Wallet directory (wallet database, activation-heights TOML),
    /// passed as `--data-dir`.
    wallet_dir: TempDir,
    /// Logs directory; per-operation stdout/stderr are appended to
    /// `stdout.log` / `stderr.log`.
    logs_dir: TempDir,
    /// Configuration the wallet was launched with.
    config: ZingoCliConfig,
}

impl ZingoCli {
    /// Wallet directory (wallet database, activation-heights TOML).
    pub fn wallet_dir(&self) -> &TempDir {
        &self.wallet_dir
    }

    /// Configuration the wallet was launched with.
    pub fn config(&self) -> &ZingoCliConfig {
        &self.config
    }

    /// Locate the zingo-cli binary: `TEST_BINARIES_DIR` first, then
    /// `PATH`.
    fn executable() -> PathBuf {
        match std::env::var("TEST_BINARIES_DIR") {
            Ok(dir) => {
                let candidate = PathBuf::from(dir).join(EXECUTABLE_NAME);
                if candidate.exists() {
                    candidate
                } else {
                    PathBuf::from(EXECUTABLE_NAME)
                }
            }
            Err(_) => PathBuf::from(EXECUTABLE_NAME),
        }
    }

    /// Write the regtest `--activation-heights` TOML and return its
    /// path, or `None` for main/test (where the flag is invalid).
    ///
    /// The schema is shared with zcash-devtool: one optional
    /// `<upgrade> = <height>` line per upgrade, a missing key meaning
    /// "never activates". At NU6.2 the binary's schema has no `nu6_3`
    /// key, so a schedule that activates NU6.3 must fail loudly here
    /// rather than be silently truncated.
    fn write_activation_heights_toml(
        config: &ZingoCliConfig,
        wallet_dir: &TempDir,
    ) -> Result<Option<PathBuf>, WalletError> {
        let WalletNetwork::Regtest(ref validator_heights) = config.network else {
            return Ok(None);
        };
        let heights = validator_heights.activation_heights();
        assert!(
            heights.nu6_3().is_none(),
            "zingo-cli's NU6.2 activation-heights schema cannot express NU6.3 \
             (configured NU6.3 = {:?}); the NU6.3 bump is tracked as follow-on work",
            heights.nu6_3()
        );
        assert!(
            heights.nu7().is_none(),
            "the activation-heights TOML cannot express NU7; configured NU7 = {:?}",
            heights.nu7()
        );
        let entries = [
            ("overwinter", heights.overwinter()),
            ("sapling", heights.sapling()),
            ("blossom", heights.blossom()),
            ("heartwood", heights.heartwood()),
            ("canopy", heights.canopy()),
            ("nu5", heights.nu5()),
            ("nu6", heights.nu6()),
            ("nu6_1", heights.nu6_1()),
            ("nu6_2", heights.nu6_2()),
        ];
        let mut body = String::new();
        for (key, value) in entries {
            if let Some(height) = value {
                use std::fmt::Write as _;
                // Infallible write into a String.
                let _ = writeln!(body, "{key} = {height}");
            }
        }
        let path = wallet_dir.path().join(HEIGHTS_FILENAME);
        std::fs::write(&path, body).map_err(|io_error| WalletError::SpawnFailed {
            operation: "launch",
            io_error: format!("writing activation-heights file: {io_error}"),
        })?;
        Ok(Some(path))
    }

    /// The arguments common to every invocation: chain, heights file
    /// (regtest), server, and data dir.
    fn base_args(&self) -> Vec<String> {
        let mut args = vec![
            "--chain".to_string(),
            self.config.chain_name().to_string(),
            "--server".to_string(),
            format!("http://127.0.0.1:{}", self.config.indexer_port),
            "--data-dir".to_string(),
            self.wallet_dir
                .path()
                .to_str()
                .expect("tempdir paths are UTF-8")
                .to_string(),
        ];
        if matches!(self.config.network, WalletNetwork::Regtest(_)) {
            args.push("--activation-heights".to_string());
            args.push(
                self.wallet_dir
                    .path()
                    .join(HEIGHTS_FILENAME)
                    .to_str()
                    .expect("tempdir paths are UTF-8")
                    .to_string(),
            );
        }
        args
    }

    /// Run one one-shot invocation to completion, append its output to
    /// the logs, and return stdout on success.
    fn run(
        &self,
        operation: &'static str,
        extra_flags: &[&str],
        command_and_args: &[&str],
    ) -> Result<String, WalletError> {
        let mut args = self.base_args();
        args.extend(extra_flags.iter().map(ToString::to_string));
        args.extend(command_and_args.iter().map(ToString::to_string));
        let output = std::process::Command::new(Self::executable())
            .args(&args)
            .output()
            .map_err(|io_error| WalletError::SpawnFailed {
                operation,
                io_error: io_error.to_string(),
            })?;
        self.append_logs(operation, &output);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.status.success() {
            return Err(WalletError::OperationFailed {
                operation,
                exit_status: output.status,
                stdout,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(stdout)
    }

    /// Append one operation's captured output to the logs directory,
    /// with a banner line per operation.
    fn append_logs(&self, operation: &str, output: &std::process::Output) {
        use std::io::Write as _;
        for (log_name, bytes) in [
            ("stdout.log", &output.stdout),
            ("stderr.log", &output.stderr),
        ] {
            let path = self.logs_dir.path().join(log_name);
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let _ = writeln!(file, "=== {operation} ===");
                let _ = file.write_all(bytes);
            }
        }
    }
}

impl Wallet for ZingoCli {
    type Config = ZingoCliConfig;

    async fn launch(config: Self::Config) -> Result<Self, WalletError> {
        let make_tempdir = |purpose: &'static str| {
            TempDir::new().map_err(|io_error| WalletError::SpawnFailed {
                operation: "launch",
                io_error: format!("creating {purpose} tempdir: {io_error}"),
            })
        };
        let wallet_dir = make_tempdir("wallet")?;
        let logs_dir = make_tempdir("logs")?;
        Self::write_activation_heights_toml(&config, &wallet_dir)?;
        let wallet = Self {
            wallet_dir,
            logs_dir,
            config,
        };
        // Restoring from a mnemonic requires the seed flags exactly
        // once: the wallet database this invocation creates is what
        // every later invocation loads.
        let mnemonic = wallet.config.mnemonic.clone();
        let birthday = wallet.config.birthday.to_string();
        wallet.run(
            "launch",
            &["--nosync", "--seed", &mnemonic, "--birthday", &birthday],
            &["version"],
        )?;
        Ok(wallet)
    }

    async fn sync(&self) -> Result<(), WalletError> {
        // The one operation without --nosync: let the startup sync
        // run, block on it with --waitsync, and persist the result
        // with `save` before the process exits.
        self.run("sync", &["--waitsync"], &["save"])?;
        Ok(())
    }

    async fn send(&self, address: &str, value_zats: u64) -> Result<String, WalletError> {
        let stdout = self.run(
            "send",
            &["--nosync"],
            &["quicksend", address, &value_zats.to_string()],
        )?;
        parse::single_txid("send", &stdout)
    }

    async fn shield(&self) -> Result<String, WalletError> {
        let stdout = self.run("shield", &["--nosync"], &["quickshield"])?;
        parse::single_txid("shield", &stdout)
    }

    async fn balance(&self) -> Result<WalletBalance, WalletError> {
        let balance_stdout = self.run("balance", &["--nosync"], &["balance"])?;
        let pools = parse::account_balance("balance", &balance_stdout)?;
        let height_stdout = self.run("balance", &["--nosync"], &["height"])?;
        let chain_tip_height = parse::height("balance", &height_stdout)?;
        Ok(WalletBalance {
            total: pools.total(),
            sapling_spendable: pools.confirmed_sapling,
            orchard_spendable: pools.confirmed_orchard,
            // The NU6.2 wallet has no ironwood pool; the field exists
            // in the trait contract and is identically zero here.
            ironwood_spendable: 0,
            transparent_spendable: pools.confirmed_transparent,
            chain_tip_height,
        })
    }

    async fn address(&self, receiver: AddressReceiver) -> Result<String, WalletError> {
        let stdout = self.run("address", &["--nosync"], &["addresses"])?;
        let unified = parse::first_unified_address("address", &stdout)?;
        match receiver {
            AddressReceiver::Unified => Ok(unified),
            receiver => parse::receiver_from_unified("address", &unified, receiver),
        }
    }

    async fn get_info(&self) -> Result<GetInfo, WalletError> {
        let stdout = self.run("get_info", &["--nosync"], &["info"])?;
        parse::get_info("get_info", &stdout)
    }

    async fn rescan(&self) -> Result<(), WalletError> {
        // zingo-cli's `rescan` command only *launches* a background
        // rescan, which a run-to-completion invocation cannot wait
        // for. `clear` synchronously rolls the wallet back to its
        // empty, keys-only state — the trait's "wipe and re-restore";
        // the prescribed follow-up sync rebuilds from the birthday.
        self.run("rescan", &["--nosync"], &["clear"])?;
        Ok(())
    }
}
